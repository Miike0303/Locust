import json,re,collections
from pathlib import Path
from audit import source_map,LIT
G=Path(__file__).parent
extras=[json.loads(l) for l in (G/'extras.jsonl').read_text('utf8').splitlines()]
ms=[json.loads(l) for l in (G/'classified-misses.jsonl').read_text('utf8').splitlines()]
for b in ms:
 if b['class']=='other parser rejection':
  m=LIT.search(b['current'])
  raw=m[0][1:-1] if m else ''
  if ('/' in raw or '\\' in raw) and ' ' not in raw:b['class']='escaped no-space dialogue misidentified as path'
asts={ (e['game'],e['id']):e for e in (json.loads(l) for l in (G/'ast-extras.jsonl').read_text('utf8').splitlines()) }
for r in json.loads((G/'selected.json').read_text()):
 src,origin,_=source_map(r);pyrefs=set()
 for file,ls in src.items():
  active=False;ind=0
  for n,line in enumerate(ls,1):
   t=line.strip();at=len(line)-len(line.lstrip())
   if not t or t.startswith('#'):continue
   if active and at<=ind:active=False
   if re.match(r'^(?:init\b[^:]*\s)?python\b[^:]*:',t):active=True;ind=at
   elif active:pyrefs.add((file,n))
 for e in [x for x in extras if x['game']==r['name']]:
  if (e['game'],e['id']) in asts:
   a=asts[e['game'],e['id']];e['verified_class']=a['verified_class'];e['ast_contexts']=a.get('ast_contexts',[])
  elif e.get('ref') and tuple(e['ref']) in pyrefs:e['verified_class']='loose Python block literal misclassified as dialogue'
  else:e['verified_class']='live unmatched: tl coverage or non-dialogue context unresolved'
with (G/'classified-extras.jsonl').open('w',encoding='utf8') as f:
 for x in extras:f.write(json.dumps(x,ensure_ascii=False)+'\n')
with (G/'classified-misses.jsonl').open('w',encoding='utf8') as f:
 for x in ms:f.write(json.dumps(x,ensure_ascii=False)+'\n')
print('MISSES',collections.Counter(x['class'] for x in ms));print('EXTRAS',collections.Counter(x['verified_class'] for x in extras))
