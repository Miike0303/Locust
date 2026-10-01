from pathlib import Path
import UnityPy,json,collections,sys
from UnityPy.helpers.TypeTreeGenerator import TypeTreeGenerator
ROOT=Path(__file__).resolve().parent
sys.stdout.reconfigure(encoding='utf-8',errors='backslashreplace')
def walk(v,p=()):
    if isinstance(v,str):yield '.'.join(map(str,p)),v
    elif isinstance(v,dict):
        for k,x in v.items():yield from walk(x,p+(k,))
    elif isinstance(v,list):
        for i,x in enumerate(v):yield from walk(x,p+(i,))
for game in ['CCTV_WINDOWS_1_3_FULL','CCTV_USSR_WINDOWS_FULL','Sunkissed_windows_full','Out of Touch','es/BOXMAN_v0.5.02_x64']:
    data=next((Path(r'D:\juegos\unity')/game).glob('*_Data'))
    candidates=list(data.glob('*.assets'))+list(data.glob('level*'))+list(data.glob('data.unity3d'))
    out=[];err=[];classes=collections.Counter()
    generator=None;scriptcache={}
    for p in candidates:
        if p.suffix=='.resS' or not p.is_file():continue
        try:env=UnityPy.load(str(p))
        except Exception as e:err.append([str(p),str(e)]);continue
        if generator is None:
            ver=next(iter(env.objects)).assets_file.unity_version
            print('GENERATOR',game,repr(ver),flush=True)
            generator=TypeTreeGenerator('2022.3.32f1' if str(ver).startswith('6000') else str(ver));generator.load_local_dll_folder(str(data/'Managed'))
        env.typetree_generator=generator
        for o in env.objects:
            if o.type.name not in ['MonoBehaviour','TextMesh','GUIText']:continue
            try:
                cls=o.type.name
                if cls=='MonoBehaviour':
                    h=o.parse_monobehaviour_head();ptr=h.m_Script
                    sk=(o.assets_file.name,ptr.m_FileID,ptr.m_PathID)
                    if sk not in scriptcache:
                        sd=ptr.deref().read()
                        scriptcache[sk]=(sd.m_ClassName,sd.m_AssemblyName,(sd.m_Namespace+'.' if sd.m_Namespace else '')+sd.m_ClassName)
                    cls,assembly,fullname=scriptcache[sk]
                    if cls not in {'TextMeshProUGUI','TextMeshPro','Text','QuickTextSequencer','DialogButton','TooltipTargetUI','Script','ManagedTextProvider','ScriptAsset','TMP_Dropdown','StringTableCollection','LocalizationConfiguration','ProjectResources','ItemBubble'}:continue
                    if cls not in classes:print('CLASS',game,cls,o.path_id,flush=True)
                tree=o.read_typetree(nodes=generator.get_nodes_up(assembly,fullname) if o.type.name=='MonoBehaviour' else None)
                fields=list(walk(tree))
                fields=[(k,v) for k,v in fields if v]
                if fields:
                    classes[cls]+=1
                    out.append(dict(file=str(p),node=o.assets_file.name,path_id=o.path_id,cls=cls,fields=fields))
            except Exception as e:err.append([str(p),o.path_id,str(e)])
    (ROOT/(game.replace('/','-')+'-trees.json')).write_text(json.dumps(dict(objects=out,errors=err),ensure_ascii=True),encoding='utf-8')
    print(game,'classes',classes,'errors',len(err),flush=True)
    groups=collections.defaultdict(list)
    for o in out:
        for k,v in o['fields']:
            if k!='m_Name':groups[(o['cls'],k)].append(v)
    for (c,k),vs in groups.items():
        print(c,k,len(vs),repr(vs[:3]),flush=True)
