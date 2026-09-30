import os,subprocess,sys
from pathlib import Path
G=Path(__file__).parent
steps=['inventory.py','extract.py','audit.py','ast_audit.py','menu_audit.py','classify.py','classify_extra.py','report.py']
env=os.environ.copy();env.update(LOCUST_DATA_DIR=str(G/'profile'),TMP=str(G/'temp'),TEMP=str(G/'temp'),PYTHONIOENCODING='utf-8')
for step in steps:
 print('RUN',step,flush=True)
 p=subprocess.run([sys.executable,str(G/step)],cwd=G,env=env)
 if p.returncode:raise SystemExit(p.returncode)
p=subprocess.run([sys.executable,str(G/'regression.py')],cwd=G,env=env)
if p.returncode!=1:raise SystemExit('Expected the menu regression assertion to fail against current CLI')
print('Audit complete. The menu regression failed as expected. Results: '+str(G/'report.txt'))
