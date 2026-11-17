"""The acceptance runner's allow-list, its transport and the TV steps against a recorded transcript."""

from collections.abc import Sequence
from pathlib import Path
import re
import sys
import tempfile
from typing import cast
import unittest

from acceptance_fakes import NO_OPTIONS, transcript_of, untyped

from prysm_acceptance import commands, config, stage_tv, transport, tv, tv_full
from prysm_acceptance.checks import Answer, Failed
from prysm_acceptance.commands import Flavour
from prysm_acceptance.options import Options
from prysm_acceptance.plan import Context
from prysm_acceptance.stage_data import TvData
from prysm_acceptance.transport import Result

# The runner's project: scripts/acceptance.
ACCEPTANCE = Path(__file__).resolve().parents[1]
PACKAGE = ACCEPTANCE / 'prysm_acceptance'

# Shell words that must never appear in a TV command.
FORBIDDEN = re.compile(
    r'\b(dd|mkfs\w*|flash\w*|mtd\w*|ubi\w*|mount|umount|reboot|insmod|rmmod|chmod|chown|'
    r'mv|cp|ln|tee|sed|kill|killall|pkill|opkg|ares-install)\b|rm\s+-r|/dev/|/etc/|rootfs|tvservice'
)
# The commands a TV command may run (the first word of each part).
TV_PROGRAMS = {
    'echo',
    'if',
    'for',
    'then',
    'else',
    'fi',
    'done',
    'do',
    'case',
    'esac',
    'cat',
    'mkdir',
    'rm',
    'timeout',
    'luna-send',
    'grep',
    'readlink',
    'continue',
    'l=$(readlink',
    'p=${e#/proc/};',
    '[',
    commands.SETUP,
    commands.SUPERVISOR,
}


def without_uris(text: str | None) -> str:
    """The remote text without Luna and HTTP URIs (services, not files) and error redirections."""
    return re.sub(r'(luna|https?)://[^\s\'"]+', 'URI', text or '').replace('2>/dev/null', '')


def paths(text: str | None) -> list[str]:
    return re.findall(r'(?<![\w$*%#])/[A-Za-z0-9_.*\[\]/-]+', without_uris(text))


