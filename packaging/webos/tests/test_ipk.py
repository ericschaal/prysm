import importlib.util
import io
from pathlib import Path
import subprocess
import tarfile
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('secure_ipk', Path(__file__).parents[1] / 'secure-ipk.py')
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class PackageTests(unittest.TestCase):
    def test_package_modes_and_reject_install_hooks(self):
        with tempfile.TemporaryDirectory() as directory:
            directory = Path(directory)
            package = directory / 'test.ipk'
            (directory / 'debian-binary').write_text('2.0\n')
            def archive(name, filename):
                with tarfile.open(directory / name, 'w:gz') as tar:
                    member = tarfile.TarInfo(filename)
                    member.size = 1
                    member.mode = 0o777
                    tar.addfile(member, io.BytesIO(b'x'))
            def build():
                module.write_ar(package, {name: (directory / name).read_bytes() for name in ['debian-binary', 'control.tar.gz', 'data.tar.gz']})
            archive('control.tar.gz', 'control')
            archive('data.tar.gz', 'usr/palm/services/io.prysm.capture.service/service.js')
            build()
            module.secure(package)
            with tarfile.open(fileobj=io.BytesIO(subprocess.check_output(['ar', '-p', str(package), 'data.tar.gz'])), mode='r:gz') as tar:
                self.assertEqual(tar.getmembers()[0].mode, 0o644)
                self.assertEqual(tar.getmembers()[0].uid, 0)
            archive('control.tar.gz', 'postinst')
            build()
            with self.assertRaisesRegex(ValueError, 'installation scripts'):
                module.secure(package)
            archive('control.tar.gz', 'control')
            archive('data.tar.gz', 'etc/hosts')
            build()
            with self.assertRaisesRegex(ValueError, 'unexpected package path'):
                module.secure(package)


if __name__ == '__main__':
    unittest.main()
