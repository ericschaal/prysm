import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest


class BuildTests(unittest.TestCase):
    def test_hardware_feature_and_pkg_config_are_targeted(self):
        script = Path(__file__).parents[1] / 'build-ipk.sh'
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            sdk = root / 'sdk'
            sdk.mkdir()
            output = root / 'build.json'
            cargo = root / 'cargo'
            cargo.write_text('#!/usr/bin/env python3\n'
                             'import json, os, sys\n'
                             'with open(os.environ["BUILD_OUTPUT"], "w") as f:\n'
                             '    json.dump([sys.argv[1:], dict(os.environ)], f)\n'
                             'sys.exit(77)\n')
            cargo.chmod(0o755)
            packager = root / 'ares-package'
            packager.write_text('#!/bin/sh\nexit 0\n')
            packager.chmod(0o755)
            env = dict(os.environ, PATH=f'{root}{os.pathsep}{os.environ["PATH"]}',
                       WEBOS_SDK=str(sdk), BUILD_OUTPUT=str(output))
            hardware = 'prysm-webos-capture/hardware-h264,prysm-webos-service/hardware-h264'
            for flags in ([], ['--embedded-prysm']):
                with self.subTest(flags=flags):
                    result = subprocess.run(['bash', str(script), *flags], env=env,
                                            capture_output=True, text=True)
                    self.assertEqual(result.returncode, 77, result.stderr)
                    args, actual_env = json.loads(output.read_text())
                    self.assertIn('--no-default-features', args)
                    features = [args[i + 1] for i, arg in enumerate(args) if arg == '--features']
                    expected = [hardware]
                    if flags:
                        expected.append('prysm-webos-service/embedded-prysm')
                    self.assertEqual(features, expected)
                    target_dir = Path(args[args.index('--target-dir') + 1])
                    self.assertEqual(target_dir.name, 'embedded' if flags else 'target')
                    suffix = '_armv7_unknown_linux_gnueabi'
                    self.assertEqual(actual_env['PKG_CONFIG' + suffix], str(sdk / 'bin/pkg-config'))
                    self.assertEqual(actual_env['PKG_CONFIG_PATH' + suffix], '')
                    self.assertEqual(actual_env['PKG_CONFIG_SYSROOT_DIR' + suffix],
                                     str(sdk / 'arm-webos-linux-gnueabi/sysroot'))
            for flag in ('--hardware-h264', '--unknown'):
                with self.subTest(flag=flag):
                    result = subprocess.run(['bash', str(script), flag], env=env,
                                            capture_output=True, text=True)
                    self.assertEqual(result.returncode, 1)
                    self.assertIn('Usage:', result.stderr)


if __name__ == '__main__':
    unittest.main()
