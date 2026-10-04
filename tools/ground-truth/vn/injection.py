from audit import *
import shutil,difflib
def fresh(p):
    # Only remove a verified child under our authorized scratch root.
    assert p.resolve().is_relative_to(ROOT.resolve()) and p.resolve()!=ROOT.resolve()
    if p.exists():shutil.rmtree(p)
    p.mkdir(parents=True)
def snapshot(root):return {str(p.relative_to(root)).replace('\\','/'):p.read_bytes() for p in root.rglob('*') if p.is_file() and '.locust' not in str(p.relative_to(root))}
def probe(case,label,selected,mode='prefix'):
    src=Path(case['root']);game=ROOT/'injection-copies'/label;fresh(game)
    for rel in selected:
        p=game/rel;p.parent.mkdir(parents=True,exist_ok=True);shutil.copy2(src/rel,p)
    before=snapshot(game);db=ROOT/(label+'-inject.db')
    if db.exists():db.unlink()
    assert run(['extract',game,'-f',case['engine'],'-o',db],label+'-copy-extract.log')==0
    rr=sorted(rows(db),key=lambda r:r['id']);picks=[]
    if mode=='controls':picks=[r for r in rr if '[' in r['source'] and any(k=='text' and s.strip() for k,a,b,s in tokens(r['source']))][:3]
    elif mode=='mixed_newlines':picks=rr[:3]
    else:
        for rel in selected:
            candidates=[r for r in rr if r['id'].rsplit('#',1)[0]==rel or r['id'].rsplit('#arg',1)[0]==rel]
            picks.extend(candidates[:3])
    c=sqlite3.connect(db);target={}
    for r in picks:
        s=r['source'];t='AUDIT '+(''.join(s for k,a,b,s in tokens(s) if k=='text').rstrip('\\') if mode=='controls' else s)
        c.execute("update strings set translation=?,status='translated',provider_used='ground-audit' where id=?",(t,r['id']));target[r['id']]=t
    c.commit();c.close()
    rc=run(['inject',game,'-P',db,'-l','es','--direct'],label+'-inject.log');after=snapshot(game);details=[];stat=collections.Counter();locs={}
    for fn,b in before.items():
        a=after.get(fn)
        if a is None:stat['missing_file']+=1;continue
        if case['engine']=='yuris':
            if b[:4]!=b'YSTB':
                if a!=b:stat['changed_non_ystb']+=1
                continue
            old=ystb(b);new=ystb(a);oldaa=old['attributes'];newaa=new['attributes'];cursor=0
            for r in sorted([r for r in rr if r['id'].rsplit('#arg',1)[0]==fn],key=lambda r:int(r['id'].rsplit('#arg',1)[1])):
                match=next((x for x in oldaa[cursor:] if x['text']==r['source']),None)
                if match:locs[r['id']]=(fn,match['index']);cursor=match['index']+1
            requested={locs[i][1] for i in target if i in locs and locs[i][0]==fn}
            for key in ['version','key','instructions','lines','tail']:
                if old[key]!=new[key]:stat['unexpected_'+key]+=1;details.append(dict(file=fn,kind=key,before=old[key],after=new[key]))
            for x,y in zip(oldaa,newaa):
                if x['id']!=y['id'] or x['type']!=y['type']:stat['attribute_schema_changed']+=1
                if x['index'] not in requested and x['raw']!=y['raw']:stat['untouched_attribute_changed']+=1;details.append(dict(file=fn,kind='untouched_attribute',before=x,after=y))
                if x['index'] in requested:details.append(dict(file=fn,kind='target_attribute',index=x['index'],before=x['text'],after=y['text']))
        else:
            bs=b.decode('cp932') if case['engine']=='nscripter' else decode_ks(b);as_=a.decode('cp932') if case['engine']=='nscripter' else decode_ks(a);expected=bs.split('\n');selected_lines={}
            for i,t in (target.items() if rc==0 else []):
                rel,no=i.split('#',1)
                # Adapter (cycle 116): KiriKiri locators are now kag:<line> or kag:<line>:attr:...
                if no.startswith('kag:'):no=no[4:]
                if rel==fn and ':' not in no:
                    no=int(no);orig=expected[no-1];suffix='\r' if orig.endswith('\r') else ''
                    row=next(r for r in picks if r['id']==i)
                    if 'speaker' in json.loads(row['tags']):
                        t=orig[:len(orig)-len(orig.lstrip())]+'#'+t+(':'+orig.rstrip('\r').split(':',1)[1] if ':' in orig else '')
                    elif case['engine']=='nscripter' and t.lstrip()[0].isascii() and not t.lstrip().startswith('`'):
                        t='`'+t
                    expected[no-1]=t+suffix;selected_lines[no]=(orig.rstrip('\r'),t)
            es='\n'.join(expected)
            if es!=as_:
                stat['files_with_untouched_text_or_newline_mutation']+=1
                details.append(dict(file=fn,kind='unexpected_text',diff=''.join(difflib.unified_diff(es.splitlines(True),as_.splitlines(True),fromfile='expected',tofile='actual'))))
            oldnl=collections.Counter(re.findall(r'\r\n|\r|\n',bs));newnl=collections.Counter(re.findall(r'\r\n|\r|\n',as_))
            if oldnl!=newnl:details.append(dict(file=fn,kind='newline_counts',before=dict(oldnl),after=dict(newnl)));stat['newline_mutated_files']+=1
            if not b.startswith(b'\xef\xbb\xbf') and a.startswith(b'\xef\xbb\xbf'):stat['unexpected_utf8_bom_added']+=1
            for no,(old,t) in selected_lines.items():
                oldtags=[s for k,x,y,s in tokens(old) if k=='tag'];newtags=[s for k,x,y,s in tokens(t) if k=='tag']
                actual_lines=as_.split('\n');actual_line=actual_lines[no-1] if no<=len(actual_lines) else ''
                actualtags=[s for k,x,y,s in tokens(actual_line) if k=='tag']
                if oldtags!=actualtags:stat['row_locator_control_mismatch']+=1;details.append(dict(file=fn,kind='row_locator_control_mismatch',line=no,before=old,requested=t,actual=actual_line,before_tags=oldtags,after_tags=actualtags))
            if case['engine']!='nscripter':
                originaltags=[s for k,x,y,s in tokens(bs) if k=='tag'];actualtags=[s for k,x,y,s in tokens(as_) if k=='tag']
                if originaltags!=actualtags:stat['global_engine_tag_sequence_changed_files']+=1
    redb=ROOT/(label+'-roundtrip.db')
    if redb.exists():redb.unlink()
    rrc=run(['extract',game,'-f',case['engine'],'-o',redb],label+'-roundtrip-extract.log');ar=rows(redb) if rrc==0 else [];am={r['id']:r['source'] for r in ar}
    expectations={i:('`'+t if case['engine']=='nscripter' and t.lstrip()[0].isascii() and not t.lstrip().startswith('`') else t) for i,t in target.items()}
    mismatches=[dict(id=i,expected=t,actual=am.get(i)) for i,t in expectations.items() if am.get(i)!=t]
    stat['requested_rows_not_exact_on_reextract']=len(mismatches)
    stat['admitted_rows_not_exact_on_reextract']=len(mismatches) if rc==0 else 0
    stat['safely_rejected_requested_rows']=len(target) if rc!=0 and all(before[f]==after.get(f) for f in before) else 0
    unchanged=[dict(id=r['id'],before=r['source'],after=am.get(r['id'])) for r in rr if r['id'] not in target and am.get(r['id'])!=r['source']]
    stat['untouched_rows_missing_or_changed_on_reextract']=len(unchanged)
    result=dict(label=label,engine=case['engine'],copy=str(game),input_files=list(before),requested=len(target),extract_rows_before=len(rr),extract_rows_after=len(ar),inject_exit=rc,reextract_exit=rrc,stats=dict(stat),target_rows=[dict(id=r['id'],source=r['source'],translation=target[r['id']]) for r in picks],mismatches=mismatches,untouched_row_changes=unchanged,details=details,added_files=sorted(set(after)-set(before)),changed_files=[f for f in before if before[f]!=after.get(f)])
    dump(label+'-injection.json',result);print(label,result['stats'],flush=True);return result
