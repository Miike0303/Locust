import json,re,collections,sqlite3
from pathlib import Path
from audit import source_map,parse_tl,statement,norm,refpath
G=Path(__file__).parent
out=[]
for r in json.loads((G/'selected.json').read_text()):
 gt,old,*_=parse_tl(r);src,origin,_=source_map(r)
 conn=sqlite3.connect('file:'+str(G/(r['name']+'.locust.db'))+'?mode=ro',uri=True)
 texts={norm(e[0]) for e in conn.execute('select source from strings')}
 choices={};missed=[];unlisted_ui=[]
 byfile={}
 for file,ls in src.items():
  byfile[file]=[]
  for n,line in enumerate(ls,1):
   code=line.strip();p=statement(code)
   if p and not p[0] and p[2].endswith(':'):byfile[file].append((n,p[1],code))
 for b in old:
  if not b['ref']:continue
  file,n=b['ref']
  for at,txt,code in byfile.get(file,[]):
   if txt==b['text']:choices[file,at,txt]=dict(b,ref=[file,at],statement=code,origin=origin[file])
 for b in choices.values():
  if b['text'] not in texts:missed.append(b)
 out.append({'game':r['name'],'live_tl_menu_choices':len(choices),'text_covered':len(choices)-len(missed),'misses':missed})
 print(r['name'],len(choices),len(missed),flush=True)
(G/'menu-summary.json').write_text(json.dumps(out,ensure_ascii=False,indent=2),encoding='utf8')
