"""The TV cells' parts against recorded transcripts and fakes on loopback, an existing Prysm's backup and
restore, the end-of-run warning, and the TV cells' command line and dry runs. Nothing contacts a TV."""

from collections.abc import Callable, Iterator, Sequence
import contextlib
import io
import json
from pathlib import Path
import socket
import subprocess
import sys
import tempfile
from typing import Any
import unittest
from unittest import mock

from acceptance_fakes import FakePrysm, ok, untyped
from test_acceptance_commands import confined
from test_acceptance_host import EXAMPLE, runner

from prysm_acceptance import bench, commands, config, host_deb, installed, run, transport, tv, tv_cells
from prysm_acceptance.checks import Failed
from prysm_acceptance.config import Installed
from prysm_acceptance.installed import Backup
from prysm_acceptance.options import TV_CELLS, Flavour
from prysm_acceptance.transport import Result
from prysm_acceptance.web import JsonObject

HOST = 'root@tv.example'
STATE = (
    '{"returnValue":true,"flavor":"full","preferences":{"autostart":false,"instance":null},"enabled":true,'
    '"lighting":"processed","power":"active","video":"available","worker":{"phase":"running"},'
    '"capturing":true,"fps":30,"portal":"http://192.0.2.50:8787","failure":null,"encoding":null,'
    '"paused":false,"log":["prysm-webos: portal on 0.0.0.0:8787"]}'
)
SLIM = STATE.replace('"flavor":"full"', '"flavor":"slim"').replace('"portal":"http://192.0.2.50:8787"', '"portal":null')
SLIM_PREFERENCES: JsonObject = {'autostart': True, 'instance': 'http://192.0.2.20:8787'}
NO_SERVICE = Result(1, '', 'Service does not exist: io.prysm.tv.service')
ABSENT = ''.join(f'absent {path}\n' for path in (*commands.INSTALLED_PATHS, commands.DATA_DIR, commands.STAGING))
INSTALLED = ABSENT.replace('absent /media', 'present /media').replace(f'absent {commands.DATA_DIR}', 'present /var/lib')
DATA_ONLY = ABSENT.replace(f'absent {commands.DATA_DIR}', f'present {commands.DATA_DIR}')
# The templates that only read the TV.
READING = {'tv.reachable', 'tv.paths', 'tv.processes', 'tv.get-state', 'tv.read-data', 'tv.sample-worker'}


def recorded(*entries: tuple[str, Result]) -> tv.Tv:
    """The TV on a recorded transcript."""
    return tv.Tv(transport.Transcript(entries), HOST)


def calls(found: tv.Tv) -> list[str]:
    return [command.template for command in untyped(found.transport).calls]


def absent_data() -> list[tuple[str, Result]]:
    return [('tv.read-data', ok('__absent__\n'))] * len(commands.DATA_FILES)


def no_wait() -> contextlib.AbstractContextManager[object]:
    return mock.patch.object(tv.time, 'sleep')


