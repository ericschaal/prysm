"""The Linux package and image cells' seams, their templates and preflight, `--prepare` and `acceptance all`,
against recorded transcripts and fakes on loopback (no device, no Docker, no apt, no build)."""

import io
import json
from pathlib import Path
import re
import tarfile
import tempfile
from typing import Literal, cast
import unittest
from unittest import mock

from acceptance_fakes import FakePrysm, ok
from test_acceptance_host import EXAMPLE, runner

from prysm_acceptance import commands, config, host_cells, host_deb, host_image, prepare, run, transport
from prysm_acceptance.commands import ComposeAction, UnitAction
from prysm_acceptance.options import CELLS, Cell, Delivery, Platform, Source
from prysm_acceptance.transport import Result

TARGET = 'pi@pi.example'
RUN = '20261117-101500-host-linux-deb-device'
DIRECTORY = f'$HOME/.prysm-acceptance/{RUN}'
DEVICE = '/dev/v4l/by-id/usb-EXAMPLE-video-index0'
IMAGE_ID = 'sha256:' + 'a' * 64
VERSION = '1.0.0~alpha.0-1'
# What `deb.deliver` and `image.deliver` print: the run directory and the target's clock.
DELIVERED = ok('/home/pi/x\n1763380000\n')
DEB_CELL = Cell(Platform.LINUX, Delivery.DEB, Source.DEVICE)


def ar_member(name: str, data: bytes) -> bytes:
    header = f'{name + "/":<16}{0:<12}{0:<6}{0:<6}{100644:<8}{len(data):<10}`\n'.encode()
    return header + data + (b'\n' if len(data) % 2 else b'')


def tar_of(files: dict[str, bytes], mode: Literal['w', 'w:xz'] = 'w:xz') -> bytes:
    buffer = io.BytesIO()
    with tarfile.open(fileobj=buffer, mode=mode) as tar:
        for name, data in files.items():
            info = tarfile.TarInfo(name)
            info.size = len(data)
            tar.addfile(info, io.BytesIO(data))
    return buffer.getvalue()


def write_deb(path: Path, architecture: str = 'arm64') -> Path:
    """A Debian package as `deb_control` reads it: `ar` with debian-binary and control.tar.xz."""
    control = f'Package: prysm-headless\nVersion: {VERSION}\nArchitecture: {architecture}\nDescription: x\n y\n'
    path.write_bytes(
        b'!<arch>\n'
        + ar_member('debian-binary', b'2.0\n')
        + ar_member('control.tar.xz', tar_of({'./control': control.encode()}))
    )
    return path


def write_image(path: Path, architecture: str = 'arm64', tag: str = 'prysm-headless:local') -> Path:
    """A `docker load` archive as `image_archive` reads it: manifest.json and the image's config."""
    manifest = [{'Config': 'blobs/sha256/c0nf', 'RepoTags': [tag], 'Layers': []}]
    files = {
        'manifest.json': json.dumps(manifest).encode(),
        'blobs/sha256/c0nf': json.dumps({'architecture': architecture, 'os': 'linux'}).encode(),
    }
    path.write_bytes(tar_of(files, 'w'))
    return path


def temporary(test: unittest.TestCase) -> Path:
    return Path(test.enterContext(tempfile.TemporaryDirectory()))


