import json,collections
from pathlib import Path
G=Path(__file__).parent
d=[json.loads(l) for l in (G/'ast-misses.jsonl').read_text('utf8').splitlines()]
def reason(t):
 t=t.strip()
 if len(t.encode('utf8'))<2 or not any(c.isalpha() for c in t):return 'no alphabetic character / too short'
 if t.startswith(('renpy','store.','_')):return 'prefix rejected'
 if '/' in t or '\\' in t:return 'slash/backslash (including text tags)'
 if any(x in t for x in ['==','!=','>=','<=','+=','-=']):return 'operator substring'
 if t.startswith(('not ','if ','elif ','import ','def ','class ')):return 'code-like prefix'
 if ' ' not in t and ('.' in t or '_' in t):return 'single-word period/underscore'
 return 'passes predicate: scanner/normalization gap'
cs=collections.Counter(reason(x['text']) for x in d if x['verified_class']=='compiled Say.what omitted by heuristic');print(cs)
for k in cs:
 print('\n',k)
 for b in [x for x in d if x['verified_class']=='compiled Say.what omitted by heuristic' and reason(x['text'])==k][:3]:print(b['game'],b['ref'],repr(b['statement']),b['ast_contexts'][:1])