class TheTv(unittest.TestCase):
    def test_install_follows_the_documented_path(self) -> None:
        found = recorded(
            ('local.verify-ipk', ok()),
            ('tv.stage', ok()),
            ('tv.install', ok('{"details":{"state":"installed"}}')),
            ('tv.unstage', ok()),
            ('tv.setup', ok()),
            ('tv.get-state', ok(STATE)),
        )
        self.assertEqual(found.install('dist/prysm-tv-full.ipk')['flavor'], 'full')
        verify, stage = untyped(found.transport).calls[:2]
        self.assertEqual(verify.argv[-2:], ('verify', 'dist/prysm-tv-full.ipk'))
        self.assertEqual((stage.argv[-2], stage.stdin_file), (HOST, 'dist/prysm-tv-full.ipk'))

    def test_a_package_the_verifier_refuses_is_never_staged(self) -> None:
        found = recorded(('local.verify-ipk', Result(1, '', 'outside the app and service directories')))
        with self.assertRaises(transport.CommandFailed):
            found.install('dist/prysm-tv-full.ipk')
        self.assertEqual(calls(found), ['local.verify-ipk'])

    def test_a_failed_install_still_removes_the_staged_package(self) -> None:
        found = recorded(
            ('local.verify-ipk', ok()),
            ('tv.stage', ok()),
            ('tv.install', Result(1, '{"returnValue":false,"errorText":"bad package"}')),
            ('tv.unstage', ok()),
        )
        with self.assertRaises(Failed):
            found.install('dist/prysm-tv-slim.ipk')
        self.assertEqual(calls(found), ['local.verify-ipk', 'tv.stage', 'tv.install', 'tv.unstage'])

    def test_uninstall_leaves_nothing_but_the_data(self) -> None:
        found = recorded(
            ('tv.supervisor-uninstall', ok()),
            ('tv.remove-app', ok()),
            ('tv.paths', ok(DATA_ONLY)),
            ('tv.processes', ok()),
            ('tv.get-state', NO_SERVICE),
        )
        with no_wait():
            self.assertEqual(found.uninstall(), [])

    def test_uninstall_reports_what_is_left(self) -> None:
        found = recorded(
            ('tv.supervisor-uninstall', ok()),
            ('tv.remove-app', ok()),
            ('tv.paths', ok(ABSENT.replace(f'absent {commands.HOOK}', f'present {commands.HOOK}'))),
            ('tv.processes', ok(f'812 {commands.WORKER}\n')),
            ('tv.get-state', NO_SERVICE),
        )
        with no_wait():
            self.assertEqual(found.uninstall(), [commands.HOOK, f'process 812 {commands.WORKER}'])

    def test_an_app_the_install_service_does_not_remove_fails_the_uninstall(self) -> None:
        found = recorded(
            ('tv.supervisor-uninstall', ok()),
            ('tv.remove-app', Result(1, '{"returnValue":false,"errorText":"denied"}')),
            ('tv.paths', ok(ABSENT.replace(f'absent {commands.APP_DIR}', f'present {commands.APP_DIR}'))),
            ('tv.processes', ok()),
            ('tv.get-state', NO_SERVICE),
        )
        with no_wait(), self.assertRaisesRegex(Failed, 'did not report removed'):
            found.uninstall()

    def test_the_full_portal_is_where_getstate_says(self) -> None:
        self.assertEqual(recorded(('tv.get-state', ok(STATE))).portal().base, 'http://192.0.2.50:8787')
        with self.assertRaisesRegex(Failed, 'no portal address'):
            recorded(('tv.get-state', ok(SLIM))).portal()

    def test_worker_load_from_proc(self) -> None:
        reading = (
            '1000.00 900.00\n0.50 0.40 0.30 1/100 812\n'
            'stat 812 (prysm-webos-wor) S 1 812 0 0 -1 0 0 0 0 0 300 100 0 0 20 0\nVmRSS:\t   10240 kB\n'
        )
        later = reading.replace('1000.00', '1010.00').replace(' 300 100 ', ' 600 200 ')
        found = recorded(('tv.sample-worker', ok(reading)), ('tv.sample-worker', ok(later)))
        first, second = found.sample_worker(), found.sample_worker()
        self.assertEqual(first['rss_kb'], 10240)
        self.assertEqual(tv.cpu_percent(first, second), 40.0)