class Templates(unittest.TestCase):
    def test_exact_command_lines(self) -> None:
        project = f'prysm-acceptance-{RUN}'
        compose = f'cd {DIRECTORY} && docker compose -p {project} -f compose.yaml -f compose.acceptance.yaml '
        found = {
            'deb.sudo': commands.deb_sudo(TARGET),
            'deb.install': commands.deb_install(TARGET, RUN),
            'deb.dropin': commands.deb_dropin(TARGET, '0.0.0.0:18787'),
            'deb.systemctl stop': commands.deb_systemctl(TARGET, UnitAction.STOP),
            'deb.systemctl restart': commands.deb_systemctl(TARGET, UnitAction.RESTART),
            'deb.installed': commands.deb_installed(TARGET),
            'deb.unit': commands.deb_unit(TARGET),
            'deb.main-user': commands.deb_main_user(TARGET),
            'deb.journal': commands.deb_journal(TARGET, 1763380000),
            'deb.purge': commands.deb_purge(TARGET),
            'deb.leftovers': commands.deb_leftovers(TARGET),
            'deb.deliver': commands.deb_deliver(TARGET, RUN, 'dist/prysm-headless-linux-arm64.deb'),
            'image.docker': commands.image_docker(TARGET),
            'image.load': commands.image_load(TARGET, RUN),
            'image.present': commands.image_present(TARGET, RUN),
            'image.remove': commands.image_remove(TARGET),
            'image.device': commands.image_device(TARGET, DEVICE),
            'image.deliver': commands.image_deliver(TARGET, RUN, 'dist/prysm-headless-linux-arm64.tar'),
            **{f'image.compose {action}': commands.image_compose(TARGET, RUN, action) for action in ComposeAction},
        }
        self.assertEqual(
            {name: command.remote for name, command in found.items()},
            {
                'deb.sudo': 'sudo -n true',
                'deb.install': f'cd {DIRECTORY} && sudo -n env DEBIAN_FRONTEND=noninteractive apt-get install -y '
                '--no-install-recommends ./prysm-headless.deb',
                'deb.dropin': "sudo -n mkdir -p /run/systemd/system/prysm-headless.service.d && printf '%s\\n' "
                "'[Service]' 'ExecStart=' 'ExecStart=/usr/bin/prysm-headless /var/lib/prysm/settings.json "
                "--listen 0.0.0.0:18787 --metrics' | sudo -n tee "
                '/run/systemd/system/prysm-headless.service.d/prysm-acceptance.conf > /dev/null && '
                'sudo -n systemctl daemon-reload',
                'deb.systemctl stop': 'sudo -n systemctl stop prysm-headless.service',
                'deb.systemctl restart': 'sudo -n systemctl restart prysm-headless.service',
                'deb.installed': "dpkg-query -W -f='${db:Status-Status} ${Version}\\n' prysm-headless 2>/dev/null "
                '|| echo not-installed',
                'deb.unit': 'systemctl show -p LoadState -p ActiveState -p SubState -p Result -p ExecMainCode '
                '-p ExecMainStatus -p MainPID prysm-headless.service',
                'deb.main-user': 'ps -o user= -p "$(systemctl show -p MainPID --value prysm-headless.service)"',
                'deb.journal': 'sudo -n journalctl -u prysm-headless.service -q --no-pager -o short-iso '
                '--since @1763380000',
                'deb.purge': 'sudo -n rm -f /run/systemd/system/prysm-headless.service.d/prysm-acceptance.conf; '
                'sudo -n rmdir /run/systemd/system/prysm-headless.service.d 2>/dev/null; '
                'sudo -n env DEBIAN_FRONTEND=noninteractive apt-get purge -y prysm-headless; s=$?; '
                'sudo -n systemctl daemon-reload; exit $s',
                'deb.leftovers': 'for p in /usr/bin/prysm-headless /usr/lib/systemd/system/prysm-headless.service '
                '/usr/share/doc/prysm-headless '
                '/etc/systemd/system/prysm-headless.service /etc/systemd/system/prysm-headless.service.d '
                '/etc/systemd/system/multi-user.target.wants/prysm-headless.service /var/lib/prysm '
                '/var/lib/private/prysm /run/systemd/system/prysm-headless.service.d; do '
                'if sudo -n test -e "$p" || sudo -n test -L "$p"; then echo "$p"; fi; done; '
                "getent passwd prysm-headless > /dev/null && echo 'user prysm-headless'; "
                "getent group prysm-headless > /dev/null && echo 'group prysm-headless'; "
                "dpkg-query -W -f='${db:Status-Status}\\n' prysm-headless 2>/dev/null; true",
                'deb.deliver': f'mkdir -p $HOME/.prysm-acceptance && mkdir -m 700 {DIRECTORY} && '
                f'cat > {DIRECTORY}/prysm-headless.deb && cd {DIRECTORY} && pwd -P && date +%s',
                'image.docker': "docker info --format '{{.ServerVersion}}' && docker compose version --short",
                'image.load': f"docker load -i {DIRECTORY}/prysm-headless.tar && docker image inspect -f '{{{{.Id}}}}' "
                'prysm-headless:local',
                'image.present': "docker image inspect -f '{{.Id}}' prysm-headless:local 2>/dev/null; "
                f'docker ps -aq --filter label=com.docker.compose.project={project}; '
                f'docker volume ls -q --filter label=com.docker.compose.project={project}; true',
                'image.remove': "if docker image inspect -f '{{.Id}}' prysm-headless:local > /dev/null 2>&1; "
                'then docker image rm prysm-headless:local; else echo gone; fi',
                'image.device': f'if [ -e {DEVICE} ]; then stat -L -c %g {DEVICE}; else echo absent; fi',
                'image.deliver': f'mkdir -p $HOME/.prysm-acceptance && mkdir -m 700 {DIRECTORY} && '
                f'cat > {DIRECTORY}/prysm-headless.tar && cd {DIRECTORY} && pwd -P && date +%s',
                'image.compose seed': compose + 'run --rm -T --no-deps --entrypoint sh prysm -c '
                "'cat > /data/settings.json' < settings.json",
                'image.compose up': compose + 'up -d --pull never --no-build',
                'image.compose state': compose + "ps -a --format '{{.State}} {{.ExitCode}}' prysm",
                'image.compose stop': compose + 'stop prysm',
                'image.compose recreate': compose + 'up -d --pull never --no-build --force-recreate',
                'image.compose logs': compose + 'logs --no-color --timestamps prysm',
                'image.compose down': compose + 'down --volumes --remove-orphans',
            },
        )
        self.assertEqual(found['deb.deliver'].stdin_file, 'dist/prysm-headless-linux-arm64.deb')
        self.assertEqual(commands.compose_project('RUN-CELL'), 'prysm-acceptance-run-cell')
        build = commands.xtask_dist(['deb-arm64', 'image-arm64'])
        self.assertEqual(
            (build.argv, build.cwd), (('cargo', 'xtask', 'dist', 'deb-arm64', 'image-arm64'), str(commands.ROOT))
        )

    def test_parameters_are_checked(self) -> None:
        refused = [
            lambda: commands.deb_install(TARGET, '../x'),
            lambda: commands.deb_dropin(TARGET, '0.0.0.0:18787 --start'),
            lambda: commands.deb_dropin(TARGET, "0.0.0.0:1'; reboot"),
            lambda: commands.deb_systemctl(TARGET, cast('UnitAction', 'disable')),
            lambda: commands.deb_journal(TARGET, cast('int', '@0; reboot')),
            lambda: commands.deb_deliver(TARGET, RUN, '-x.deb'),
            lambda: commands.image_compose(TARGET, RUN, cast('ComposeAction', 'down')),
            lambda: commands.image_compose(TARGET, '$(reboot)', ComposeAction.UP),
            lambda: commands.image_remove('-oProxyCommand=sh'),
            lambda: commands.image_device(TARGET, '/dev/../etc/shadow'),
            lambda: host_image.override('/dev/video0"]\n    privileged: true #', '44'),
            lambda: commands.xtask_dist([]),
            lambda: commands.xtask_dist(['deb-arm64', '--release']),
            lambda: commands.xtask_dist(['deb-arm64; rm -rf /']),
        ]
        for build in refused:
            with self.assertRaises(commands.Refused):
                build()

    def test_what_the_package_and_image_templates_may_change(self) -> None:
        """Outside the run directory only: apt's own files (install, purge), the runtime drop-in, the unit's
        state (systemctl), and Docker's own storage (load, compose, rm). sudo only in deb.*, as `sudo -n`."""
        found = {name: factory() for name, factory in commands.TEMPLATES.items() if name.startswith(('deb.', 'image.'))}
        self.assertEqual(len(found), 18)
        allowed_writes = r'^(/dev/null|/data/settings\.json|\$HOME/\.prysm-acceptance(/RUN-CELL(/[\w.-]+)?)?)$'
        for name, command in found.items():
            with self.subTest(name):
                remote = command.remote or ''
                self.assertEqual(command.argv[0], 'ssh')
                self.assertIn('BatchMode=yes', command.argv)
                self.assertIsNone(re.search(r'/etc/(?!systemd/system/)|\bdd\b|\bmount\b|\breboot\b|prune', remote))
                if name.startswith('image.'):
                    self.assertNotIn('sudo', remote)
                for sudo in re.findall(r'sudo\s+(\S+)', remote):
                    self.assertEqual(sudo, '-n', remote)
                pattern = r"(?:[^2]>|rm -rf|rm -f|rmdir|tee|mkdir -p|mkdir -m 700)\s+([^\s;'\"]+)"
                for written in re.findall(pattern, remote):
                    if not written.startswith(commands.DEB_DROPIN_DIR):  # the runtime drop-in under /run
                        self.assertRegex(written, allowed_writes, remote)
                if 'apt-get' in remote:
                    self.assertRegex(
                        remote,
                        r'apt-get (install -y --no-install-recommends \./prysm-headless\.deb|purge -y prysm-headless)',
                    )
                if re.search(r'docker (image )?rm\b', remote):
                    self.assertIn(f'docker image rm {commands.IMAGE_TAG}', remote)
                if 'docker compose -' in remote:
                    self.assertIn('-p prysm-acceptance-run-cell', remote)


