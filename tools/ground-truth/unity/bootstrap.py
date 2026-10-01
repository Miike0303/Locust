from pathlib import Path
import subprocess, zipfile, io, json, hashlib, os, sys
ROOT=Path(__file__).resolve().parent
REPO=Path(r'C:\Projects\Locust')
commit=(ROOT/'snapshot-commit.txt').read_text().strip() if (ROOT/'snapshot-commit.txt').exists() and '--live-unity' not in sys.argv else subprocess.check_output(['git','rev-parse','HEAD'],cwd=REPO,text=True).strip()
(ROOT/'snapshot-commit.txt').write_text(commit,encoding='utf-8')
zipdata=subprocess.check_output(['git','archive','--format=zip',commit],cwd=REPO)
with zipfile.ZipFile(io.BytesIO(zipdata)) as z: z.extractall(ROOT/'workspace')
files=[]
for p in Path(r'D:\juegos\unity').rglob('*'):
    if p.is_file(): files.append({'path':str(p),'size':p.stat().st_size})
(ROOT/'inventory.json').write_text(json.dumps(files,indent=2),encoding='utf-8')
print('Files',len(files))
for f in files:
    p=Path(f['path'])
    if p.suffix.lower() in {'.txt','.csv','.json','.xml','.bytes','.loc','.nani'} or any(x in str(p).lower() for x in ['translation','localization','language','bepinex','autotranslator']):print(f)