class Wake(unittest.TestCase):
    """A wake-on-LAN packet only when the TV does not answer and `[tv] mac` is set."""

    MAC = '00:00:5E:00:53:01'

    def test_an_answering_tv_is_not_woken(self) -> None:
        found = recorded(('tv.reachable', ok('prysm-acceptance-ok\n')))
        self.assertIsNone(found.reach(self.MAC, lambda: '192.0.2.10'))
        self.assertEqual(calls(found), ['tv.reachable'])

    def test_without_a_hardware_address_nothing_is_sent(self) -> None:
        found = recorded(('tv.reachable', Result(255, '', 'No route to host')))
        self.assertEqual(found.reach('', lambda: '192.0.2.10'), f'the TV ({HOST}) does not answer over SSH')
        self.assertEqual(calls(found), ['tv.reachable'])

    def test_an_unreachable_tv_is_woken_once_and_waited_for(self) -> None:
        found = recorded(
            ('tv.reachable', Result(255, '')),
            ('local.wake-on-lan', ok()),
            ('tv.reachable', Result(255, '')),
            ('tv.reachable', ok('prysm-acceptance-ok\n')),
        )
        with no_wait():
            self.assertIsNone(found.reach(self.MAC, lambda: '192.0.2.10'))
        wake = untyped(found.transport).calls[1]
        self.assertEqual(wake.datagram.source, '192.0.2.10')
        self.assertEqual(calls(found).count('local.wake-on-lan'), 1)

    def test_a_tv_that_stays_off_is_a_reason(self) -> None:
        found = recorded(('tv.reachable', Result(255, '')), ('local.wake-on-lan', ok()))
        with mock.patch.object(tv, 'WAKE_SECONDS', 0):
            self.assertIn('after a wake-on-LAN packet', found.reach(self.MAC, lambda: '192.0.2.10') or '')

    def test_a_packet_that_cannot_be_sent_is_a_reason(self) -> None:
        def no_route() -> str:
            raise socket.gaierror('nodename nor servname provided')

        found = recorded(('tv.reachable', Result(255, '')))
        self.assertIn('no route to it for a wake-on-LAN packet', found.reach(self.MAC, no_route) or '')
        found = recorded(('tv.reachable', Result(255, '')), ('local.wake-on-lan', Result(1, '', 'Network is down')))
        self.assertIn('packet failed: Network is down', found.reach(self.MAC, lambda: '192.0.2.10') or '')


class Existing:
    """An `installed.Existing` that records what was asked of it, in order."""

    name = 'tv'

    def __init__(self, backup: Callable[[], Backup], left: Sequence[str] = ()) -> None:
        self.make = backup
        self.left = list(left)
        self.done: list[str] = []

    def found(self) -> list[str]:
        return ['x']

    def backup(self, found: list[str]) -> Backup:
        self.done.append('backup')
        return self.make()

    def remove(self) -> list[str]:
        self.done.append('remove')
        return self.left

    def restore(self, backup: Backup) -> list[str]:
        return []


def a_backup(**values: Any) -> Backup:
    backup: Backup = {
        'target': 'tv',
        'host': HOST,
        'taken': '2026-11-17T10:00:00+00:00',
        'found': ['x'],
        'portal': None,
        'luna': {'flavor': 'slim', 'preferences': SLIM_PREFERENCES, 'enabled': True, 'lighting': None},
        'files': {'tv.json': json.dumps(SLIM_PREFERENCES)},
    }
    return {**backup, **values}  # pyright: ignore[reportReturnType]


