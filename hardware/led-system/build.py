#!/usr/bin/env python3
"""Rebuild and validate the three KiCad projects and fabrication packages."""
import csv
import heapq
import json
import math
import zipfile
from pathlib import Path
import uuid
import pcbnew as pcb

ROOT = Path(__file__).resolve().parent
KICAD = Path('/Applications/KiCad/KiCad.app/Contents/SharedSupport')
LIB = KICAD / 'footprints'
NS = uuid.UUID('6cac9905-7752-4b5a-bd9c-d9d46846978c')
def uid(s): return str(uuid.uuid5(NS, s))
def q(s): return json.dumps(str(s))
def mm(n): return pcb.FromMM(n)
def point(x, y): return pcb.VECTOR2I(mm(x), mm(y))
def xy(v): return (pcb.ToMM(v.x), pcb.ToMM(v.y))

RFP = 'Resistor_THT:R_Axial_DIN0207_L6.3mm_D2.5mm_P7.62mm_Horizontal'
VH2 = 'Connector_JST:JST_VH_B2P-VH-B_1x02_P3.96mm_Vertical'
FUSE = 'Fuse:Fuseholder_Blade_Mini_Keystone_3568'

# Explicit pin maps are the source of truth for both schematic and PCB.
ISO = 'Package_SO:SOIC-8_3.9x4.9mm_P1.27mm'
VH4 = 'Connector_JST:JST_VH_B4P-VH_1x04_P3.96mm_Vertical'
CAP = 'Capacitor_THT:C_Disc_D5.0mm_W2.5mm_P5.00mm'
HAT = [
 ('J1','Pi GPIO underside', 'Connector_PinSocket_2.54mm:PinSocket_2x20_P2.54mm_Vertical',
  {1:'+3V3_PI',6:'GND_PI',19:'MOSI_3V3',23:'SCLK_3V3'}, (108.37,74.77,-90), (45,55), 'gpio'),
 ('U1','ISO7720FD',ISO,
  {1:'+3V3_PI',2:'MOSI_3V3',3:'SCLK_3V3',4:'GND_PI',5:'GND_A',6:'CLOCK_RAW',7:'DATA_RAW',8:'+5V_A'},
  (131,88,0),(135,55),'isolator'),
 ('C1','100nF 16V',CAP,{1:'+3V3_PI',2:'GND_PI'},(125,86.095,-90),(210,55),'capacitor'),
 ('C2','100nF 16V',CAP,{1:'+5V_A',2:'GND_A'},(137,86.095,-90),(290,55),'capacitor'),
 ('R1','10k',RFP,{1:'MOSI_3V3',2:'GND_PI'},(110,84,0),(135,115),'resistor'),
 ('R2','10k',RFP,{1:'SCLK_3V3',2:'GND_PI'},(110,94,0),(210,115),'resistor'),
 ('R3','33R',RFP,{1:'DATA_RAW',2:'DATA_5V'},(141,85,0),(135,145),'resistor'),
 ('R4','33R',RFP,{1:'CLOCK_RAW',2:'CLOCK_5V'},(139,94,0),(210,145),'resistor'),
 ('J3','TOP LOCAL + D C G',VH4,{1:'+5V_A',2:'DATA_5V',3:'CLOCK_5V',4:'GND_A'},(155,93,90),(290,115),'conn4'),
]
POWER = [
 ('J1','5V INPUT / XT30','Connector_AMASS:AMASS_XT30UPB-M_1x02_P5.0mm_Vertical',
  {1:'GND',2:'+5V_IN'},(110,110,0),(45,55),'conn2'),
 ('F1','15A MINI',FUSE,{1:'+5V_IN',2:'+5V_BUS'},(125,110,0),(135,55),'fuse'),
 ('F2','7.5A MINI / GROUP A',FUSE,{1:'+5V_BUS',2:'+5V_A'},(145,130,0),(135,90),'fuse'),
 ('F3','7.5A MINI / GROUP B',FUSE,{1:'+5V_BUS',2:'+5V_B'},(145,150,0),(135,125),'fuse'),
 ('J2','GROUP A / VH2',VH2,{1:'+5V_A',2:'GND'},(162,130,0),(235,90),'conn2'),
 ('J3','GROUP B / VH2',VH2,{1:'+5V_B',2:'GND'},(162,150,0),(235,125),'conn2'),
 ('C1','1000uF 10V','Capacitor_THT:CP_Radial_D10.0mm_P5.00mm',
  {1:'+5V_BUS',2:'GND'},(122,160,90),(45,150),'capacitor'),
]
CORNER = [
 ('J1','RIGHT OUT + D C G',VH4,{1:'+5V_A',2:'DATA_A',3:'CLOCK_A',4:'GND_A'},(108,123,90),(45,55),'conn4'),
 ('U1','ISO7720FD',ISO,
  {1:'+5V_A',2:'DATA_A',3:'CLOCK_A',4:'GND_A',5:'GND_B',6:'CLOCK_RAW',7:'DATA_RAW',8:'+5V_B'},
  (131,117,0),(135,55),'isolator'),
 ('C1','100nF 16V',CAP,{1:'+5V_A',2:'GND_A'},(125,115.095,-90),(210,55),'capacitor'),
 ('C2','100nF 16V',CAP,{1:'+5V_B',2:'GND_B'},(137,115.095,-90),(290,55),'capacitor'),
 ('R1','10k',RFP,{1:'DATA_A',2:'GND_A'},(116,111,0),(135,115),'resistor'),
 ('R2','10k',RFP,{1:'CLOCK_A',2:'GND_A'},(116,126,0),(210,115),'resistor'),
 ('R3','33R',RFP,{1:'DATA_RAW',2:'DATA_B'},(141,117,0),(135,145),'resistor'),
 ('R4','33R',RFP,{1:'CLOCK_RAW',2:'CLOCK_B'},(139,126,0),(210,145),'resistor'),
 ('J2','BOTTOM IN + D C G',VH4,{1:'+5V_B',2:'DATA_B',3:'CLOCK_B',4:'GND_B'},(155,123,90),(290,115),'conn4'),
]
PROJECTS = {'gpio-interface': HAT, 'power-distribution': POWER, 'corner-interface': CORNER}

