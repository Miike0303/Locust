import json,subprocess,os,sqlite3
from pathlib import Path
G=Path(__file__).parent;bin=r'C:\Projects\Locust\target\debug\locust.exe'
env=os.environ.copy();env.update(LOCUST_DATA_DIR=str(G/'profile'),TMP=str(G/'temp'),TEMP=str(G/'temp'))
root=G/'regression';(root/'game').mkdir(parents=True,exist_ok=True)
script='''define e = Character("Eileen")
label start:
    menu:
        "Choose a route"
        "First":
            e "A visible branch line."
            "A visible narration line."
        "Second":
            e "Another visible branch line."
    e "An outside line."
'''
(root/'game'/'script.rpy').write_text(script,encoding='utf8')
p=subprocess.run([bin,'extract',str(root),'-o',str(G/'regression.locust.db')],env=env,capture_output=True,text=True,encoding='utf8',errors='replace')
(G/'regression.extract.log').write_text(p.stdout+p.stderr,encoding='utf8')
c=sqlite3.connect(G/'regression.locust.db');rows=list(c.execute('select id,source,context,tags,metadata from strings'));print(json.dumps(rows,indent=2))
expected={'A visible branch line.','A visible narration line.','Another visible branch line.','An outside line.'}
found={r[1] for r in rows};missing=sorted(expected-found)
(G/'regression-result.json').write_text(json.dumps({'expected':sorted(expected),'rows':rows,'missing':missing},indent=2))
assert not missing, 'Visible RenPy say statements dropped: '+repr(missing)
