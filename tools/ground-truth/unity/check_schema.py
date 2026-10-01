from pathlib import Path
from UnityPy.helpers.TypeTreeGenerator import TypeTreeGenerator
from UnityPy.helpers.Tpk import get_typetree_node
import UnityPy,json
ROOT=Path(__file__).resolve().parent
r=Path(r'D:\juegos\unity\CCTV_WINDOWS_1_3_FULL')
g=TypeTreeGenerator('2022.3.32f1');g.load_local_dll_folder(str(r/'CCTV_Data/Managed'))
e=UnityPy.load(str(r/'CCTV_Data/data.unity3d'));e.typetree_generator=g
for cls in ['TextMeshProUGUI','QuickTextSequencer','DialogButton','TooltipTargetUI']:
    o=next(o for o in e.objects if o.type.name=='MonoBehaviour' and o.parse_monobehaviour_head().m_Script.deref().read().m_ClassName==cls)
    n=o.generate_monobehaviour_node()
    print(cls,'id',o.path_id,'raw',o.get_raw_data()[:64].hex(),flush=True)
    print('NODES',[(x.m_Type,x.m_Name,x.m_MetaFlag) for x in n.m_Children[:15]],flush=True)
    try:print('TREE',o.read_typetree(nodes=n),flush=True)
    except Exception as ex:print('ERR',str(ex),flush=True)
    # m_Enabled is a byte followed by 4-byte alignment in actual engine serialization.
    n.m_Children[1].m_MetaFlag|=0x4000
    try:print('ALIGNED',o.read_typetree(nodes=n),flush=True)
    except Exception as ex:print('ALIGNED ERR',str(ex),flush=True)
