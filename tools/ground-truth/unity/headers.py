from pathlib import Path
import UnityPy,json,sys
ROOT=Path(__file__).resolve().parent
out=[]
scripts={}
for game in ['CCTV_WINDOWS_1_3_FULL','CCTV_USSR_WINDOWS_FULL','es/BOXMAN_v0.5.02_x64','Out of Touch','Sunkissed_windows_full']:
    data=next((Path(r'D:\juegos\unity')/game).glob('*_Data'))
    for p in list(data.glob('*.assets'))+list(data.glob('level*'))+list(data.glob('data.unity3d')):
        if p.suffix=='.resS' or not p.is_file():continue
        for o in UnityPy.load(str(p)).objects:
            if o.type.name!='MonoBehaviour':continue
            try:
                h=o.parse_monobehaviour_head()
                if h.m_Name:
                    k=(str(p),o.assets_file.name,h.m_Script.m_FileID,h.m_Script.m_PathID)
                    if k not in scripts:
                        try:scripts[k]=h.m_Script.deref().read().m_ClassName
                        except Exception:scripts[k]='unknown'
                    out.append(dict(game=game,file=str(p/o.assets_file.name) if p.name=='data.unity3d' else str(p),path_id=o.path_id,name=h.m_Name,cls=scripts[k]))
            except Exception:pass
(ROOT/'independent-headers.json').write_text(json.dumps(out,ensure_ascii=True,indent=2),encoding='utf-8')
print('Independent m_Name fields',len(out))