class Replace(unittest.TestCase):
    """`installed`: refuse stops before any change; replace backs up, verifies, warns, then uninstalls."""

    def setUp(self) -> None:
        self.path = Path(tempfile.mkdtemp()) / installed.BACKUP
        self.warnings: list[str] = []

    def replace(self, existing: installed.Existing) -> str:
        return installed.replace(existing, ['x'], self.path, self.warnings.append)

    def test_nothing_on_the_tv_is_nothing_found(self) -> None:
        found = recorded(('tv.paths', ok(ABSENT)), ('tv.processes', ok()), ('tv.get-state', NO_SERVICE), *absent_data())
        self.assertEqual(found.found(), [])
        self.assertIsNone(installed.refusal(found, []))

    def test_refuse_stops_before_any_change(self) -> None:
        found = recorded(
            ('tv.paths', ok(INSTALLED)),
            ('tv.processes', ok(f'812 {commands.WORKER}\n')),
            ('tv.get-state', ok(STATE)),
            ('tv.read-data', ok('{"autostart":false}')),
            ('tv.read-data', ok('{}')),
            ('tv.read-data', ok('__absent__\n')),
        )
        reason = installed.refusal(found, found.found())
        self.assertIn(f'Prysm is on the tv target: {commands.APP_DIR}', reason or '')
        self.assertIn(f'process 812 {commands.WORKER}', reason or '')
        self.assertIn(f'{commands.DATA_DIR}/settings.json. Nothing was changed', reason or '')
        self.assertIn('installed = "replace"', reason or '')
        self.assertLessEqual(set(calls(found)), READING)

    def test_a_backup_that_cannot_be_taken_removes_nothing(self) -> None:
        def unanswered() -> Backup:
            raise RuntimeError('Prysm is installed but the Luna service does not answer getState')

        existing = Existing(unanswered)
        with self.assertRaisesRegex(Failed, 'nothing was removed: Prysm is installed but'):
            self.replace(existing)
        self.assertEqual((existing.done, self.warnings), (['backup'], []))

    def test_a_backup_that_does_not_read_back_removes_nothing(self) -> None:
        existing = Existing(a_backup)
        with (
            mock.patch.object(Path, 'read_text', return_value='{"target": "tv"}'),
            self.assertRaisesRegex(Failed, 'does not read back as written, so nothing was removed'),
        ):
            self.replace(existing)
        self.assertEqual((existing.done, self.warnings), (['backup'], []))

    def test_the_warning_comes_before_the_uninstall(self) -> None:
        existing = Existing(a_backup)
        warned: list[list[str]] = []
        installed.replace(existing, ['x'], self.path, lambda text: warned.append(list(existing.done)))
        self.assertEqual((warned, existing.done), ([['backup']], ['backup', 'remove']))
        self.assertEqual(json.loads(self.path.read_text()), a_backup())

    def test_what_the_uninstall_leaves_fails(self) -> None:
        with self.assertRaisesRegex(Failed, f'left on the tv target: {commands.HOOK}'):
            self.replace(Existing(a_backup, [commands.HOOK]))
        self.assertIn('REMOVED', self.warnings[0])

    def test_the_warning_says_what_to_do(self) -> None:
        self.replace(Existing(a_backup))
        self.assertEqual(
            self.warnings,
            [
                f'Prysm was REMOVED from the tv target ({HOST}) and NOT reinstalled. Reinstalling the right version '
                f'is your responsibility. Its settings are backed up in {self.path}; once Prysm is installed and '
                f'running there again, restore them with: {installed.RESTORE} tv {self.path}'
            ],
        )

    def test_a_tv_whose_service_does_not_answer_is_not_backed_up(self) -> None:
        found = recorded(*absent_data(), ('tv.get-state', NO_SERVICE))
        with self.assertRaisesRegex(Failed, 'nothing was removed: Prysm is installed but'):
            installed.replace(found, [commands.APP_DIR], self.path, self.warnings.append)
        self.assertNotIn('tv.supervisor-uninstall', calls(found))
        self.assertEqual(self.warnings, [])

    def test_only_data_files_are_backed_up_and_nothing_is_removed(self) -> None:
        data = f'{commands.DATA_DIR}/tv.json'
        found = recorded(
            ('tv.read-data', ok('{"autostart":true}')),
            ('tv.read-data', ok('__absent__\n')),
            ('tv.read-data', ok('__absent__\n')),
            ('tv.get-state', NO_SERVICE),
        )
        installed.replace(found, [data], self.path, self.warnings.append)
        backup = installed.load(self.path, 'tv')
        self.assertEqual((backup['luna'], backup['files']['tv.json']), (None, '{"autostart":true}'))
        self.assertIn('was not installed on the tv target', self.warnings[0])
        self.assertNotIn('tv.supervisor-uninstall', calls(found))

    def test_a_slim_tv_is_backed_up_from_getstate_and_its_files(self) -> None:
        found = recorded(
            ('tv.read-data', ok(json.dumps(SLIM_PREFERENCES))),
            ('tv.read-data', ok('__absent__\n')),
            ('tv.read-data', ok('{"boot":"b","intent":"processed"}')),
            ('tv.get-state', ok(SLIM)),
        )
        backup = found.backup([commands.APP_DIR])
        self.assertEqual(
            (backup['portal'], untyped(backup['luna'])['preferences'], backup['files']['resume.json']),
            (None, {'autostart': False, 'instance': None}, '{"boot":"b","intent":"processed"}'),
        )