class Packaging(unittest.TestCase):
    """What the seams take from packaging/ (the runner copies these values), read from the files themselves."""

    def test_the_unit_and_package(self) -> None:
        unit = (commands.ROOT / 'packaging/debian/prysm-headless.service').read_text()
        start = re.search(r'^ExecStart=(.+)$', unit, re.MULTILINE)
        # The drop-in runs the unit's own command with --listen and --metrics added.
        self.assertEqual(start and start[1], f'/usr/bin/prysm-headless {commands.DEB_SETTINGS}')
        self.assertIn('StateDirectory=prysm', unit)
        build = (commands.ROOT / 'packaging/debian/build-deb.sh').read_text()
        self.assertIn(f'package={commands.DEB_PACKAGE}', build)
        for path in ('/usr/bin/', '/usr/lib/systemd/system/', '/usr/share/doc/'):
            self.assertIn(f'"$root{path}', build.replace('$package', 'prysm-headless'))

    def test_the_compose_file_and_image(self) -> None:
        compose = host_image.COMPOSE_FILE.read_text()
        self.assertIn(f'image: {commands.IMAGE_TAG}', compose)
        self.assertIn(f'"{commands.IMAGE_PORT}:8787"', compose)
        self.assertIn(f'- {host_image.CONTAINER_DEVICE}:{host_image.CONTAINER_DEVICE}', compose)
        self.assertRegex(compose, r'(?m)^  prysm:$')
        self.assertIn('prysm-data:/data', compose)
        self.assertIn('stop_grace_period: 30s', compose)
        xtask = (commands.ROOT / 'crates/tools/xtask/src/dist/docker.rs').read_text()
        self.assertIn(f'const IMAGE: &str = "{commands.IMAGE_TAG}";', xtask)

    def test_the_dist_files_and_targets(self) -> None:
        dist = (commands.ROOT / 'crates/tools/xtask/src/dist.rs').read_text()
        files = {file: name for table in prepare.TARGETS.values() for file, name in table.items()}
        for file in files:
            self.assertIn(f'"{re.sub(r"(arm64|amd64)", "{}", file)}"', dist, file)
        self.assertEqual(set(files.values()) | {'tv-full', 'tv-slim'}, set(commands.XTASK_TARGETS))


