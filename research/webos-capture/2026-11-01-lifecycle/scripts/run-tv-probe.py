import argparse,subprocess,pathlib,json,shlex
p=argparse.ArgumentParser();p.add_argument('--role',action='store_true');p.add_argument('--trace',action='store_true');p.add_argument('--preflight',action='store_true');p.add_argument('--delay',type=int,default=0);p.add_argument('--binary',default='/tmp/prysm-vt-lifecycle');p.add_argument('--name',required=True);p.add_argument('args',nargs=argparse.REMAINDER);a=p.parse_args()
assert a.name.replace('-','').isalnum() and 0<=a.delay<=10
ssh=['ssh','-o','BatchMode=yes','-o','ConnectTimeout=5','root@192.168.18.10']
def run(cmd,**kw):return subprocess.run(ssh+[cmd],check=True,**kw)
def write(path,s):run('set -euC; umask 077; cat > '+shlex.quote(path),input=s,text=True,capture_output=True)
r=run('mktemp -d /tmp/prysm-vt.XXXXXX',capture_output=True,text=True).stdout.strip();assert r.startswith('/tmp/prysm-vt.')
m='/var/luna-service2/manifests.d/io.prysm.vtprobe.'+r.rsplit('.',1)[1]+'.json';registered=False
try:
 subprocess.run(['scp','-O',a.binary,'root@192.168.18.10:'+r+'/probe'],check=True)
 env=['env','PRYSM_PREFLIGHT=1'] if a.preflight else []
 if a.trace:
  subprocess.run(['scp','-O','/tmp/prysm-vt-trace.so','root@192.168.18.10:'+r+'/trace.so'],check=True)
  env=['env']+(['PRYSM_PREFLIGHT=1'] if a.preflight else [])+['LD_PRELOAD='+r+'/trace.so:/usr/lib/libvtcapture.so.1.0.0','PRYSM_DELAY_WAIT='+str(a.delay)]
 if a.role:
  role=json.loads(pathlib.Path('crates/capture/webos-capture/permissions/role.json').read_text());role['exeName']=r+'/probe';write(r+'/role.json',json.dumps(role));write(m,json.dumps({'id':'io.prysm.vtprobe.'+r.rsplit('.',1)[1],'version':'1.0.0','roleFiles':[r+'/role.json']}));registered=True;run('ls-control scan-services',capture_output=True)
 args=a.args[1:] if a.args and a.args[0]=='--' else a.args
 code='''import subprocess,sys,pathlib
p=subprocess.Popen(COMMAND,stdout=subprocess.PIPE,stderr=subprocess.PIPE)
try:
 out,err=p.communicate(timeout=8)
except subprocess.TimeoutExpired:
 sys.stderr.write("WATCHDOG timed out pid="+str(p.pid)+"\\n")
 for task in pathlib.Path('/proc/'+str(p.pid)+'/task').iterdir():
  for field in ['comm','wchan','syscall','stack']:
   try: sys.stderr.write(str(task)+'/'+field+' '+(task/field).read_text()+'\\n')
   except OSError as e: sys.stderr.write(str(e)+'\\n')
 p.kill()
 out,err=p.communicate(timeout=3)
sys.stdout.buffer.write(out);sys.stderr.buffer.write(err)
sys.exit(p.returncode)
'''.replace('COMMAND',repr(env+[r+'/probe']+args))
 result=subprocess.run(ssh+['python3 -c '+shlex.quote(code)],capture_output=True,text=True,timeout=20)
 dest=pathlib.Path('/tmp/prysm-cleanup-evidence')/a.name;dest.with_suffix('.stdout').write_text(result.stdout);dest.with_suffix('.stderr').write_text(result.stderr)
 print(a.name,'exit',result.returncode,'saved',dest);print(result.stderr[-2000:])
finally:
 if registered:run('rm -f '+shlex.quote(m)+'; ls-control scan-services',capture_output=True)
 run('rm -f '+shlex.join([r+'/probe',r+'/trace.so',r+'/role.json'])+'; rmdir '+shlex.quote(r),capture_output=True)
