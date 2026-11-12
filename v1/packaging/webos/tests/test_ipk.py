"""Host tests for the v1 TV package files and ipk.py (no SDK needed)."""
import importlib.util
import io
import json
from pathlib import Path
import re
import tarfile
import tempfile
import unittest

HERE = Path(__file__).resolve().parents[1]
V1 = HERE.parents[1]
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
        methods = re.findall(r'"(\w+)"', re.search(r'pub const METHODS: [^=]*= \[(.*?)\];', service_rs, re.S).group(1))
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

    def test_appinfo_version_is_the_workspace_version(self):
        cargo = (V1 / 'Cargo.toml').read_text()
        version = re.search(r'^version = "(\d+\.\d+\.\d+)', cargo, re.M).group(1)
        self.assertEqual(load('app/appinfo.json')['version'], version)


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
        self.assertIn('run --sandbox ' + directory + ' --run-for 60', files['probe.service'])
        status = json.loads(smoke.STATUS)
        self.assertEqual(set(status), {'type', 'capturing', 'target', 'portal', 'failure'})


if __name__ == '__main__':
    unittest.main()
