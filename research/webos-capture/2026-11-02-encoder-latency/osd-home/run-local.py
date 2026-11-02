"""Stage only this probe; retrieve evidence before deleting its private TV directory."""
import io
from pathlib import Path
import subprocess
import tarfile

root = Path(__file__).resolve().parent
payload = io.BytesIO()
with tarfile.open(fileobj=payload, mode='w') as archive:
    for name in ['probe', 'run-tv.py', 'capture.role.json']:
        archive.add(root/name, arcname=name)
remote = '''set -eu
umask 077
probe_dir=$(mktemp -d /tmp/prysm-osd-home.XXXXXX)
trap 'rm -rf "$probe_dir"' EXIT
tar -x -C "$probe_dir"
set +e
python3 "$probe_dir/run-tv.py" "$probe_dir" >&2
result=$?
set -e
tar -cz -C "$probe_dir" --exclude=probe .
exit "$result"
'''
with (root/'evidence.tgz').open('xb') as out, (root/'run.log').open('xb') as err:
    result = subprocess.run(['/usr/bin/ssh', '-T', '-o', 'BatchMode=yes',
                             '-o', 'ConnectTimeout=5', '-o', 'ServerAliveInterval=5',
                             '-o', 'ServerAliveCountMax=2', 'root@192.168.18.10', remote],
                            input=payload.getvalue(), stdout=out, stderr=err, timeout=110)
print('Remote exit:', result.returncode)
print((root/'run.log').read_text())
with tarfile.open(root/'evidence.tgz') as archive:
    archive.extractall(root/'evidence', filter='data')
raise SystemExit(result.returncode)
