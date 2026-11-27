"""The acceptance runner's allow-list and its transport."""

import os
from pathlib import Path
import re
import shlex
import socket
import subprocess
import sys
import tempfile
import time
from typing import cast
import unittest
from unittest import mock

from acceptance_fakes import FakeColoursStream, sse

from prysm_acceptance import commands, prysm_api, transport, web

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


def confined(test: unittest.TestCase, command: commands.Command) -> None:
    """`command` runs on the TV over ssh and names, reads and writes only the allow-listed paths."""
    test.assertEqual(command.argv[0], 'ssh')
    test.assertEqual(command.argv[-1], command.remote)
    test.assertIn('BatchMode=yes', command.argv)
    if command.template == 'tv.reboot':
        test.assertEqual(command.remote, '/sbin/reboot')
        return
    test.assertIsNone(FORBIDDEN.search(without_uris(command.remote)), command.remote)
    for path in paths(command.remote):
        test.assertTrue(
            path.startswith(commands.TV_READ_PATHS + commands.TV_WRITE_PATHS), f'{path} in {command.remote}'
        )
    remote = command.remote or ''
    for written in re.findall(r'(?:>|rm -f|mkdir -p)\s+(\S+)', remote):
        if written.startswith('/'):
            test.assertIn(written, commands.TV_WRITE_PATHS, command.remote)
        else:
            test.assertRegex(written, r'^(/dev/null|"\$\w+")$')
    test.assertNotIn(
        commands.DATA_DIR + '/',
        re.sub(rf'(cat|-f) {re.escape(commands.DATA_DIR)}/\w+\.json', '', remote),
    )


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
            and name not in ('ssh_target', 'adb_serial', 'run_dir', 'v4l2_device', 'compose_project', 'field')
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
        # The `device.*` templates run on the TV too (and on a Linux target): the same rules hold for them.
        tv_templates = [
            factory() for name, factory in commands.TEMPLATES.items() if name.startswith(('tv.', 'device.'))
        ]
        self.assertTrue(tv_templates)
        for command in tv_templates:
            with self.subTest(command.template):
                confined(self, command)

    def test_luna_calls_only_the_listed_methods(self) -> None:
        for factory in commands.TEMPLATES.values():
            command = factory()
            for uri in re.findall(r'luna://\S+', command.remote or ''):
                self.assertRegex(
                    uri,
                    rf'^luna://({re.escape(commands.SERVICE)}/(getState|setPreferences|setEnabled|uninstall)|'
                    r'com\.webos\.appInstallService/dev/(install|remove)|'
                    r'com\.webos\.service\.tvpower/power/powerOff|'
                    r'com\.webos\.service\.applicationManager/(getForegroundAppInfo|launch|closeByAppId))$',
                )
            if command.remote and 'luna-send' in command.remote:
                self.assertTrue(command.hold_stdin, command.template)
                self.assertTrue(command.remote.startswith('timeout '), command.template)

    def test_the_application_manager_only_reads_or_names_prysms_app_or_the_home_screen(self) -> None:
        manager = 'luna://com.webos.service.applicationManager'
        self.assertEqual(
            {
                name: factory().remote
                for name, factory in commands.TEMPLATES.items()
                if 'applicationManager' in (factory().remote or '')
            },
            {
                'tv.foreground': f"timeout 12 luna-send -n 1 {manager}/getForegroundAppInfo '{{}}'",
                'tv.launch-app': f'timeout 12 luna-send -n 1 {manager}/launch \'{{"id":"{commands.APP}"}}\'',
                'tv.launch-home': f'timeout 12 luna-send -n 1 {manager}/launch \'{{"id":"com.webos.app.home"}}\'',
                'tv.close-app': f'timeout 12 luna-send -n 1 {manager}/closeByAppId \'{{"id":"{commands.APP}"}}\'',
            },
        )

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
            lambda: commands.tv_remove_app('root@tv;reboot'),
            lambda: commands.wake_on_lan('00:00:5E:00:53:01; reboot', '192.0.2.10'),
            lambda: commands.wake_on_lan('00-00-5E-00-53-01', '192.0.2.10'),
            lambda: commands.wake_on_lan('00:00:5E:00:53:01', 'tv.example'),
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

    def test_the_wake_is_one_magic_packet(self) -> None:
        command = commands.wake_on_lan('00:00:5E:00:53:01', '192.0.2.10')
        self.assertEqual(
            (command.template, command.argv),
            ('local.wake-on-lan', ('wake-on-LAN magic packet for', '00:00:5E:00:53:01')),
        )
        self.assertEqual(
            command.datagram,
            commands.Datagram('192.0.2.10', '255.255.255.255', 9, b'\xff' * 6 + bytes.fromhex('00005E005301') * 16),
        )
        self.assertEqual(
            command.display(),
            '(UDP from 192.0.2.10 to 255.255.255.255:9) wake-on-LAN magic packet for 00:00:5E:00:53:01',
        )

    def test_prysm_headless_runs_with_its_metrics(self) -> None:
        command = commands.headless('dist/prysm-headless-macos-arm64', 'settings.json', '0.0.0.0:18787')
        self.assertEqual(
            command.argv,
            ('dist/prysm-headless-macos-arm64', 'settings.json', '--listen', '0.0.0.0:18787', '--metrics'),
        )
        self.assertTrue(command.background)

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