class Archives(unittest.TestCase):
    def setUp(self) -> None:
        self.directory = temporary(self)

    def test_the_control_file_of_a_package(self) -> None:
        fields = host_deb.deb_control(write_deb(self.directory / 'p.deb'))
        self.assertEqual(
            (fields['Package'], fields['Version'], fields['Architecture']), ('prysm-headless', VERSION, 'arm64')
        )
        (self.directory / 'x.deb').write_bytes(b'not a package')
        self.assertEqual(host_deb.deb_control(self.directory / 'x.deb'), {})
        self.assertEqual(host_deb.deb_control(self.directory / 'missing.deb'), {})

    def test_the_manifest_of_an_image(self) -> None:
        self.assertEqual(
            host_image.image_archive(write_image(self.directory / 'i.tar')), ('arm64', ['prysm-headless:local'])
        )
        full = write_image(self.directory / 'f.tar', 'amd64', 'docker.io/library/prysm-headless:local')
        self.assertEqual(host_image.image_archive(full), ('amd64', ['prysm-headless:local']))
        (self.directory / 'x.tar').write_bytes(b'nothing')
        self.assertEqual(host_image.image_archive(self.directory / 'x.tar'), ('', []))

    def test_the_compose_override(self) -> None:
        self.assertEqual(
            host_image.override(DEVICE, '44'),
            '# Written by the acceptance runner for one run, over packaging/docker/compose.yaml.\n'
            'services:\n  prysm:\n    restart: "no"\n    command: ["--metrics"]\n'
            f'    devices: !override ["{DEVICE}:/dev/video0"]\n    group_add: !override ["44"]\n',
        )
        self.assertEqual(host_image.compose_version('v2.26.1\n'), (2, 26, 1))
        self.assertLess(host_image.compose_version('2.20.2'), host_image.COMPOSE_OVERRIDE)


class SeamTest(unittest.TestCase):
    """A seam's run directory here, the cell's, with a fake Prysm on loopback."""

    cell = ''

    def setUp(self) -> None:
        self.out_dir = temporary(self) / '20261117-101500-host' / self.cell
        self.out_dir.mkdir(parents=True)
        self.prysm = FakePrysm()
        self.addCleanup(self.prysm.close)

    def all_run(self, recorded: transport.Transcript) -> None:
        self.assertEqual([template for template, _ in recorded.entries], [], 'commands expected but not run')


