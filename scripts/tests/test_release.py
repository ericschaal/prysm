import hashlib
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
ROOT = release.ROOT
PUBLISH = ROOT / 'packaging/release/publish.sh'
spec = importlib.util.spec_from_file_location('ipk', ROOT / 'packaging/webos/ipk.py')
ipk = importlib.util.module_from_spec(spec)
spec.loader.exec_module(ipk)


def copy_version_files(root):
    for name in ('VERSION', 'Cargo.toml', '.release-please-manifest.json', release.COMPOSE):
        (root / name).parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(ROOT / name, root / name)
    (root / 'crates/engine/version/src').mkdir(parents=True)
    shutil.copyfile(ROOT / 'crates/engine/version/src/lib.rs', root / 'crates/engine/version/src/lib.rs')


def set_version(root, version):
    """Do what the release pull request does: VERSION, the pin and the manifest."""
    current = (root / 'VERSION').read_text().strip()
    (root / 'VERSION').write_text(version + '\n')
    compose = root / release.COMPOSE
    compose.write_text(compose.read_text().replace(f'prysm-headless:{current} ', f'prysm-headless:{version} '))
    (root / '.release-please-manifest.json').write_text(json.dumps({'.': version}, indent=2) + '\n')


class VersionTests(unittest.TestCase):
    def test_the_repository_carries_one_version(self):
        version = release.release_version(ROOT)
        self.assertEqual(version, (ROOT / 'VERSION').read_text().strip())

    def test_a_release_needs_the_tag_and_the_manifest_to_match(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            copy_version_files(root)
            set_version(root, '1.2.3')
            self.assertEqual(release.release_version(root, 'v1.2.3'), '1.2.3')
            for tag in ('1.2.3', 'v1.2.4', 'v1.2.3-rc.1', 'v1.2.3\ninjected', 'main'):
                with self.subTest(tag=tag), self.assertRaisesRegex(ValueError, 'tag'):
                    release.release_version(root, tag)
            (root / '.release-please-manifest.json').write_text('{".": "1.2.2"}\n')
            self.assertEqual(release.release_version(root), '1.2.3')
            with self.assertRaisesRegex(ValueError, 'manifest'):
                release.release_version(root, 'v1.2.3')

    def test_nothing_else_carries_the_product_version(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            copy_version_files(root)
            set_version(root, '1.2.3')
            cargo = root / 'Cargo.toml'
            cargo.write_text(cargo.read_text().replace('version = "0.0.0"', 'version = "1.2.3"', 1))
            with self.assertRaisesRegex(ValueError, r'workspace\.package'):
                release.release_version(root)
            shutil.copyfile(ROOT / 'Cargo.toml', cargo)
            source = root / 'crates/hosts/common/src/args.rs'
            source.parent.mkdir(parents=True)
            source.write_text('const V: &str = env!("CARGO_PKG_VERSION");\n')
            with self.assertRaisesRegex(ValueError, 'CARGO_PKG_VERSION'):
                release.release_version(root)

    def test_the_truenas_pin_follows_version(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            copy_version_files(root)
            set_version(root, '1.2.3')
            (root / 'VERSION').write_text('1.2.4\n')
            with self.assertRaisesRegex(ValueError, 'compose'):
                release.release_version(root)

    def test_versions_must_be_semantic(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            copy_version_files(root)
            set_version(root, '1.0.0')
            for bad in ('1.0', '01.0.0', 'v1.0.0', '1.0.0-', ''):
                (root / 'VERSION').write_text(bad + '\n')
                with self.subTest(version=bad), self.assertRaisesRegex(ValueError, 'VERSION'):
                    release.release_version(root)


STUB = r'''#!/usr/bin/env python3
import json, os, sys
from pathlib import Path
tool, args = Path(sys.argv[0]).name, sys.argv[1:]
with open(os.environ['CALLS'], 'a') as log:
    log.write(json.dumps([tool, *args]) + '\n')
state = Path(os.environ['STATE'])
if tool == 'dpkg-deb':
    name = Path(args[1]).name
    arch = 'arm64' if 'arm64' in name else 'amd64'
    print({'Package': os.environ.get('DEB_PACKAGE', 'prysm-headless'), 'Architecture': arch,
           'Version': os.environ['DEB_VERSION']}[args[-1]])
elif tool == 'gh':
    assert os.environ.get('GH_REPO') == os.environ['GITHUB_REPOSITORY'], 'gh needs the repository'
    if args[:2] == ['release', 'view']:
        if os.environ['RELEASE_STATE'] == 'missing' and not (state / 'created').exists():
            print('release not found', file=sys.stderr)
            sys.exit(1)
        print(json.dumps({'isDraft': os.environ['RELEASE_STATE'] != 'published'}))
    elif args[:2] == ['release', 'create']:
        (state / 'created').touch()
    elif args[0] == 'api':
        print('\n'.join(os.environ['STABLE_TAGS'].split()))
elif tool == 'docker':
    mark = lambda ref: state / ('pushed-' + ref.replace('/', '_').replace(':', '_'))
    if args[0] == 'pull' and '@' not in args[-1]:
        if not mark(args[-1]).exists():
            print('manifest unknown', file=sys.stderr)
            sys.exit(1)
    elif args[:2] == ['image', 'push']:
        mark(args[-1]).touch()
    elif args[:2] == ['image', 'inspect']:
        print(os.environ['REVISION'])
    elif args[:3] == ['buildx', 'imagetools', 'inspect']:
        if not (state / 'index').exists() and os.environ.get('INDEX') != '1':
            print('manifest unknown', file=sys.stderr)
            sys.exit(1)
        if '--raw' in args:
            print(json.dumps({'manifests': [
                {'platform': {'os': 'linux', 'architecture': arch}, 'digest': 'sha256:' + arch}
                for arch in ('amd64', 'arm64')]}))
        elif '--format' in args:
            print(json.dumps({'digest': 'sha256:index'}))
    elif args[:3] == ['buildx', 'imagetools', 'create']:
        (state / 'index').touch()
'''


# The CI artifacts a release run downloads, one directory each, as
# `cargo xtask dist` writes them (ci.yml's `docker` and `webos-ipk` jobs).
DIST = {
    'prysm-dist-linux-amd64': ['prysm-headless-linux-amd64', 'prysm-headless-linux-amd64.deb',
                               'prysm-headless-linux-amd64.tar'],
    'prysm-dist-linux-arm64': ['prysm-headless-linux-arm64', 'prysm-headless-linux-arm64.deb',
                               'prysm-headless-linux-arm64.tar'],
    'prysm-dist-tv': ['prysm-tv-full.ipk', 'prysm-tv-slim.ipk'],
}


def write_dist(artifacts, version, layout, edit):
    """Each directory's files and the manifest.json that records them; `edit`
    may change an entry after it is written (name, entry)."""
    for directory, names in layout.items():
        (artifacts / directory).mkdir()
        entries = {}
        for name in names:
            (artifacts / directory / name).write_text(name)
            entries[name] = {'version': version, 'commit': 'commit', 'dirty': False,
                             'sha256': hashlib.sha256(name.encode()).hexdigest(),
                             'built_at': '2026-11-17T12:00:00Z'}
            edit(name, entries[name])
        (artifacts / directory / 'manifest.json').write_text(json.dumps({'artifacts': entries}))


class PublishTests(unittest.TestCase):
    def run_publish(self, version='1.2.0', release_state='draft', revision='commit',
                    stable_tags='v0.2.0 v1.1.0 v1.2.0', layout=DIST, edit=lambda name, entry: None,
                    **env):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        root = Path(directory.name)
        bin_dir, artifacts, state = root / 'bin', root / 'artifacts', root / 'state'
        for path in (bin_dir, artifacts, state):
            path.mkdir()
        stub = bin_dir / 'stub'
        stub.write_text(STUB)
        stub.chmod(0o755)
        for name in ('docker', 'gh', 'dpkg-deb'):
            (bin_dir / name).symlink_to(stub)
        deb = version.replace('-', '~') + '-1'
        write_dist(artifacts, version, layout, edit)
        calls = root / 'calls'
        environment = dict(
            os.environ, PATH=f'{bin_dir}{os.pathsep}{os.environ["PATH"]}',
            GITHUB_REPOSITORY='ericschaal/Prysm', RELEASE_DIRECTORY=str(artifacts),
            RELEASE_TAG=f'v{version}', RELEASE_VERSION=version, RELEASE_SHA='commit',
            CALLS=str(calls), STATE=str(state), RELEASE_STATE=release_state, REVISION=revision,
            STABLE_TAGS=stable_tags, DEB_VERSION=deb)
        environment.update(env)
        result = subprocess.run(['bash', str(PUBLISH)], capture_output=True, text=True, env=environment)
        logged = [json.loads(line) for line in calls.read_text().splitlines()] if calls.exists() else []
        return result, logged, artifacts

    @staticmethod
    def find(calls, *prefix):
        return [call for call in calls if call[:len(prefix)] == list(prefix)]

    def test_a_draft_release_is_completed_and_becomes_latest(self):
        result, calls, artifacts = self.run_publish()
        self.assertEqual(result.returncode, 0, result.stderr)
        image = 'ghcr.io/ericschaal/prysm-headless'
        pushed = [call[-1] for call in self.find(calls, 'docker', 'image', 'push')]
        self.assertEqual(pushed, [f'{image}:1.2.0-amd64', f'{image}:1.2.0-arm64'])
        self.assertIn(['docker', 'buildx', 'imagetools', 'create', '--tag', f'{image}:1.2.0',
                       f'{image}:1.2.0-amd64', f'{image}:1.2.0-arm64'], calls)
        upload = self.find(calls, 'gh', 'release', 'upload')
        self.assertEqual(len(upload), 1)
        for name in ('prysm-headless_1.2.0_amd64.deb', 'prysm-headless_1.2.0_arm64.deb',
                     f'{ipk.APP_ID}-full_1.2.0_all.ipk', f'{ipk.APP_ID}-slim_1.2.0_all.ipk',
                     'docker-image.txt', 'SHA256SUMS'):
            self.assertTrue(any(arg.endswith(name) for arg in upload[0]), name)
        self.assertEqual(len(upload[0][5:]), 6, 'four packages, docker-image.txt and SHA256SUMS')
        self.assertIn(['docker', 'image', 'load', '--input', 'prysm-headless-linux-arm64.tar'], calls)
        self.assertIn(['gh', 'release', 'edit', 'v1.2.0', '--draft=false', '--prerelease=false',
                       '--latest=false'], calls)
        self.assertIn(['gh', 'release', 'edit', 'v1.2.0', '--latest'], calls)
        self.assertTrue(self.find(calls, 'docker', 'buildx', 'imagetools', 'create', '--tag', f'{image}:latest'))
        self.assertEqual((artifacts / 'docker-image.txt').read_text(), f'{image}@sha256:index\n')
        self.assertFalse(self.find(calls, 'gh', 'release', 'create'))

    def test_a_tag_without_a_release_gets_one(self):
        result, calls, _ = self.run_publish(release_state='missing')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(len(self.find(calls, 'gh', 'release', 'create')), 1)

    def test_a_pre_release_never_becomes_latest(self):
        result, calls, _ = self.run_publish(version='2.0.0-rc.1', stable_tags='v1.2.0')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn(['gh', 'release', 'edit', 'v2.0.0-rc.1', '--draft=false', '--prerelease=true',
                       '--latest=false'], calls)
        self.assertFalse(self.find(calls, 'gh', 'release', 'edit', 'v2.0.0-rc.1', '--latest'))
        self.assertFalse([call for call in calls if any(arg.endswith(':latest') for arg in call)])
        # GitHub replaces `~` in an asset's name, so the Debian version is not in it.
        upload = self.find(calls, 'gh', 'release', 'upload')[0]
        for arch in ('amd64', 'arm64'):
            self.assertTrue(any(arg.endswith(f'prysm-headless_2.0.0-rc.1_{arch}.deb') for arg in upload), arch)
        self.assertFalse([arg for arg in upload if '~' in arg])

    def test_an_older_version_does_not_move_latest(self):
        result, calls, _ = self.run_publish(stable_tags='v1.2.0 v1.10.0')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse(self.find(calls, 'gh', 'release', 'edit', 'v1.2.0', '--latest'))
        self.assertFalse([call for call in calls if any(arg.endswith(':latest') for arg in call)])

    def test_a_published_release_is_only_checked(self):
        result, calls, _ = self.run_publish(release_state='published', INDEX='1')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse(self.find(calls, 'gh', 'release', 'upload'))
        self.assertFalse(self.find(calls, 'docker', 'image', 'load'))

    def test_an_image_from_another_commit_is_refused(self):
        result, calls, _ = self.run_publish(revision='other')
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(self.find(calls, 'docker', 'image', 'push'))
        self.assertFalse(self.find(calls, 'gh', 'release', 'upload'))

    def test_missing_or_wrong_packages_are_refused(self):
        # Each manifest refusal has a Rust test (crates/tools/xtask/src/release.rs);
        # one here proves publish.sh stops on it.
        def from_other_commit(name, entry):
            if name == 'prysm-tv-slim.ipk':
                entry['commit'] = 'other'

        for case, kwargs in (
                ('no slim ipk', {'layout': {**DIST, 'prysm-dist-tv': ['prysm-tv-full.ipk']}}),
                ('no arm64 artifact', {'layout': {key: DIST[key] for key in DIST if 'arm64' not in key}}),
                ('another commit', {'edit': from_other_commit}),
                ('wrong package', {'DEB_PACKAGE': 'prysm-led'}),
                ('wrong deb version', {'DEB_VERSION': '1.1.0-1'})):
            with self.subTest(case=case):
                result, calls, _ = self.run_publish(**kwargs)
                self.assertNotEqual(result.returncode, 0)
                if case == 'another commit':
                    self.assertIn('prysm-tv-slim.ipk is from other', result.stderr)
                self.assertFalse(self.find(calls, 'docker', 'image', 'push'))
                self.assertFalse(self.find(calls, 'gh', 'release', 'upload'))
