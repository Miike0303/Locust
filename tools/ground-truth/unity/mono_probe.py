import UnityPy,collections,json
from pathlib import Path
ROOT=Path(__file__).resolve().parent
for g in ['CCTV_WINDOWS_1_3_FULL','Sunkissed_windows_full','es/BOXMAN_v0.5.02_x64']:
    data=next((Path(r'D:\juegos\unity')/g).glob('*_Data'))
    files=list(data.glob('*.assets'))+list(data.glob('level*'))+list(data.glob('data.unity3d'))
    c=collections.Counter();out={}
    for p in files:
        if p.suffix=='.resS' or not p.is_file():continue
        for o in UnityPy.load(str(p)).objects:
            if o.type.name!='MonoBehaviour':continue
            d=o.read(check_read=False)
            try:cl=d.m_Script.deref().read().m_ClassName
            except:cl='unknown'
            c[cl]+=1
            if cl not in out:out[cl]=dict(file=str(p),node=o.assets_file.name,id=o.path_id,name=d.m_Name,raw=o.get_raw_data().hex())
    (ROOT/(g.replace('/','-')+'-mono-samples.json')).write_text(json.dumps(out,indent=2),encoding='utf-8')
    print(g,c,flush=True)
