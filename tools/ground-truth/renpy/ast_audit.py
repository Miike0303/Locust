import pickle,io,zlib,struct,json,collections
from pathlib import Path
G=Path(__file__).parent
class Index(pickle.Unpickler):
 def find_class(self,m,n):raise ValueError((m,n))
class Dummy:
 def __new__(cls,*a,**k):return object.__new__(cls)
 def __init__(self,*a,**k):self._args=a
 def __setstate__(self,state):
  self._state=state
  if type(self).__name__=='PyCode' and isinstance(state,tuple) and len(state)>=4:
   self.source=state[1];self.location=state[2];self.mode=state[3]
  if isinstance(state,dict):self.__dict__.update(state)
  elif isinstance(state,tuple):
   for s in state:
    if isinstance(s,dict):self.__dict__.update(s)
class RevDict(dict):
 def __setstate__(self,s):self._state=s
class RevList(list):
 def __setstate__(self,s):self._state=s
class RevSet(set):
 def __setstate__(self,s):self._state=s
class Expr(str):
 def __new__(cls,*a):return str.__new__(cls,str(a[0]) if a else '')
 def __setstate__(self,s):self._state=s
TYPES={}
class Inert(pickle.Unpickler):
 def find_class(self,m,n):
  if (m,n) not in TYPES:TYPES[m,n]=type(n,(Expr if n in ['PyExpr','PyExprCache'] else RevDict if n in ['RevertableDict','OrderedDict'] else RevList if n=='RevertableList' else RevSet if n=='RevertableSet' else Dummy,),{'_module':m})
  return TYPES[m,n]
def payload(data):
 if not data.startswith(b'RENPY RPC2'):return None
 i=10
 while i+12<=len(data):
  slot,off,n=struct.unpack_from('<III',data,i);i+=12
  if slot==1:return zlib.decompress(data[off:off+n])
  if slot==0:break

RECORD_COUNTS=collections.Counter()
def record(strings,text,context):
 sig=(text,context['owner'],context['field'],context['expr'])
 if RECORD_COUNTS[sig]<3:strings[text].append(context)
 RECORD_COUNTS[sig]+=1
def walk(obj,seen,strings,location=None,owner='',field=''):
 if isinstance(obj,str):
  record(strings,obj,{'owner':owner,'field':field,'location':location,'expr':isinstance(obj,Expr)})
  return
 if id(obj) in seen:return
 seen.add(id(obj))
 if isinstance(obj,Dummy):
  owner=type(obj).__name__
  if getattr(obj,'filename',None) and getattr(obj,'linenumber',None):location=[obj.filename,obj.linenumber]
  if owner=='PyCode' and getattr(obj,'location',None):location=list(obj.location[:2])
  for k,v in obj.__dict__.items():
   if k.startswith('_'):continue
   record(strings,k,{'owner':'AST field name','field':'key','location':location,'expr':False})
   if owner=='Menu' and k=='items':
    for item in v:
     if isinstance(item,(tuple,list)) and len(item)==3:
      walk(item[0],seen,strings,location,owner,'caption');walk(item[1],seen,strings,location,owner,'condition');walk(item[2],seen,strings,location,owner,'block')
   else:walk(v,seen,strings,location,owner,k)
 elif isinstance(obj,dict):
  for k,v in obj.items():walk(v,seen,strings,location,owner,str(k))
 elif isinstance(obj,(tuple,list,set)):
  for v in obj:walk(v,seen,strings,location,owner,field)

def load_game(r):
 RECORD_COUNTS.clear();strings=collections.defaultdict(list);errors=[];files=0
 for a in Path(r['game']).glob('*.rpa'):
  with a.open('rb') as f:
   hd=f.readline().split()
   if len(hd)<3:continue
   off=int(hd[1],16);key=int(hd[2],16);f.seek(off)
   try:idx=Index(io.BytesIO(zlib.decompress(f.read()))).load()
   except Exception as e:errors.append([str(a),str(e)]);continue
   for name,segs in idx.items():
    if isinstance(name,bytes):name=name.decode()
    if not name.endswith('.rpyc') or name.startswith('tl/'):continue
    data=b''
    for seg in segs:
     offset,length=seg[:2];f.seek(offset^key);prefix=seg[2] if len(seg)>2 else b''
     if isinstance(prefix,str):prefix=prefix.encode()
     data+=prefix+f.read(length^key)
    p=payload(data)
    if p is None:continue
    out=G/'ast-pickles'/r['name']/name
    out.parent.mkdir(parents=True,exist_ok=True);out.write_bytes(p)
    try:obj=Inert(io.BytesIO(p)).load();walk(obj,set(),strings);files+=1
    except Exception as e:errors.append([name,str(e)])
 return strings,errors,files
extras=[json.loads(l) for l in (G/'extras.jsonl').read_text('utf8').splitlines()]
misses=[json.loads(l) for l in (G/'misses.jsonl').read_text('utf8').splitlines()]
verified=[];missverified=[];summary=[]
for r in json.loads((G/'selected.json').read_text()):
 es=[e for e in extras if e['game']==r['name'] and 'rpyc' in json.loads(e['tags'])]
 if not es:continue
 strings,errors,files=load_game(r)
 (G/(r['name']+'.ast-errors.json')).write_text(json.dumps(errors,indent=2))
 counts=collections.Counter()
 for e in es:
  contexts=strings.get(e['source'],[])
  visible=[c for c in contexts if (c['owner']=='Say' and c['field']=='what') or (c['owner']=='Menu' and c['field']=='caption')]
  code=[c for c in contexts if c['expr'] or (c['owner']=='PyCode' and c['field']=='source') or c['owner']=='AST field name' or (c['owner']=='Say' and c['field'] in ['attributes','temporary_attributes']) or (c['owner']=='Image' and c['field']=='imgname') or (c['owner']=='PyCode' and c['field']=='mode') or (c['owner']=='Label' and c['field']=='name') or c['field'] in ['who','condition','filename','label','expression','target']]
  if visible:cls='compiled visible text absent from tl'
  elif code:cls='compiled code/expression or locator';e['ast_contexts']=code[:3]
  elif contexts:cls='compiled other AST fields (unresolved)';e['ast_contexts']=contexts[:3]
  else:cls='compiled string not mapped (unresolved)'
  e.setdefault('ast_contexts',visible[:3]);e['verified_class']=cls;counts[cls]+=1;verified.append(e)
 for b in [b for b in misses if b['game']==r['name'] and b['class']=='compiled/archive source unavailable']:
  contexts=strings.get(b['text'],[]);say=[c for c in contexts if c['owner']=='Say' and c['field']=='what']
  b['ast_contexts']=say[:3];b['verified_class']='compiled Say.what omitted by heuristic' if say else 'compiled TL mismatch/unresolved'
  missverified.append(b)
 summary.append({'game':r['name'],'ast_files':files,'errors':len(errors),'extras':dict(counts),'misses':dict(collections.Counter(b['verified_class'] for b in missverified if b['game']==r['name']))})
 print(summary[-1],flush=True)
for name,ls in [('ast-extras',verified),('ast-misses',missverified)]:
 with (G/(name+'.jsonl')).open('w',encoding='utf8') as f:
  for x in ls:f.write(json.dumps(x,ensure_ascii=False)+'\n')
(G/'ast-summary.json').write_text(json.dumps(summary,ensure_ascii=False,indent=2),encoding='utf8')

