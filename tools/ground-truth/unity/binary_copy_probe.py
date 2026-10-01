from pathlib import Path
import UnityPy,json,hashlib,subprocess,os,sqlite3,uuid
ROOT=Path(__file__).resolve().parent
original=Path(r'D:\juegos\unity\CCTV_WINDOWS_1_3_FULL\CCTV_Data\data.unity3d')
def sha(p):return hashlib.sha256(p.read_bytes()).hexdigest()
before=sha(original)
env=UnityPy.load(str(original))
o=next(o for o in env.objects if o.assets_file.name=='resources.assets')
sf=o.assets_file
run_id=uuid.uuid4().hex[:8]
copy=ROOT/('binary-inject-copy-'+run_id)/'CCTV_Data';copy.mkdir(parents=True,exist_ok=True)
p=copy/'resources.assets';p.write_bytes(sf.reader.bytes)
objects={str(i):hashlib.sha256(o.get_raw_data()).hexdigest() for i,o in sf.objects.items()}
exe=ROOT/'target/debug/locust.exe';en=dict(os.environ,LOCUST_DATA_DIR=str(ROOT/'locust-data'),LOCUST_RESEARCH_LOCK_DIR=str(ROOT/'locks'))
def run(a,log):
    r=subprocess.run([str(exe),*map(str,a)],env=en,capture_output=True);(ROOT/log).write_bytes(r.stdout+r.stderr);assert r.returncode==0
db=ROOT/('binary-inject-'+run_id+'.db');run(['extract',p,'-o',db],'binary-copy-extract.log')
with sqlite3.connect(db) as c:
    c.row_factory=sqlite3.Row;rows=[dict(r) for r in c.execute('select * from strings')]
    c.execute("update strings set translation=NULL,status='pending'")
    selected=sorted([r for r in rows if (r['source']=='Confirm' or len(r['source'])>60) and not any(t in r['source'] for t in ['<','\\','{'])],key=lambda r:r['id'])[:3]
    for r in selected:c.execute("update strings set translation=?,status='translated' where id=?",('AUDIT',r['id']))
    c.commit()
run(['inject',copy.parent,'-P',db,'-l','es','--direct'],'binary-copy-inject.log')
afterenv=UnityPy.load(str(p));asf=next(iter(afterenv.objects)).assets_file
changed=[];unexpected=[]
allowed={json.loads(r['metadata'])['path_id']:r for r in selected}
for i,o in sf.objects.items():
    actual=asf.objects[i].get_raw_data();old=o.get_raw_data()
    if actual!=old:
        changed.append(i)
        if i not in allowed:unexpected.append(dict(path_id=i,kind='untargeted_object_changed'))
        else:
            m=json.loads(allowed[i]['metadata']);off=m['mono_string_offset']-o.byte_start;length=m['mono_string_byte_len']
            exp=bytearray(old);exp[off+4:off+4+length]=b'AUDIT'+b' '*(length-5)
            if actual!=exp:unexpected.append(dict(path_id=i,kind='outside_target_slot_changed'))
assert sha(original)==before,'Original bundle changed'
out=dict(original=str(original),original_hash=before,original_changes=0,copy=str(p),objects_checked=len(objects),selected=selected,changed_objects=changed,unexpected_changes=unexpected)
(ROOT/'binary-injection.json').write_text(json.dumps(out,ensure_ascii=True,indent=2),encoding='utf-8')
print(json.dumps({k:v for k,v in out.items() if k!='selected'},indent=2))
