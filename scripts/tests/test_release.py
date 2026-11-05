import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('release', Path(__file__).parents[1] / 'check-release.py')
release = importlib.util.module_from_spec(spec)
spec.loader.exec_module(release)


class ReleaseTests(unittest.TestCase):
    def test_tag_and_installer_versions_must_match(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for name in ('VERSION', 'apps/prysm/Cargo.toml', 'Cargo.lock',
                         'crates/web/prysm-webos/public/appinfo.json', '.release-please-manifest.json',
                         'compose.yaml', 'packaging/docker/compose.truenas.yaml'):
                target = root / name
                target.parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(release.ROOT / name, target)
            version = release.release_version(root)
            self.assertEqual(release.release_version(root, f'v{version}'), version)
            for tag in ('main', 'v9.9.9', f'v{version}-rc.1', f'v{version}\ninjected'):
                with self.subTest(tag=tag), self.assertRaises(ValueError):
                    release.release_version(root, tag)
            app = root / 'crates/web/prysm-webos/public/appinfo.json'
            app.write_text(json.dumps({'version': '0.1.43'}))
            with self.assertRaisesRegex(ValueError, 'mismatch'):
                release.release_version(root)

    def test_publish_and_recovery_preserve_versions(self):
        for published, newer, revision in ((False, False, 'commit'), (True, False, 'commit'),
                                            (True, True, 'commit'), (False, False, 'wrong')):
            with self.subTest(published=published, newer=newer, revision=revision), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                bin_dir = root / 'bin'
                bin_dir.mkdir()
                stub = bin_dir / 'tool'
                stub.write_text('''#!/usr/bin/env python3
import json, os
from pathlib import Path
import sys
tool, args = Path(sys.argv[0]).name, sys.argv[1:]
with open(os.environ['CALLS'], 'a') as log:
    log.write(json.dumps([tool, *args]) + '\\n')
if tool == 'dpkg-deb':
    print({'Package':'prysm-led', 'Architecture':'arm64', 'Version':'0.2.0-1'}[args[-1]])
elif tool == 'gh':
    assert os.environ.get('GH_REPO') == os.environ['GITHUB_REPOSITORY'], 'gh requires repository context outside the checkout'
    if args[:2] == ['release', 'view']:
        print(json.dumps({'isDraft': os.environ['ALREADY_PUBLISHED'] != 'true'}))
    elif args[0] == 'api':
        print('v0.1.0\\nv0.2.0')
        if os.environ['NEWER'] == 'true':
            print('v0.3.0')
elif tool == 'docker':
    if args[0] == 'pull' and ':' in args[-1] and '@' not in args[-1]:
        print('manifest unknown', file=sys.stderr)
        sys.exit(1)
    if args[:2] == ['image', 'inspect']:
        print(os.environ['REVISION'])
    if args[:3] == ['buildx', 'imagetools', 'inspect']:
        if '--raw' in args:
            print(json.dumps({'manifests': [
                {'platform': {'os':'linux', 'architecture': arch}, 'digest': 'sha256:' + arch}
                for arch in ('amd64', 'arm64')]}))
        elif '--format' in args:
            print(json.dumps({'digest':'sha256:index'}))
        else:
            print('manifest unknown', file=sys.stderr)
            sys.exit(1)
''')
                stub.chmod(0o755)
                for name in ('docker', 'gh', 'dpkg-deb'):
                    (bin_dir / name).symlink_to(stub)
                for name in ('prysm-led_0.2.0-1_arm64.deb', 'io.prysm.capture_0.2.0_all.ipk',
                             'prysm-amd64.tar', 'prysm-arm64.tar'):
                    (root / name).write_text(name)
                env = dict(os.environ, PATH=f'{bin_dir}{os.pathsep}{os.environ["PATH"]}',
                           GITHUB_REPOSITORY='ericschaal/prysm', RELEASE_DIRECTORY=str(root),
                           GITHUB_WORKSPACE=str(release.ROOT),
                           RELEASE_VERSION='0.2.0', RELEASE_TAG='v0.2.0', RELEASE_SHA='commit',
                           ALREADY_PUBLISHED=str(published).lower(), CALLS=str(root / 'calls'),
                           NEWER=str(newer).lower(), REVISION=revision)
                result = subprocess.run(['bash', str(release.ROOT / 'packaging/release/publish.sh')],
                                        env=env, capture_output=True, text=True)
                calls = [json.loads(line) for line in (root / 'calls').read_text().splitlines()]
                uploads = [call for call in calls if call[:3] == ['gh', 'release', 'upload']]
                loads = [call for call in calls if call[:3] == ['docker', 'image', 'load']]
                latest = [call for call in calls if 'ghcr.io/ericschaal/prysm:latest' in call]
                if revision == 'wrong':
                    self.assertNotEqual(result.returncode, 0)
                    self.assertFalse(uploads)
                    self.assertFalse(latest)
                else:
                    self.assertEqual(result.returncode, 0, result.stderr)
                    self.assertEqual(len(uploads), 0 if published else 1)
                    self.assertEqual(len(loads), 0 if published else 2)
                    self.assertEqual(len(latest), 0 if newer else 1)
                    self.assertEqual((root / 'docker-image.txt').read_text(),
                                     'ghcr.io/ericschaal/prysm@sha256:index\n')
                    if not published:
                        checksum = (root / 'SHA256SUMS').read_text()
                        self.assertIn('io.prysm.capture_0.2.0_all.ipk', checksum)
                        self.assertIn('prysm-led_0.2.0-1_arm64.deb', checksum)
