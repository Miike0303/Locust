from pathlib import Path
import struct,subprocess,os,sqlite3,json,uuid,re
ROOT=Path(__file__).resolve().parent
BASE=ROOT/('regression-'+uuid.uuid4().hex[:8]);BASE.mkdir()
EXE=Path(os.environ.get('LOCUST_EXE',str(ROOT/'target/debug/locust.exe')))
ENV=dict(os.environ,LOCUST_DATA_DIR=str(ROOT/'locust-data'),LOCUST_RESEARCH_LOCK_DIR=str(ROOT/'locks'))
def aligned(s):
    b=s.encode();return struct.pack('<I',len(b))+b+b'\0'*((-len(b))%4)
def asset(cls,payload):
    meta=bytearray(b'2019.4.0f1\0'+struct.pack('<I',1)+b'\0'+struct.pack('<i',1)+struct.pack('<iBh',cls,0,-1))
    if cls==114:meta+=b'\0'*16
    meta+=b'\0'*16+struct.pack('<i',1);meta+=b'\0'*((-len(meta))%4)
    meta+=struct.pack('<qIIi',10,0,len(payload),0)
    off=(20+len(meta)+15)&~15
    return struct.pack('>IIII',len(meta),off+len(payload),17,off)+b'\0'*4+meta+b'\0'*(off-20-len(meta))+payload
def mono(name,strings):return asset(114,b'\0'*12+b'\1\0\0\0'+b'\0'*12+aligned(name)+b''.join(aligned(s) for s in strings))
def game(n,script=None,binary=None):
    g=BASE/n;d=g/(n+'_Data');d.mkdir(parents=True)
    if script is not None:
        s=d/'SCRIPTS~';s.mkdir();(s/'Dialogue.txt').write_text(script,encoding='utf-8')
    if binary is not None:(d/'resources.assets').write_bytes(binary)
    return g
def run(args):
    p=subprocess.run([str(EXE),*map(str,args)],env=ENV,capture_output=True)
    (BASE/('call-'+uuid.uuid4().hex[:6]+'.log')).write_bytes(p.stdout+p.stderr)
    assert p.returncode==0,(args,p.stderr.decode(errors='replace'))
def extract(g):
    db=BASE/(g.name+'.db');run(['extract',g,'-o',db])
    with sqlite3.connect(db) as c:
        c.row_factory=sqlite3.Row;rs=[dict(r) for r in c.execute('select * from strings')]
    return db,rs
out=[]
def check(n,fn):
    try:fn();out.append(dict(test=n,passed=True))
    except AssertionError as e:out.append(dict(test=n,passed=False,error=str(e)))
def controls():
    g=game('Controls','CJ Hello \\bworld\\b.\n');db,rs=extract(g)
    with sqlite3.connect(db) as c:
        c.execute("update strings set translation=?,status='translated'",('AUDIT '+rs[0]['source'],));c.commit()
    run(['inject',g,'-P',db,'-l','es','--direct'])
    s=next(g.rglob('Dialogue.txt')).read_text();assert re.findall(r'\\b',s)==['\\b','\\b'],s
def modifiers():
    _,rs=extract(game('Modifiers','CJ Existing dialogue.\nCJ +CJ_Lgr Hello friend.\n'))
    assert any(r['source']=='Hello friend.' for r in rs),'Missing inline-modifier dialogue'
def binary_priority():
    _,rs=extract(game('Mixed','CJ Existing dialogue.\n',asset(49,aligned('UI')+aligned('Welcome traveler!'))))
    assert any(r['source']=='Welcome traveler!' for r in rs),'Binary text bypassed when SCRIPTS~ exists'
def names():
    _,rs=extract(game('ObjectNames',binary=mono('LiftGammaGain',['Hello traveler!'])))
    assert not any(r['source']=='LiftGammaGain' for r in rs),'Technical m_Name extracted'
def renderer():
    with sqlite3.connect(ROOT/'cctv.db') as c:
        found=c.execute("select count(*) from strings where source='MENU' and json_extract(metadata,'$.path_id')=1802 and file_path like '%level0'").fetchone()[0]
    assert found==1,'Real CCTV TextMeshProUGUI m_text MENU (level0 object 1802) missing'
check('preserve_internal_controls',controls);check('inline_sprite_dialogue',modifiers);check('scripts_and_binary_both_extract',binary_priority);check('do_not_extract_object_name',names)
check('known_renderer_uppercase',renderer)
(ROOT/'regression-results.json').write_text(json.dumps(out,indent=2),encoding='utf-8')
print(json.dumps(out,indent=2))
raise SystemExit(0 if all(t['passed'] for t in out) else 1)
