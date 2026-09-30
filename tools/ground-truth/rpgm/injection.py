import shutil,collections,copy
from audit import *
MARK='AUDIT '
def norm(s):return ' '.join(s.split())
CTRL=re.compile(r'\\(?:[A-Za-z]+\[[^\]]*\]|[A-Za-z]+<[^>]*>|[A-Za-z]+|[{}$.|!><^\\])|%[1-9]')
def controls(s):return CTRL.findall(s)
def canonical(v,stats=None,p=()):
    if isinstance(v,dict):return {k:canonical(x,stats,p+(k,)) for k,x in v.items()}
    if isinstance(v,list):
        if p and p[-1]=='list' and all(isinstance(x,dict) and 'code' in x for x in v):
            out=[];i=0
            while i<len(v):
                c=copy.deepcopy(v[i]);code=c['code'];j=i+1
                if code in (401,405):
                    texts=[c['parameters'][0]]
                    while j<len(v) and v[j]['code']==code:
                        texts.append(v[j]['parameters'][0]);j+=1
                    c['parameters'][0]=norm('\n'.join(texts))
                    if stats is not None:stats.append(dict(path=jp(p+(i,)),code=code,lines=j-i,text='\n'.join(texts)))
                out.append(canonical(c,None,p+(len(out),)));i=j
            return out
        return [canonical(x,stats,p+(i,)) for i,x in enumerate(v)]
    return v
def compare(a,b,p=()):
    if type(a)!=type(b):yield dict(kind='type_change',path=jp(p),before=a,after=b);return
    if isinstance(a,dict):
        if a.keys()!=b.keys():yield dict(kind='keys_change',path=jp(p))
        for k in a.keys()&b.keys():yield from compare(a[k],b[k],p+(k,))
    elif isinstance(a,list):
        if len(a)!=len(b):yield dict(kind='array_length_change',path=jp(p),before=len(a),after=len(b));return
        for i,(x,y) in enumerate(zip(a,b)):yield from compare(x,y,p+(i,))
    elif a!=b:yield dict(kind='string_change' if isinstance(a,str) else 'nonstring_change',path=jp(p),before=a,after=b)
def hashes(root):
    return {str(p.relative_to(root)):hashlib.file_digest(p.open('rb'),'sha256').hexdigest() for p in root.rglob('*') if p.is_file()}
