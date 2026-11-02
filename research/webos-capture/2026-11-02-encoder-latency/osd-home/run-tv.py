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
foreground = request('luna://com.webos.surfacemanager/getForegroundAppInfo', {})
original_app = next(x['appId'] for x in foreground['foregroundAppInfo'] if x.get('primary'))
(root/'foreground-before.json').write_text(json.dumps(foreground,indent=2))
home_opened = False

def snapshot(name):
    foreground = request('luna://com.webos.surfacemanager/getForegroundAppInfo', {})
    video = request('luna://com.webos.service.videooutput/getStatus', {})
    assert video['returnValue'],video
    (root/(name+'-foreground.json')).write_text(json.dumps(foreground,indent=2))
    (root/(name+'-video.json')).write_text(json.dumps(video,indent=2))
    return foreground, next(v for v in video['video'] if v['sink']=='MAIN')

def check_home(name):
    foreground, main = snapshot(name)
    assert any(x.get('primary') and x['appId']=='com.webos.app.home' for x in foreground['foregroundAppInfo']),foreground
    assert not main['connected'] and main['width']==0 and main['height']==0 and main['frameRate']==0,main

try:
    service('stop')
    for _ in range(30):
        state = service('status')
        if not state['senderRunning'] and not state['stopping']:break
        time.sleep(.1)
    assert not state['senderRunning'] and not state['enabled'],state
    with open('/tmp/prysm-capture.lock','r+') as lock:
        fcntl.flock(lock,fcntl.LOCK_EX|fcntl.LOCK_NB)
    role=json.loads((root/'capture.role.json').read_text())
    role['exeName']=str(root/'probe')
    role_path=root/'probe.role.json';role_path.write_text(json.dumps(role))
    with manifest.open('x') as f:
        json.dump(dict(id='io.prysm.ui.'+root.name,version='1.0.0',roleFiles=[str(role_path)]),f)
    registered=True
    subprocess.run(['ls-control','scan-services'],check=True,timeout=5,stdout=subprocess.DEVNULL)
    home_opened=True
    launched=request('luna://com.webos.applicationManager/launch',{'id':'com.webos.app.home'})
    assert launched['returnValue'],launched
    time.sleep(2)
    for index in range(2):
        name='home-osd-'+str(index+1)
        check_home(name+'-before')
        toast=request('luna://com.webos.notification/createToast',{'message':'Prysm OSD cold-start test '+str(index+1)})
        (root/(name+'-toast.json')).write_text(json.dumps(toast))
        assert toast['returnValue'],toast
        results[name]=run(name,[str(root/'probe'),'4',str(root/name)],20)
        check_home(name+'-after')
        with open('/tmp/prysm-capture.lock','r+') as lock:
            fcntl.flock(lock,fcntl.LOCK_EX|fcntl.LOCK_NB)
        assert results[name] not in [124,137],'probe timed out'
    restored=request('luna://com.webos.applicationManager/launch',{'id':original_app})
    assert restored['returnValue'],restored
    time.sleep(2)
    _,main=snapshot('control')
    if main['connected'] and main['width']>0:
        toast=request('luna://com.webos.notification/createToast',{'message':'Prysm OSD control toast'})
        assert toast['returnValue'],toast
        time.sleep(.6)
        results['video-osd-control']=run('video-osd-control',[str(root/'probe'),'4',str(root/'video-osd-control')],12)
        assert results['video-osd-control']==0,'OSD control failed with video active'
finally:
    (root/'probe-results.json').write_text(json.dumps(results,indent=2))
    errors=[]
    if home_opened:
        try:
            restored=request('luna://com.webos.applicationManager/launch',{'id':original_app})
            assert restored['returnValue'],restored
            time.sleep(1)
            snapshot('restored')
        except Exception as e:errors.append('input restoration: '+repr(e))
    if registered:
        try:
            manifest.unlink()
            subprocess.run(['ls-control','scan-services'],check=True,timeout=5,stdout=subprocess.DEVNULL)
        except Exception as e:errors.append('role cleanup: '+repr(e))
    try:
        if before['enabled']:service('start')
        for _ in range(40):
            after=service('status')
            if not before['capturing'] or after['capturing']:break
            time.sleep(.25)
        (root/'after.json').write_text(json.dumps(after,indent=2))
        assert after['config']==before['config'] and after['enabled']==before['enabled'],after
        assert Path('/var/lib/prysm-capture/config.json').read_bytes()==saved_config
        assert after['error'] is None and (not before['capturing'] or after['capturing']),after
        print('RESTORED',json.dumps(after),flush=True)
    except Exception as e:errors.append('sender restoration: '+repr(e))
    assert not errors,errors
