import re,sys,statistics as st
from pathlib import Path
for file in sys.argv[1:]:
 txt=Path(file).read_text()
 data=[list(map(int,match.groups())) for match in re.finditer(r'PROFILE_MAIN (\d+) (\d+) (\d+) (\d+) (\d+)',txt)]
 sampler=[list(map(int,m.groups())) for m in re.finditer(r'PROFILE_SAMPLER (\d+) (\d+)',txt)]
 assert len(data)==len(sampler),(len(data),len(sampler))
 groups={k:[] for k in ['acquire','convert','processor','DDP','grid','projection','other_processor','work','interval']}
 for i,((t,c,r,p,d),(g,w)) in enumerate(zip(data,sampler)):
    if i==0 or t<5000000:continue
    for k,v in zip(groups,[c,r,p,d,g,w,p-g-w,c+r+p+d,t-data[i-1][0]]):
      groups[k].append(v/1000)
 print(file, 'n=',len(groups['interval']))
 for k,v in groups.items():
    print(f'{k:16s} mean {st.mean(v):6.2f} med {st.median(v):6.2f} p95 {st.quantiles(v,n=100)[94]:6.2f} max {max(v):6.2f}')
 print('late intervals >50ms:',sum(x>50 for x in groups['interval']))
