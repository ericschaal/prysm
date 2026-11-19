"""The host cells through the runner's own command line: the dry run, the skip-or-fail rule and, opt-in, a
real run of the Mac file cell against a prysm-headless and the fake WLED on loopback.

The loopback run needs a built prysm-headless, which `cargo xtask check scripts` does not build: give its
path in PRYSM_ACCEPTANCE_BINARY (`cargo build -p prysm-headless`, then
PRYSM_ACCEPTANCE_BINARY=target/debug/prysm-headless). It also needs FFmpeg 9+ and UDP port 4048 on
127.0.0.1 free (Prysm always sends DDP there), so it is not run in parallel with another bench.
"""

import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

from prysm_acceptance.fake_wled import FakeWled

ACCEPTANCE = Path(__file__).resolve().parents[1]
EXAMPLE = ACCEPTANCE / 'config.example.toml'
BINARY = os.environ.get('PRYSM_ACCEPTANCE_BINARY', '')

# Runs the runner's command line with every network connection and process start refused.
OFFLINE = """
import socket, subprocess, sys
def refused(*args, **kwargs):
    raise AssertionError('the dry run contacted or started something')
socket.socket.connect = refused
socket.socket.sendto = refused
socket.create_connection = refused
subprocess.Popen.__init__ = refused
from prysm_acceptance import run
sys.exit(run.main(sys.argv[1:]))
"""


def runner(*arguments: str, offline: bool = False, timeout: float = 120) -> subprocess.CompletedProcess[str]:
    program = ['-c', OFFLINE] if offline else ['-m', 'prysm_acceptance.run']
    return subprocess.run(
        [sys.executable, *program, *arguments], capture_output=True, text=True, timeout=timeout, check=False
    )


class DryRun(unittest.TestCase):
    def test_contacts_and_starts_nothing(self) -> None:
        for arguments in (
            ['host', 'mac'],
            ['host', 'mac', '--source', 'device', '--display-check'],
            ['host', 'linux'],
            ['tv', 'full', '--tv-is-free'],
            ['tv', 'slim', '--receiver', 'linux', '--tv-is-free'],
            ['eyes'],
        ):
            done = runner(*arguments, '--dry-run', '--config', str(EXAMPLE), offline=True)
            self.assertEqual(done.returncode, 0, done.stdout[-3000:] + done.stderr)

    def test_prints_each_cell_in_order(self) -> None:
        text = runner('host', 'mac', '--dry-run', '--config', str(EXAMPLE)).stdout
        file, device = text.index('Cell mac-binary-file'), text.index('Cell mac-binary-device')
        cell = text[file:device]
        order = [cell.index(f'  {name}: ') for name in ('artifact', 'wled', 'clip', 'running', 'test_wled_live')]
        self.assertEqual(order, sorted(order))
        self.assertIn('$ dist/prysm-headless-macos-arm64 RUN_DIR/CELL/headless-settings.json', cell)
        self.assertIn('GET /api/v1/candidates/capture/device', text[device:])
        self.assertNotIn('cargo', text)

    def test_prints_the_linux_cells_remote_commands(self) -> None:
        text = runner('host', 'linux', '--dry-run', '--config', str(EXAMPLE)).stdout
        file, device = text.index('Cell linux-binary-file'), text.index('Cell linux-binary-device')
        self.assertNotIn('Cell mac-', text)
        cell = text[file:device]
        order = [
            cell.index(f'  {name}: ') for name in ('artifact', 'target', 'wled', 'clip', 'running', 'test_sigterm')
        ]
        self.assertEqual(order, sorted(order))
        for remote in (
            "'uname -sm'",
            'ffmpeg -version',
            "'cat > $HOME/.prysm-acceptance/RUN_DIR-CELL/clip' < RUN_DIR/CELL/edges.mp4",
            '< dist/prysm-headless-linux-arm64',
            'RUST_LOG=warn,prysm=info nohup sh -c',
            'GET http://192.168.1.70:18787/api/v1/state',
            'journalctl -k',
            "'rm -rf $HOME/.prysm-acceptance/RUN_DIR-CELL;",
        ):
            self.assertIn(remote, cell)
        self.assertIn('readlink -f /dev/v4l/by-id/usb-EXAMPLE-video-index0', text[device:])

    def test_selection_is_pytests(self) -> None:
        text = runner('host', 'mac', '--source', 'file', '--dry-run', '--config', str(EXAMPLE), '-k', 'fps').stdout
        self.assertIn('test_fps', text)
        self.assertNotIn('test_colours', text)
        self.assertNotIn('mac-binary-device', text)

    def test_a_remote_mac_is_refused(self) -> None:
        config = Path(tempfile.mkdtemp()) / 'config.toml'
        config.write_text('[mac]\ntarget = "ssh://me@mac.example"\n')
        done = runner('host', 'mac', '--dry-run', '--config', str(config))
        self.assertEqual(done.returncode, 2)
        self.assertIn('a remote Mac is not implemented', done.stdout + done.stderr)


