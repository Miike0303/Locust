import os,sys,json,re,sqlite3,subprocess,collections,hashlib
from pathlib import Path
ROOT=Path(__file__).resolve().parent
sys.stdout.reconfigure(encoding='utf-8',errors='backslashreplace')
EXE=Path('C:/Projects/Locust/target/debug/locust.exe')
ENV=dict(os.environ,LOCUST_DATA_DIR=str(ROOT/'cli-data'),PYTHONUTF8='1')
def has_literal(text):
    t=re.sub(r'\\[A-Za-z]+(?:\[[^\]]*\]|<[^>]*>|\{[^}]*\})?|\\[{}$.|!><^]', '', text)
    t=re.sub(r'<[^>]*>|\{\{[^}]*\}\}', '', t)
    return any(c.isalpha() for c in t)
def dump(name,value): (ROOT/name).write_text(json.dumps(value,ensure_ascii=False,indent=2),encoding='utf-8')
def load(p):return json.loads(Path(p).read_text(encoding='utf-8-sig'))
def run(args,log):
    p=subprocess.run([str(EXE),*map(str,args)],env=ENV,capture_output=True,encoding='utf-8',errors='replace',timeout=1800)
    (ROOT/log).write_text(p.stdout+p.stderr,encoding='utf-8')
    if p.returncode:print('CLI ERROR',args,p.returncode,p.stderr[:300],flush=True)
    return p.returncode
def rows(db):
    c=sqlite3.connect(db);c.row_factory=sqlite3.Row
    r=[dict(x) for x in c.execute('select * from strings')];c.close();return r
def walk(v,p=()):
    if isinstance(v,str):
        yield p,v
        if p and p[-1]=='choices' and 'parameters' in p and 'list' in p:
            try:
                choices=json.loads(v)
                if isinstance(choices,list):
                    for i,choice in enumerate(choices):
                        cp=p+('@json',i)
                        if isinstance(choice,str):choice=json.loads(choice);cp+=('@json',)
                        if isinstance(choice,dict) and isinstance(choice.get('label'),str):yield cp+('label',),choice['label']
            except (ValueError,TypeError):pass
    elif isinstance(v,dict):
        for k,x in v.items():yield from walk(x,p+(k,))
    elif isinstance(v,list):
        for i,x in enumerate(v):yield from walk(x,p+(i,))
def at(v,p):
    for k in p:v=json.loads(v) if k=='@json' else v[k]
    return v
def jp(p):return '$'+''.join('['+str(k)+']' if isinstance(k,int) else '.'+k for k in p)
def classify(fn,p,text,doc=None):
    stem=Path(fn).stem
    if stem.startswith('lang_'):return 'localization_pack',True
    if fn=='plugins.js':return 'plugins.js_parameter',False
    if 'parameters' in p and 'list' in p:
        ci=p.index('list')+1
        try:code=at(doc,p[:ci+1])['code']
        except Exception:code=-1
        if code in (401,405):return f'event_{code}',has_literal(text)
        if code==102:return 'choices_102',True
        if code==101 and p[-1]==4:return 'speaker_101' if has_literal(text) else 'speaker_101_dynamic_control',has_literal(text)
        if code in (320,324,325) and p[-1]==1:return f'actor_event_{code}',True
        if code==402:return 'branch_label_402_redundant',False
        if code in (108,408):return 'comment_108_408',False
        if code in (355,655):return 'script_355_655',False
        if code==356 and re.match(r'^(D_TEXT|SHOW_TEXT|T_TEXT|GN_TEXT)\s',text):return 'plugin_command_356_display',True
        if code==357 and '@json' in p and p[-1]=='label':return 'plugin_command_357_nested_choice_label',has_literal(text)
        if code==357 and len(p)>2 and p[-2]==3 and p[-1] in ('text','destination','label','message','description'):
            return 'plugin_command_357_display' if has_literal(text) else 'plugin_command_357_lookup_control',has_literal(text)
        if code in (356,357):return f'plugin_command_{code}',False
        return 'technical_event',False
    if p and p[-1]=='note':return 'note',False
    if stem=='MapInfos':return 'MapInfos_editor_name',False
    if re.fullmatch(r'Map\d+',stem) and p==('displayName',):return 'map_displayName',True
    if stem=='System':
        if p and p[0]=='terms':return 'system_terms',True
        if p and p[0] in ('gameTitle','currencyUnit','armorTypes','elements','equipTypes','skillTypes','weaponTypes'):return 'system_'+str(p[0]),True
        if p and p[0]=='plugins':return 'system_plugin_parameter',False
    if stem in ('Actors','Classes','Skills','Items','Weapons','Armors','Enemies','States','Troops') and p and p[-1] in ('name','description','message1','message2','message3','message4','nickname','profile'):
        if stem=='Troops' and p[-1]=='name':return 'troop_editor_name',False
        return 'database_'+str(p[-1]),True
    return 'other_technical',False
