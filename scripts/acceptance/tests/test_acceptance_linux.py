"""The Linux binary cell's seam over SSH, its templates and preflight, against a recorded transcript (no device)."""

import json
from pathlib import Path
import platform
import re
import tempfile
from typing import cast
import unittest

from acceptance_fakes import FakePrysm, ok, untyped

from prysm_acceptance import commands, config, feed_chromecast, host, host_cells, transport
from prysm_acceptance.commands import RunFile, Signal
from prysm_acceptance.options import Cell, Delivery, Platform, Source
from prysm_acceptance.transport import Result

TARGET = 'pi@pi.example'
RUN = '20261117-101500-host-linux-binary-device'
DIRECTORY = f'$HOME/.prysm-acceptance/{RUN}'
RESOLVED = f'/home/pi/.prysm-acceptance/{RUN}'


def elf(machine: int) -> bytes:
    """An ELF header (64-bit, little-endian) for `machine`, as far as `host.elf_machine` reads it."""
    return b'\x7fELF\x02\x01\x01' + bytes(11) + machine.to_bytes(2, 'little') + bytes(44)


class Templates(unittest.TestCase):
    def test_the_start_runs_detached_in_the_run_directory(self) -> None:
        command = commands.linux_start(TARGET, RUN, '0.0.0.0:18787')
        self.assertEqual(command.argv[:-1], commands.tv_reachable(TARGET).argv[:-1])
        self.assertEqual(
            command.remote,
            "RUST_LOG=warn,prysm=info nohup sh -c '"
            f'cd {DIRECTORY} || exit 1; ./prysm-headless settings.json --listen 0.0.0.0:18787 --metrics '
            "> headless.log 2>&1 < /dev/null & echo $! > headless.pid; wait $!; echo $? > exit-code' "
            '> /dev/null 2>&1 < /dev/null &',
        )
        self.assertFalse(command.background)  # ssh returns at once: a plain run

    def test_exact_command_lines(self) -> None:
        self.assertEqual(
            commands.linux_signal(TARGET, RUN, 4242, Signal.TERM).remote,
            f'if [ "$(readlink /proc/4242/exe)" = "$(readlink -f {DIRECTORY}/prysm-headless)" ]; '
            'then kill -TERM 4242 && echo sent; else echo gone; fi',
        )
        self.assertEqual(
            commands.linux_deliver(TARGET, RUN, 'dist/prysm-headless-linux-arm64').remote,
            f'mkdir -p $HOME/.prysm-acceptance && mkdir -m 700 {DIRECTORY} && cat > {DIRECTORY}/prysm-headless && '
            f'chmod 700 {DIRECTORY}/prysm-headless && cd {DIRECTORY} && pwd -P && date +%s',
        )
        self.assertEqual(commands.linux_put(TARGET, RUN, RunFile.CLIP, 'edges.mp4').remote, f'cat > {DIRECTORY}/clip')
        self.assertEqual(commands.linux_route(TARGET, '192.0.2.40').remote, 'ip -4 route get 192.0.2.40')
        self.assertEqual(
            commands.linux_kernel(TARGET, 1763380000).remote,
            'journalctl -k -q --no-pager -o short-iso --since @1763380000',
        )
        self.assertEqual(commands.linux_port_free(TARGET, 18787).remote, 'ss -Htln sport = :18787')
        self.assertEqual(
            commands.linux_remove(TARGET, RUN).remote,
            f'rm -rf {DIRECTORY}; rmdir $HOME/.prysm-acceptance 2>/dev/null; true',
        )

    def test_the_read_only_lines(self) -> None:
        sample = 'RUN-CELL'
        directory = f'$HOME/.prysm-acceptance/{sample}'
        self.assertEqual(
            {
                name: factory().remote
                for name, factory in commands.TEMPLATES.items()
                if name in ('linux.probe', 'linux.ffmpeg-version', 'linux.device', 'linux.status')
                or name in ('linux.wait-exit', 'linux.read-log')
            },
            {
                'linux.probe': 'uname -sm',
                'linux.ffmpeg-version': 'if command -v ffmpeg >/dev/null; then ffmpeg -version; else echo absent; fi',
                'linux.device': 'if [ -r /dev/v4l/by-id/usb-EXAMPLE-video-index0 ]; then '
                'readlink -f /dev/v4l/by-id/usb-EXAMPLE-video-index0; else echo unreadable; fi',
                'linux.status': f'p=$(cat {directory}/headless.pid 2>/dev/null); '
                f'if [ -s {directory}/exit-code ]; then echo "exited $(cat {directory}/exit-code)"; '
                f'elif [ -n "$p" ] && [ "$(readlink /proc/$p/exe)" = "$(readlink -f {directory}/prysm-headless)" ]; '
                'then echo "running $p"; else echo starting; fi',
                'linux.wait-exit': f'i=0; while [ ! -s {directory}/exit-code ] && [ $i -lt 30 ]; do sleep 1; '
                f'i=$((i+1)); done; if [ -s {directory}/exit-code ]; then cat {directory}/exit-code; '
                'else echo running; fi',
                'linux.read-log': f'if [ -f {directory}/headless.log ]; then cat {directory}/headless.log; fi',
            },
        )

    def test_parameters_are_checked(self) -> None:
        refused = [
            lambda: commands.linux_remove(TARGET, '../x'),
            lambda: commands.linux_remove(TARGET, 'run/x'),
            lambda: commands.linux_remove(TARGET, ''),
            lambda: commands.linux_remove(TARGET, '$(reboot)'),
            lambda: commands.linux_remove(TARGET, '-rf'),
            lambda: commands.linux_status('-oProxyCommand=sh', RUN),
            lambda: commands.linux_signal(TARGET, RUN, cast('int', '4242; reboot'), Signal.TERM),
            lambda: commands.linux_signal(TARGET, RUN, True, Signal.TERM),
            lambda: commands.linux_signal(TARGET, RUN, 1, Signal.TERM),
            lambda: commands.linux_signal(TARGET, RUN, 4242, cast('Signal', 'TERM')),
            lambda: commands.linux_put(TARGET, RUN, cast('RunFile', 'settings.json'), 'x.json'),
            lambda: commands.linux_put(TARGET, RUN, RunFile.SETTINGS, '-x.json'),
            lambda: commands.linux_device(TARGET, 'video0'),
            lambda: commands.linux_device(TARGET, '/dev/../etc/passwd'),
            lambda: commands.linux_device(TARGET, '/dev/video0; reboot'),
            lambda: commands.linux_device(TARGET, '/dev/v4l/by-id/$(reboot)'),
            lambda: commands.linux_route(TARGET, 'wled.example'),
            lambda: commands.linux_route(TARGET, '192.0.2.40;reboot'),
            lambda: commands.linux_port_free(TARGET, 0),
            lambda: commands.linux_kernel(TARGET, cast('int', '@0; reboot')),
            lambda: commands.linux_wait_exit(TARGET, RUN, 0),
            lambda: commands.linux_start(TARGET, RUN, '0.0.0.0:18787 --allow-origin x'),
        ]
        for build in refused:
            with self.assertRaises(commands.Refused):
                build()

    def test_linux_commands_stay_in_the_run_directory(self) -> None:
        found = [factory() for name, factory in commands.TEMPLATES.items() if name.startswith('linux.')]
        self.assertEqual(len(found), 14)
        for command in found:
            with self.subTest(command.template):
                remote = command.remote or ''
                self.assertEqual(command.argv[0], 'ssh')
                self.assertIn('BatchMode=yes', command.argv)
                self.assertIsNone(re.search(r'\bsudo\b|/etc/|\bdd\b|\bmount\b|\breboot\b', remote), remote)
                for written in re.findall(r"(?:>|rm -rf|mkdir -p|mkdir -m 700|chmod 700|rmdir)\s+([^\s;']+)", remote):
                    self.assertRegex(
                        written, r'^(/dev/null|\$HOME/\.prysm-acceptance(/RUN-CELL(/[\w.-]+)?)?|[\w.-]+)$', remote
                    )
                for path in re.findall(r'(?<![\w$*%#.])/[A-Za-z0-9_.*$/-]+', remote):
                    self.assertRegex(path, r'^(/dev/null|/proc/|/dev/v4l/by-id/)', remote)
                if 'kill ' in remote:
                    self.assertIn('readlink /proc/', remote)  # only the run's own binary


