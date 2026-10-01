from pathlib import Path
import json,re,sqlite3,collections,hashlib,subprocess,os,shutil,uuid
ROOT=Path(__file__).resolve().parent
GAME=Path(r'D:\juegos\unity\Out of Touch')
SCRIPTS=GAME/'OoT_Data/SCRIPTS~'
EXE=ROOT/'target/debug/locust.exe'
ENV=dict(os.environ,LOCUST_DATA_DIR=str(ROOT/'locust-data'),LOCUST_RESEARCH_LOCK_DIR=str(ROOT/'locks'))
def dump(name,v):(ROOT/name).write_text(json.dumps(v,ensure_ascii=True,indent=2),encoding='utf-8')
def rows(db):
    with sqlite3.connect(db) as c:
        c.row_factory=sqlite3.Row
        return [dict(r) for r in c.execute('select * from strings')]
def run(args,log):
    p=subprocess.run([str(EXE),*map(str,args)],env=ENV,capture_output=True)
    (ROOT/log).write_bytes(p.stdout+p.stderr)
    assert p.returncode==0,(args,p.returncode)
def hashes(files):return {str(p):hashlib.sha256(p.read_bytes()).hexdigest() for p in files}
def quoted(s):
    m=re.search(r'"((?:\\.|[^"\\])*)"',s)
    return m.group(1) if m else None
files=sorted(SCRIPTS.rglob('*.txt'))
before=hashes(files)
chars=set()
for line in (SCRIPTS/'Characters.txt').read_text(encoding='utf-8-sig').splitlines():
    m=re.match(r'\s*character\s+(\S+)',line)
    if m:chars.add(m[1])
truth=[];rawlines={}
for f in files:
    for n,line in enumerate(f.read_text(encoding='utf-8-sig').splitlines(),1):
        s=line.strip();rawlines[(str(f),n)]=s
        tokens=s.split(maxsplit=1)
        if not tokens:continue
        token=tokens[0];text=tokens[1] if len(tokens)>1 else ''
        cl=None
        if token in chars and text:cl='script_dialogue'
        elif token=='button' and quoted(text) is not None:cl='script_menu';text=quoted(text)
        elif token=='character' and quoted(text) is not None:cl='character_display_name';text=quoted(text)
        if cl:truth.append(dict(file=str(f),path=f'line:{n}',line=n,text=text,cls=cl))
allrows=rows(ROOT/'oot-alone.db')
rs=[r for r in allrows if Path(r['file_path']).suffix.lower()=='.txt' and '#' in r['id']]
lookup={(str(Path(r['file_path'])),int(r['id'].rsplit('#',1)[1])):r for r in rs}
miss=[];hit=[]
for t in truth:
    (hit if (t['file'],t['line']) in lookup else miss).append(t)
keys={(t['file'],t['line']) for t in truth}
fps=[];unknown=[]
for (f,n),r in lookup.items():
    if (f,n) not in keys:unknown.append(dict(file=f,path=f'line:{n}',text=r['source'],raw=rawlines.get((f,n)),cls='undeclared_speaker_or_continuation'))
for t in miss:
    if t['cls']=='script_dialogue':
        t['cls']='dialogue_inline_sprite' if t['text'].startswith('+') else 'dialogue_single_character' if len(t['text'])<2 else 'dialogue_other'
# A real-copy inject probe: only selected script files and an untouched definitions file.
selected=[]
for r in rs:
    f=Path(r['file_path']);n=int(r['id'].rsplit('#',1)[1]);line=rawlines[(str(f),n)]
    if re.search(r'(?<!^)\\[ibp-]',line) and any(code in line[line.find(' ')+1:] for code in ['\\b','\\i','\\p']):
        # Require a control strictly inside visible text, not just at the boundary.
        body=line.split(' ',1)[1]
        if re.search(r'\w.*\\[ibp].*\w',body):selected.append(r)
    if len(selected)==3:break
run_id=uuid.uuid4().hex[:8]
copy=ROOT/('oot-inject-copy-'+run_id);db=ROOT/('oot-inject-'+run_id+'.db');data=copy/'OoT_Data/SCRIPTS~';data.mkdir(parents=True,exist_ok=True)
for f in {Path(r['file_path']) for r in selected}|{SCRIPTS/'Characters.txt'}:
    dest=data/f.relative_to(SCRIPTS);dest.parent.mkdir(parents=True,exist_ok=True);shutil.copy2(f,dest)
shutil.copy2(GAME/'UnityPlayer.dll',copy/'UnityPlayer.dll')
run(['extract',copy,'-o',db],'oot-inject-extract.log')
cr=rows(db);changed=[]
with sqlite3.connect(db) as c:
    c.execute("update strings set translation=NULL,status='pending'")
    for s in selected:
        r=next(r for r in cr if Path(r['file_path']).name==Path(s['file_path']).name and r['id']==s['id'])
        c.execute("update strings set translation=?,status='translated' where id=?",('AUDIT '+r['source'],r['id']))
        f=Path(r['file_path']);n=int(r['id'].rsplit('#',1)[1]);old=f.read_text(encoding='utf-8').splitlines()[n-1]
        changed.append(dict(file=s['file_path'],copy=str(f),path=f'line:{n}',id=r['id'],text=old,translation='AUDIT '+r['source']))
    c.commit()
copyfiles=list(copy.rglob('*'));copybefore=hashes(p for p in copyfiles if p.is_file())
run(['inject',copy,'-P',db,'-l','es','--direct'],'oot-inject.log')
for item in changed:
    item['after']=Path(item['copy']).read_text(encoding='utf-8').splitlines()[int(item['path'][5:])-1]
    item['controls_before']=re.findall(r'\\[ibpn-]',item['text']);item['controls_after']=re.findall(r'\\[ibpn-]',item['after'])
after=hashes(files);assert before==after,'Original script files changed'
copyafter=hashes(Path(p) for p in copybefore)
unrequested=[]
allowed=collections.defaultdict(set)
for x in changed:allowed[x['copy']].add(int(x['path'][5:]))
for f,oldhash in copybefore.items():
    if f not in allowed and copyafter[f]!=oldhash:unrequested.append(f)
result=dict(truth=len(truth),hits=len(hit),misses=len(miss),extracted=len(rs),false_positives=len(fps),unclassified=len(unknown),recall=len(hit)/len(truth),precision_classified=len(hit)/(len(rs)-len(unknown)),precision_lower_bound=len(hit)/len(rs),precision_upper_bound=1.0,miss_classes=dict(collections.Counter(x['cls'] for x in miss)),fp_classes=dict(collections.Counter(x['cls'] for x in fps)),truth_classes=dict(collections.Counter(x['cls'] for x in truth)),original_scripts_hashed=len(before),original_changes=0,inject_controls_damaged=sum(x['controls_before']!=x['controls_after'] for x in changed),unrequested_files_changed=unrequested)
dump('oot-result.json',result);dump('oot-misses.json',miss);dump('oot-fps.json',fps);dump('oot-truth.json',truth);dump('oot-injection.json',changed);dump('oot-original-hashes.json',before)
dump('oot-unclassified.json',unknown)
print(json.dumps(result,indent=2))
