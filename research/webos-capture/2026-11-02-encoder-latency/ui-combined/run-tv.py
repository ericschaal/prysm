"""Temporarily stop the sender, serialize bounded probes, restore prior state."""
import fcntl
import json
from pathlib import Path
import subprocess
import sys
import time

root = Path(sys.argv[1])


def request(uri, payload):
    p = subprocess.Popen(['luna-send', '-a', 'io.prysm.capture', '-n', '1',
                          '-w', '6000', uri, json.dumps(payload)],
                         stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    try:
        p.wait(timeout=8)
        out, err = p.stdout.read(), p.stderr.read()
        assert p.returncode == 0, (out, err)
        return json.loads(out)
    finally:
        if p.poll() is None:
            p.kill()
            p.wait()
        p.stdin.close()
        p.stdout.close()
        p.stderr.close()


def service(method):
    result = request('luna://io.prysm.capture.service/' + method, {})
    assert result['returnValue'], result
    return result


def run(name, command, seconds):
    p = subprocess.run(['timeout', '-s', 'TERM', '-k', '2', str(seconds)] + command,
                       capture_output=True, timeout=seconds+4)
    (root/(name+'.stdout')).write_bytes(p.stdout)
    (root/(name+'.stderr')).write_bytes(p.stderr)
    print(name, 'exit', p.returncode, p.stderr[-1500:].decode(errors='replace'), flush=True)
    return p.returncode


before = service('status')
(root/'before.json').write_text(json.dumps(before, indent=2))
assert before['error'] is None, before
saved_config = Path('/var/lib/prysm-capture/config.json').read_bytes()
manifest = Path('/var/luna-service2/manifests.d')/('io.prysm.ui.'+root.name+'.json')
registered = False
results = {}
try:
    service('stop')
    for _ in range(30):
        state = service('status')
        if not state['senderRunning'] and not state['stopping']:
            break
        time.sleep(.1)
    assert not state['senderRunning'] and not state['enabled'], state
    with open('/tmp/prysm-capture.lock', 'r+') as lock:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
    roles = []
    for binary in ['worker', 'probe']:
        role = json.loads((root/'capture.role.json').read_text())
        role['exeName'] = str(root/binary)
        path = root/(binary+'.role.json')
        path.write_text(json.dumps(role))
        roles.append(str(path))
    with manifest.open('x') as f:
        json.dump(dict(id='io.prysm.ui.'+root.name, version='1.0.0', roleFiles=roles), f)
    registered = True
    subprocess.run(['ls-control','scan-services'],check=True,timeout=5,stdout=subprocess.DEVNULL)
    results['worker'] = run('worker', ['python3', str(root/'wire-probe.py'), str(root), '3'], 20)
    assert results['worker'] == 0, 'instrumented worker failed'
    for mode in [2, 3, 4, 2]:
        name = 'tap-'+str(mode)+('-after' if 'tap-2' in results and mode == 2 else '')
        toast = request('luna://com.webos.notification/createToast',
                        {'message': 'Prysm capture test — mode '+str(mode)})
        (root/(name+'.toast.json')).write_text(json.dumps(toast))
        time.sleep(.6)
        results[name] = run(name, [str(root/'probe'), str(mode), str(root/name)], 12)
        with open('/tmp/prysm-capture.lock', 'r+') as lock:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        # A failed default capture or forced timeout needs investigation before another tap.
        assert results[name] not in [124, 137], 'probe timed out'
        if mode == 2:
            assert results[name] == 0, 'default tap failed'
finally:
    (root/'probe-results.json').write_text(json.dumps(results, indent=2))
    if registered:
        manifest.unlink()
        subprocess.run(['ls-control','scan-services'],check=True,timeout=5,stdout=subprocess.DEVNULL)
    if before['enabled']:
        service('start')
    for _ in range(30):
        after = service('status')
        if not before['capturing'] or after['capturing']:
            break
        time.sleep(.2)
    (root/'after.json').write_text(json.dumps(after,indent=2))
    assert after['config'] == before['config'] and after['enabled'] == before['enabled'], after
    assert Path('/var/lib/prysm-capture/config.json').read_bytes() == saved_config
    assert after['error'] is None, after
    assert not before['capturing'] or after['capturing'], after
    print('RESTORED', json.dumps(after), flush=True)
