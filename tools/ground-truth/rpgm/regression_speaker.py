"""Expected to FAIL on current main. All fixture/output writes stay in scratch."""
from audit import *
game=ROOT/'regression-speaker';data=game/'data';data.mkdir(parents=True,exist_ok=True)
(data/'System.json').write_text(json.dumps({'gameTitle':'Test','terms':{}}),encoding='utf-8')
(data/'Map001.json').write_text(json.dumps({'events':[None,{'pages':[{'list':[
    {'code':101,'indent':0,'parameters':['',0,0,2,'Alice']},
    {'code':401,'indent':0,'parameters':['Hello!']},
    {'code':0,'indent':0,'parameters':[]}
]}]}]}),encoding='utf-8')
db=ROOT/'regression-speaker.locust.db'
assert run(['extract',game,'-f','rpgmaker-mv','-o',db],'regression-speaker.log')==0
got=rows(db)
print('sources:',[r['source'] for r in got]);print('contexts:',[r['context'] for r in got])
assert any(r['source']=='Alice' for r in got), 'VISIBLE MZ SPEAKER Alice exists only as context; no translatable entry'