class Seam(unittest.TestCase):
    def setUp(self) -> None:
        self.out_dir = Path(tempfile.mkdtemp()) / '20261117-101500-host' / 'linux-binary-device'
        self.out_dir.mkdir(parents=True)
        self.binary = self.out_dir.parent / 'prysm-headless-linux-arm64'
        self.binary.write_bytes(elf(0xB7))
        self.prysm = FakePrysm()
        self.addCleanup(self.prysm.close)

    def seam(self, entries: list[tuple[str, Result]], listen: str = '') -> tuple[host.SshBinary, transport.Transcript]:
        recorded = transport.Transcript(entries)
        seam = host.SshBinary(recorded, TARGET, self.binary, listen or self.prysm.listen, self.out_dir, '192.0.2.40:80')
        return seam, recorded

    def remaining(self, recorded: transport.Transcript) -> None:
        self.assertEqual(recorded.entries, [], 'commands expected but not run')

    def test_url_and_run_name(self) -> None:
        seam, _ = self.seam([], listen='0.0.0.0:18787')
        self.assertEqual(seam.url, 'http://pi.example:18787')
        self.assertEqual(seam.run, RUN)

    def test_the_colours_are_read_and_timed_on_the_target(self) -> None:
        printed = '0.105 {"type": "colors"}\n2.104 {"type": "colors", "top": [[0, 255, 0]]}\n'
        seam, recorded = self.seam([('device.colours', ok(printed))], listen='0.0.0.0:18787')
        self.assertEqual(
            seam.colours(10.0),
            [(0.105, {'type': 'colors'}), (2.104, {'type': 'colors', 'top': [[0, 255, 0]]})],
        )
        self.remaining(recorded)
        self.assertEqual(recorded.calls[0].argv[-2], TARGET)
        self.assertTrue((recorded.calls[0].remote or '').endswith(' http://127.0.0.1:18787/api/v1/colors/stream 10'))
        # Prysm on one address of the target: read there, still on the target.
        seam, _ = self.seam([], listen='192.0.2.70:18787')
        self.assertIn(' http://192.0.2.70:18787/', seam.colours_command(10.0).remote or '')
        local = host.LocalBinary(transport.Refusing(), self.binary, '127.0.0.1:18787', self.out_dir, '192.0.2.40:80')
        self.assertIsNone(local.colours_command(10.0))

    def test_a_failed_read_on_the_target_fails(self) -> None:
        for result in (
            Result(1, '0.105 {"type": "colors"}\n', 'the stream ended 0.2 s after the start'),
            Result(124, ''),
        ):
            seam, _ = self.seam([('device.colours', result)])
            with self.subTest(result.returncode), self.assertRaisesRegex(transport.CommandFailed, 'device.colours'):
                seam.colours(10.0)
        seam, _ = self.seam([('device.colours', ok('Traceback (most recent call last):\n'))])
        with self.assertRaises(ValueError):
            seam.colours(10.0)

    def test_deliver_start_stop_close(self) -> None:
        seam, recorded = self.seam(
            [
                ('linux.deliver', ok(f'{RESOLVED}\n1763380000\n')),
                ('linux.put', ok()),
                ('linux.put', ok()),
                ('linux.start', ok()),
                ('linux.status', ok('starting\n')),
                ('linux.status', ok('running 4242\n')),
                ('linux.signal', ok('sent\n')),
                ('linux.wait-exit', ok('0\n')),
                ('linux.read-log', ok('INFO prysm started\nINFO prysm stopped\n')),
                (
                    'linux.kernel',
                    ok(
                        '2026-11-17T10:15:01 pi kernel: usb 1-1: reset high-speed USB device\n'
                        '2026-11-17T10:15:02 pi kernel: wlan0: something else\n'
                    ),
                ),
                ('linux.remove', ok()),
            ]
        )
        self.assertIn(RESOLVED, seam.deliver())
        self.assertEqual(seam.place(Path('edges.mp4')), f'{RESOLVED}/clip')
        state = seam.start(host.settings({'kind': 'wled'}))
        self.assertEqual(untyped(state)['target'], 'off')
        self.assertEqual(json.loads((self.out_dir / 'headless-settings.json').read_text())['output'], {'kind': 'wled'})
        self.assertEqual(seam.stop(), 0)
        self.assertEqual(seam.stop(), 0)  # twice: nothing more runs
        seam.close()
        seam.close()  # twice: nothing more runs
        self.remaining(recorded)
        self.assertEqual(untyped(recorded.calls[1].stdin_file), str(Path('edges.mp4')))
        self.assertIn('--since @1763380000', recorded.calls[9].remote or '')
        self.assertIn('prysm stopped', (self.out_dir / 'headless.log').read_text())
        self.assertEqual(
            (self.out_dir / 'kernel-usb.log').read_text(),
            '2026-11-17T10:15:01 pi kernel: usb 1-1: reset high-speed USB device\n',
        )

    def test_no_exit_within_the_time_is_killed(self) -> None:
        seam, recorded = self.seam(
            [
                ('linux.status', ok('running 4242\n')),
                ('linux.signal', ok('sent\n')),
                ('linux.wait-exit', ok('running\n')),
                ('linux.signal', ok('sent\n')),
                ('linux.wait-exit', ok('137\n')),
            ]
        )
        seam.delivered = True
        self.assertEqual(seam.stop(seconds=3), 'killed after no exit within 3 s')
        self.assertIn('kill -KILL 4242', recorded.calls[3].remote or '')
        self.remaining(recorded)

    def test_a_failed_start_is_reported_and_cleaned_up(self) -> None:
        seam, recorded = self.seam(
            [
                ('linux.deliver', ok(f'{RESOLVED}\n1763380000\n')),
                ('linux.put', ok()),
                ('linux.start', ok()),
                ('linux.status', ok('exited 1\n')),
                ('linux.read-log', ok('Error: settings rejected\n')),
                ('linux.kernel', Result(1, '', 'Failed to open journal')),
                ('linux.read-log', ok('Error: settings rejected\n')),
                ('linux.kernel', ok('')),
                ('linux.remove', ok()),
            ]
        )
        seam.deliver()
        with self.assertRaisesRegex(RuntimeError, 'exited 1'):
            seam.start(host.settings({'kind': 'wled'}))
        self.assertIn('journalctl -k failed', (self.out_dir / 'kernel-usb.log').read_text())
        self.assertEqual(seam.stop(), 1)  # known from the exit file: nothing runs
        seam.close()
        self.assertIn('no USB lines', (self.out_dir / 'kernel-usb.log').read_text())
        self.remaining(recorded)

    def test_a_failed_copy_still_removes_the_run_directory(self) -> None:
        seam, recorded = self.seam(
            [
                ('linux.deliver', Result(1, '', 'mkdir: cannot create directory')),
                ('linux.status', ok('starting\n')),
                ('linux.read-log', ok()),
                ('linux.remove', ok()),
            ]
        )
        with self.assertRaises(transport.CommandFailed):
            seam.deliver()
        self.assertIsNone(seam.stop())
        seam.close()
        self.remaining(recorded)

    def test_nothing_delivered_nothing_to_do(self) -> None:
        seam, recorded = self.seam([])
        self.assertIsNone(seam.stop())
        seam.close()
        self.assertEqual(recorded.calls, [])

    def test_the_sender_is_the_targets_source_on_the_route_to_wled(self) -> None:
        seam, recorded = self.seam([('linux.route', ok('192.0.2.40 dev wlan0 src 192.0.2.70 uid 1000\n    cache\n'))])
        self.assertEqual(seam.sender(), '192.0.2.70')
        self.assertEqual(recorded.calls[0].remote, 'ip -4 route get 192.0.2.40')