class Restore(unittest.TestCase):
    """`settings restore`: the backup into the reinstalled Prysm through its API; never lighting."""

    def test_a_slim_backup_sets_the_preferences_and_leaves_lighting(self) -> None:
        found = recorded(('tv.get-state', ok(SLIM)), ('tv.set-preferences', ok('{"returnValue":true}')))
        done = found.restore(a_backup())
        sent = untyped(found.transport).calls[1].remote
        self.assertIn('"autostart":true,"instance":"http://192.0.2.20:8787"', sent)
        self.assertIn('not switched on', done[-1])
        self.assertNotIn('tv.set-enabled', calls(found))

    def test_nothing_answering_is_refused(self) -> None:
        found = recorded(('tv.get-state', NO_SERVICE))
        with self.assertRaisesRegex(RuntimeError, f'no Prysm answers on {HOST}'):
            found.restore(a_backup())
        self.assertEqual(calls(found), ['tv.get-state'])

    def test_another_package_is_refused(self) -> None:
        with self.assertRaisesRegex(RuntimeError, 'the backup is of the slim package; root@tv.example has full'):
            recorded(('tv.get-state', ok(STATE))).restore(a_backup())

    def test_the_linux_package_round_trip(self) -> None:
        prysm = FakePrysm()
        self.addCleanup(prysm.close)
        existing = host_deb.DebPrysm(transport.Refusing(), 'pi@pi.example')
        existing.url = f'http://{prysm.listen}'
        backup = existing.backup(['/usr/bin/prysm-headless'])
        self.assertEqual(untyped(backup['portal'])['target'], 'off')
        self.assertIn('not switched on', existing.restore(backup)[-1])
        self.assertEqual(len(prysm.saved), 1)

    def test_the_linux_package_refuses_an_admin_unit_and_a_silent_prysm(self) -> None:
        existing = host_deb.DebPrysm(transport.Refusing(), 'pi@pi.example')
        with self.assertRaisesRegex(RuntimeError, 'would outlive the purge'):
            existing.backup(['/etc/systemd/system/prysm-headless.service.d'])
        with socket.socket() as probe:
            probe.bind(('127.0.0.1', 0))
            existing.url = f'http://127.0.0.1:{probe.getsockname()[1]}'
        with self.assertRaises(RuntimeError):
            existing.restore(a_backup(portal={'url': existing.url, 'settings': {}, 'target': 'off'}))

    def test_the_command_refuses_when_nothing_answers(self) -> None:
        """No Prysm answering, a refused Luna call or an instance the template refuses: exit 1, no traceback."""
        path = Path(tempfile.mkdtemp()) / installed.BACKUP
        path.write_text(json.dumps(a_backup()))
        args = run.arguments(['settings', 'restore', 'tv', str(path), '--config', str(EXAMPLE)])
        silent = recorded(('tv.get-state', NO_SERVICE))
        refusing = recorded(
            ('tv.get-state', ok(SLIM)), ('tv.set-preferences', ok('{"returnValue":false,"errorText":"denied"}'))
        )
        for existing, said in ((silent, 'no Prysm answers'), (refusing, 'tv.set-preferences refused: denied')):
            errors = io.StringIO()
            with mock.patch.object(run, '_existing', return_value=existing), contextlib.redirect_stderr(errors):
                self.assertEqual(run.restore_settings(args, config.load(EXAMPLE)), 1)
            self.assertIn(said, errors.getvalue())
        unsafe = a_backup(luna={'flavor': 'slim', 'preferences': {'instance': "http://x'$(reboot)"}})
        with self.assertRaises(commands.Refused):
            recorded(('tv.get-state', ok(SLIM))).restore(unsafe)

    def test_the_dry_run_and_a_backup_of_another_target(self) -> None:
        path = Path(tempfile.mkdtemp()) / installed.BACKUP
        path.write_text(json.dumps(a_backup()))
        done = runner('settings', 'restore', 'tv', str(path), '--dry-run', '--config', str(EXAMPLE), offline=True)
        self.assertEqual(done.returncode, 0, done.stderr)
        self.assertIn('luna://io.prysm.tv.service/setPreferences', done.stdout)
        self.assertIn('"autostart":true,"instance":"http://192.0.2.20:8787"', done.stdout)
        self.assertIn('lighting is not switched on', done.stdout)
        other = runner('settings', 'restore', 'linux', str(path), '--dry-run', '--config', str(EXAMPLE), offline=True)
        self.assertEqual(other.returncode, 2)
        self.assertIn('is not a backup of the linux target', other.stderr)


WARNED = """
import pytest
from prysm_acceptance.bench import warn_at_end

def test_removes(request):
    warn_at_end(request, 'PRYSM WAS REMOVED: restore it with the command')
    {end}
"""