def candidate(cl):return cl not in ('other_technical','technical_event')
def documents(g):
    d=Path(g['data']); out={};errors=[]
    from lzbase64 import decode
    for p in list(d.glob('*.json'))+list(d.glob('*.jsono')):
        try:out[p.name]=json.loads(decode(p.read_text(encoding='utf-8-sig'))) if p.suffix=='.jsono' else load(p)
        except Exception as e:errors.append([str(p),str(e)])
    base=d.parent
    p=base/'js/plugins.js'
    if p.exists():
        raw=p.read_text(encoding='utf-8-sig');m=re.search(r'\$plugins\s*=\s*(\[.*\])\s*;?',raw,re.S)
        if m:
            try:out['plugins.js']=json.loads(m[1])
            except Exception as e:errors.append([str(p),str(e)])
    packs=[n for n in out if n.startswith('lang_')]
    available={Path(n).stem.rsplit('_',1)[-1] for n in packs}
    source=next((x for x in ('en','jp','ja','zh') if x in available),next(iter(sorted(available)),None))
    for n in packs:
        if Path(n).stem.rsplit('_',1)[-1]!=source:del out[n]
    return out,errors
def coverage(rs,docs):
    covered=set(); unmapped=[]; mapped={}
    for r in rs:
        fn=Path(r['file_path']).name; parts=r['id'].split('#')[1:]; p=None;paths=[]
        try:
            cmd=next((x for x in parts if x.startswith('cmd_')),None)
            if cmd:
                idx=int(cmd[4:]); ev=next((x for x in parts if x.startswith('event_')),None);page=next((x for x in parts if x.startswith('page_')),None)
                if ev:p=('events',int(ev[6:]),'pages',int(page[5:]),'list',idx)
                elif page:p=(int(parts[0]),'pages',int(page[5:]),'list',idx)
                else:p=(int(parts[0]),'list',idx)
                c=at(docs[fn],p);code=c['code']
                if parts[-1]=='msg':
                    line=idx
                    while True:
                        q=p[:-1]+(line,); cc=at(docs[fn],q)
                        if cc['code']!=code:break
                        paths.append(q+('parameters',0));line+=1
                elif parts[-1].startswith('choice_'):paths=[p+('parameters',0,int(parts[-1][7:]))]
                elif any(x.startswith('arg_') for x in parts):
                    ai=next(i for i,x in enumerate(parts) if x.startswith('arg_'));key=parts[ai][4:]
                    if key=='choices' and len(parts)>ai+2:
                        ci=int(parts[ai+1]);choice=json.loads(c['parameters'][3]['choices'])[ci]
                        paths=[p+('parameters',3,'choices','@json',ci)+(('@json',) if isinstance(choice,str) else ())+('label',)]
                    else:paths=[p+('parameters',3,key)]
                else:paths=[p+('parameters',1 if code==320 else 0)]
            elif parts[0]=='plugins': paths=[('plugins',int(parts[1]),'parameters',parts[3])]
            else:paths=[tuple(int(x) if x.isdigit() else x for x in parts)]
        except (IndexError,KeyError,ValueError,TypeError):pass
        valid=[p for p in paths if fn in docs and isinstance(at(docs[fn],p),str)]
        if not valid:unmapped.append(r['id'])
        physical='\n'.join(at(docs[fn],p) for p in valid)
        matches=r['source']==physical or ('plugin_cmd' in json.loads(r['tags']) and '#arg_' not in r['id'] and r['source']==physical.strip())
        if matches:
            for p in valid:covered.add((fn,p,at(docs[fn],p)))
        mapped[r['id']]=valid
    return covered,unmapped,mapped
def diffs(a,b,p=(),stats=None):
    if isinstance(a,str) and isinstance(b,str):
        if a!=b and a.strip():yield p,a,b
    elif isinstance(a,dict) and isinstance(b,dict):
        if 'code' in a and a.get('code')!=b.get('code'):
            stats['command_mismatch']+=1;return
        for k in a.keys()&b.keys():yield from diffs(a[k],b[k],p+(k,),stats)
        stats['key_difference']+=len(a.keys()^b.keys())
    elif isinstance(a,list) and isinstance(b,list):
        if len(a)!=len(b):stats['array_length_mismatch']+=1;return
        for i,(x,y) in enumerate(zip(a,b)):yield from diffs(x,y,p+(i,),stats)
def make_pairs(gs):
    byroot={g['root']:g for g in gs};pairs={}
    originals=[g for g in gs if '\\en\\' in g['root'] and '-es' not in g['root'].lower()]
    for a in originals:
        rel=a['root'].split('\\en\\')[1]; top=rel.split('\\')[0]
        possibilities=[b for b in gs if '\\es\\' in b['root'] and b['root'].split('\\es\\')[1].split('\\')[0].casefold()==(top+'-es').casefold()]
        if possibilities:pairs[a['root']]=possibilities[0]['root']
    manual=[(92,93),(2,71),(5,76),(6,77),(8,51),(20,59),(24,78),(26,44),(38,82),(47,85),(90,79),(1,49)]
    for i,j in manual:pairs[gs[i]['root']]=gs[j]['root']
    return pairs