def inject_one(idx):
    gs=load(ROOT/'inventory-rpgm.json')['games'];g=gs[idx];slug=f'g{idx:02d}';src=Path(g['root']);game=ROOT/'copies'/slug
    if game.exists():
        assert hashes(game)==load(ROOT/(slug+'-before-hashes.json')), 'Existing scratch copy has changed; use a fresh scratch directory'
    else:
        print('COPY',slug,src,flush=True);shutil.copytree(src,game)
    before_hash=hashes(game);dump(slug+'-before-hashes.json',before_hash)
    cg=dict(g,root=str(game),data=str(game/Path(g['data']).relative_to(src)))
    before,errors=documents(cg);db=ROOT/(slug+'-injection.locust.db')
    assert run(['extract',game,'-o',db],slug+'-copy-extract.log')==0
    rs=rows(db);covered,unmapped,mapped=coverage(rs,before)
    c=sqlite3.connect(db)
    for r in rs:c.execute("update strings set translation=?,status='translated',provider_used='ground-audit' where id=?",(MARK+r['source'],r['id']))
    c.commit();c.close()
    assert run(['inject',game,'-P',db,'-l','es','--direct'],slug+'-inject.log')==0
    after_hash=hashes(game);after,parse_errors=documents(cg);details=[];stats=collections.Counter()
    for fn,a in before.items():
        if fn not in after:stats['missing_document']+=1;continue
        aa=[];bb=[];ac=canonical(a,aa);bc=canonical(after[fn],bb)
        changes=list(compare(ac,bc)); stats['changed_string_fields']+=sum(x['kind']=='string_change' for x in changes)
        stats['nonstring_changes']+=sum(x['kind']!='string_change' for x in changes)
        for change in changes:
            change['file']=str(Path(cg['data'])/fn)
            if change['kind']!='string_change':details.append(change);continue
            old=change['before'];new=change['after']
            if norm(new)!=norm(MARK+old):stats['unexpected_text_mutation']+=1;details.append(dict(kind='unexpected_text_mutation',**{k:v for k,v in change.items() if k!='kind'}))
            if controls(old)!=controls(new):stats['control_mutation']+=1;details.append(dict(kind='control_mutation',**{k:v for k,v in change.items() if k!='kind'},old_controls=controls(old),new_controls=controls(new)))
        if len(aa)!=len(bb):stats['message_group_count_change']+=1
        for x,y in zip(aa,bb):
            if x['lines']!=y['lines']:stats['rewrapped_message_groups']+=1
            stats['before_message_lines']+=x['lines'];stats['after_message_lines']+=y['lines']
            if x['text'].strip() and norm(y['text'])!=norm(MARK+x['text']):stats['message_group_text_mutation']+=1
    # Verify expected physical targets through grouped canonical values, with full remaining structure equality.
    expected=copy.deepcopy(before)
    for r in rs:
        fn=Path(r['file_path']).name;ps=mapped[r['id']]
        if not ps:continue
        if r['id'].endswith('#msg'):
            for i,p in enumerate(ps):
                par=at(expected[fn],p[:-1]);par[p[-1]]=(MARK if i==0 else '')+at(before[fn],p)
        else:
            p=ps[0];par=at(expected[fn],p[:-1]);par[p[-1]]=MARK+r['source']
    unexpected=[]
    for fn in expected:
        if fn in after:
            for d in compare(canonical(expected[fn]),canonical(after[fn])):
                if d['kind']=='string_change' and norm(d['before'])==norm(d['after']):continue
                unexpected.append(dict(file=fn,**d))
    stats['unexpected_structural_changes_vs_targets']=len(unexpected);details.extend(dict(kind='target_mismatch',**{k:v for k,v in d.items() if k!='kind'}) for d in unexpected)
    alljson=list(game.rglob('*.json'));json_errors=[]
    for p in alljson:
        try:load(p)
        except Exception as e:json_errors.append(dict(file=str(p),error=str(e)))
    redb=ROOT/(slug+'-roundtrip.locust.db');assert run(['extract',game,'-o',redb],slug+'-roundtrip-extract.log')==0
    rr=rows(redb);expected_counts=collections.Counter(norm(MARK+r['source']) for r in rs);actual_counts=collections.Counter(norm(r['source']) for r in rr)
    missing=expected_counts-actual_counts;extra=actual_counts-expected_counts
    stats['roundtrip_missing']=sum(missing.values());stats['roundtrip_extra']=sum(extra.values());stats['roundtrip_exact_source']=sum((collections.Counter(MARK+r['source'] for r in rs)&collections.Counter(r['source'] for r in rr)).values())
    plugins=[]
    for r in rs:
        if 'plugin_cmd' in json.loads(r['tags']) and not '#arg_' in r['id']:
            plugins.append(dict(file=r['file_path'],id=r['id'],before=r['source'],after=MARK+r['source'],before_command=r['source'].split()[0],after_command=(MARK+r['source']).split()[0]))
    stats['MV_plugin_command_dispatch_changed']=len(plugins);details.extend(dict(kind='plugin_dispatch_changed',**x) for x in plugins)
    changed=[k for k,v in before_hash.items() if after_hash.get(k)!=v];unchanged=[k for k,v in before_hash.items() if after_hash.get(k)==v];added=[k for k in after_hash if k not in before_hash]
    result=dict(game=slug,original=str(src),copy=str(game),rows_before=len(rs),rows_after=len(rr),json_files_checked=len(alljson),json_parse_errors=json_errors,document_errors=parse_errors,stats=dict(stats),changed_files=changed,byte_identical_files=len(unchanged),added_files=added,deleted_files=[k for k in before_hash if k not in after_hash],unmapped=unmapped,roundtrip_missing_examples=list(missing.items())[:3],roundtrip_extra_examples=list(extra.items())[:3])
    dump(slug+'-injection-result.json',result);dump(slug+'-injection-details.json',details);print(json.dumps(result,ensure_ascii=False),flush=True)
    return result
if __name__=='__main__':
    results=[]
    for idx in map(int,sys.argv[1:] or [1,9,92]):results.append(inject_one(idx));dump('injection-results.json',results)