class Deb(SeamTest):
    cell = 'linux-deb-device'

    def setUp(self) -> None:
        super().setUp()
        self.deb = write_deb(self.out_dir.parent / 'prysm-headless-linux-arm64.deb')

    def seam(self, *entries: tuple[str, Result]) -> tuple[host_deb.SshDeb, transport.Transcript]:
        recorded = transport.Transcript(entries)
        seam = host_deb.SshDeb(recorded, TARGET, self.deb, self.prysm.listen, self.out_dir, '192.0.2.40')
        seam.version = VERSION
        return seam, recorded

    def preflight(self, *entries: tuple[str, Result]) -> str | None:
        seam, recorded = self.seam(*entries)
        reason = seam.preflight(Source.DEVICE, DEVICE)
        self.all_run(recorded)
        return reason

    def test_a_clean_bench_is_ready(self) -> None:
        reason = self.preflight(
            ('linux.probe', ok('Linux aarch64\n')),
            ('deb.sudo', ok()),
            ('deb.leftovers', ok('not-installed\n')),
            ('linux.device', ok('/dev/video0\n')),
            ('linux.port-free', ok()),
        )
        self.assertIsNone(reason)

    def test_refusals(self) -> None:
        self.assertEqual(
            self.preflight(('linux.probe', ok('Linux x86_64\n'))),
            'prysm-headless-linux-arm64.deb is for arm64; pi@pi.example is x86_64',
        )
        self.assertIn(
            'needs passwordless sudo',
            self.preflight(
                ('linux.probe', ok('Linux aarch64\n')), ('deb.sudo', Result(1, '', 'a password is required'))
            )
            or '',
        )
        reason = self.preflight(
            ('linux.probe', ok('Linux aarch64\n')),
            ('deb.sudo', ok()),
            ('deb.leftovers', ok('/usr/bin/prysm-headless\n/var/lib/prysm\nuser prysm-headless\ninstalled\n')),
        )
        self.assertEqual(
            reason,
            'Prysm is on the linux target: /usr/bin/prysm-headless, /var/lib/prysm, user prysm-headless, installed. '
            'Nothing was changed. Remove it there, or set [linux] installed = "replace" to back up its settings and '
            'uninstall it (it is not reinstalled)',
        )
        self.deb.write_bytes(b'not a package')
        self.assertIn('no prysm-headless control file', self.preflight() or '')

    def test_replace_leaves_an_installed_package_for_the_cell(self) -> None:
        recorded = transport.Transcript(
            [
                ('linux.probe', ok('Linux aarch64\n')),
                ('deb.sudo', ok()),
                ('deb.leftovers', ok('/usr/bin/prysm-headless\ninstalled\n')),
                ('linux.device', ok('/dev/video0\n')),
                ('linux.port-free', ok()),
            ]
        )
        seam = host_deb.SshDeb(
            recorded, TARGET, self.deb, self.prysm.listen, self.out_dir, '192.0.2.40', config.Installed.REPLACE
        )
        self.assertIsNone(seam.preflight(Source.DEVICE, DEVICE))
        self.assertEqual(seam.found, ['/usr/bin/prysm-headless', 'installed'])
        self.all_run(recorded)

    def test_install_start_stop_close(self) -> None:
        seam, recorded = self.seam(
            ('deb.deliver', DELIVERED),
            ('deb.install', ok('Setting up prysm-headless\n')),
            ('deb.installed', ok(f'installed {VERSION}\n')),
            ('deb.dropin', ok()),
            ('deb.systemctl', ok()),
            ('deb.unit', ok('ActiveState=activating\n')),
            ('deb.unit', ok('ActiveState=active\n')),
            ('deb.main-user', ok('prysm-headless\n')),
            ('deb.systemctl', ok()),
            ('deb.unit', ok('ActiveState=inactive\nResult=success\nExecMainCode=1\nExecMainStatus=0\n')),
            ('deb.journal', ok('prysm started\n')),
            ('linux.kernel', ok('')),
            ('deb.purge', ok()),
            ('deb.leftovers', ok('not-installed\n')),
            ('linux.remove', ok()),
        )
        self.assertIn(f'({VERSION})', seam.deliver())
        state = seam.start({'output': {'kind': 'wled'}, 'processing': {}})
        self.assertEqual(state['settings'], {'output': {'kind': 'wled'}, 'processing': {}})
        self.assertEqual(
            self.prysm.saved, [{'expected': 1, 'settings': {'output': {'kind': 'wled'}, 'processing': {}}}]
        )
        self.assertEqual(seam.main_user(), 'prysm-headless')
        self.assertEqual(seam.stop(), 0)
        self.assertEqual(seam.stop(), 0)  # twice: nothing more runs
        seam.close()
        seam.close()
        self.all_run(recorded)
        self.assertIn('Setting up', (self.out_dir / 'apt.log').read_text())
        self.assertIn('prysm started', (self.out_dir / 'journal.log').read_text())
        self.assertIn('restart', recorded.calls[4].remote or '')

    def test_a_kill_or_a_signal_is_no_clean_exit(self) -> None:
        for unit, expected in (
            ('Result=timeout\nExecMainCode=2\nExecMainStatus=9\n', 'killed by systemd'),
            ('Result=success\nExecMainCode=2\nExecMainStatus=15\n', 'ended by signal 15'),
            ('Result=exit-code\nExecMainCode=1\nExecMainStatus=1\n', '1'),
        ):
            seam, _ = self.seam(('deb.systemctl', ok()), ('deb.unit', ok(unit)))
            seam.started = True
            self.assertIn(expected, str(seam.stop()))

    def test_a_failed_install_still_purges(self) -> None:
        for install, installed, error in (
            (Result(100, '', 'E: dependency problems'), [], 'apt-get install exited 100'),
            (ok(), [('deb.installed', ok('installed 0.9.0-1\n'))], "dpkg reports 'installed 0.9.0-1'"),
        ):
            with self.subTest(error):
                seam, recorded = self.seam(
                    ('deb.deliver', DELIVERED),
                    ('deb.install', install),
                    *installed,
                    ('deb.journal', ok()),
                    ('linux.kernel', ok()),
                    ('deb.purge', ok()),
                    ('deb.leftovers', ok()),
                    ('linux.remove', ok()),
                )
                with self.assertRaisesRegex(RuntimeError, error):
                    seam.deliver()
                self.assertIsNone(seam.stop())  # never started: nothing runs
                seam.close()
                self.all_run(recorded)

    def test_a_failed_start_is_reported_and_cleaned_up(self) -> None:
        seam, recorded = self.seam(
            ('deb.deliver', DELIVERED),
            ('deb.install', ok()),
            ('deb.installed', ok(f'installed {VERSION}\n')),
            ('deb.dropin', ok()),
            ('deb.systemctl', ok()),
            ('deb.unit', ok('ActiveState=failed\nResult=exit-code\n')),
            ('deb.journal', ok('Error: bad settings\n')),
            ('linux.kernel', ok()),
            ('deb.systemctl', ok()),
            ('deb.unit', ok('ActiveState=failed\nResult=exit-code\nExecMainCode=1\nExecMainStatus=1\n')),
            ('deb.journal', ok()),
            ('linux.kernel', ok()),
            ('deb.purge', ok()),
            ('deb.leftovers', ok()),
            ('linux.remove', ok()),
        )
        seam.deliver()
        with self.assertRaisesRegex(RuntimeError, 'failed on pi@pi.example'):
            seam.start({'processing': {}})
        self.assertIn('bad settings', (self.out_dir / 'journal.log').read_text())
        self.assertEqual(seam.stop(), 1)
        seam.close()
        self.all_run(recorded)

    def test_what_the_purge_leaves_fails_the_close_and_the_directory_still_goes(self) -> None:
        seam, recorded = self.seam(
            ('deb.journal', ok()),
            ('linux.kernel', ok()),
            ('deb.purge', ok()),
            ('deb.leftovers', ok('/var/lib/private/prysm\nuser prysm-headless\ngroup prysm-headless\nnot-installed\n')),
            ('linux.remove', ok()),
        )
        seam.delivered, seam.since = True, 1763380000
        with self.assertRaisesRegex(
            RuntimeError, 'left on pi@pi.example: /var/lib/private/prysm, user prysm-headless, group prysm-headless$'
        ):
            seam.close()
        self.all_run(recorded)


