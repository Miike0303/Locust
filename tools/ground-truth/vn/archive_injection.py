from injection import *
def xp3_members(p):return {n:xp3_read(p,e) for n,e in xp3_entries(p)}
def main():
    reports=[]
    for engine,label,original,files in [
        ('kirikiri','xp3-real',Path('D:/juegos/VN/Ochiru Hitozuma'),['patch.xp3','patch2.xp3']),
        ('yuris','ypf-real',Path('D:/juegos/VN/Injuu Kangoku RE/pac'),['ysbin.ypf'])]:
        game=ROOT/'injection-copies'/label;fresh(game)
        for name in files:shutil.copy2(original/name,game/name)
        reader=xp3_members if engine=='kirikiri' else ypf_members
        before={f:reader(game/f) for f in files};db=ROOT/(label+'.db')
        if db.exists():db.unlink()
        rc=run(['extract',game,'-f',engine,'-o',db],label+'-extract.log');assert rc==0
        rs=sorted(rows(db),key=lambda r:r['id']);picks=rs[:3];target={r['id']:'AUDIT '+r['source'] for r in picks}
        c=sqlite3.connect(db)
        for i,t in target.items():c.execute("update strings set translation=?,status='translated',provider_used='ground-audit' where id=?",(t,i))
        c.commit();c.close();rc=run(['inject',game,'-P',db,'-l','es','--direct'],label+'-inject.log')
        after={f:reader(game/f) for f in files};deleted=[];changed=[];untargeted=[];structural=[]
        requested={(r['file_path'].replace('\\','/').split('.xp3/',1)[-1] if engine=='kirikiri' else r['file_path'].replace('\\','/').split('.ypf/',1)[-1]) for r in picks}
        for f,old in before.items():
            for name,b in old.items():
                a=after[f].get(name)
                if a is None:deleted.append(dict(archive=f,member=name,bytes=len(b)));continue
                if a==b:continue
                changed.append(dict(archive=f,member=name))
                if name not in requested:untargeted.append(dict(archive=f,member=name))
                elif engine=='yuris' and b[:4]==b'YSTB':
                    x=ystb(b);y=ystb(a)
                    for k in ['instructions','lines','tail']:
                        if x[k]!=y[k]:structural.append(dict(member=name,kind=k))
                    # Translation changes at most three attribute payloads;
                    # record all raw mutations for independent review.
                    diffs=[(xx['index'],xx['text'],yy['text']) for xx,yy in zip(x['attributes'],y['attributes']) if xx['raw']!=yy['raw']]
                    structural.append(dict(member=name,kind='attribute_payload_changes',diffs=diffs))
        redb=ROOT/(label+'-after.db')
        if redb.exists():redb.unlink()
        rrc=run(['extract',game,'-f',engine,'-o',redb],label+'-reextract.log');ar=rows(redb) if rrc==0 else [];am={r['id']:r['source'] for r in ar}
        mismatches=[dict(id=i,expected=t,actual=am.get(i)) for i,t in target.items() if am.get(i)!=t]
        result=dict(label=label,engine=engine,original=str(original),copy=str(game),requested=len(target),inject_exit=rc,rows_before=len(rs),rows_after=len(ar),members_before={f:len(x) for f,x in before.items()},members_after={f:len(x) for f,x in after.items()},deleted_members=deleted,changed_members=changed,untargeted_changed_members=untargeted,structural=structural,roundtrip_failures=mismatches,target_rows=picks)
        dump(label+'-injection.json',result);reports.append(result);print(label,'deleted',len(deleted),'untargeted',len(untargeted),'roundtrip',len(mismatches),'members',result['members_before'],result['members_after'],flush=True)
    dump('archive-injection-results.json',reports)
if __name__=='__main__':main()