class AllowList(unittest.TestCase):
    def test_every_command_builder_is_listed(self) -> None:
        builders = {
            name
            for name, value in vars(commands).items()
            if callable(value)
            and not name.startswith('_')
            and getattr(value, '__module__', '') == 'prysm_acceptance.commands'
            and name[0].islower()
            and name not in ('ssh_target', 'adb_serial', 'field')
        }
        produced = {factory().template for factory in commands.TEMPLATES.values()}
        self.assertEqual(set(commands.TEMPLATES), produced)
        self.assertEqual(len(builders), len(commands.TEMPLATES), sorted(builders))

    def test_every_template_is_bounded_and_named(self) -> None:
        for name, factory in commands.TEMPLATES.items():
            command = factory()
            self.assertEqual(command.template, name)
            if command.background:
                self.assertEqual(name, 'local.headless')
            else:
                self.assertGreater(command.timeout, 0, name)
                self.assertLessEqual(command.timeout, 3600, name)

    def test_tv_commands_touch_only_prysm_paths(self) -> None:
        tv_templates = [factory() for name, factory in commands.TEMPLATES.items() if name.startswith('tv.')]
        self.assertTrue(tv_templates)
        for command in tv_templates:
            with self.subTest(command.template):
                self.assertEqual(command.argv[0], 'ssh')
                self.assertEqual(command.argv[-1], command.remote)
                self.assertIn('BatchMode=yes', command.argv)
                self.assertIsNone(FORBIDDEN.search(without_uris(command.remote)), command.remote)
                for path in paths(command.remote):
                    self.assertTrue(
                        path.startswith(commands.TV_READ_PATHS + commands.TV_WRITE_PATHS), f'{path} in {command.remote}'
                    )
                remote = command.remote or ''
                for written in re.findall(r'(?:>|rm -f|mkdir -p)\s+(\S+)', remote):
                    if written.startswith('/'):
                        self.assertIn(written, commands.TV_WRITE_PATHS, command.remote)
                    else:
                        self.assertRegex(written, r'^(/dev/null|"\$\w+")$')
                self.assertNotIn(
                    commands.DATA_DIR + '/',
                    re.sub(
                        r'cat /var/lib/io\.prysm\.tv/\w+\.json|'
                        r'-f /var/lib/io\.prysm\.tv/\w+\.json',
                        '',
                        remote,
                    ),
                )

    def test_luna_calls_only_prysm_and_the_install_service(self) -> None:
        for factory in commands.TEMPLATES.values():
            command = factory()
            for uri in re.findall(r'luna://\S+', command.remote or ''):
                self.assertRegex(
                    uri,
                    r'^luna://(io\.prysm\.tv\.service/(getState|setPreferences|setEnabled)|'
                    r'com\.webos\.appInstallService/dev/install)$',
                )
            if command.remote and 'luna-send' in command.remote:
                self.assertTrue(command.hold_stdin, command.template)
                self.assertTrue(command.remote.startswith('timeout '), command.template)

    def test_adb_runs_only_the_keep_awake_commands(self) -> None:
        serial = '0123456789ABCD'
        self.assertEqual(
            {name: factory().argv for name, factory in commands.TEMPLATES.items() if name.startswith('adb.')},
            {
                'adb.get-state': ('adb', '-s', serial, 'get-state'),
                'adb.wake': ('adb', '-s', serial, 'shell', 'input keyevent KEYCODE_WAKEUP'),
                'adb.wakefulness': ('adb', '-s', serial, 'shell', 'dumpsys power | grep mWakefulness'),
                'adb.wifi-address': ('adb', '-s', serial, 'shell', 'ip -f inet -o addr show wlan0'),
                'adb.stay-on-get': ('adb', '-s', serial, 'shell', 'settings get global stay_on_while_plugged_in'),
                'adb.stay-on-put': (
                    'adb',
                    '-s',
                    '192.0.2.60:37000',
                    'shell',
                    'settings put global stay_on_while_plugged_in 7',
                ),
            },
        )
        self.assertEqual(commands.adb_stay_on_put(serial, 0).argv[-1], 'settings put global stay_on_while_plugged_in 0')
        for name, factory in commands.TEMPLATES.items():
            if factory().argv[0] == 'adb':
                self.assertTrue(name.startswith('adb.'), name)

    def test_parameters_are_checked_not_quoted(self) -> None:
        refused = [
            lambda: commands.tv_reachable('-oProxyCommand=sh'),
            lambda: commands.tv_reachable('root@tv;reboot'),
            lambda: commands.tv_read_data('root@tv', '../../etc/passwd'),
            lambda: commands.tv_stage('root@tv', '-oProxyCommand.ipk'),
            lambda: commands.tv_set_preferences('root@tv', False, 'http://x/;reboot'),
            lambda: commands.tv_set_preferences('root@tv', False, "http://x'$(reboot)"),
            lambda: commands.tv_set_preferences('root@tv', cast('bool', 'yes')),
            lambda: commands.tv_set_enabled('root@tv', cast('bool', 1)),
            lambda: commands.ares_remove('tv; rm -rf /'),
            lambda: commands.verify_ipk('-evil.ipk'),
            lambda: commands.headless('bin', 'settings.json', '0.0.0.0:8787 --start'),
            lambda: commands.ffmpeg_edges_clip(
                'c.mp4', 640, 360, 30, 16, ("ff0000';x", '00ff00', '0000ff', 'ffff00'), 2
            ),
            lambda: commands.ffmpeg_edges_clip('c.mp4', 640, 360, 30, 16, ('ff0000', '00ff00', '0000ff'), 2),
            lambda: commands.ffmpeg_edges_clip('c.mp4', 640, 360, 30, 10, ('ff0000', '00ff00', '0000ff', 'ffff00'), 2),
            lambda: commands.ffmpeg_edges_clip('-c.mp4', 640, 360, 30, 16, ('ff0000', '00ff00', '0000ff', 'ffff00'), 2),
            lambda: commands.ffmpeg_edges_clip('c.mp4', 640, 360, 30, 16, ('ff0000', '00ff00', '0000ff', 'ffff00'), 0),
            lambda: commands.adb_get_state('-H evil'),
            lambda: commands.adb_get_state('0123; reboot'),
            lambda: commands.adb_get_state('ABC'),
            lambda: commands.adb_wake('host:port'),
            lambda: commands.adb_stay_on_put('0123456789ABCD', 16),
            lambda: commands.adb_stay_on_put('0123456789ABCD', cast('int', '7; reboot')),
            lambda: commands.adb_stay_on_put('0123456789ABCD', True),
        ]
        for build in refused:
            with self.assertRaises(commands.Refused):
                build()

    def test_the_clip_is_one_exact_ffmpeg_command(self) -> None:
        command = commands.ffmpeg_edges_clip('clip.mp4', 100, 50, 10, 16, ('ff0000', '00ff00', '0000ff', 'ffff00'), 2)

        def box(x: int, y: int, w: int, h: int, colour: str, step: int) -> str:
            return f"drawbox=x={x}:y={y}:w={w}:h={h}:color=0x{colour}:t=fill:enable='eq(floor(n/20),{step})'"

        # Clockwise from the top-left corner, each corner's two half-bands; the colours move one corner on per step.
        corners = [
            ((0, 0, 50, 10), (0, 0, 20, 25)),
            ((50, 0, 50, 10), (80, 0, 20, 25)),
            ((50, 40, 50, 10), (80, 25, 20, 25)),
            ((0, 40, 50, 10), (0, 25, 20, 25)),
        ]
        colours = [
            ('ff0000', '00ff00', '0000ff', 'ffff00'),
            ('ffff00', 'ff0000', '00ff00', '0000ff'),
            ('0000ff', 'ffff00', 'ff0000', '00ff00'),
            ('00ff00', '0000ff', 'ffff00', 'ff0000'),
        ]
        boxes = [
            box(*part, colour, step)
            for step in range(4)
            for parts, colour in zip(corners, colours[step], strict=False)
            for part in parts
        ]
        self.assertEqual(boxes[0], "drawbox=x=0:y=0:w=50:h=10:color=0xff0000:t=fill:enable='eq(floor(n/20),0)'")
        self.assertEqual(
            command.argv,
            (
                'ffmpeg',
                '-nostdin',
                '-v',
                'error',
                '-y',
                '-f',
                'lavfi',
                '-i',
                'color=c=0x404040:s=100x50:r=10:d=8,' + ','.join(boxes) + ',loop=loop=1:size=80',
                '-c:v',
                'libx264',
                '-pix_fmt',
                'yuv420p',
                '-movflags',
                '+faststart',
                'clip.mp4',
            ),
        )
        self.assertEqual((command.template, command.timeout), ('local.ffmpeg-clip', 300))

    def test_only_the_allow_list_names_ssh_or_spawns(self) -> None:
        for path in PACKAGE.glob('*.py'):
            text = path.read_text()
            if path.name != 'commands.py':
                self.assertNotIn("'ssh'", text, path.name)
                self.assertNotIn("'adb'", text, path.name)
                self.assertNotIn('luna://', text, path.name)
            if path.name != 'transport.py':
                self.assertIsNone(re.search(r'subprocess\.(run|Popen|call|check_\w+)\(|os\.system', text), path.name)

    def test_the_host_cells_only_know_their_feed(self) -> None:
        text = (PACKAGE / 'host_cells.py').read_text().lower()
        for word in ('chromecast', 'adb', 'clipserver', 'clip server', 'feed_chromecast'):
            self.assertNotIn(word, text)
        self.assertIsNone(re.search(r'\bcast', re.sub(r'\bcast\(|, cast\b', '', text)))  # typing.cast is no cast

    def test_no_real_addresses(self) -> None:
        documents = [*ACCEPTANCE.glob('*'), *PACKAGE.glob('*'), ACCEPTANCE.parents[1] / 'docs/developers/acceptance.md']
        for path in documents:
            # config.toml is the owner's own, untracked copy.
            if path.is_file() and path.name != 'config.toml':
                self.assertNotIn('192.168.18.', path.read_text(), path.name)