class Spawn(unittest.TestCase):
    def test_a_background_command_runs_in_a_session_of_its_own(self) -> None:
        # A Ctrl-C on the runner's terminal must not reach it: the runner's stop is its only signal.
        command = commands.Command('test', (sys.executable, '-c', 'import time; time.sleep(30)'), 0, background=True)
        process = transport.Real(log=lambda line: None).spawn(
            command, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL
        )
        try:
            self.assertEqual(os.getsid(process.pid), process.pid)
            self.assertNotEqual(os.getpgid(process.pid), os.getpgid(0))
        finally:
            process.kill()
            process.wait()


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
        with self.assertRaises(RuntimeError):
            transport.Refusing().run(commands.wake_on_lan('00:00:5E:00:53:01', '192.0.2.10'))

    def test_a_datagram_is_sent_and_nothing_started(self) -> None:
        sockets = mock.MagicMock()
        with (
            mock.patch.object(transport.socket, 'socket', sockets),
            mock.patch.object(transport.subprocess, 'Popen') as popen,
        ):
            result = transport.Real(log=lambda line: None).run(commands.wake_on_lan('00:00:5E:00:53:01', '192.0.2.10'))
        self.assertTrue(result.ok)
        popen.assert_not_called()
        sockets.assert_called_once_with(socket.AF_INET, socket.SOCK_DGRAM)
        sender = sockets.return_value.__enter__.return_value
        sender.bind.assert_called_once_with(('192.0.2.10', 0))
        sender.sendto.assert_called_once_with(b'\xff' * 6 + bytes.fromhex('00005E005301') * 16, ('255.255.255.255', 9))


COLOURS = {'type': 'colors', 'top': [[255, 0, 0]]}


def read_here(url: str, seconds: str) -> transport.Result:
    """`commands.COLOURS_READER` run with this Python against `url`, as `device.colours` runs it on a device (the
    device's `timeout` aside)."""
    command = commands.Command('device.colours', (sys.executable, '-c', commands.COLOURS_READER, url, seconds), 20)
    return transport.Real(log=lambda line: None).run(command, check=False)


