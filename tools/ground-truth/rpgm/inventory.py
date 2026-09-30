import os,json,re,sys
from pathlib import Path
ROOT=Path(__file__).resolve().parent
sys.stdout.reconfigure(encoding='utf-8',errors='backslashreplace')
out=[]; errors=[]
def err(e): errors.append(str(e))
for d,dirs,files in os.walk(sys.argv[1] if len(sys.argv)>1 else 'D:/juegos',onerror=err):
    dirs[:]=[x for x in dirs if x.lower() not in {'img','images','audio','video','movies','node_modules','swiftshader','locales','.git','fonts','effects','cache','sdk','lib','python-packages','.locust_backups','.locust-injections'}]
    # Do not prune a directory literally called game: some RPG deployments use it.
    lower={x.lower():x for x in files}
    engine=None
    if 'system.json' in lower or 'system.jsono' in lower:
        engine='MV/MZ'
    elif 'system.rvdata2' in lower: engine='VX Ace'
    elif any(x.endswith('.rgss3a') for x in lower): engine='VX Ace archive'
    if not engine: continue
    p=Path(d); root=p.parent if p.name.lower() in {'data'} else p
    if root.name.lower()=='www': root=root.parent
    core=root/'js'; core=core if core.exists() else root/'www/js'
    if (core/'rmmz_core.js').exists(): engine='MZ'
    elif (core/'rpg_core.js').exists(): engine='MV'
    system=p/lower.get('system.json',lower.get('system.jsono','System.rvdata2'))
    title=None; protected=False
    if system.suffix=='.json':
        try:
            s=json.loads(system.read_text(encoding='utf-8-sig'));title=s.get('gameTitle');protected='uid' in s and 'data' in s
        except Exception as e: errors.append(f'{system}: {e}')
    out.append(dict(root=str(root),data=str(p),engine=engine,title=title,protected=protected,json_files=sum(x.endswith('.json') for x in lower),jsono_files=sum(x.endswith('.jsono') for x in lower),rvdata_files=sum(x.endswith('.rvdata2') for x in lower)))
    print('FOUND',engine,root,flush=True)
out.sort(key=lambda x:x['root'])
(ROOT/('inventory-rpgm.json' if len(sys.argv)>1 else 'inventory.json')).write_text(json.dumps(dict(games=out,errors=errors),ensure_ascii=False,indent=2),encoding='utf-8')
for i,g in enumerate(out):print(i,g['engine'],g['json_files'],g['jsono_files'],g['rvdata_files'],g['protected'],g['root'],repr(g['title']),flush=True)
print('TOTAL',len(out),'ERRORS',len(errors))