class TheWarningAtTheEnd(unittest.TestCase):
    """The warning a cell registers is printed at the end of the run, whatever ended it."""

    def run_ending(self, end: str) -> subprocess.CompletedProcess[str]:
        directory = Path(tempfile.mkdtemp())
        (directory / 'test_warned.py').write_text(WARNED.format(end=end))
        (directory / 'config.toml').write_text('[wled]\naddress = "127.0.0.1:9"\n')
        return subprocess.run(
            [
                sys.executable,
                '-m',
                'pytest',
                str(directory / 'test_warned.py'),
                '-p',
                'prysm_acceptance.bench',
                '-p',
                'no:cacheprovider',
                '--rootdir',
                str(directory),
                '--bench-config',
                str(directory / 'config.toml'),
                '--runs',
                str(directory / 'runs'),
            ],
            capture_output=True,
            text=True,
            timeout=60,
            check=False,
        )

    def test_after_success_failure_and_ctrl_c(self) -> None:
        for end in ('pass', 'raise AssertionError("failed")', 'raise KeyboardInterrupt'):
            with self.subTest(end):
                done = self.run_ending(end)
                self.assertIn('PRYSM WAS REMOVED: restore it with the command', done.stdout, done.stdout[-2000:])
                self.assertIn('WARNING', done.stdout)


def planned_commands(values: config.Config) -> Iterator[commands.Command]:
    """Every command the TV cells' plans name for `values`, as built."""
    built: list[commands.Command] = []
    real = commands._ssh  # pyright: ignore[reportPrivateUsage]

    def spy(*args: Any, **kwargs: Any) -> commands.Command:
        command = real(*args, **kwargs)
        built.append(command)
        return command

    with mock.patch.object(commands, '_ssh', spy):
        for cell in TV_CELLS:
            for plan in bench.PLANS[tv_cells.__name__].values():
                plan.does(cell, values)
                plan.undoes(cell, values)
        existing = tv.Tv(transport.Refusing(), values.tv.ssh)
        run._restore_plan(existing, a_backup())  # pyright: ignore[reportPrivateUsage]
    return iter(built)


class TheCells(unittest.TestCase):
    def test_every_tv_command_the_cells_plan_stays_on_prysms_paths(self) -> None:
        for installed_rule in Installed:
            values = config.load(EXAMPLE)
            values.tv.installed = installed_rule
            found = [command for command in planned_commands(values) if command.argv[-2] == values.tv.ssh]
            self.assertGreater(len(found), 20)
            for command in found:
                with self.subTest(command.template):
                    confined(self, command)

    def test_data_files_an_earlier_cell_of_the_run_left_are_not_the_owners(self) -> None:
        found = [commands.APP_DIR, f'{commands.DATA_DIR}/tv.json', f'{commands.DATA_DIR}/settings.json']
        self.assertEqual(tv_cells.owners(found, installed_by_run=False), found)
        self.assertEqual(tv_cells.owners(found, installed_by_run=True), [commands.APP_DIR])

    def test_a_missing_package_skips_or_fails_and_contacts_nothing(self) -> None:
        directory = Path(tempfile.mkdtemp())
        path = directory / 'config.toml'
        path.write_text(
            '[wled]\naddress = "127.0.0.1:9"\n[leds]\ntop = 1\n'
            '[tv]\ntarget = "ssh://root@tv.example"\nipk_full = "no/such.ipk"\nipk_slim = "no/such.ipk"\n'
        )
        named = runner(
            'tv', 'full', '--tv-is-free', '--config', str(path), '--runs', str(directory / 'a'), offline=True
        )
        self.assertEqual(named.returncode, 1, named.stdout[-2000:])
        self.assertIn('tv-full: no package: [tv] ipk_full is', named.stdout)
        every = runner('all', '--tv-is-free', '--config', str(path), '--runs', str(directory / 'b'), offline=True)
        self.assertEqual(every.returncode, 0, every.stdout[-2000:])
        self.assertIn('tv-slim-linux: no package', every.stdout)

    def test_the_capture_is_the_packages_own(self) -> None:
        self.assertEqual(
            tv_cells.capture(Flavour.FULL, 640, 360, 30),
            {'kind': 'webos', 'width': 640, 'height': 360, 'fps': 30, 'include_ui': True},
        )
        self.assertEqual(tv_cells.capture(Flavour.SLIM, 640, 360, 30, tv.Encoding.RAW)['encoding'], 'raw')