class ColoursOnTheDevice(unittest.TestCase):
    def test_the_template_is_bounded_on_the_device_and_here(self) -> None:
        command = commands.device_colours('pi@pi.example', 18787, 10.0)
        remote = command.remote or ''
        self.assertTrue(remote.startswith('timeout 20 python3 -c '), remote)
        self.assertTrue(remote.endswith(' http://127.0.0.1:18787/api/v1/colors/stream 10'), remote)
        self.assertIn(shlex.quote(commands.COLOURS_READER), remote)
        self.assertNotIn("'", commands.COLOURS_READER)  # quoted once for the shell, as it is
        self.assertGreater(command.timeout, 20)  # the device's `timeout` ends it first
        on_one = commands.device_colours('root@tv.example', 8787, 2.5, '192.0.2.30').remote or ''
        self.assertIn(' http://192.0.2.30:8787/', on_one)
        self.assertTrue((commands.device_colours('root@tv.example', 8787, 2.5).remote or '').endswith(' 2.5'))
        for build in (
            lambda: commands.device_colours('pi@pi.example', 18787, 0),
            lambda: commands.device_colours('pi@pi.example', 18787, 121),
            lambda: commands.device_colours('pi@pi.example', 18787, True),
            lambda: commands.device_colours('pi@pi.example', 18787, cast('float', '10; reboot')),
            lambda: commands.device_colours('pi@pi.example', 0, 10),
            lambda: commands.device_colours('pi@pi.example', 18787, 10, 'localhost'),
        ):
            with self.assertRaises(commands.Refused):
                build()

    def test_it_reads_the_window_and_stops(self) -> None:
        stream = FakeColoursStream([(0.1, sse(COLOURS)), (0.4, sse({'type': 'colors'}))])
        self.addCleanup(stream.close)
        start = time.monotonic()
        result = read_here(stream.url, '1')
        self.assertLess(time.monotonic() - start, 3)  # the window, not the stream held open, ends it
        self.assertEqual(result.returncode, 0, result.stderr)
        samples = prysm_api.timed_lines(result.stdout)
        self.assertEqual([event for _, event in samples], [COLOURS, {'type': 'colors'}])
        times = [at for at, _ in samples]
        self.assertEqual(times, sorted(times))
        self.assertTrue(0.1 <= times[0] < times[1] < 1.0, times)

    def test_the_remote_text_runs_in_a_shell_as_quoted(self) -> None:
        stream = FakeColoursStream([(0.1, sse(COLOURS))])
        self.addCleanup(stream.close)
        port = int(stream.url.split(':')[2].split('/')[0])
        remote = (commands.device_colours('pi@pi.example', port, 1).remote or '').replace('python3', sys.executable)
        # `timeout` stood in for (macOS has none): the rest is the remote text as the target's shell gets it.
        command = commands.Command('device.colours', ('sh', '-c', 'timeout() { shift; "$@"; }; ' + remote), 20)
        result = transport.Real(log=lambda line: None).run(command, check=False)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual([event for _, event in prysm_api.timed_lines(result.stdout)], [COLOURS])

    def test_an_event_split_across_two_chunks_is_one_event(self) -> None:
        event = sse(COLOURS)
        stream = FakeColoursStream([(0.1, event[:12]), (0.3, event[12:]), (0.4, sse({'type': 'colors'}))])
        self.addCleanup(stream.close)
        result = read_here(stream.url, '1')
        self.assertEqual(result.returncode, 0, result.stderr)
        samples = prysm_api.timed_lines(result.stdout)
        self.assertEqual([event for _, event in samples], [COLOURS, {'type': 'colors'}])
        self.assertGreaterEqual(samples[0][0], 0.3)  # timed when it is whole

    def test_a_body_that_ends_early_fails(self) -> None:
        # The last chunk ends the body while the connection stays open: the framing, not a close, says so.
        stream = FakeColoursStream([(0.0, sse(COLOURS))], final=True)
        self.addCleanup(stream.close)
        start = time.monotonic()
        result = read_here(stream.url, '5')
        self.assertLess(time.monotonic() - start, 3)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('the stream ended', result.stderr)
        self.assertEqual([event for _, event in prysm_api.timed_lines(result.stdout)], [COLOURS])

    def test_the_local_reader_reassembles_a_split_event_too(self) -> None:
        event = sse(COLOURS)
        stream = FakeColoursStream([(0.1, event[:12]), (0.3, event[12:])])
        self.addCleanup(stream.close)
        self.assertEqual([found for _, found in web.events(stream.url, 1, timed=True)], [COLOURS])

    def test_the_reader_tools_are_found_or_named_missing(self) -> None:
        directory = Path(tempfile.mkdtemp())
        (directory / 'python3').symlink_to(sys.executable)
        remote = commands.device_reader_tools('pi@pi.example').remote or ''
        command = commands.Command('device.reader-tools', ('/bin/sh', '-c', remote), 10, env={'PATH': str(directory)})
        result = transport.Real(log=lambda line: None).run(command)
        self.assertEqual(result.stdout, f'{directory}/python3\nmissing timeout\n')

    def test_another_status_fails(self) -> None:
        stream = FakeColoursStream([], status=503)
        self.addCleanup(stream.close)
        result = read_here(stream.url, '1')
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('HTTP 503', result.stderr)

    def test_its_lines_are_read_as_timed_events(self) -> None:
        self.assertEqual(
            prysm_api.timed_lines('0.105 {"type": "colors"}\n2.110 {"type": "colors", "top": [[255, 0, 0]]}\n'),
            [(0.105, {'type': 'colors'}), (2.11, COLOURS)],
        )
        self.assertEqual(prysm_api.timed_lines(''), [])
        for text in ('0.1', 'soon {"type": "colors"}', '0.1 {"type"'):
            with self.subTest(text), self.assertRaises(ValueError):
                prysm_api.timed_lines(text)


if __name__ == '__main__':
    unittest.main()