def main():
    cs={x['slug']:x for x in json.loads((ROOT/'cases.json').read_text(encoding='utf-8'))};results=[]
    results.append(probe(cs['My'],'kag-prefix',['unencrypted/scenario/00_000.ks','unencrypted/scenario/_first.ks']))
    results.append(probe(cs['Taimanin'],'kag-mixed-newlines',['unencrypted/newgame01.ks'],'mixed_newlines'))
    results.append(probe(cs['Taimanin'],'kag-controls',['unencrypted/end.ks','unencrypted/newgame03.ks'],'controls'))
    results.append(probe(cs['Injuu'],'yuris-prefix',['yst00182.ybn','yst00023.ybn','yst00160.ybn']))
    results.append(probe(cs['TyranoOfficial'],'tyrano-prefix',['data/scenario/cg.ks','data/scenario/scene1.ks','data/scenario/config.ks']))
    results.append(probe(cs['TyranoOfficial'],'tyrano-controls',['data/scenario/scene1.ks'],'controls'))
    ncase=dict(engine='nscripter',root=str(ROOT/'corpus/NScripterJP'))
    if (ROOT/'corpus/NScripterJP/0.txt').exists():results.append(probe(ncase,'nscripter-prefix',['0.txt']))
    dump('injection-results.json',results)
if __name__=='__main__':main()
