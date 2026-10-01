from injection import *
def execute(name,engine,files,check):
    game=ROOT/'regressions'/name;fresh(game)
    for fn,b in files.items():
        p=game/fn;p.parent.mkdir(parents=True,exist_ok=True);p.write_bytes(b)
    db=ROOT/(name+'-regression.db')
    if db.exists():db.unlink()
    rc=run(['extract',game,'-f',engine,'-o',db],name+'-regression-extract.log');rr=rows(db) if rc==0 else []
    return dict(name=name,engine=engine,passed=check(rr),rows=[dict(id=r['id'],source=r['source']) for r in rr],exit=rc,fixture=str(game))

def integrity_cases():
    def chunk(tag,b):return tag+struct.pack('<Q',len(b))+b
    def xp3(payloads):
        # Independent minimal public XP3 framing: specs/ArcXP3.cs:97/:111/:150/:216.
        data=bytearray(b'XP3\r\n \n\x1a\x8b\x67\x01'+bytes(8));index=b''
        for name,b in payloads.items():
            off=len(data);data.extend(b);n=name.encode('utf-16-le')
            info=struct.pack('<IQQH',0,len(b),len(b),len(n)//2)+n
            seg=struct.pack('<IQQQ',0,off,len(b),len(b))
            index+=chunk(b'File',chunk(b'info',info)+chunk(b'segm',seg)+chunk(b'adlr',struct.pack('<I',zlib.adler32(b))))
        struct.pack_into('<Q',data,11,len(data));data.extend(b'\x00'+struct.pack('<Q',len(index))+index);return bytes(data)
    name='kag-xp3-integrity';game=ROOT/'regressions'/name;fresh(game)
    (game/'patch.xp3').write_bytes(xp3({'keep.bin':b'UNTOUCHED RESOURCE'}))
    (game/'patch2.xp3').write_bytes(xp3({'story.ks':b'Hello.\n'}))
    originals=snapshot(game);db=ROOT/(name+'-regression.db')
    if db.exists():db.unlink()
    assert run(['extract',game,'-f','kirikiri','-o',db],name+'-regression-extract.log')==0
    rr=rows(db);assert len(rr)==1
    c=sqlite3.connect(db);c.execute("update strings set translation='AUDIT Hello.',status='translated',provider_used='ground-audit'");c.commit();c.close()
    rc=run(['inject',game,'-P',db,'-l','es','--direct'],name+'-regression-inject.log')
    members={f:{n:xp3_read(game/f,e) for n,e in xp3_entries(game/f)} for f in originals}
    preserved=members['patch.xp3'].get('keep.bin')==b'UNTOUCHED RESOURCE'
    redb=ROOT/(name+'-after-regression.db')
    if redb.exists():redb.unlink()
    assert run(['extract',game,'-f','kirikiri','-o',redb],name+'-regression-reextract.log')==0
    effective=any(r['source']=='AUDIT Hello.' for r in rows(redb))
    identical=all((game/f).read_bytes()==b for f,b in originals.items())
    log=(ROOT/(name+'-regression-inject.log')).read_text(encoding='utf-8')
    zero_claim=bool(re.search(r'Strings written\s*\|\s*0\b',log))
    safe=identical and (rc!=0 or zero_claim)
    first=dict(name=name,engine='kirikiri',passed=preserved and (effective or safe),fixture=str(game),inject_exit=rc,untouched_member_preserved=preserved,effective_translation=effective,safe_refusal=safe)
    name='kag-newline-integrity';game=ROOT/'regressions'/name;fresh(game)
    # Target one standalone LF line; unrelated bare-CR text must stay exact.
    original=b'Hello.\n;comment\rGoodbye.\r';(game/'story.ks').write_bytes(original)
    db=ROOT/(name+'-regression.db')
    if db.exists():db.unlink()
    assert run(['extract',game,'-f','kirikiri','-o',db],name+'-regression-extract.log')==0
    c=sqlite3.connect(db);c.execute("update strings set translation='AUDIT Hello.',status='translated',provider_used='ground-audit' where source='Hello.'");c.commit();c.close()
    rc=run(['inject',game,'-P',db,'-l','es','--direct'],name+'-regression-inject.log')
    actual=(game/'story.ks').read_bytes();expected=b'AUDIT '+original
    second=dict(name=name,engine='kirikiri',passed=rc==0 and actual==expected,fixture=str(game),inject_exit=rc,before_hex=original.hex(),expected_hex=expected.hex(),actual_hex=actual.hex())
    return [first,second]

def main():
    # Valid minimal YSTB authored from the public opcode/layout, not Locust tests.
    def ybn(raw):return b'YSTB'+struct.pack('<7I',0x22b,1,4,12,len(raw),4,0)+bytes([108,1,0,0])+struct.pack('<HhII',0,0,len(raw),0)+raw+struct.pack('<I',1)
    tests=[]
    tests.append(execute('kag-bare-cr','kirikiri',{'story.ks':b';comment\rHello.\rGoodbye.\r'},lambda rr:[r['source'] for r in rr]==['Hello.','Goodbye.']))
    tests.append(execute('kag-script-block','kirikiri',{'story.ks':b'[iscript]\nSystem.exit();\n[endscript]\nHello.\n'},lambda rr:[r['source'] for r in rr]==['Hello.']))
    tests.append(execute('yuris-word-control','yuris',{'story.ybn':ybn(b'Hello\xef\xf0world')},lambda rr:len(rr)==1 and rr[0]['source']=='Hello\r\nworld'))
    tests.append(execute('tyrano-code-choice','tyrano',{'data/scenario/story.ks':b'[iscript]\nwindow.flag=1;\n[endscript]\n[glink text="Continue" target="*next"]\n'},lambda rr:[r['source'] for r in rr]==['Continue']))
    tests.append(execute('tyrano-bracket-tag','tyrano',{'data/scenario/story.ks':b'[eval exp="f.x[0]=1"]\nHello.[p]\n'},lambda rr:[r['source'] for r in rr]==['Hello.[p]']))
    tests.append(execute('nscripter-menu','nscripter',{'0.txt':b'*define\nrmenu "Skip",skip,"Hide",windowerase,"Restart",reset\ngame\n*start\n`Hello.\\\n'},lambda rr:all(any(r['source']==s for r in rr) for s in ['Skip','Hide','Restart'])))
    tests.extend(integrity_cases())
    dump('regression-results.json',tests)
    for t in tests:print(t['name'],'PASS' if t['passed'] else 'FAIL (confirmed old-code defect)',flush=True)
    # Running an audit succeeds when it records defects; --require-fixed turns
    # the same independent checks into an acceptance gate for a new CLI.
    if '--require-fixed' in sys.argv and not all(t['passed'] for t in tests):raise SystemExit(1)
if __name__=='__main__':main()
