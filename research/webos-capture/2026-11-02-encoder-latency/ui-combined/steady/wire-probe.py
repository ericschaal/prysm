"""Bounded TV-local receiver for staged workers. Run under an external watchdog."""
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

def ticks(pid):
    fields = pathlib.Path('/proc',str(pid),'stat').read_text().rsplit(')',1)[1].split()
    return int(fields[11])+int(fields[12])

results=[]
for backend in ['baseline', 'combined']:
    for width,height in [(640,360)]:
        fps=30 if width==640 else 15
        count=240
        name=f'{backend}-{width}x{height}'
        toast=subprocess.Popen(['luna-send','-a','io.prysm.capture','-n','1','-w','4000','luna://com.webos.notification/createToast',json.dumps(dict(message='Prysm '+backend+' capture test'))],stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.PIPE)
        try:
            toast.wait(timeout=6)
            response=json.loads(toast.stdout.read());assert response['returnValue'],response
            (root/(name+'.toast.json')).write_text(json.dumps(response))
        finally:
            if toast.poll() is None:toast.kill();toast.wait()
            toast.stdin.close();toast.stdout.close();toast.stderr.close()
        with socket.socket() as server:
            server.bind(('127.0.0.1',0));server.listen(1);server.settimeout(5)
            read_fd,write_fd=os.pipe();events=[]
            def read_events():
                with os.fdopen(read_fd) as pipe:
                    for line in pipe: events.append(json.loads(line))
            reader=threading.Thread(target=read_events);reader.start()
            env=os.environ.copy()
            env.update(PRYSM_WORKER_CONFIG=json.dumps(dict(receiver=f'127.0.0.1:{server.getsockname()[1]}',source='video' if backend=='combined' else 'both',compression='h264-hardware',metrics=True)),PRYSM_EVENTS_FD=str(write_fd),GST_REGISTRY=str(root/'registry.bin'),GST_DEBUG='rvenc:4',GST_DEBUG_NO_COLOR='1')
            with open(root/(name+'.log'),'wb') as log:
                child=subprocess.Popen([str(root/('combined-worker' if backend=='combined' else 'worker'))],stdin=subprocess.PIPE,stdout=log,stderr=log,env=env,pass_fds=(write_fd,))
                os.close(write_fd)
                child.stdin.write(b'1');child.stdin.flush()
                try:
                    connection,_=server.accept()
                    with connection:
                        connection.settimeout(4)
                        size=struct.unpack('>I',exact(connection,4))[0];assert 1<=size<=128
                        assert json.loads(exact(connection,size))==dict(type='hello',version=version)
                        settings=json.dumps(dict(type='capture_settings',refresh_rate_hz=fps,capture_resolution=[width,height])).encode()
                        connection.sendall(struct.pack('>I',len(settings))+settings)
                        started=time.monotonic();timings=[];cpu_start=None
                        with open(root/(name+'.wire'),'wb') as frames:
                            for index in range(count):
                                header=exact(connection,24)
                                magic,w,h,fmt,codec,size=struct.unpack('>4sIIIII',header)
                                assert magic==b'PRY'+str(version).encode() and (w,h,fmt,codec)==(width,height,0,2) and 0<size<=8388608
                                payload=exact(connection,size);frames.write(header+payload);timings.append(time.monotonic())
                                if index==19:
                                    (root/(name+'.warm.metrics')).write_bytes(urllib.request.urlopen('http://127.0.0.1:9101/metrics',timeout=2).read())
                                    cpu_start=ticks(child.pid);sample_start=time.monotonic()
                        elapsed=time.monotonic()-sample_start
                        cpu=(ticks(child.pid)-cpu_start)/os.sysconf('SC_CLK_TCK')/elapsed*100
                        metrics=urllib.request.urlopen('http://127.0.0.1:9101/metrics',timeout=2).read()
                        (root/(name+'.metrics')).write_bytes(metrics)
                        teardown=time.monotonic()
                        if width==640: connection.shutdown(socket.SHUT_RDWR)
                        else: child.send_signal(signal.SIGTERM)
                    assert child.wait(timeout=5)==0,'worker failed or native teardown timed out'
                    reader.join(timeout=2);assert not reader.is_alive()
                    assert any(e['type']=='capturing' for e in events)
                    assert events[-1]['type']=='disconnected'
                    if backend in ['baseline', 'combined']:
                        assert any(e['type']=='encoder_busy' for e in events),'hardware never attempted'
                        assert b'using OpenH264' not in (root/(name+'.log')).read_bytes(),'software fallback invalidates hardware sample'
                    else:
                        assert not any(e['type']=='encoder_busy' for e in events),'software selection attempted hardware'
                    results.append(dict(name=name,frames=count,steady_cpu_percent=cpu,steady_wall_seconds=elapsed,first_frame_ms=(timings[0]-started)*1000,frame_interval_ms=(timings[-1]-timings[19])*1000/(count-20),teardown_ms=(time.monotonic()-teardown)*1000,events=events))
                except BaseException:
                    print((root/(name+'.log')).read_text(errors='replace'),file=sys.stderr)
                    raise
                finally:
                    if child.poll() is None:
                        child.kill();child.wait(timeout=2)
                    child.stdin.close();reader.join(timeout=2)
(root/'results.json').write_text(json.dumps(results,indent=2))
print(json.dumps([{k:v for k,v in r.items() if k!='events'} for r in results]),file=sys.stderr)