class Preflight(unittest.TestCase):
    def setUp(self) -> None:
        self.out_dir = Path(tempfile.mkdtemp()) / 'RUN' / 'CELL'
        self.binary = self.out_dir.parent / 'prysm-headless-linux-arm64'
        self.out_dir.mkdir(parents=True)
        self.binary.write_bytes(elf(0xB7))

    def preflight(self, source: Source, *entries: tuple[str, Result]) -> tuple[str | None, host.SshBinary]:
        recorded = transport.Transcript(entries)
        seam = host.SshBinary(recorded, TARGET, self.binary, '0.0.0.0:18787', self.out_dir, '192.0.2.40')
        reason = seam.preflight(source, '/dev/v4l/by-id/usb-EXAMPLE-video-index0')
        self.assertEqual(recorded.entries, [])
        return reason, seam

    def test_a_ready_device_target(self) -> None:
        reason, seam = self.preflight(
            Source.DEVICE,
            ('linux.probe', ok('Linux aarch64\n')),
            ('linux.device', ok('/dev/video0\n')),
            ('linux.port-free', ok()),
        )
        self.assertIsNone(reason)
        self.assertEqual(seam.listed('/dev/v4l/by-id/usb-EXAMPLE-video-index0'), '/dev/video0')

    def test_a_ready_file_target(self) -> None:
        reason, _ = self.preflight(
            Source.FILE,
            ('linux.probe', ok('Linux aarch64\n')),
            ('linux.ffmpeg-version', ok('ffmpeg version 9.0 Copyright (c) 2000-2026\nbuilt with gcc\n')),
            ('linux.port-free', ok()),
        )
        self.assertIsNone(reason)

    def test_unreachable(self) -> None:
        reason, _ = self.preflight(Source.DEVICE, ('linux.probe', Result(255, '', 'Permission denied (publickey).')))
        self.assertRegex(reason or '', r'ssh pi@pi.example failed .*Permission denied')

    def test_wrong_architecture_or_system(self) -> None:
        reason, _ = self.preflight(Source.DEVICE, ('linux.probe', ok('Linux x86_64\n')))
        self.assertEqual(reason, 'prysm-headless-linux-arm64 is built for aarch64; pi@pi.example is x86_64')
        reason, _ = self.preflight(Source.DEVICE, ('linux.probe', ok('Darwin arm64\n')))
        self.assertEqual(reason, 'pi@pi.example runs Darwin, not Linux')

    def test_old_or_missing_ffmpeg_skips_the_file_cell(self) -> None:
        reason, _ = self.preflight(
            Source.FILE,
            ('linux.probe', ok('Linux aarch64\n')),
            ('linux.ffmpeg-version', ok('ffmpeg version 7.1.1-1+b1 Copyright (c) 2000-2025\n')),
        )
        self.assertEqual(reason, 'the file source needs FFmpeg 9+ on the target; it has FFmpeg 7')
        reason, _ = self.preflight(
            Source.FILE, ('linux.probe', ok('Linux aarch64\n')), ('linux.ffmpeg-version', ok('absent\n'))
        )
        self.assertEqual(reason, "the file source needs FFmpeg 9+ on the target; found 'absent'")

    def test_missing_device_or_busy_port(self) -> None:
        reason, _ = self.preflight(
            Source.DEVICE, ('linux.probe', ok('Linux aarch64\n')), ('linux.device', ok('unreadable\n'))
        )
        self.assertIn('capture device /dev/v4l/by-id/usb-EXAMPLE-video-index0 is missing', reason or '')
        reason, _ = self.preflight(
            Source.DEVICE,
            ('linux.probe', ok('Linux aarch64\n')),
            ('linux.device', ok('/dev/video0\n')),
            ('linux.port-free', ok('LISTEN 0 128 0.0.0.0:18787 0.0.0.0:*\n')),
        )
        self.assertEqual(reason, 'port 18787 is already in use on pi@pi.example')

    def test_a_linux_binary_does_not_run_on_this_mac(self) -> None:
        seam = host.LocalBinary(transport.Refusing(), self.binary, '127.0.0.1:18787', self.out_dir, '192.0.2.40')
        reason = seam.preflight(Source.FILE, '')
        if platform.system() != 'Linux':
            self.assertEqual(
                reason, f'prysm-headless-linux-arm64 is a Linux binary; this computer runs {platform.system()}'
            )
        mac = self.out_dir / 'prysm-headless-macos-arm64'
        mac.write_bytes(b'\xcf\xfa\xed\xfe' + bytes(28))
        self.assertIsNone(
            host.LocalBinary(transport.Refusing(), mac, '127.0.0.1:1', self.out_dir, 'w').preflight(Source.FILE, '')
        )


