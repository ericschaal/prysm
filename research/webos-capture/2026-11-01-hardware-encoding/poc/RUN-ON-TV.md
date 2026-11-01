# Bounded TV reproduction

Active synthetic encoder test on the owner-authorized project TV. Inspect
current health first. The user disabled capture for the successful run; do
not assume it is still stopped. Arrange an idle window. The probe refuses a
held `/tmp/prysm-capture.lock`; never bypass it. No service/configuration
changes or automatic capture re-enable are included.

Set `TV` to the existing authorized SSH destination or alias. Keep credentials
out of this package. From the Prysm repository root, choose fresh local output
paths if preserving earlier results:

```bash
test -n "$TV"
POC=research/webos-capture/2026-11-01-hardware-encoding/poc
ssh -T -o BatchMode=yes -o ConnectTimeout=5 -o ServerAliveInterval=5 -o ServerAliveCountMax=2 "$TV" '
set -eu
umask 077
probe_dir=$(mktemp -d /tmp/prysm-encoder-pitch.XXXXXX)
trap '\''rm -f "$probe_dir/probe.py" "$probe_dir/output.h264" "$probe_dir/registry.bin"; rmdir "$probe_dir"'\'' EXIT
cat > "$probe_dir/probe.py"
timeout -s TERM -k 2 12 env GST_DEBUG=rvenc:6 GST_DEBUG_NO_COLOR=1 GST_REGISTRY="$probe_dir/registry.bin" python3 "$probe_dir/probe.py" "$probe_dir/output.h264" >&2
tar -c -C "$probe_dir" output.h264
' < "$POC/probe.py" > /tmp/prysm-hardware-encoding-output.tar 2> /tmp/prysm-hardware-encoding-probe.log
```

Apply a local 30-second process deadline when orchestrating SSH. The remote
12-second watchdog plus two-second kill grace bounds native hangs. A timeout
is failure, not clean teardown. The shared capture lock remains after exit.

Require SSH exit 0, EOS and normal stop/close/finalize logs, plus exactly one
nonempty regular archive member named `output.h264`. Validate archive paths
before extraction, decode the clip and inspect its SPS. Appsrc push success
alone proves nothing. Recheck health and process/temporary-file cleanup; leave
the user's capture state as found.

Read health before and after the experiment with the existing service's
read-only status endpoint. This TV requires keeping luna-send's stdin open:

```bash
ssh -T -o BatchMode=yes -o ConnectTimeout=5 "$TV" 'python3 -' <<'PY'
import json
import subprocess

child = subprocess.Popen(
    ['luna-send', '-n', '1', '-w', '3000',
     'luna://io.prysm.capture.service/status', '{}'],
    stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
try:
    child.wait(timeout=4)
except subprocess.TimeoutExpired:
    child.kill()
    child.wait()
    raise RuntimeError('status request timed out')
assert child.returncode == 0, 'status request failed'
status = json.loads(child.stdout.read())
print(json.dumps({key: status.get(key) for key in
    ['capturing', 'receiverConnected', 'senderRunning', 'error']}))
PY
```

For an isolated test, require capturing=false, senderRunning=false and
error=null before proceeding. After testing, require the same idle/error-free
state and inspect `/tmp/prysm-encoder-pitch.*` and the probe process for cleanup.
A busy capture lock is a refusal to test, not permission to stop production.