class Image(SeamTest):
    cell = 'linux-image-device'

    def setUp(self) -> None:
        super().setUp()
        self.image = write_image(self.out_dir.parent / 'prysm-headless-linux-arm64.tar')

    def seam(self, *entries: tuple[str, Result]) -> tuple[host_image.SshImage, transport.Transcript]:
        recorded = transport.Transcript(entries)
        seam = host_image.SshImage(recorded, TARGET, self.image, self.out_dir, '192.0.2.40')
        seam.url = f'http://{self.prysm.listen}'
        return seam, recorded

    def preflight(self, *entries: tuple[str, Result]) -> tuple[str | None, host_image.SshImage]:
        seam, recorded = self.seam(*entries)
        reason = seam.preflight(Source.DEVICE, DEVICE)
        self.all_run(recorded)
        return reason, seam

    def test_a_clean_bench_is_ready(self) -> None:
        reason, seam = self.preflight(
            ('linux.probe', ok('Linux aarch64\n')),
            ('image.docker', ok('26.1.5+dfsg1\n2.26.1\n')),
            ('image.present', ok('')),
            ('image.device', ok('44\n')),
            ('linux.port-free', ok()),
        )
        self.assertIsNone(reason)
        self.assertEqual(
            (seam.listed(DEVICE), seam.capture_path(DEVICE), seam.group), ('/dev/video0', '/dev/video0', '44')
        )

    def test_refusals(self) -> None:
        probe = ('linux.probe', ok('Linux aarch64\n'))
        docker = ('image.docker', ok('26.1.5\n2.26.1\n'))
        for entries, expected in (
            ([probe, ('image.docker', Result(1, '', 'permission denied ... docker.sock'))], 'docker info or docker'),
            ([probe, ('image.docker', ok('26.1.5\n2.20.2\n'))], "docker compose '2.20.2'"),
            ([probe, docker, ('image.present', ok(IMAGE_ID + '\n'))], 'prysm-headless:local or compose project'),
            ([probe, docker, ('image.present', ok()), ('image.device', ok('absent\n'))], f'device {DEVICE} is missing'),
            ([('linux.probe', ok('Linux x86_64\n'))], 'prysm-headless-linux-arm64.tar is for arm64; pi@pi.example'),
        ):
            with self.subTest(expected):
                self.assertIn(expected, self.preflight(*entries)[0] or '')
        write_image(self.image, tag='other:latest')
        self.assertIn('no prysm-headless:local image', self.preflight()[0] or '')

    def test_load_start_stop_close(self) -> None:
        seam, recorded = self.seam(
            ('image.deliver', DELIVERED),
            ('image.load', ok(f'Loaded image: prysm-headless:local\n{IMAGE_ID}\n')),
            ('linux.put', ok()),
            ('linux.put', ok()),
            ('linux.put', ok()),
            ('image.compose', ok()),
            ('image.compose', ok()),
            ('image.compose', ok('created 0\n')),
            ('image.compose', ok('running 0\n')),
            ('image.compose', ok()),
            ('image.compose', ok('exited 0\n')),
            ('image.compose', ok('2026-11-17T10:15:01Z prysm started\n')),
            ('linux.kernel', ok()),
            ('image.compose', ok()),
            ('image.remove', ok('Untagged: prysm-headless:local\n')),
            ('linux.remove', ok()),
        )
        seam.device, seam.group = DEVICE, '44'
        self.assertIn(IMAGE_ID, seam.deliver())
        state = seam.start({'processing': {}})
        self.assertEqual(state['target'], 'off')
        self.assertEqual(seam.stop(), 0)
        self.assertEqual(seam.stop(), 0)
        seam.close()
        seam.close()
        self.all_run(recorded)
        actions = [
            re.sub(r'.* -f compose.acceptance.yaml ', '', c.remote or '')
            for c in recorded.calls
            if c.template == 'image.compose'
        ]
        self.assertEqual(
            [action.split()[0] for action in actions], ['run', 'up', 'ps', 'ps', 'stop', 'ps', 'logs', 'down']
        )
        self.assertEqual(recorded.calls[2].stdin_file, str(host_image.COMPOSE_FILE))
        self.assertIn('!override', (self.out_dir / 'compose.acceptance.yaml').read_text())
        self.assertIn('prysm started', (self.out_dir / 'container.log').read_text())

    def test_a_kill_after_the_grace_period_is_no_clean_exit(self) -> None:
        seam, _ = self.seam(('image.compose', ok()), ('image.compose', ok('exited 137\n')))
        seam.composed = True
        self.assertEqual(seam.stop(), 'killed after no exit within the stop grace period (30 s)')

    def test_a_failed_start_is_reported_and_cleaned_up(self) -> None:
        seam, recorded = self.seam(
            ('image.deliver', DELIVERED),
            ('image.load', ok(f'{IMAGE_ID}\n')),
            ('linux.put', ok()),
            ('linux.put', ok()),
            ('linux.put', ok()),
            ('image.compose', ok()),
            ('image.compose', ok()),
            ('image.compose', ok('exited 1\n')),
            ('image.compose', ok('Error: no device\n')),
            ('linux.kernel', ok()),
            ('image.compose', ok()),
            ('image.compose', ok('exited 1\n')),
            ('image.compose', ok()),
            ('linux.kernel', ok()),
            ('image.compose', ok()),
            ('image.remove', ok()),
            ('linux.remove', ok()),
        )
        seam.device, seam.group = DEVICE, '44'
        seam.deliver()
        with self.assertRaisesRegex(RuntimeError, 'exited with code 1'):
            seam.start({'processing': {}})
        self.assertEqual(seam.stop(), 1)
        seam.close()
        self.all_run(recorded)

    def test_a_failed_load_still_removes_the_tag_and_the_run_directory(self) -> None:
        seam, recorded = self.seam(
            ('image.deliver', DELIVERED),
            ('image.load', Result(1, '', 'no space left on device')),
            ('linux.kernel', ok()),
            ('image.remove', ok('gone\n')),
            ('linux.remove', ok()),
        )
        with self.assertRaises(transport.CommandFailed):
            seam.deliver()
        self.assertIsNone(seam.stop())
        seam.close()
        self.all_run(recorded)


