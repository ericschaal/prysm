"""Exercise production Both/video/UI selection and controlled readiness transitions."""
import json, os, pathlib, signal, socket, struct, subprocess, sys, threading, time, urllib.request
root = pathlib.Path(sys.argv[1])
version = int(sys.argv[2])

def exact(sock, size):
    data = bytearray()
    while len(data) < size:
        part = sock.recv(size-len(data))
        assert part, 'unexpected disconnect'
        data.extend(part)
    return bytes(data)

def metrics(name):
    text = urllib.request.urlopen('http://127.0.0.1:9101/metrics', timeout=2).read().decode()
    (root/(name+'.metrics')).write_text(text)
    return text

def encoded(text, content):
    return sum(float(line.rsplit(' ', 1)[1]) for line in text.splitlines()
               if line.startswith('prysm_pipeline_frames_total{')
               and 'boundary="Encoded"' in line and 'content="'+content+'"' in line)

def toast():
    p = subprocess.Popen(['luna-send','-a','io.prysm.capture','-n','1','-w','4000',
                          'luna://com.webos.notification/createToast',
                          json.dumps(dict(message='Prysm combined capture qualification'))],
                         stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.PIPE)
    try:
        p.wait(timeout=6)
        response=json.loads(p.stdout.read())
        assert response['returnValue'],response
    finally:
        if p.poll() is None:p.kill();p.wait()
        p.stdin.close();p.stdout.close();p.stderr.close()

results=[]
for source,width,height,count in [('both',640,360,240),('video',640,360,30),('ui',320,180,30)]:
    name=f'{source}-{width}x{height}'
    toast()
    with socket.socket() as server:
        server.bind(('127.0.0.1',0));server.listen(1);server.settimeout(5)
        read_fd,write_fd=os.pipe();events=[]
        def read_events():
            with os.fdopen(read_fd) as pipe:
                for line in pipe:events.append(json.loads(line))
        reader=threading.Thread(target=read_events);reader.start()
        env=dict(os.environ,PRYSM_WORKER_CONFIG=json.dumps(dict(
            receiver=f'127.0.0.1:{server.getsockname()[1]}',source=source,
            compression='h264-hardware',metrics=True)),PRYSM_EVENTS_FD=str(write_fd),
            GST_REGISTRY=str(root/'registry.bin'))
        with (root/(name+'.log')).open('wb') as log:
            child=subprocess.Popen([str(root/'worker')],stdin=subprocess.PIPE,stdout=log,stderr=log,
                                   env=env,pass_fds=(write_fd,))
            os.close(write_fd)
            child.stdin.write(b'1');child.stdin.flush()
            try:
                connection,_=server.accept()
                with connection:
                    connection.settimeout(4)
                    size=struct.unpack('>I',exact(connection,4))[0];assert 1<=size<=128
                    assert json.loads(exact(connection,size))==dict(type='hello',version=version)
                    settings=json.dumps(dict(type='capture_settings',refresh_rate_hz=30,
                                             capture_resolution=[width,height])).encode()
                    connection.sendall(struct.pack('>I',len(settings))+settings)
                    phases=[];times=[]
                    with (root/(name+'.wire')).open('wb') as frames:
                        for index in range(count):
                            if source=='both' and index in [90,120,180]:
                                command={90:b'0',120:b'1',180:b'r'}[index]
                                child.stdin.write(command);child.stdin.flush()
                                if index==90:toast()
                            header=exact(connection,24)
                            magic,w,h,fmt,codec,size=struct.unpack('>4sIIIII',header)
                            assert (magic,w,h,fmt,codec)==(b'PRY3',width,height,0,2) and 0<size<=8388608
                            frames.write(header+exact(connection,size));times.append(time.monotonic())
                            if source=='both' and index in [89,119,179,239]:
                                text=metrics(name+'-'+str(index))
                                phases.append({c:encoded(text,c) for c in ['Video','Ui','Blended']})
                    text=metrics(name)
                    if source=='both':
                        assert phases[0]['Blended']>=85,phases
                        assert phases[1]['Ui']-phases[0]['Ui']>=28,phases
                        assert phases[2]['Blended']-phases[1]['Blended']>=58,phases
                        assert phases[3]['Blended']-phases[2]['Blended']>=55,phases
                        assert 'backend="VtcaptureBlended"} 1' in text,text
                    else:
                        assert encoded(text,'Video' if source=='video' else 'Ui')>=count-1
                        assert encoded(text,'Blended')==0
                    connection.shutdown(socket.SHUT_RDWR)
                assert child.wait(timeout=5)==0
                reader.join(timeout=2);assert not reader.is_alive()
                assert events[-1]['type']=='disconnected',events
                assert 'using OpenH264' not in (root/(name+'.log')).read_text(), 'software fallback'
                results.append(dict(name=name,frames=count,phases=phases,
                                    mean_frame_interval_ms=1000*(times[-1]-times[0])/(count-1)))
            finally:
                if child.poll() is None:child.kill();child.wait(timeout=2)
                child.stdin.close();reader.join(timeout=2)
(root/'results.json').write_text(json.dumps(results,indent=2))
print(json.dumps(results),flush=True)
