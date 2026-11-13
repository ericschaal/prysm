"""Host tests for the v1 TV package files and ipk.py (no SDK needed)."""
import importlib.util
import io
import json
from pathlib import Path
import re
import shutil
import subprocess
import tarfile
import tempfile
import unittest

HERE = Path(__file__).resolve().parents[1]
V1 = HERE.parents[1]
FRONTEND = V1 / 'frontend'
spec = importlib.util.spec_from_file_location('ipk', HERE / 'ipk.py')
ipk = importlib.util.module_from_spec(spec)
spec.loader.exec_module(ipk)

INSTALLED = '/media/developer/apps/usr/palm/services/io.prysm.tv.service/'


def load(name):
    return json.loads((HERE / name).read_text())


class MetadataTests(unittest.TestCase):
    def test_package_files_match_the_supervisor(self):
        identity = (V1 / 'hosts/webos-service/src/identity.rs').read_text()
        service_rs = (V1 / 'hosts/webos-service/src/service.rs').read_text()
        app_id = re.search(r'pub const APP_ID: &str = "(.+?)";', identity).group(1)
        supervisor = re.search(r'pub const SUPERVISOR: &str = "(.+?)";', identity).group(1)
        worker = re.search(r'pub const WORKER: &str = "(.+?)";', identity).group(1)
        methods = re.findall(r'Self::\w+ => "(\w+)"', re.search(r'pub const fn name\(self\).*?\n    \}', service_rs, re.S).group(0))
        self.assertEqual((app_id, supervisor, worker), (ipk.APP_ID, 'prysm-webos-service', 'prysm-webos-worker'))

        app = load('app/appinfo.json')
        info = load('service/services.json')
        role = load('service/supervisor.role.json')
        worker_role = load('service/worker.role.json')
        api = load('service/supervisor.api.json')
        client = load('service/supervisor.client.json')
        self.assertEqual(app['id'], app_id)
        self.assertEqual(app['requiredACG'], ['public'])
        self.assertEqual(info['id'], app_id + '.service')
        self.assertEqual(info['engine'], 'native')
        self.assertEqual(info['executable'], supervisor)
        self.assertEqual(info['services'], [{'name': info['id']}])
        self.assertEqual(role['exeName'], INSTALLED + supervisor)
        self.assertEqual(role['allowedNames'], [info['id']])
        self.assertEqual(role['permissions'], [{
            'service': info['id'], 'inbound': ['*'],
            'outbound': ['com.webos.service.tvpower', 'com.webos.service.videooutput']}])
        self.assertEqual(worker_role['exeName'], INSTALLED + worker)
        self.assertEqual(worker_role['allowedNames'], ['com.webos.service.capture.client*', 'com.webos.rm.client.*'])
        self.assertEqual(api, {'public': [info['id'] + '/' + method for method in methods]})
        self.assertEqual(methods, ['getState', 'setPreferences', 'setEnabled', 'uninstall'])
        self.assertEqual(list(client), [info['id']])
        unit = (HERE / 'service/supervisor.service').read_text().splitlines()
        self.assertIn('Name=' + info['id'], unit)
        self.assertIn('Exec=' + INSTALLED + supervisor, unit)
        self.assertIn(INSTALLED + supervisor + ' setup', (HERE / 'service/setup.sh').read_text())
        self.assertFalse((HERE / 'service/package.json').exists())
        self.assertEqual(list((HERE / 'service').glob('*.js')), [])

    def test_install_helper_uses_the_install_service_then_setup(self):
        script = HERE / 'install-ipk.sh'
        text = script.read_text()
        self.assertNotIn('ares-install ', text.split('set -eu', 1)[1])
        self.assertIn('luna://com.webos.appInstallService/dev/install', text)
        with tempfile.TemporaryDirectory() as directory:
            # The names build-ipk.sh writes and the release attaches.
            self.assertIn('package="io.prysm.tv-${flavour}_${version}_all.ipk"',
                          (HERE / 'build-ipk.sh').read_text())
            for flavour in ('full', 'slim'):
                ipk_file = Path(directory) / f'io.prysm.tv-{flavour}_1.0.0_all.ipk'
                ipk_file.write_bytes(b'')
                run = subprocess.run(['sh', str(script), '--dry-run', 'root@tv', str(ipk_file)],
                                     capture_output=True, text=True, check=True)
                plan = run.stderr
                self.assertIn(f'/media/developer/temp/io.prysm.tv-{flavour}_1.0.0_all.ipk', plan)
                # Either flavour installs as the one app.
                self.assertIn('"id":"io.prysm.tv"', plan)
                # luna-send keeps its stdin: every call is fed by a sleep.
                self.assertEqual(plan.count('luna-send'), plan.count('(sleep '))
                self.assertLess(plan.index('appInstallService'), plan.index(INSTALLED + 'setup.sh'))
            # Anything else is not a Prysm TV package, the unsuffixed name included.
            for other in ('other.ipk', 'io.prysm.tv_1.0.0_all.ipk', 'io.prysm.tv-other_1.0.0_all.ipk'):
                path = Path(directory) / other
                path.write_bytes(b'')
                refused = subprocess.run(['sh', str(script), '--dry-run', 'root@tv', str(path)],
                                         capture_output=True, text=True)
                self.assertEqual(refused.returncode, 2, other)

    def test_appinfo_version_is_the_workspace_version(self):
        # One place to bump: the source appinfo.json has no version of its own.
        self.assertNotIn('version', load('app/appinfo.json'))
        self.assertIn('ipk.py" appinfo "$v1/Cargo.toml"', (HERE / 'build-ipk.sh').read_text())
        workspace = ipk.workspace_version((V1 / 'Cargo.toml').read_text())
        with tempfile.TemporaryDirectory() as directory:
            staged = Path(directory) / 'appinfo.json'
            version = ipk.write_appinfo(V1 / 'Cargo.toml', HERE / 'app/appinfo.json', staged)
            appinfo = json.loads(staged.read_text())
        self.assertEqual(appinfo['version'], version)
        self.assertEqual(appinfo['id'], ipk.APP_ID)
        # ares-package's rule: three dot-separated numbers, nothing else.
        self.assertRegex(version, r'^(?:[1-9]\d{0,8}|\d)\.(?:[1-9]\d{0,8}|\d)\.(?:[1-9]\d{0,8}|\d)$')
        self.assertTrue(workspace == version or workspace.startswith(version + '-'), workspace)
        self.assertEqual(ipk.app_version('1.0.0-alpha.0'), '1.0.0')
        self.assertEqual(ipk.app_version('2.10.3'), '2.10.3')
        for bad in ('1.0', '01.0.0', 'v1.0.0'):
            with self.subTest(bad=bad), self.assertRaises(ValueError):
                ipk.app_version(bad)

    @unittest.skipUnless(shutil.which('node') and (FRONTEND / 'node_modules/react/LICENSE').is_file(),
                         'needs node and v1/frontend/node_modules')
    def test_javascript_notices_include_react(self):
        # The frontend build writes dist/<host>/licenses.txt with this module;
        # build-ipk.sh ships it as licenses/javascript.txt.
        build_ipk = (HERE / 'build-ipk.sh').read_text()
        self.assertIn('dist/$host/licenses.txt', build_ipk)
        self.assertIn('licenses/javascript.txt', build_ipk)
        self.assertIn(ipk.SERVICE + 'licenses/javascript.txt', ipk.SERVICE_FILES)
        bundled = [FRONTEND / 'node_modules/react/cjs/react.production.js',
                   FRONTEND / 'node_modules/react-dom/cjs/react-dom-client.production.js',
                   FRONTEND / 'packages/tv-ui/index.ts']
        script = ("import { packageDir, notices } from './scripts/licenses.mjs';"
                  "const ids = JSON.parse(process.argv[1]);"
                  "process.stdout.write(await notices(ids.map(packageDir).filter(Boolean)));")
        text = subprocess.run(['node', '--input-type=module', '-e', script, json.dumps([str(p) for p in bundled])],
                              cwd=FRONTEND, capture_output=True, text=True, check=True).stdout
        react = json.loads((FRONTEND / 'node_modules/react/package.json').read_text())
        self.assertIn(f"react {react['version']} (MIT)", text)
        self.assertIn((FRONTEND / 'node_modules/react/LICENSE').read_text().strip(), text)
        self.assertIn('react-dom ', text)
        # Prysm's own packages are covered by prysm.txt, not listed here.
        self.assertNotIn('@prysm', text)


