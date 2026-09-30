from audit import *
gs=load(ROOT/'inventory-rpgm.json')['games']+load(ROOT/'inventory.json')['games'];rr=load(ROOT/'extraction-results.json');all_bad=[]
for r in rr:
    g=next(g for g in gs if g['root']==r['root']);docs,err=documents(g);rs=rows(ROOT/(r['slug']+'.locust.db'));covered,unmapped,mapped=coverage(rs,docs);bad=[]
    for entry in rs:
        fn=Path(entry['file_path']).name;ps=mapped[entry['id']]
        if not ps:continue
        original='\n'.join(at(docs[fn],p) for p in ps)
        if entry['source']!=original and entry['source']!=original.strip():
            cl,visible=classify(fn,ps[0],original,docs[fn])
            rec=dict(game=r['slug'],file=entry['file_path'],id=entry['id'],path=jp(ps[0]),original=original,extracted=entry['source'],classification=cl,visible=visible)
            bad.append(rec);all_bad.append(rec)
    print(r['slug'],'source mismatches',len(bad),flush=True)
dump('source-value-mismatches.json',all_bad)
print('TOTAL',len(all_bad),'visible rows',sum(x['visible'] for x in all_bad),'examples',all_bad[:3],flush=True)
