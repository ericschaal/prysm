#!/usr/bin/env python3
"""Keep elevated service code read-only to other TV users in the generated IPK."""
import io
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile


def write_ar(destination, members):
    with Path(destination).open('wb') as output:
        output.write(b'!<arch>\n')
        for name, data in members.items():
            if len(name) > 15:
                raise ValueError('IPK member name is too long')
            header = f'{name:<16}{0:<12}{0:<6}{0:<6}{"100644":<8}{len(data):<10}`\n'
            output.write(header.encode('ascii'))
            output.write(data)
            if len(data) % 2:
                output.write(b'\n')


def secure(package):
    package = Path(package).resolve()
    members = subprocess.check_output(['ar', '-t', str(package)], text=True).splitlines()
    if members != ['debian-binary', 'control.tar.gz', 'data.tar.gz']:
        raise ValueError('expected an unsigned webOS IPK with exactly three members')
    with tempfile.TemporaryDirectory(prefix='prysm-ipk-') as directory:
        directory = Path(directory)
        for name in members:
            (directory / name).write_bytes(subprocess.check_output(['ar', '-p', str(package), name]))
        for name in members[1:]:
            source = (directory / name).read_bytes()
            with tarfile.open(fileobj=io.BytesIO(source), mode='r:gz') as old, \
                    tarfile.open(directory / name, mode='w:gz', format=tarfile.GNU_FORMAT) as new:
                for member in old.getmembers():
                    if name == 'control.tar.gz' and member.name not in ['control', './control', '.', './']:
                        raise ValueError('installation scripts are not allowed in the Prysm IPK')
                    if name == 'data.tar.gz':
                        normalized = member.name.removeprefix('./').rstrip('/')
                        allowed = [
                            'usr/palm/applications/io.prysm.capture',
                            'usr/palm/services/io.prysm.capture.service',
                            'usr/palm/packages/io.prysm.capture',
                        ]
                        if not any(normalized == root or normalized.startswith(root + '/') or
                                   (member.isdir() and root.startswith(normalized + '/')) for root in allowed):
                            raise ValueError('unexpected package path: ' + member.name)
                    if not (member.isdir() or member.isfile()):
                        raise ValueError('unexpected link or special file: ' + member.name)
                    member.mode = 0o755 if member.isdir() or Path(member.name).name in ['setup.sh', 'prysm-webos-capture', 'prysm-webos-service'] else 0o644
                    member.uid = member.gid = 0
                    member.uname = member.gname = 'root'
                    new.addfile(member, old.extractfile(member) if member.isfile() else None)
        write_ar(package, {name: (directory / name).read_bytes() for name in members})


if __name__ == '__main__':
    secure(sys.argv[1])