class Prepare(unittest.TestCase):
    def values(self, **linux: str) -> config.Config:
        return config.parse({'mac': {'binary': 'dist/prysm-headless-macos-arm64'}, 'linux': linux})

    def test_one_build_for_the_configured_dist_files(self) -> None:
        values = self.values(
            binary='dist/prysm-headless-linux-arm64',
            deb='dist/prysm-headless-linux-arm64.deb',
            image='dist/prysm-headless-linux-amd64.tar',
        )
        build = prepare.command(CELLS, values)
        self.assertEqual(
            build and build.argv, ('cargo', 'xtask', 'dist', 'mac', 'linux-arm64', 'deb-arm64', 'image-amd64')
        )
        deb_only = prepare.command([DEB_CELL], values)
        self.assertEqual(deb_only and deb_only.argv[3:], ('deb-arm64',))
        self.assertIsNone(prepare.command([DEB_CELL], self.values()))

    def test_another_path_is_refused(self) -> None:
        with self.assertRaisesRegex(prepare.PrepareError, r'dist/prysm-headless-linux-arm64\.deb'):
            prepare.command([DEB_CELL], self.values(deb='work/my.deb'))

    def test_a_failed_build_ends_the_run_before_anything_is_contacted(self) -> None:
        args = run.arguments(['host', 'linux', '--prepare', '--config', str(EXAMPLE)])
        failed = mock.Mock()
        failed.return_value.run.return_value = Result(101, '', 'error: could not compile')
        with mock.patch.object(run.transport, 'Real', failed), mock.patch.object(run.pytest, 'main') as tests:
            self.assertEqual(run.cells(args), run.PREPARATION_FAILED)
        tests.assert_not_called()
        self.assertEqual(failed.return_value.run.call_args.args[0].argv, ('cargo', 'xtask', 'dist', 'linux-arm64'))

    def test_the_dry_run_prints_the_build_and_runs_nothing(self) -> None:
        done = runner('all', '--prepare', '--dry-run', '--config', str(EXAMPLE), offline=True)
        self.assertEqual(done.returncode, 0, done.stdout[-2000:] + done.stderr)
        self.assertIn(
            '--prepare (dry run, not run): $ cargo xtask dist mac linux-arm64 deb-arm64 image-arm64', done.stdout
        )
        path = temporary(self) / 'config.toml'
        path.write_text('[linux]\ntarget = "ssh://pi@pi.example"\ndeb = "elsewhere/p.deb"\n')
        done = runner('host', 'linux', '--delivery', 'deb', '--prepare', '--dry-run', '--config', str(path))
        self.assertEqual(done.returncode, run.PREPARATION_FAILED, done.stdout + done.stderr)
        self.assertIn('--prepare builds only dist/', done.stderr)


