from injection import *
gs=load(ROOT/'inventory-rpgm.json')['games'];results=[]
for path in sorted(ROOT.glob('g*-injection-result.json')):
    r=load(path);g=gs[int(r['game'][1:])];src=Path(g['root']);dst=Path(r['copy']);cg=dict(g,root=str(dst),data=str(dst/Path(g['data']).relative_to(src)))
    a,_=documents(g);b,_=documents(cg);stats=collections.Counter();detail=[]
    for fn in a.keys()&b.keys():
        aa=[];bb=[];canonical(a[fn],aa);canonical(b[fn],bb)
        for old,new in zip(aa,bb):
            if controls(old['text'])!=controls(new['text']):
                stats['raw_control_mutation']+=1;detail.append(dict(kind='raw_control_mutation',file=fn,path=old['path'],before=old['text'],after=new['text']))
            if any(not line.strip() for line in old['text'].split('\n')) and not any(not line.strip() for line in new['text'].split('\n')):
                stats['blank_line_collapsed']+=1;detail.append(dict(kind='blank_line_collapsed',file=fn,path=old['path'],before=old['text'],after=new['text']))
            if old['code']==405:
                stats['scroll_groups']+=1
                if old['lines']!=new['lines']:stats['scroll_group_line_count_changed']+=1
    # Source-byte invariance compares original game files against the pre-injection full-copy hash manifest.
    saved=load(ROOT/(r['game']+'-before-hashes.json'));changed=[]
    for rel,h in saved.items():
        original=src/rel
        if not original.exists() or hashlib.file_digest(original.open('rb'),'sha256').hexdigest()!=h:changed.append(rel)
    stats['original_files_checked']=len(saved);stats['original_byte_changes']=len(changed)
    result=dict(game=r['game'],stats=dict(stats),examples=detail[:9],original_changed=changed)
    results.append(result);print(json.dumps(result,ensure_ascii=False),flush=True)
dump('additional-injection-checks.json',results)
