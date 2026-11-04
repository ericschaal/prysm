import socket,time,sys
s=socket.socket(socket.AF_PACKET,socket.SOCK_DGRAM,socket.htons(3))
s.settimeout(.5)
times=[]; changed=0; prev=None; dst=socket.inet_aton(sys.argv[1]); end=time.monotonic()+15
while time.monotonic()<end:
 try:b,a=s.recvfrom(2048)
 except socket.timeout:continue
 if a[2]!=4 or len(b)<38 or b[9]!=17 or b[16:20]!=dst:continue
 h=(b[0]&15)*4
 if b[h+2:h+4]!=bytes((15,208)) or len(b)-h-8!=10+162*3:continue
 pixels=b[h+18:];times.append(time.monotonic())
 if prev is not None and pixels!=prev:changed+=1
 prev=pixels
assert len(times)>2
gaps=sorted(1000*(b-a) for a,b in zip(times,times[1:]))
print("packets",len(times),"fps",(len(times)-1)/(times[-1]-times[0]),"changed",changed,"p95_gap_ms",gaps[int(.95*len(gaps))],"max_gap_ms",gaps[-1])
