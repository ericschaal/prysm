import os,contextlib,hashlib,json,pathlib,re,shlex,socket,subprocess as sp,sys,time,urllib.request
run=pathlib.Path(__file__).resolve().parent
wt=pathlib.Path(sp.check_output(['git','-C',str(run),'rev-parse','--show-toplevel'],text=True).strip())
if len(sys.argv) != 4: raise SystemExit('usage: run.py SECONDS BINARY CLIP')
assert 25 <= int(sys.argv[1]) <= 600
os.environ['PRYSM_CLIP']=str(pathlib.Path(sys.argv[3]).resolve(strict=True))
import tempfile
result_dir=pathlib.Path(tempfile.mkdtemp(prefix='prysm-tv-trial-'))
print('Trial results:',result_dir,flush=True)
binary=pathlib.Path(sys.argv[2])
tv=os.environ['PRYSM_TV_SSH']
ssh=['ssh','-o','BatchMode=yes','-o','ConnectTimeout=5','-o','ServerAliveInterval=4','-o','ServerAliveCountMax=2']
def remote(cmd, data=None, timeout=12):
    return sp.check_output(ssh+[tv,cmd],input=data,timeout=timeout)
def get(url):
    with urllib.request.urlopen(url, timeout=4) as r:return json.load(r)
def api(path):return get(os.environ['PRYSM_RECEIVER_URL'].rstrip('/')+'/api/'+path)
def wled():return get(os.environ['PRYSM_WLED_URL'].rstrip('/')+'/json/info')
def luna(method,body={},service=False):
    args=['luna-send']+(['-a','io.prysm.capture'] if service else [])+['-n','1','-w','7000','luna://'+method,json.dumps(body)]
    val=json.loads(remote(shlex.join(args)+' </dev/zero'))
    assert val['returnValue'], val
    return val
def service(action):return luna('io.prysm.capture.service/'+action,service=True)
def foreground():return luna('com.webos.applicationManager/getForegroundAppInfo')['appId']
def launch(app):return luna('com.webos.applicationManager/launch',{'id':app})
def play(action):return sp.check_output(['node',str(run/'playback.mjs'),action],timeout=12).decode().strip()
def save(name,x):(result_dir/(name+'.json')).write_text(json.dumps(x,indent=2))
def preview():
    try:return any(p.get('description')=='io.prysm.capture' for p in get('http://127.0.0.1:28387/json/list'))
    except OSError:return False
def wait_for(fn):
    for _ in range(60):
        if fn():return
        time.sleep(.2)
    raise TimeoutError(fn)
def stage_write(path, data):
    remote('cat > '+shlex.quote(path),data)