class TheCommandLine(unittest.TestCase):
    def dry(self, *arguments: str, path: Path = EXAMPLE) -> subprocess.CompletedProcess[str]:
        return runner(*arguments, '--dry-run', '--config', str(path), offline=True)

    def test_a_tv_cell_needs_the_owners_word(self) -> None:
        for arguments in (['tv', 'full'], ['tv', 'slim', '--receiver', 'mac']):
            done = self.dry(*arguments)
            self.assertEqual(done.returncode, 2)
            self.assertIn('--tv-is-free', done.stderr)
        for arguments in (['tv', 'slim', '--tv-is-free'], ['tv', 'full', '--receiver', 'mac', '--tv-is-free']):
            self.assertEqual(self.dry(*arguments).returncode, 2)

    def test_each_tv_cell_prints_its_plan(self) -> None:
        for arguments, cell in (
            (['tv', 'full'], 'tv-full'),
            (['tv', 'slim', '--receiver', 'mac'], 'tv-slim-mac'),
            (['tv', 'slim', '--receiver', 'linux'], 'tv-slim-linux'),
        ):
            with self.subTest(cell):
                done = self.dry(*arguments, '--tv-is-free')
                self.assertEqual(done.returncode, 0, done.stdout[-3000:] + done.stderr)
                self.assertEqual(done.stdout.count('\nCell '), 1)
                text = done.stdout[done.stdout.index(f'Cell {cell}') :]
                order = [
                    text.index(f'  {name}: ')
                    for name in ('artifact', 'target', 'wled', 'cleared', 'package', 'prysm', 'test_registered')
                ]
                self.assertEqual(order, sorted(order))
                for line in (
                    'wake-on-LAN magic packet for 00:00:5E:00:53:01',
                    'test_wled_live',
                    'test_fps',
                    'test_soak',
                    'prysm-webos-service uninstall',
                    'refused, nothing is changed',
                ):
                    self.assertIn(line, text)
        mac = self.dry('tv', 'slim', '--receiver', 'mac', '--tv-is-free').stdout
        self.assertIn('--listen 0.0.0.0:18787 --metrics &', mac)
        self.assertIn('test_encodings', mac)
        self.assertNotIn('test_capture_modes', mac)

    def test_all_runs_the_tv_cells_only_with_the_owners_word(self) -> None:
        without = self.dry('all')
        self.assertEqual(without.returncode, 0, without.stderr)
        self.assertNotIn('Cell tv-', without.stdout)
        done = self.dry('all', '--tv-is-free')
        self.assertEqual(done.returncode, 0, done.stdout[-3000:] + done.stderr)
        cells = [line.split()[1] for line in done.stdout.splitlines() if line.startswith('Cell ')]
        self.assertEqual(cells[-3:], ['tv-full', 'tv-slim-mac', 'tv-slim-linux'])

    def test_prepare_builds_the_package_and_the_receiver(self) -> None:
        done = self.dry('tv', 'slim', '--receiver', 'linux', '--tv-is-free', '--prepare')
        self.assertIn('--prepare (dry run, not run): $ cargo xtask dist tv-slim linux-arm64', done.stdout)

    def test_replace_is_shown_with_its_backup_and_uninstall(self) -> None:
        path = Path(tempfile.mkdtemp()) / 'config.toml'
        path.write_text(EXAMPLE.read_text().replace('installed = "refuse"', 'installed = "replace"'))
        done = self.dry('tv', 'full', '--tv-is-free', path=path)
        cleared = done.stdout[done.stdout.index('  cleared: ') : done.stdout.index('  package: ')]
        self.assertIn(installed.BACKUP, cleared)
        self.assertIn('prysm-webos-service uninstall', cleared)
        deb = self.dry('host', 'linux', '--delivery', 'deb', path=path).stdout
        self.assertIn('apt-get purge -y prysm-headless', deb[deb.index('  cleared: ') : deb.index('  running: ')])


if __name__ == '__main__':
    unittest.main()