def tar(*entries):
    output = io.BytesIO()
    with tarfile.open(fileobj=output, mode='w:gz') as archive:
        for name, kind in entries:
            member = tarfile.TarInfo(name)
            member.mode = 0o777
            member.uid = member.gid = 1000
            if kind == 'dir':
                member.type = tarfile.DIRTYPE
                archive.addfile(member)
            elif kind == 'link':
                member.type = tarfile.SYMTYPE
                member.linkname = '/etc/passwd'
                archive.addfile(member)
            else:
                member.size = 1
                archive.addfile(member, io.BytesIO(b'x'))
    return output.getvalue()


def complete(*extra):
    files = sorted(ipk.SERVICE_FILES | ipk.APP_REQUIRED | {ipk.PACKAGE_INFO})
    return [('usr', 'dir'), ('usr/palm', 'dir')] + [(name, 'file') for name in files] + list(extra)


class PackageTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.package = Path(self.directory.name) / 'test.ipk'

    def tearDown(self):
        self.directory.cleanup()

    def build(self, data, control=(('control', 'file'),)):
        ipk.write_ar(self.package, {
            'debian-binary': b'2.0\n',
            'control.tar.gz': tar(*control),
            'data.tar.gz': tar(*data),
        })

    def test_secure_sets_root_ownership_and_modes(self):
        self.build(complete())
        ipk.secure(self.package)
        listing = {name: mode for mode, _, name in ipk.verify(self.package)}
        self.assertEqual(listing[ipk.SERVICE + 'prysm-webos-worker'], 0o755)
        self.assertEqual(listing[ipk.SERVICE + 'prysm-webos-service'], 0o755)
        self.assertEqual(listing[ipk.SERVICE + 'setup.sh'], 0o755)
        self.assertEqual(listing[ipk.SERVICE + 'worker.role.json'], 0o644)
        self.assertEqual(listing['usr/'], 0o755)
        self.assertFalse(any(mode & 0o022 for mode in listing.values()))

    def test_verify_rejects_an_unsecured_package(self):
        self.build(complete())
        with self.assertRaisesRegex(ValueError, 'unexpected owner or mode'):
            ipk.verify(self.package)

    def test_rejections(self):
        cases = [
            (complete(('etc/hosts', 'file')), 'unexpected package path'),
            (complete((ipk.SERVICE + 'nested/../x', 'file')), 'unexpected package path'),
            (complete(('/' + ipk.SERVICE + 'x', 'file')), 'unexpected package path'),
            (complete((ipk.SERVICE + 'prysm-webos-local', 'file')), 'unexpected service file'),
            (complete((ipk.APP + 'evil', 'link')), 'unexpected link'),
            (complete()[:-1], 'missing package files'),
            (complete((ipk.APP + 'index.html', 'file')), 'duplicate package file'),
            # The old app's paths are not part of a v1 package.
            (complete(('usr/palm/services/io.prysm.capture.service/x', 'file')), 'unexpected package path'),
        ]
        for data, error in cases:
            with self.subTest(error=error):
                self.build(data)
                with self.assertRaisesRegex(ValueError, error):
                    ipk.secure(self.package)

    def test_encoder_notices_are_allowed_but_not_required(self):
        notices = sorted(ipk.OPTIONAL_SERVICE_FILES)
        self.assertEqual(notices, [ipk.SERVICE + 'licenses/' + name for name in
                                   ('gstreamer-rs.txt', 'gtk-rs.txt', 'openh264.txt')])
        self.build(complete(*[(name, 'file') for name in notices]))
        ipk.secure(self.package)
        listed = {name for _, _, name in ipk.verify(self.package)}
        self.assertTrue(set(notices) <= listed)
        self.build(complete())
        ipk.secure(self.package)
        ipk.verify(self.package)
        # The notices the slim package copies exist in the repository.
        for path in ('adapters/h264/LICENSE.openh264', 'adapters/h264-rvenc-sys/LICENSE.gstreamer-rs',
                     'adapters/h264-rvenc-sys/LICENSE.gtk-rs'):
            self.assertTrue((V1 / path).is_file(), path)
        self.assertIn('LICENSE.openh264', (HERE / 'build-ipk.sh').read_text())

    def test_gstreamer_libraries_are_a_separate_allowance(self):
        self.assertFalse(ipk.GSTREAMER_LIBRARIES & ipk.LIBRARIES)
        self.assertIn('libgstreamer-1.0.so.0', ipk.GSTREAMER_LIBRARIES)

    def test_install_scripts_are_rejected(self):
        self.build(complete(), control=(('control', 'file'), ('postinst', 'file')))
        with self.assertRaisesRegex(ValueError, 'installation scripts'):
            ipk.secure(self.package)

    def test_only_three_ar_members(self):
        ipk.write_ar(self.package, {'debian-binary': b'2.0\n', 'control.tar.gz': tar(('control', 'file'))})
        with self.assertRaisesRegex(ValueError, 'exactly'):
            ipk.secure(self.package)


