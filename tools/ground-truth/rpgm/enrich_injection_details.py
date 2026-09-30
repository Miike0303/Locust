from audit import *
gs=load(ROOT/'inventory-rpgm.json')['games']
for p in ROOT.glob('g*-injection-details.json'):
    idx=int(p.name[1:3]);g=gs[idx];before,_=documents(g)
    r=load(ROOT/(f'g{idx:02d}-injection-result.json'));root=Path(r['copy']);cg=dict(g,root=str(root),data=str(root/Path(g['data']).relative_to(Path(g['root']))));after,_=documents(cg)
    details=load(p)
    for d in details:
        fn=Path(d['file']).name
        if fn not in before:continue
        if 'before' in d and 'after' in d:
            bp=[q for q,t in walk(before[fn]) if t==d['before']];ap=[q for q,t in walk(after[fn]) if t==d['after']]
            d['before_file']=str(Path(g['data'])/fn);d['before_paths']=[jp(q) for q in bp];d['after_file']=str(Path(cg['data'])/fn);d['after_paths']=[jp(q) for q in ap]
            if d.get('kind')=='unexpected_text_mutation':
                d['original_command_codes']=[at(before[fn],q[:q.index('list')+2]).get('code') for q in bp if 'list' in q]
    dump(p.name,details)
print('Enriched physical JSON paths in all injection detail files')
