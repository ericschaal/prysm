"""The replacement package must retire the proxy before unpack removes its binary."""
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

PREINST = Path(__file__).resolve().parents[1] / 'preinst'


class UpgradeTest(unittest.TestCase):
    def run_preinst(self, action, fail=False):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            log = root / 'calls'
            for command in ('deb-systemd-invoke', 'deb-systemd-helper'):
                stub = root / command
                stub.write_text('#!/bin/sh\necho "${0##*/} $*" >> "$CALL_LOG"\n'
                                + ('exit 1\n' if fail else ''))
                stub.chmod(0o755)
            env = {**os.environ, 'PATH': directory, 'CALL_LOG': str(log)}
            subprocess.run(['/bin/sh', str(PREINST), action], env=env, check=True)
            return log.read_text().splitlines() if log.exists() else []

    def test_upgrade_stops_and_disables_only_the_removed_proxy(self):
        self.assertEqual(self.run_preinst('upgrade'), [
            'deb-systemd-invoke stop prysm-web.service',
            'deb-systemd-helper disable prysm-web.service',
        ])

    def test_fresh_install_and_abort_leave_services_alone(self):
        for action in ('install', 'abort-upgrade'):
            with self.subTest(action=action):
                self.assertEqual(self.run_preinst(action), [])

    def test_missing_or_unavailable_service_does_not_block_upgrade(self):
        self.assertEqual(len(self.run_preinst('upgrade', fail=True)), 2)