class SmokeProbeTests(unittest.TestCase):
    def test_probe_files_follow_the_packaged_permissions(self):
        smoke_spec = importlib.util.spec_from_file_location('smoke', HERE / 'tests/native-smoke.py')
        smoke = importlib.util.module_from_spec(smoke_spec)
        smoke_spec.loader.exec_module(smoke)
        directory = '/tmp/prysm-supervisor.abcdef'
        self.assertTrue(smoke.DIRECTORY.match(directory))
        self.assertFalse(smoke.DIRECTORY.match('/tmp/prysm-supervisor.abcdef/../etc'))
        files, manifest = smoke.probe_files(directory, 'slim', True, 60)
        role = json.loads(files['probe.role.json'])
        self.assertEqual(role['exeName'], directory + '/prysm-webos-slim-service')
        self.assertEqual(role['allowedNames'], [smoke.SERVICE])
        api = json.loads(files['probe.api.json'])['public']
        self.assertEqual(api, [smoke.SERVICE + '/' + m for m in ('getState', 'setPreferences', 'setEnabled', 'uninstall')])
        self.assertEqual(json.loads(files['probe.worker.role.json'])['exeName'], directory + '/prysm-webos-slim')
        self.assertIn(directory + '/probe.worker.role.json', json.loads(manifest)['roleFiles'])
        # The probe's Luna manifest has the packaged app's version.
        version = ipk.app_version(ipk.workspace_version((V1 / 'Cargo.toml').read_text()))
        self.assertEqual(json.loads(manifest)['version'], version)
        self.assertIn('run --sandbox ' + directory + ' --run-for 60', files['probe.service'])
        status = json.loads(smoke.STATUS)
        self.assertEqual(set(status), {'type', 'capturing', 'target', 'portal', 'failure'})


if __name__ == '__main__':
    unittest.main()