class MissingArtifact(unittest.TestCase):
    """A missing artifact skips a cell, or fails it when it was named; nothing else is touched."""

    def run_without_binary(self, *arguments: str) -> subprocess.CompletedProcess[str]:
        config = Path(tempfile.mkdtemp()) / 'config.toml'
        config.write_text(
            '[wled]\naddress = "127.0.0.1:9"\n[leds]\ntop = 1\n[mac]\nbinary = "no/such/prysm-headless"\n'
        )
        runs = str(config.parent / 'runs')
        return runner('host', 'mac', *arguments, '--config', str(config), '--runs', runs, offline=False)

    def test_skips_unless_named(self) -> None:
        done = self.run_without_binary()
        self.assertEqual(done.returncode, 0, done.stdout[-3000:])
        self.assertIn('no artifact', done.stdout)

    def test_fails_when_named(self) -> None:
        done = self.run_without_binary('--source', 'file')
        self.assertEqual(done.returncode, 1, done.stdout[-3000:])
        self.assertIn('mac-binary-file: no artifact', done.stdout)


class TargetPreflight(unittest.TestCase):
    """A target that cannot run the cell skips it, or fails it when it was named, before WLED is touched."""

    def run_with_foreign_binary(self, *arguments: str) -> subprocess.CompletedProcess[str]:
        directory = Path(tempfile.mkdtemp())
        binary = directory / 'prysm-headless-linux-other'
        binary.write_bytes(b'\x7fELF\x02\x01\x01' + bytes(11) + (0x1234).to_bytes(2, 'little') + bytes(44))
        config = directory / 'config.toml'
        config.write_text(
            '[wled]\naddress = "127.0.0.1:9"\n[leds]\ntop = 1\n'
            f'[linux]\ntarget = "local"\nbinary = "{binary}"\ndevice = "/dev/video0"\n'
        )
        return runner('host', 'linux', *arguments, '--config', str(config), '--runs', str(directory / 'runs'))

    def test_skips_unless_named(self) -> None:
        done = self.run_with_foreign_binary('-k', 'device')
        self.assertEqual(done.returncode, 0, done.stdout[-3000:])
        self.assertIn('linux-binary-device: prysm-headless-linux-other is', done.stdout)

    def test_fails_when_named(self) -> None:
        done = self.run_with_foreign_binary('--source', 'device')
        self.assertEqual(done.returncode, 1, done.stdout[-3000:])
        self.assertIn('linux-binary-device: prysm-headless-linux-other is', done.stdout)


@unittest.skipUnless(BINARY, 'set PRYSM_ACCEPTANCE_BINARY to a built prysm-headless (see the module docstring)')
class Loopback(unittest.TestCase):
    def test_the_mac_file_cell_passes_and_leaves_wled_as_found(self) -> None:
        wled = FakeWled(http=('127.0.0.1', 0), ddp=('127.0.0.1', 4048)).start()
        self.addCleanup(wled.close)
        before = wled.si()['state']
        directory = Path(tempfile.mkdtemp())
        config = directory / 'config.toml'
        config.write_text(
            f'[wled]\naddress = "{wled.http_address}"\n[leds]\ntop = 30\nright = 18\nbottom = 30\nleft = 18\n'
            f'[checks]\nseconds = 3\n[mac]\nbinary = "{Path(BINARY).resolve()}"\n'
        )
        done = runner('host', 'mac', '--source', 'file', '--config', str(config), '--runs', str(directory), timeout=600)
        self.assertEqual(done.returncode, 0, done.stdout[-5000:] + done.stderr)
        self.assertIn('6 passed', done.stdout)
        self.assertEqual(wled.si()['state'], before)
        self.assertEqual(len(list(directory.glob('*-host/report.xml'))), 1)


if __name__ == '__main__':
    unittest.main()
