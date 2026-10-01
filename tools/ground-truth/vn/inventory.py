import os, sys, json, collections, hashlib, urllib.request
from pathlib import Path
ROOT=Path(__file__).resolve().parent
sys.stdout.reconfigure(encoding='utf-8',errors='backslashreplace')
def dump(name,obj): (ROOT/name).write_text(json.dumps(obj,ensure_ascii=False,indent=2),encoding='utf-8')
def digest(p):
    with p.open('rb') as f: return hashlib.file_digest(f,'sha256').hexdigest()
def main():
    games=[]; selected=[]
    for base in ['VN','parches','otro']:
        for p in sorted(Path('D:/juegos',base).iterdir()):
            if not p.is_dir(): continue
            files=[]
            for d,ds,fs in os.walk(p):
                ds[:]=[n for n in ds if n.lower() not in {'.git','node_modules','lib','renpy','save','savedata','images','audio','voice','video'}]
                files.extend(Path(d,n) for n in fs)
            ext=collections.Counter(f.suffix.lower() for f in files)
            markers=[str(f) for f in files if f.suffix.lower() in {'.xp3','.ypf','.xfl','.pck','.rgss3a','.asar','.nw'} or f.name.lower() in {'nscript.dat','nscr_sec.dat','nscript.___','0.txt','00.txt','system.json','system.rvdata2','rpg_core.js','rmmz_core.js'}]
            engine='unidentified'
            if (p/'renpy').is_dir(): engine='RenPy'
            elif ext['.xp3']: engine='KiriKiri/KAG'
            elif ext['.ypf'] or ext['.ybn']: engine='YU-RIS'
            elif ext['.xfl']: engine='Liar-soft XFL'
            elif (p/'System.dat').exists() and (p/'System.dat').open('rb').read(9)==b'DataPack5': engine='GsPack/GsWin (DataPack5)'
            elif any(f.name.lower() in {'rpg_core.js','rmmz_core.js'} for f in files): engine='RPG Maker MV/MZ'
            elif ext['.pck']: engine='Godot'
            elif (p/'tyrano').is_dir() or any('/data/scenario/' in f.as_posix() for f in files): engine='Tyrano'
            elif any(f.name.lower() in {'nscript.dat','nscr_sec.dat','nscript.___','0.txt','00.txt'} for f in files): engine='NScripter candidate'
            games.append(dict(root=str(p),engine=engine,extensions=dict(ext),markers=markers,files=len(files)))
            if base=='VN':
                selected.extend(f for f in files if f.suffix.lower() in {'.ks','.ybn','.json','.tjs'} and not 'Certified Mother' in str(f))
                selected.extend(f for f in files if f.suffix.lower()=='.xp3' and f.stem.lower() in {'data','patch','patch2','scn'})
                selected.extend(f for f in files if f.name.lower() in {'ysbin.ypf','ysbin.ypf.bak','readme.md','ysbin-patch.ps1','patch-config.txt'})
    dump('inventory.json',games)
    hashes={str(p):digest(p) for p in sorted(set(selected))}
    if (ROOT/'original-hashes-before.json').exists():
        before=json.loads((ROOT/'original-hashes-before.json').read_text(encoding='utf-8'))
        dump('original-hashes-check.json',dict(checked=len(before),changes=[p for p,h in before.items() if hashes.get(p)!=h],scope='all audited loose scripts, YBN/JSON and selected scenario XP3/YPF archives; media excluded'))
    else: dump('original-hashes-before.json',hashes)
    for g in games: print(g['engine'],g['root'],g['files'],flush=True)
    print('HASHED',len(hashes),flush=True)
if __name__=='__main__': main()