class HeldStdin(unittest.TestCase):
    def test_stops_at_the_expected_reply(self) -> None:
        command = commands.Command(
            'test',
            (sys.executable, '-c', 'import time; print(\'{"state":"installed"}\', flush=True); time.sleep(30)'),
            10,
            hold_stdin=True,
            until='"installed"',
        )
        result = transport.Real(log=lambda line: None).run(command)
        self.assertTrue(result.ok)
        self.assertLess(result.seconds, 5)

    def test_keeps_stdin_open_until_the_timeout(self) -> None:
        # Like luna-send, this exits as soon as its stdin closes.
        command = commands.Command(
            'test', (sys.executable, '-c', 'import sys; sys.stdin.read(); print("eof")'), 1, hold_stdin=True
        )
        result = transport.Real(log=lambda line: None).run(command, check=False)
        self.assertEqual(result.returncode, 124)
        self.assertNotIn('eof', result.stdout)

    def test_a_failure_reply_fails(self) -> None:
        command = commands.Command(
            'test',
            (sys.executable, '-c', 'import time; print(\'{"returnValue":false}\', flush=True); time.sleep(30)'),
            10,
            hold_stdin=True,
            until='"installed"',
            fail=r'"returnValue":\s*false',
        )
        with self.assertRaises(transport.CommandFailed):
            transport.Real(log=lambda line: None).run(command)

    def test_the_dry_run_transport_refuses_everything(self) -> None:
        with self.assertRaises(RuntimeError):
            transport.Refusing().run(commands.tv_reachable('root@tv'))