class Configuration(unittest.TestCase):
    def test_an_ssh_target_listens_on_the_lan(self) -> None:
        values = config.parse(
            {'linux': {'target': 'ssh://pi@pi.example', 'device': '/dev/v4l/by-id/usb-X-video-index0'}}
        )
        self.assertEqual(values.linux.listen, '0.0.0.0:18787')
        self.assertEqual(config.parse({'linux': {'target': 'local'}}).linux.listen, '127.0.0.1:18787')
        given = config.parse({'linux': {'target': 'ssh://pi@pi.example', 'listen': '0.0.0.0:8787'}})
        self.assertEqual(given.linux.listen, '0.0.0.0:8787')

    def test_mistakes_are_refused(self) -> None:
        for linux in (
            {'target': 'pi@pi.example'},
            {'target': 'ssh://pi@pi.example;reboot'},
            {'target': 'ssh://'},
            {'target': 'ssh://pi@pi.example', 'listen': '127.0.0.1:18787'},
            {'device': 'video0'},
            {'device': '/dev/../etc/shadow'},
        ):
            with self.assertRaises(config.ConfigError, msg=linux):
                config.parse({'linux': linux})
        with self.assertRaisesRegex(config.ConfigError, 'a remote Mac is not implemented'):
            config.parse({'mac': {'target': 'ssh://me@mac.example'}})

    def test_the_linux_cells_use_the_linux_feed_and_seam(self) -> None:
        values = config.parse(
            {
                'mac': {'feed': {'chromecast': {'adb_serial': 'MAC0000001'}}},
                'linux': {
                    'target': 'ssh://pi@pi.example',
                    'binary': 'dist/prysm-headless-linux-arm64',
                    'device': '/dev/v4l/by-id/usb-X-video-index0',
                    'feed': {'chromecast': {'adb_serial': 'LINUX00001'}},
                },
            }
        )
        cell = Cell(Platform.LINUX, Delivery.BINARY, Source.DEVICE)
        feed = host_cells._feed(cell, values)  # pyright: ignore[reportPrivateUsage]
        self.assertIsInstance(feed, feed_chromecast.ChromecastCast)
        self.assertEqual(cast('feed_chromecast.ChromecastCast', feed).serial, 'LINUX00001')
        seam = host_cells._seam(cell, values, transport.Refusing(), Path('RUN/CELL'))  # pyright: ignore[reportPrivateUsage]
        self.assertIsInstance(seam, host.SshBinary)
        local = config.parse({'linux': {'target': 'local'}})
        seam = host_cells._seam(cell, local, transport.Refusing(), Path('RUN/CELL'))  # pyright: ignore[reportPrivateUsage]
        self.assertIsInstance(seam, host.LocalBinary)


if __name__ == '__main__':
    unittest.main()
