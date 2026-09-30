import json,collections,re
from pathlib import Path
G=Path(__file__).parent
ms=[json.loads(l) for l in (G/'misses.jsonl').read_text('utf8').splitlines()]
def fileish(t):
 t=t.strip()
 return t.endswith(tuple('.'+x for x in ['png','jpg','jpeg','webp','gif','svg','bmp','mp3','ogg','wav','flac','mp4','webm','avi','ogv','ttf','otf','woff','rpy','rpyc','rpa','json','txt','xml','csv'])) or (('/' in t or '\\' in t) and ' ' not in t) or (t.startswith('#') and len(t)<=9 and re.fullmatch('[0-9a-fA-F]*',t[1:]))
d=[x for x in ms if x['class']=='other parser rejection'];print('fileish',sum(fileish(x['text']) for x in d))
for x in [x for x in d if not fileish(x['text'])][:15]:print(x['game'],x['ref'],repr(x['statement']),repr(x['current']),x['trace'])
for x in ms:
 if x['class']=='other parser rejection' and fileish(x['text']):x['class']='visible markup/text misidentified as file reference'
with (G/'classified-misses.jsonl').open('w',encoding='utf8') as f:
 for x in ms:f.write(json.dumps(x,ensure_ascii=False)+'\n')
print(collections.Counter(x['class'] for x in ms))