STATE = (
    '{"returnValue":true,"flavor":"full","preferences":{"autostart":false},"enabled":true,'
    '"lighting":"processed","power":"active","video":"available","worker":{"phase":"running"},'
    '"capturing":true,"fps":30,"portal":"http://192.0.2.50:8787","failure":null,"encoding":null,'
    '"paused":false,"log":[]}'
)
ABSENT = (
    '\n'.join(f'absent {p}' for p in (*commands.INSTALLED_PATHS, commands.STAGING)) + f'\npresent {commands.DATA_DIR}\n'
)


def context(entries: list[tuple[str, Result]], answers: Sequence[tuple[Answer, str]] = ()) -> Context[TvData]:
    settings = config.parse({'tv': {'address': 'tv.example', 'ares_device': 'tv'}, 'wled': {'address': 'wled.example'}})
    replies = list(answers)
    out = Path(tempfile.mkdtemp())
    return Context[TvData](
        settings,
        NO_OPTIONS,
        transport.Transcript(entries),
        out,
        ask=lambda question, choices=(): replies.pop(0),
        log=lambda line: None,
    )


class RecordedTv(unittest.TestCase):
    def test_install_follows_the_documented_path(self) -> None:
        ctx = context(
            [
                ('local.verify-ipk', Result(0, '')),
                ('tv.stage', Result(0, '')),
                ('tv.install', Result(0, '{"details":{"state":"installed"}}')),
                ('tv.unstage', Result(0, '')),
                ('tv.setup', Result(0, '')),
                ('tv.get-state', Result(0, STATE)),
            ]
        )
        state = tv.install(ctx, 'dist/prysm-tv-full.ipk')
        self.assertEqual(state['flavor'], 'full')
        verify, stage = transcript_of(ctx).calls[:2]
        self.assertEqual(verify.argv[-2:], ('verify', 'dist/prysm-tv-full.ipk'))
        self.assertEqual(stage.argv[-2], 'root@tv.example')
        self.assertEqual(stage.stdin_file, 'dist/prysm-tv-full.ipk')

    def test_a_package_the_verifier_refuses_is_never_staged(self) -> None:
        ctx = context([('local.verify-ipk', Result(1, '', 'outside the app and service directories'))])
        with self.assertRaises(transport.CommandFailed):
            tv.install(ctx, 'dist/prysm-tv-full.ipk')
        self.assertEqual([c.template for c in transcript_of(ctx).calls], ['local.verify-ipk'])

    def test_a_failed_install_still_removes_the_staged_package(self) -> None:
        ctx = context(
            [
                ('local.verify-ipk', Result(0, '')),
                ('tv.stage', Result(0, '')),
                ('tv.install', Result(1, '{"returnValue":false,"errorText":"bad package"}')),
                ('tv.unstage', Result(0, '')),
            ]
        )
        with self.assertRaises(Failed):
            tv.install(ctx, 'dist/prysm-tv-slim.ipk')
        self.assertEqual(
            [c.template for c in transcript_of(ctx).calls], ['local.verify-ipk', 'tv.stage', 'tv.install', 'tv.unstage']
        )

    def test_uninstall_checks_that_nothing_is_left(self) -> None:
        ctx = context(
            [
                ('tv.supervisor-uninstall', Result(0, '')),
                ('local.ares-remove', Result(0, '')),
                ('tv.paths', Result(0, ABSENT)),
                ('tv.processes', Result(0, '')),
                ('tv.get-state', Result(1, '', 'Service does not exist')),
            ]
        )
        detail, evidence = stage_tv.uninstall(ctx)
        self.assertIn('nothing left', detail)
        self.assertIn(commands.DATA_DIR, untyped(evidence)['kept_by_design'])

    def test_uninstall_fails_on_leftovers(self) -> None:
        left = ABSENT.replace(f'absent {commands.HOOK}', f'present {commands.HOOK}')
        ctx = context(
            [
                ('tv.supervisor-uninstall', Result(0, '')),
                ('local.ares-remove', Result(0, '')),
                ('tv.paths', Result(0, left)),
                ('tv.processes', Result(0, f'812 {commands.WORKER}\n')),
                ('tv.get-state', Result(1, '')),
            ]
        )
        with self.assertRaises(Failed) as failure:
            stage_tv.uninstall(ctx)
        self.assertIn(commands.HOOK, str(failure.exception))

    def test_restore_reinstalls_the_package_found_and_its_preferences(self) -> None:
        ok = Result(0, '{"returnValue":true}')
        slim = STATE.replace('"flavor":"full"', '"flavor":"slim"')
        ctx = context(
            [
                ('tv.get-state', Result(1, '')),  # nothing installed after the uninstall check
                ('local.verify-ipk', Result(0, '')),
                ('tv.stage', Result(0, '')),
                ('tv.install', Result(0, '"installed"')),
                ('tv.unstage', Result(0, '')),
                ('tv.setup', Result(0, '')),
                ('tv.get-state', Result(0, slim)),
                ('tv.set-preferences', ok),
                ('tv.set-enabled', ok),
            ]
        )
        ctx.data['ipk'] = {Flavour.SLIM: 'dist/prysm-tv-slim.ipk'}
        ctx.data['as_found'] = {
            'flavor': Flavour.SLIM,
            'enabled': True,
            'lighting': None,
            'preferences': {'autostart': True, 'instance': 'http://192.0.2.20:8787'},
        }
        self.assertIn('slim installed', stage_tv.restore(ctx))
        preferences = str(transcript_of(ctx).calls[-2].remote)
        self.assertIn('"autostart":true,"instance":"http://192.0.2.20:8787"', preferences)
        self.assertIn('"enabled":true', str(transcript_of(ctx).calls[-1].remote))

    def test_the_full_portal_is_where_getstate_says(self) -> None:
        ctx = context([('tv.get-state', Result(0, STATE))])
        self.assertEqual(tv_full.portal(ctx).base, 'http://192.0.2.50:8787')
        ctx = context([('tv.get-state', Result(0, STATE.replace('"http://192.0.2.50:8787"', 'null')))])
        with self.assertRaisesRegex(Failed, 'no portal address'):
            tv_full.portal(ctx)

    def test_worker_load_from_proc(self) -> None:
        reading = (
            '1000.00 900.00\n0.50 0.40 0.30 1/100 812\n'
            'stat 812 (prysm-webos-wor) S 1 812 0 0 -1 0 0 0 0 0 300 100 0 0 20 0\nVmRSS:\t   10240 kB\n'
        )
        later = reading.replace('1000.00', '1010.00').replace(' 300 100 ', ' 600 200 ')
        ctx = context([('tv.sample-worker', Result(0, reading)), ('tv.sample-worker', Result(0, later))])
        first, second = tv.sample_worker(ctx), tv.sample_worker(ctx)
        self.assertEqual(first['rss_kb'], 10240)
        self.assertEqual(tv.cpu_percent(first, second), 40.0)

    def test_the_stage_refuses_without_the_owner_saying_the_tv_is_free(self) -> None:
        settings = config.parse(
            {'tv': {'address': 'tv.example'}, 'wled': {'address': 'wled.example'}, 'leds': {'top': 10}}
        )

        class Args:
            tv_is_free = False

        self.assertIn('--tv-is-free', str(stage_tv.refuse(settings, cast('Options', Args()))))
        Args.tv_is_free = True
        self.assertIsNone(stage_tv.refuse(settings, cast('Options', Args())))


if __name__ == '__main__':
    unittest.main()
