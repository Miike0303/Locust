from pathlib import Path
from UnityPy.helpers.TypeTreeGenerator import TypeTreeGenerator
import UnityPy
root=Path(r'D:\juegos\unity\CCTV_WINDOWS_1_3_FULL')
print('START',flush=True)
g=TypeTreeGenerator('2022.3.32f1');print('CREATED',flush=True)
for p in (root/'CCTV_Data/Managed').glob('*.dll'):
    print('LOAD',p.name,flush=True);g.load_dll(p.read_bytes())
print('LOADED',flush=True)
e=UnityPy.load(str(root/'CCTV_Data/data.unity3d'));e.typetree_generator=g
o=next(o for o in e.objects if o.type.name=='MonoBehaviour')
print(o.read_typetree(nodes=o.generate_monobehaviour_node()),flush=True)