def pin_defs(kind):
 if kind=='gpio':
  return [(str(i), ('GND' if i==6 else 'MOSI' if i==19 else 'SCLK' if i==23 else str(i)),
           -10.16 if i%2 else 10.16, -((i-1)//2)*2.54,
           0 if i%2 else 180, 'output' if i in (19,23) else 'passive') for i in range(1,41)]
 if kind=='isolator':
  names=['VCC1','INA','INB','GND1','GND2','OUTB','OUTA','VCC2']
  return [(str(i),names[i-1],-10.16 if i<=4 else 10.16,
           -(i-1)*2.54 if i<=4 else -(8-i)*2.54,0 if i<=4 else 180,
           'output' if i in (6,7) else 'power_in' if i in (1,4,5,8) else 'input') for i in range(1,9)]
 if kind in ('resistor','capacitor','fuse'):
  return [('1','1',-7.62,0,0,'passive'),('2','2',7.62,0,180,'passive')]
 if kind=='flag': return [('1','pwr',0,0,90,'power_out')]
 count=int(kind[-1])
 return [(str(i),str(i),-7.62,-(i-1)*2.54,0,'passive') for i in range(1,count+1)]

def symbol_def(kind):
 pins=pin_defs(kind)
 graphics=''
 if kind=='resistor': graphics='(rectangle (start -3.81 1.27) (end 3.81 -1.27) (stroke (width 0.254) (type default)) (fill (type none)))'
 elif kind=='capacitor':
  graphics=''.join(f'(polyline (pts (xy {x} -2.54) (xy {x} 2.54)) (stroke (width 0.254) (type default)) (fill (type none)))' for x in (-.635,.635))
  graphics+=''.join(f'(polyline (pts (xy {a} 0) (xy {z} 0)) (stroke (width 0.254) (type default)) (fill (type none)))' for a,z in [(-3.81,-.635),(.635,3.81)])
 elif kind=='fuse': graphics='(rectangle (start -3.81 1.27) (end 3.81 -1.27) (stroke (width 0.254) (type default)) (fill (type none))) (polyline (pts (xy -5.08 0) (xy 5.08 0)) (stroke (width 0.254) (type default)) (fill (type none)))'
 elif kind!='flag':
  h=max(-p[3] for p in pins)+2.54
  w=7.62 if kind in ('gpio','isolator') else 5.08
  graphics=f'(rectangle (start {-w} 2.54) (end {w} {-h}) (stroke (width 0.254) (type default)) (fill (type background)))'
 pintext=''
 for num,name,x,y,angle,typ in pins:
  length=2.54 if kind not in ('resistor','capacitor','fuse','flag') else 3.81 if kind!='flag' else 0
  pintext+=f'(pin {typ} line (at {x} {y} {angle}) (length {length}) (name {q(name)} (effects (font (size 1 1)))) (number {q(num)} (effects (font (size 1 1)))))'
 return f'''(symbol "Prysm:{kind}" (pin_names (offset 0.5)) (in_bom yes) (on_board yes)
 (property "Reference" "X" (at 0 5.08 0) (effects (font (size 1.27 1.27))))
 (property "Value" {q(kind)} (at 0 3.81 0) (effects (font (size 1.27 1.27))))
 (symbol "{kind}_0_1" {graphics}) (symbol "{kind}_1_1" {pintext}))'''

def schematic(name, comps, notes):
 rootuid=uid(name+'/sheet')
 definitions='\n'.join(symbol_def(k) for k in sorted(set(c[-1] for c in comps)|{'flag'}))
 parts=[]
 allcomps=list(comps)
 flags = {'gpio-interface':['+3V3_PI','GND_PI','+5V_A','GND_A'],
          'corner-interface':['+5V_A','GND_A','+5V_B','GND_B'],
          'power-distribution':['+5V_IN','GND']}[name]
 allcomps += [('PWR'+str(i+1),'PWR_FLAG','',{1:net},None,(45+i*55,195),'flag')
              for i,net in enumerate(flags)]
 for ref,value,fp,nets,pos,(x,y),kind in allcomps:
  x,y=round(x/1.27)*1.27,round(y/1.27)*1.27
  su=uid(name+'/'+ref)
  hidden=' (hide yes)' if kind=='flag' else ''
  props=f'''(property "Reference" {q(ref)} (at {x} {y-7.62} 0) (effects (font (size 1.27 1.27)){hidden}))
  (property "Value" {q(value)} (at {x} {y-5.08} 0) (effects (font (size 1.27 1.27)){hidden}))
  (property "Footprint" {q(fp)} (at {x} {y} 0) (effects (font (size 1.27 1.27)) (hide yes)))'''
  pinuuids=' '.join(f'(pin {q(p[0])} (uuid {uid(name+"/"+ref+"/"+p[0])}))' for p in pin_defs(kind))
  parts.append(f'''(symbol (lib_id "Prysm:{kind}") (at {x} {y} 0) (unit 1) (in_bom {'no' if kind=='flag' else 'yes'}) (on_board {'no' if kind=='flag' else 'yes'}) (dnp no) (uuid {su}) {props} {pinuuids}
   (instances (project {q(name)} (path "/{rootuid}" (reference {q(ref)}) (unit 1)))))''')
  for num,pname,dx,dy,angle,typ in pin_defs(kind):
   px,py=round(x+dx,4),round(y-dy,4)
   net=nets.get(int(num))
   if net is None:
    parts.append(f'(no_connect (at {px} {py}) (uuid {uid(name+ref+num+"nc")}))')
   else:
    endx=round(px+(-5.08 if angle==0 else 5.08 if angle==180 else 0),4)
    endy=round(py+(0 if angle in (0,180) else -5.08),4)
    parts.append(f'(wire (pts (xy {px} {py}) (xy {endx} {endy})) (stroke (width 0) (type default)) (uuid {uid(name+ref+num+"wire")}))')
    parts.append(f'(global_label {q(net)} (shape bidirectional) (at {endx} {endy} {0 if angle==0 else 180 if angle==180 else 90}) (effects (font (size 1 1)) (justify {"right" if angle==0 else "left"})) (uuid {uid(name+ref+num+"label")}))')
 for i,note in enumerate(notes):
  parts.append(f'(text {q(note)} (at 35 {225+i*6} 0) (effects (font (size 1.27 1.27)) (justify left)) (uuid {uid(name+"note"+str(i))}))')
 text=f'''(kicad_sch (version 20250114) (generator "eeschema") (uuid {rootuid}) (paper "A3")
 (title_block (title {q('Prysm '+name)}) (date "2026-10-09") (rev "C prototype"))
 (lib_symbols {definitions}) {' '.join(parts)} (embedded_fonts no))'''
 out=ROOT/name
 out.mkdir(exist_ok=True)
 (out/(name+'.kicad_sch')).write_text(text)
 # Symbols are embedded; also provide a local library for editing without
 # depending on user library-table configuration.
 localdefs=definitions.replace('"Prysm:', '"')
 (out/'Prysm.kicad_sym').write_text('(kicad_symbol_lib (version 20241209) (generator "kicad_symbol_editor") '+localdefs+')')
 (out/'sym-lib-table').write_text('(sym_lib_table (lib (name "Prysm") (type "KiCad") (uri "${KIPRJMOD}/Prysm.kicad_sym") (options "") (descr "Prysm prototype symbols")))')
 return rootuid

def track(b, net, points, width=.35, layer=pcb.F_Cu):
 for a,z in zip(points,points[1:]):
  if a==z: continue
  t=pcb.PCB_TRACK(b); t.SetStart(point(*a)); t.SetEnd(point(*z))
  t.SetWidth(mm(width)); t.SetLayer(layer); t.SetNetCode(net.GetNetCode()); b.Add(t)

def text(b, value, x, y, size=1, layer=pcb.F_SilkS):
 t=pcb.PCB_TEXT(b);t.SetText(value);t.SetPosition(point(x,y));t.SetTextSize(point(size,size));t.SetTextThickness(mm(.15));t.SetLayer(layer);b.Add(t)

def zone(b, net, bounds, layer, solid=False):
 z=pcb.ZONE(b);b.Add(z);z.SetNet(net);z.SetLayer(layer)
 z.SetLocalClearance(mm(.3));z.SetMinThickness(mm(.25))
 z.SetPadConnection(pcb.ZONE_CONNECTION_FULL if solid else pcb.ZONE_CONNECTION_THERMAL)
 z.SetThermalReliefGap(mm(.25));z.SetThermalReliefSpokeWidth(mm(.3))
 x1,y1,x2,y2=bounds
 poly=z.Outline();poly.NewOutline()
 for x,y in ((x1,y1),(x2,y1),(x2,y2),(x1,y2)):poly.Append(mm(x),mm(y))


def footprint(b, name, comp, sheetuid):
 ref,value,fp,nets,(x,y,angle),_,kind=comp
 lib,item=fp.split(':')
 f=pcb.FootprintLoad(str(LIB/(lib+'.pretty')),item)
 assert f is not None, fp
 b.Add(f)
 f.SetFPID(pcb.LIB_ID(lib,item))
 f.SetReference(ref);f.SetValue(value);f.SetPosition(point(x,y));f.SetOrientationDegrees(angle)
 if ref=='J1' and name=='gpio-interface':
  # Match KiCad's official uHAT template: underside, pin 1 at 8.37,4.77.
  f.Flip(f.GetPosition(),True);f.SetOrientationDegrees(-90)
 f.SetPath(pcb.KIID_PATH('/'+sheetuid+'/'+uid(name+'/'+ref)))
 f.Reference().SetVisible(False);f.Value().SetVisible(False)
 for pad in f.Pads():
  n=nets.get(int(pad.GetNumber())) if pad.GetNumber().isdigit() else None
  if n:
   pad.SetNet(b.FindNet(n))
   if ref=='U1' and pad.GetNumber()=='1':pad.SetLocalZoneConnection(pcb.ZONE_CONNECTION_FULL)
  elif pad.GetNumber().isdigit():
   net=pcb.NETINFO_ITEM(b, 'unconnected-('+ref+'-Pad'+pad.GetNumber()+')');b.Add(net);pad.SetNet(net)
 return f

def makeboard(name, comps, sheetuid):
 b=pcb.BOARD()
 title=pcb.TITLE_BLOCK();title.SetTitle('Prysm '+name);title.SetRevision('C prototype');title.SetDate('2026-10-09');b.SetTitleBlock(title)
 settings=b.GetDesignSettings()
 settings.m_MinClearance=mm(.25);settings.m_TrackMinWidth=mm(.25)
 settings.m_CopperEdgeClearance=mm(.5);settings.m_HoleToHoleMin=mm(.25)
 b.SetCopperLayerCount(2)
 nets={n for c in comps for n in c[3].values()}
 for n in sorted(nets):b.Add(pcb.NETINFO_ITEM(b,n))
 fs={c[0]:footprint(b,name,c,sheetuid) for c in comps}
 W,H=(65,30) if name=='gpio-interface' else (65,40) if name=='corner-interface' else (75,80)
 ox,oy=(100,70) if name=='gpio-interface' else (100,100)
 for a,z in [((ox,oy),(ox+W,oy)),((ox+W,oy),(ox+W,oy+H)),((ox+W,oy+H),(ox,oy+H)),((ox,oy+H),(ox,oy))]:
  s=pcb.PCB_SHAPE();s.SetShape(pcb.SHAPE_T_SEGMENT);s.SetStart(point(*a));s.SetEnd(point(*z));s.SetLayer(pcb.Edge_Cuts);s.SetWidth(mm(.05));b.Add(s)
 holes=[(ox+3.5,oy+3.5),(ox+W-3.5,oy+3.5)]
 if name!='gpio-interface': holes +=[(ox+3.5,oy+H-3.5),(ox+W-3.5,oy+H-3.5)]
 for i,(x,y) in enumerate(holes):
  f=pcb.FootprintLoad(str(LIB/'MountingHole.pretty'),'MountingHole_2.7mm_M2.5')
  f.SetAttributes(pcb.FP_BOARD_ONLY | pcb.FP_EXCLUDE_FROM_BOM)
  f.SetReference('H'+str(i+1));f.SetPosition(point(x,y));f.Reference().SetVisible(False);f.Value().SetVisible(False);b.Add(f)
 return b,fs,(ox,oy,ox+W,oy+H)

def route_hat(b,fs,bounds,name):
 """Small 2-layer grid router; final KiCad DRC is authoritative."""
 step=.25; x0,y0,x1,y1=bounds
 NX,NY=round((x1-x0)/step)+1,round((y1-y0)/step)+1
 pads=[p for f in sorted(b.GetFootprints(),key=lambda f:f.GetReference())
       for p in sorted(f.Pads(),key=lambda p:p.GetNumber())]
 obstacles=[]
 for p in pads:
  bb=p.GetBoundingBox()
  obstacles.append((pcb.ToMM(bb.GetLeft()),pcb.ToMM(bb.GetTop()),pcb.ToMM(bb.GetRight()),pcb.ToMM(bb.GetBottom()),p.GetNetCode()))
 routes=[]
 for t in b.GetTracks():
  isvia=isinstance(t,pcb.PCB_VIA)
  for layer in (0,1) if isvia else (0 if t.GetLayer()==pcb.F_Cu else 1,):
   routes.append((xy(t.GetStart()),xy(t.GetEnd()),pcb.ToMM(t.GetWidth(pcb.F_Cu) if isvia else t.GetWidth()),layer,t.GetNetCode()))
 def grid(v):return (round((v[0]-x0)/step),round((v[1]-y0)/step))
 def real(v):return (x0+v[0]*step,y0+v[1]*step)
 def dseg(x,y,a,z):
  dx,dy=z[0]-a[0],z[1]-a[1];t=max(0,min(1,((x-a[0])*dx+(y-a[1])*dy)/(dx*dx+dy*dy))) if dx or dy else 0
  return math.hypot(x-a[0]-t*dx,y-a[1]-t*dy)
 for net in sorted({p.GetNetname() for p in pads if p.GetNetCode() and not p.GetNetname().startswith(('GND','+','unconnected-'))}, key=lambda n: (len([p for p in pads if p.GetNetname()==n]),n)):
  group=[p for p in pads if p.GetNetname()==net]
  joined=[group.pop(0)]
  nc=b.FindNet(net).GetNetCode()
  while group:
   pair=min(((a,z) for a in joined for z in group),key=lambda az: math.dist(xy(az[0].GetPosition()),xy(az[1].GetPosition())))
   source,target=map(lambda p:xy(p.GetPosition()),pair)
   start,finish=grid(source),grid(target)
   # Precompute clearance including track radius, plus grid/rounding margin.
   invalid=[set(),set()]
   for ax,ay,bx,by,n in obstacles:
    if n==nc:continue
    inflate=.55
    for i in range(max(0,math.floor((ax-inflate-x0)/step)),min(NX,math.ceil((bx+inflate-x0)/step)+1)):
     for j in range(max(0,math.floor((ay-inflate-y0)/step)),min(NY,math.ceil((by+inflate-y0)/step)+1)):
      invalid[0].add((i,j));invalid[1].add((i,j))
   for a,z,width,layer,n in routes:
    if n==nc:continue
    margin=width/2+.175+.35
    for i in range(max(0,math.floor((min(a[0],z[0])-margin-x0)/step)),min(NX,math.ceil((max(a[0],z[0])+margin-x0)/step)+1)):
     for j in range(max(0,math.floor((min(a[1],z[1])-margin-y0)/step)),min(NY,math.ceil((max(a[1],z[1])+margin-y0)/step)+1)):
      if dseg(*real((i,j)),a,z)<margin:invalid[layer].add((i,j))
   for layer in (0,1):invalid[layer].discard(start);invalid[layer].discard(finish)
   heap=[];dist={};prev={}
   source_layers=[i for i,l in enumerate((pcb.F_Cu,pcb.B_Cu)) if pair[0].IsOnLayer(l)]
   target_layers=[i for i,l in enumerate((pcb.F_Cu,pcb.B_Cu)) if pair[1].IsOnLayer(l)]
   for layer in source_layers:
    state=(*start,layer);dist[state]=0;heapq.heappush(heap,(0,0,state))
   end=None
   while heap:
    _,cost,state=heapq.heappop(heap)
    if cost!=dist.get(state):continue
    i,j,layer=state
    if (i,j)==finish and layer in target_layers:end=state;break
    for dx,dy,dl,extra in [(1,0,0,1),(-1,0,0,1),(0,1,0,1),(0,-1,0,1),(0,0,1,15)]:
     ii,jj,ll=i+dx,j+dy,layer^dl
     if not (3<=ii<NX-3 and 3<=jj<NY-3) or (ii,jj) in invalid[ll]:continue
     if dl:
      # Additional radius for a 0.7mm via. Cannot switch near other copper.
      vx,vy=real((i,j))
      if any(n!=nc and ax-.7<vx<bx+.7 and ay-.7<vy<by+.7 for ax,ay,bx,by,n in obstacles):continue
      if any(n!=nc and dseg(vx,vy,a,z)<width/2+.7 for a,z,width,rl,n in routes):continue
     nxt=(ii,jj,ll);new=cost+extra
     if new<dist.get(nxt,1e9):
      dist[nxt]=new;prev[nxt]=state;heur=abs(ii-finish[0])+abs(jj-finish[1]);heapq.heappush(heap,(new+heur,new,nxt))
   assert end, 'Could not route '+net
   path=[end]
   while path[-1] in prev:path.append(prev[path[-1]])
   path.reverse()
   pts=[(source,path[0][2])]+[(real(s[:2]),s[2]) for s in path]+[(target,path[-1][2])]
   for (a,layer),(z,nextlayer) in zip(pts,pts[1:]):
    if layer!=nextlayer:
     v=pcb.PCB_VIA(b);v.SetPosition(point(*a));v.SetWidth(mm(.7));v.SetDrill(mm(.3));v.SetViaType(pcb.VIATYPE_THROUGH);v.SetLayerPair(pcb.F_Cu,pcb.B_Cu);v.SetNetCode(nc);b.Add(v)
     routes +=[(a,a,.7,0,nc),(a,a,.7,1,nc)]
    elif a!=z:
     track(b,b.FindNet(net),[a,z],.35,pcb.F_Cu if layer==0 else pcb.B_Cu);routes.append((a,z,.35,layer,nc))
   joined.append(pair[1]);group.remove(pair[1])
 # Separate local returns; no copper path between Pi/A or A/B domains.
 left,right=('GND_PI','GND_A') if name=='gpio-interface' else ('GND_A','GND_B')
 left_bounds=(x0+.6,y0+.6,130,y1-.6)
 right_bounds=(132,y0+8 if name=='gpio-interface' else y0+.6,x1-.6,y1-.6)
 zone(b,b.FindNet(left),left_bounds,pcb.F_Cu,True)
 zone(b,b.FindNet(right),right_bounds,pcb.F_Cu,True)
 zone(b,b.FindNet('+3V3_PI' if name=='gpio-interface' else '+5V_A'),left_bounds,pcb.B_Cu,True)
 zone(b,b.FindNet('+5V_A' if name=='gpio-interface' else '+5V_B'),right_bounds,pcb.B_Cu,True)


def stable_ids(b,name):
 # KiCad sorts objects by UUID when saving, so assign identities before serialization.
 def key(item):
  start=xy(item.GetStart()) if hasattr(item,'GetStart') else (0,0)
  end=xy(item.GetEnd()) if hasattr(item,'GetEnd') else (0,0)
  return (type(item).__name__,item.GetLayer(),xy(item.GetPosition()),start,end,
          item.GetText() if hasattr(item,'GetText') else '')
 for f in sorted(b.GetFootprints(),key=lambda f:f.GetReference()):
  base=name+'/'+f.GetReference()
  f.SetUuid(pcb.KIID(uid(base)))
  for i,item in enumerate(sorted(list(f.Pads())+list(f.GraphicalItems())+list(f.GetFields()),key=key)):
   item.SetUuid(pcb.KIID(uid(base+'/item/'+str(i))))
 for kind,items in [('drawing',b.GetDrawings()),('track',b.GetTracks()),('zone',b.Zones())]:
  for i,item in enumerate(sorted(items,key=key)):
   item.SetUuid(pcb.KIID(uid(name+'/'+kind+'/'+str(i))))

def build():
 for name,comps in PROJECTS.items():
  notes = {
   'gpio-interface':['Pi side: header1=3.3V,6=GND,19=MOSI,23=SCLK. Pi 5V pins remain NC.',
    'ISO7720FD narrow SOIC8, default LOW. Isolated level translation; no AHCT125 substitution.',
    'J3: 1=local TOP +5V_A, 2=DATA, 3=CLOCK, 4=local TOP GND_A.',
    'No distribution-to-GPIO cable. No copper connection between GND_PI and GND_A.',
    '16AWG power/ground at J3; this connector powers only U1 side2. Verify cooler clearance.'],
   'power-distribution':['5V DC ONLY. LPV-100-5: 5V 12A 60W; this is not a 15A supply.',
    'A=TOP+RIGHT; B=BOTTOM+LEFT. Max90 LEDs per group. User cap75% linear output.',
    'F1=15A; F2/F3=7.5A MINI. Fuse clearing with hiccup supply requires measurement.',
    '2oz both copper layers. VH pin1=+5V, pin2=GND; XT30 pin1=GND.',
    'No logic output: GPIO and corner isolators take power locally from strip ends.'],
   'corner-interface':['BOTTOM-RIGHT active corner: RIGHT outputs -> J1 -> ISO7720FD -> J2 -> BOTTOM inputs.',
    'Both connectors: 1=LOCAL +5V, 2=DATA, 3=CLOCK, 4=LOCAL GND. Never swap A/B.',
    'No copper +5V or GND bridge between A and B. Do not fit a parallel passive L.',
    'Use16AWG power/ground leads. Each IC side draws only its own local logic current.',
    'Low-voltage return-path separation only; not a mains safety barrier.']
  }[name]
  rootuid=schematic(name,comps,notes)
  b,fs,bounds=makeboard(name,comps,rootuid)
  if name!='power-distribution':
   # Short supply stubs to broad local planes; no thin logic-power cable from distribution.
   for pin,x in [('1',126.5),('8',135.5)]:
    pad=next(p for p in fs['U1'].Pads() if p.GetNumber()==pin)
    target=(x,pcb.ToMM(pad.GetPosition().y))
    track(b,b.FindNet(pad.GetNetname()),[xy(pad.GetPosition()),target],.6)
    v=pcb.PCB_VIA(b);v.SetPosition(point(*target));v.SetWidth(mm(1.2));v.SetDrill(mm(.6));v.SetViaType(pcb.VIATYPE_THROUGH);v.SetLayerPair(pcb.F_Cu,pcb.B_Cu);v.SetNetCode(pad.GetNetCode());b.Add(v)
   route_hat(b,fs,bounds,name)
   if name=='gpio-interface':
    text(b,'PRYSM GPIO C',126,98,.8)
    text(b,'U1 ISO7720FD',131,96,.8)
    text(b,'J3 TOP',155,77,.8)
    for value,x,y in [('C1',125,83),('C2',137,83),('R1 10k',112,81.5),('R2 10k',112,91.5),('R3 33R',142,82.5),('R4 33R',142,91.5)]:text(b,value,x,y,.8)
    for value,y in [('+',93),('D',89.04),('C',85.08),('G',81.12)]:text(b,value,162.5,y,.8)
   else:
    text(b,'PRYSM CORNER C',132,136,1)
    text(b,'A RIGHT OUT',116,104,.8);text(b,'B BOTTOM IN',148,104,.8)
    text(b,'U1 ISO7720FD',131,132,.8)
    for value,x,y in [('C1',125,112),('C2',137,112),('R1 10k',119,108.5),('R2 10k',119,123.5),('R3 33R',144,114.5),('R4 33R',143,123.5)]:text(b,value,x,y,.8)
    for value,y in [('+5',123),('D',119.04),('C',115.08),('G',111.12)]:
     text(b,value,103,y,.8);text(b,value,162.5,y,.8)
  else:
   track(b,b.FindNet('+5V_IN'),[(115,110),(120,110)],3)
   track(b,b.FindNet('+5V_IN'),[(120,110),(125,110),(125,113.4)],8)
   track(b,b.FindNet('+5V_BUS'),[(134.92,110),(143,110),(143,153.4)],8)
   zone(b,b.FindNet('+5V_BUS'),(139,118,148,154),pcb.F_Cu,True)
   for y,n in [(130,'+5V_A'),(150,'+5V_B')]:
    track(b,b.FindNet(n),[(154.92,y),(154.92,y+3.4)],3)
    track(b,b.FindNet(n),[(154.92,y),(162,y)],3)
   track(b,b.FindNet('+5V_BUS'),[(143,150),(134,160),(122,160)],3)
   zone(b,b.FindNet('GND'),(100.6,100.6,174.4,179.4),pcb.B_Cu,True)
   text(b,'PRYSM POWER rev C',133,104,1.2)
   text(b,'5V DC ONLY / 2oz Cu',132,177,1)
   for ref,value,x,y in [('J1','XT30 INPUT',110,117),('F1','F1 MAIN 15A',129,120),('F2','F2 A 7.5A',148,140),('F3','F3 B 7.5A',148,160)]:text(b,value,x,y,.9)
   for y in (130,150):text(b,'+  G',164, y-5,.9)
  stable_ids(b,name)
  b.BuildConnectivity();pcb.ZONE_FILLER(b).Fill(b.Zones())
  out=ROOT/name
  pcb.SaveBoard(str(out/(name+'.kicad_pcb')),b)
  boardpath=out/(name+'.kicad_pcb')
  copper=.07 if name=='power-distribution' else .035
  stackup=f'(stackup (layer "F.SilkS" (type "Top Silk Screen")) (layer "F.Mask" (type "Top Solder Mask") (thickness 0.01)) (layer "F.Cu" (type "copper") (thickness {copper})) (layer "dielectric 1" (type "core") (thickness {1.6-2*copper-.02}) (material "FR4") (epsilon_r 4.5) (loss_tangent 0.02)) (layer "B.Cu" (type "copper") (thickness {copper})) (layer "B.Mask" (type "Bottom Solder Mask") (thickness 0.01)) (layer "B.SilkS" (type "Bottom Silk Screen")) (copper_finish "HASL lead-free") (dielectric_constraints no))'
  boardpath.write_text(boardpath.read_text().replace('(setup', '(setup '+stackup,1))
  # Project defaults keep ERC/DRC enabled; no violations are suppressed.
  pro={'meta':{'filename':name+'.kicad_pro','version':1},'board':{'design_settings':{'rules':{'min_clearance':.25,'min_track_width':.25,'min_copper_edge_clearance':.5,'min_hole_clearance':.25,'min_hole_to_hole':.25}}}}
  (out/(name+'.kicad_pro')).write_text(json.dumps(pro,indent=2)+'\n')
  with (out/'connections.csv').open('w') as f:
   w=csv.writer(f);w.writerow(['Reference','Pin','Net'])
   for c in comps:
    for pin,net in sorted(c[3].items()):w.writerow([c[0],pin,net])
  print('Created',name,flush=True)

def deliver():
 import verify
 verify.check(ROOT,publish=True)
 for name in PROJECTS:
  out=ROOT/name
  board=out/(name+'.kicad_pcb')
  fab=out/'fabrication';fab.mkdir(exist_ok=True)
  verify.run('pcb','export','gerbers','--layers','F.Cu,B.Cu,F.SilkS,B.SilkS,F.Mask,B.Mask,Edge.Cuts',
             '--check-zones','-o',str(fab)+'/',board)
  verify.run('pcb','export','drill','--format','excellon','--excellon-separate-th','-o',str(fab)+'/',board)
  verify.run('sch','export','svg','-o',str(out/'schematic-preview')+'/',out/(name+'.kicad_sch'))
  for side in ('F','B'):
   verify.run('pcb','export','svg','--layers',side+'.Cu,Edge.Cuts','--page-size-mode','2',
              '--mode-single','--exclude-drawing-sheet','-o',out/('copper-'+side+'.svg'),board)
  verify.run('pcb','render','--side','top','--width','1400','--height','900','--quality','basic',
             '-o',out/'preview.png',board)
  with zipfile.ZipFile(out/'fabrication.zip','w',zipfile.ZIP_DEFLATED) as z:
   for path in sorted(fab.iterdir()):z.write(path,path.name)
 files=[ROOT/p for p in ('README.txt','bom.csv','build.py','verify.py','review-prompt.txt')]
 files += [p for name in PROJECTS for p in (ROOT/name).rglob('*')
           if p.is_file() and p.suffix not in ('.kicad_prl','.bak') and not p.name.startswith('.')]
 manifest={'revision':'C prototype','kicad':verify.run('version').strip(),
           'sha256':{str(p.relative_to(ROOT)):verify.sha256(p) for p in sorted(files)}}
 (ROOT/'build-manifest.json').write_text(json.dumps(manifest,indent=2)+'\n')
 with zipfile.ZipFile(ROOT/'prysm-led-system-rev-c.zip','w',zipfile.ZIP_DEFLATED) as z:
  for path in sorted(files+[ROOT/'build-manifest.json']):z.write(path,str(path.relative_to(ROOT)))
 verify.check_package(ROOT)

if __name__=='__main__':
 build()
 deliver()
