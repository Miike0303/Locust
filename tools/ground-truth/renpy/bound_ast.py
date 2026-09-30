from pathlib import Path
p=Path(__file__).parent/'ast_audit.py';s=p.read_text('utf-8-sig')
s=s.replace('def walk(obj,seen,strings,location=None,owner=\'\',field=\'\'):', '''RECORD_COUNTS=collections.Counter()
def record(strings,text,context):
 sig=(text,context['owner'],context['field'],context['expr'])
 if RECORD_COUNTS[sig]<3:strings[text].append(context)
 RECORD_COUNTS[sig]+=1
def walk(obj,seen,strings,location=None,owner='',field=''):''')
s=s.replace("strings[obj].append({'owner':owner,'field':field,'location':location,'expr':isinstance(obj,Expr)})", "record(strings,obj,{'owner':owner,'field':field,'location':location,'expr':isinstance(obj,Expr)})")
s=s.replace("strings[k].append({'owner':'AST field name','field':'key','location':location,'expr':False})", "record(strings,k,{'owner':'AST field name','field':'key','location':location,'expr':False})")
s=s.replace('strings=collections.defaultdict(list);errors=[];files=0', 'RECORD_COUNTS.clear();strings=collections.defaultdict(list);errors=[];files=0')
p.write_text(s,encoding='utf8')