before=service('status'); orig=foreground(); settings=api('settings')['settings']; state=api('state')
save('before',dict(service=before,app=orig,state=state,settings=settings,wled=wled()))
assert settings['input']['type']=='webos'
result=[]
try:
  with contextlib.ExitStack() as clean:
    tmp=remote('mktemp -d /tmp/prysm-local.XXXXXXXX').decode().strip()
    assert re.fullmatch(r'/tmp/prysm-local\.[A-Za-z0-9]{8}',tmp)
    def remove_stage():
        remote('rm -f '+ ' '.join(tmp+'/'+f for f in ['worker','role.json','settings.json','run.py','log'])+' && rmdir '+tmp)
    clean.callback(remove_stage)
    exe=tmp+'/worker'
    stage_write(exe,binary.read_bytes()); remote('chmod 700 '+exe)
    assert remote('sha256sum '+exe).split()[0].decode()==hashlib.sha256(binary.read_bytes()).hexdigest()
    role=json.loads((wt/'packaging/webos/service/capture.role.json').read_text());role['exeName']=exe
    stage_write(tmp+'/role.json',json.dumps(role).encode())
    manifest='/var/luna-service2/manifests.d/io.prysm.'+tmp.split('/')[-1]+'.json'
    manifest_data=json.dumps(dict(id='io.prysm.'+tmp.split('/')[-1],version='1.0.0',roleFiles=[tmp+'/role.json']))
    remote('set -C; cat > '+manifest,manifest_data.encode())
    clean.callback(remote,'rm -f '+manifest+' && ls-control scan-services')
    remote('ls-control scan-services')
    print('Staged and verified TV binary',flush=True)
    if before['enabled']:
        clean.callback(service,'start')
        service('stop')
    wait_for(lambda: not service('status')['senderRunning'])
    if orig!='io.prysm.capture':
        clean.callback(launch,orig)
        launch('io.prysm.capture')
    forward=sp.Popen(ssh+['-o','ExitOnForwardFailure=yes','-L','127.0.0.1:28387:127.0.0.1:9998','-N',tv])
    def stop_forward():forward.terminate();forward.wait(timeout=6)
    clean.callback(stop_forward)
    wait_for(preview)
    clean.callback(play,'stop')
    print('Pattern',play('start'),flush=True)
    assert not service('status')['senderRunning']
    stage_write(tmp+'/run.py',b'''import pathlib,subprocess,time,sys,os
p=pathlib.Path(__file__).resolve().parent
with (p/'log').open('wb') as log:
 child=subprocess.Popen([str(p/'worker'),str(p/'settings.json')],stdout=log,stderr=log)
 try:
  sample0=None
  for n in range(int(sys.argv[1])*4):
   if n>=20 and n%4==0:
    fields=(pathlib.Path('/proc')/str(child.pid)/'stat').read_text().split()
    point=(time.monotonic(), (int(fields[13])+int(fields[14]))/os.sysconf('SC_CLK_TCK'))
    if sample0 is None: sample0=point
    elif n+4>=int(sys.argv[1])*4: print('MEAN_CPU_CORES',round((point[1]-sample0[1])/(point[0]-sample0[0]),3),flush=True)
   if child.poll() is not None: break
   time.sleep(.25)
 finally:
  if child.poll() is None:child.terminate()
  try:code=child.wait(timeout=6)
  except subprocess.TimeoutExpired:child.kill();child.wait();raise
print((p/'log').read_text(errors='replace'),flush=True)
raise SystemExit(code)
''')
    for size in [[1280,720]]:
        test_settings=json.loads(json.dumps(settings));test_settings['input']['capture_resolution']=size
        stage_write(tmp+'/settings.json',json.dumps(test_settings).encode())
        child=sp.Popen(ssh+[tv,'python3 '+tmp+'/run.py '+str(int(sys.argv[1]))],stdout=sp.PIPE)
        try:
            time.sleep(3); assert child.poll() is None, child.stdout.read()
            print('Measuring',size,play('check'),flush=True)
            colors=sp.Popen(['node',str(run/'colors.mjs')],stdout=sp.PIPE)
            try:
                observations=[]
                for _ in range(10):
                    time.sleep(1); observations.append(wled())
                info=next((v for v in observations if v.get('live')),observations[-1])
                samples=json.loads(colors.communicate(timeout=20)[0])
            finally:
                if colors.poll() is None: colors.kill(); colors.wait()
            out=child.communicate(timeout=int(sys.argv[1])+10)[0].decode();print('TV capture ended',flush=True)
            (result_dir/('tv-'+str(size[0])+'.log')).write_text(out)
            assert child.returncode==0,child.returncode
            assert 'width=1280 height=720' in out, 'actual capture geometry differs'
            rows=[(int(f),int(t)) for f,t in re.findall(r'local DDP output frames=(\d+) elapsed_ms=(\d+)',out)]
            assert len(rows)>=3 and rows[-1][0]>rows[0][0], rows
            fps=(rows[-1][0]-rows[0][0])*1000/(rows[-1][1]-rows[0][1])
            result.append(dict(size=size,rows=rows,fps=fps,controller=samples,live=info.get('live'),lip=info.get('lip'),lm=info.get('lm')))
            print(result[-1],flush=True);save('results',result)
            if not (info.get('live') and samples['samples']>0 and samples['max']-samples['min']>45):
                raise AssertionError('WLED did not confirm changing output')
        finally:
            if child.poll() is None:child.wait(timeout=int(sys.argv[1])+10)
finally:
    after=service('status')
    end=dict(service=after,app=foreground(),settings=api('settings')['settings'],state=api('state'),wled=wled())
    save('after',end)
    print('Restored service',after['enabled'],'foreground',end['app'],flush=True)
    assert after['enabled']==before['enabled'] and after['config']==before['config'] and end['app']==orig and end['settings']==settings
