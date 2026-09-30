import json,subprocess,os,time
from pathlib import Path
G=Path(__file__).parent
BIN=Path(r'C:\Projects\Locust\target\debug\locust.exe')
names=['StarAvenger-CH1-pc','StarAvenger-CH2_beta-pc','SRankBreeder-2.0-win','ProjektPassion-0.15-pc','FutaInn-0.9.6-pc','Eimis_NTS_Life-v0.2.1-pc','Lukesway0.18r-pc','DimitrescuLC-pc','CelebrityHunter-0.30-pc','PhotoHunt-0.20.2-pc','B.E.S.T-Episode5-pc','FriendsinNeedSeasonOne.v.0.95b-pc','RMAWH-r4.3-pc','Area69-0.83-pc','NewFamily-pc','Lust-Academy-0.7.1f-pc']
rows=[r for r in json.loads((G/'inventory.json').read_text()) if r['name'] in names]
(G/'selected.json').write_text(json.dumps(rows,indent=2),encoding='utf-8')
env=os.environ.copy()
for sub in ['profile','temp']: (G/sub).mkdir(exist_ok=True)
env.update(LOCUST_DATA_DIR=str(G/'profile'),TEMP=str(G/'temp'),TMP=str(G/'temp'))
results=[]
for r in rows:
 db=G/(r['name']+'.locust.db'); log=G/(r['name']+'.extract.log')
 if os.environ.get('GROUND_REUSE_DBS')=='1' and db.exists() and log.exists(): print('cached',r['name'],flush=True);continue
 start=time.time()
 with log.open('w',encoding='utf-8') as out:
  p=subprocess.run([str(BIN),'extract',r['root'],'-o',str(db)],env=env,stdout=out,stderr=subprocess.STDOUT)
 print(r['name'],p.returncode,round(time.time()-start,2),flush=True)
 results.append({'name':r['name'],'exit':p.returncode,'seconds':time.time()-start})
(G/'extract-results.json').write_text(json.dumps(results,indent=2))