class Selection(unittest.TestCase):
    def test_all_runs_every_cell_one_at_a_time(self) -> None:
        """Each cell in order, a delivery's own checks and remote commands only in its cell."""
        done = runner('all', '--dry-run', '--config', str(EXAMPLE), offline=True)
        self.assertEqual(done.returncode, 0, done.stdout[-2000:] + done.stderr)
        self.assertEqual(re.findall(r'^Cell (\S+)', done.stdout, re.MULTILINE), [cell.id for cell in CELLS])
        sections = dict(re.findall(r'^Cell (\S+)(.*?)(?=^Cell |\Z)', done.stdout, re.MULTILINE | re.DOTALL))
        deb, image = sections['linux-deb-device'], sections['linux-image-device']
        for text in (
            'test_package_user',
            'test_package_restart',
            'apt-get install -y --no-install-recommends',
            'apt-get purge -y',
        ):
            self.assertIn(text, deb)
        for text in ('test_image_volume', 'docker load -i', 'down --volumes --remove-orphans'):
            self.assertIn(text, image)
        for cell, text in sections.items():
            if cell != 'linux-deb-device':
                self.assertNotIn('test_package_', text, cell)
            if cell != 'linux-image-device':
                self.assertNotIn('test_image_volume', text, cell)

    def test_a_package_or_image_delivery_names_its_one_cell(self) -> None:
        args = run.arguments(['host', 'linux', '--delivery', 'deb'])
        self.assertEqual((args.delivery, args.source), (Delivery.DEB, Source.DEVICE))
        self.assertEqual(run.chosen(args), [DEB_CELL])
        self.assertIsNone(run.arguments(['host', 'linux']).source)
        with self.assertRaises(SystemExit):
            run.arguments(['host', 'linux', '--delivery', 'image', '--source', 'file'])
        with self.assertRaises(SystemExit):
            run.arguments(['host', 'mac', '--delivery', 'deb'])

    def test_a_named_cell_with_a_missing_artifact_fails(self) -> None:
        directory = temporary(self)
        (directory / 'config.toml').write_text(
            '[wled]\naddress = "127.0.0.1:9"\n[leds]\ntop = 1\n'
            '[linux]\ntarget = "ssh://pi@pi.example"\ndevice = "/dev/video0"\ndeb = "no/such.deb"\n'
        )
        named = runner(
            'host',
            'linux',
            '--delivery',
            'deb',
            '--config',
            str(directory / 'config.toml'),
            '--runs',
            str(directory / 'runs'),
            offline=True,
        )
        self.assertEqual(named.returncode, 1, named.stdout[-2000:])
        self.assertIn('linux-deb-device: no artifact: [linux] deb is', named.stdout)
        every = runner(
            'all', '--config', str(directory / 'config.toml'), '--runs', str(directory / 'runs2'), offline=True
        )
        self.assertEqual(every.returncode, 0, every.stdout[-2000:])
        self.assertIn('SKIPPED', every.stdout)

    def test_a_local_target_skips_the_package_and_image(self) -> None:
        values = config.parse({'linux': {'target': 'local', 'deb': 'dist/prysm-headless-linux-arm64.deb'}})
        reason = host_cells._local_only(DEB_CELL, values)  # pyright: ignore[reportPrivateUsage]
        self.assertIn('"local" is not implemented', reason or '')


if __name__ == '__main__':
    unittest.main()