def audit():
    gs=load(ROOT/'inventory-rpgm.json')['games'];pairs=make_pairs(gs)
    extra=load(ROOT/'inventory.json')['games']
    cam=next((g for g in extra if g['root']=='D:\\juegos\\renpy\\CamXFamily -Eng'),None)
    cam_es=next((g for g in extra if g['root']=='D:\\juegos\\renpy\\1es\\CamXFamily -Eng-es'),None)
    if cam and cam_es:gs.append(cam);pairs[cam['root']]=cam_es['root']
    dump('pairs.json',pairs)
    selected=[g for i,g in enumerate(gs) if i not in (10,11) and (g['root'] in pairs or ('\\en\\' in g['root'] and '-es' not in g['root'].lower()) or g['root'] in [gs[i]['root'] for i in (0,1,2,87,90,91,92)])]
    only=next((a.split('=',1)[1] for a in sys.argv if a.startswith('--only=')),None)
    if only:selected=[g for g in selected if f'g{gs.index(g):02d}'==only]
    results=load(ROOT/'extraction-results.json') if only else []
    detail=load(ROOT/'extraction-details.json') if only else []
    tot=collections.Counter();example=load(ROOT/'structural-examples.json') if only else {}
    for n,g in enumerate(selected):
        slug=f'g{gs.index(g):02d}';db=ROOT/(slug+'.locust.db')
        rc=0 if '--reuse' in sys.argv and db.exists() else run(['extract',g['root'],'-o',db],slug+'-extract.log')
        if rc:results.append(dict(slug=slug,root=g['root'],error='extract failed'));continue
        rs=rows(db);docs,errors=documents(g);covered,unmapped,mapped=coverage(rs,docs)
        counts=collections.Counter();misses=collections.Counter();rawdiff=[];visdiff=[];ds=collections.Counter()
        for fn,doc in docs.items():
            for p,t in walk(doc):
                if not t.strip():continue
                cl,visible=classify(fn,p,t,doc)
                if not candidate(cl):continue
                counts['broad_total']+=1
                hit=(fn,p,t) in covered
                if hit:counts['broad_hit']+=1
                if visible:counts['visible_total']+=1;counts['visible_hit']+=int(hit)
                if not hit:
                    misses[cl]+=1
                    rec=dict(game=slug,file=str(Path(g['data'])/fn) if fn!='plugins.js' else str(Path(g['data']).parent/'js/plugins.js'),path=jp(p),text=t,classification=cl,visible=visible)
                    detail.append(dict(kind='structural_miss',**rec));example.setdefault(cl,[])
                    if len(example[cl])<3:example[cl].append(rec)
        if g['root'] in pairs:
            partner=next(x for x in gs+extra if x['root']==pairs[g['root']]);pd,pe=documents(partner)
            for fn in docs.keys()&pd.keys():
                for p,a,b in diffs(docs[fn],pd[fn],stats=ds):
                    cl,visible=classify(fn,p,a,docs[fn]);hit=(fn,p,a) in covered
                    rawdiff.append(hit)
                    if visible:visdiff.append(hit)
                    if not hit:detail.append(dict(kind='pair_miss',game=slug,file=str(Path(g['data'])/fn) if fn!='plugins.js' else str(Path(g['data']).parent/'js/plugins.js'),path=jp(p),text=a,translation=b,classification=cl,visible=visible))
        fp=collections.Counter();unknown=collections.Counter()
        for r in rs:
            fn=Path(r['file_path']).name;ps=mapped.get(r['id'],[])
            cls=[classify(fn,p,at(docs[fn],p),docs[fn])[0] for p in ps]
            cl=None
            if 'troop_editor_name' in cls:cl='troop_editor_name'
            elif re.fullmatch(r'(?:\s|\\[A-Za-z]+\[[^\]]*\]|\\[{}$.|!><^]|\{\{[^}]*\}\})+',r['source']):cl='control_or_lookup_only'
            elif 'plugin_command_356_display' in cls or 'plugin_command_356' in cls:cl='MV_command_prefix_sent_as_text'
            elif not any(classify(fn,p,at(docs[fn],p),docs[fn])[1] for p in ps):unknown['unconfirmed_candidate']+=1
            if cl:
                fp[cl]+=1;detail.append(dict(kind='false_positive',game=slug,file=r['file_path'],path=jp(ps[0]) if ps else r['id'],text=r['source'],classification=cl))
        result=dict(slug=slug,root=g['root'],engine=g['engine'],extracted=len(rs),**counts,pair=pairs.get(g['root']),diff_total=len(rawdiff),diff_hit=sum(rawdiff),visible_diff_total=len(visdiff),visible_diff_hit=sum(visdiff),diff_alignment=dict(ds),misses=dict(misses),false_positives=dict(fp),fp_total=sum(fp.values()),unconfirmed=dict(unknown),unmapped=unmapped,parse_errors=errors)
        results.append(result)
        print(slug,g['engine'],Path(g['root']).name,'rows',len(rs),'visible',f"{counts['visible_hit']}/{counts['visible_total']}",'diff',f'{sum(rawdiff)}/{len(rawdiff)}','FP',sum(fp.values()),flush=True)
        dump('extraction-results.json',results)
    dump('extraction-results.json',results);dump('extraction-details.json',detail);dump('structural-examples.json',example)
if __name__=='__main__':audit()
