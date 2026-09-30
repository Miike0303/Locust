from pathlib import Path
p=Path(__file__).parent/'ast_audit.py';s=p.read_text('utf-8-sig')
s=s.replace("walk(v,seen,strings,location,owner,k)","strings[k].append({'owner':'AST field name','field':'key','location':location,'expr':False})\n   if owner=='Menu' and k=='items':\n    for item in v:\n     if isinstance(item,(tuple,list)) and len(item)==3:\n      walk(item[0],seen,strings,location,owner,'caption');walk(item[1],seen,strings,location,owner,'condition');walk(item[2],seen,strings,location,owner,'block')\n   else:walk(v,seen,strings,location,owner,k)")
s=s.replace("c['owner']=='Menu' and c['field']=='items'", "c['owner']=='Menu' and c['field']=='caption'")
s=s.replace("or c['field'] in ['who','condition','filename','label','expression','target']", "or c['owner']=='AST field name' or (c['owner']=='Say' and c['field'] in ['attributes','temporary_attributes']) or (c['owner']=='Image' and c['field']=='imgname') or (c['owner']=='PyCode' and c['field']=='mode') or (c['owner']=='Label' and c['field']=='name') or c['field'] in ['who','condition','filename','label','expression','target']")
s=s.replace("e['ast_contexts']=code", "e['ast_contexts']=code[:3]").replace("e['ast_contexts']=contexts", "e['ast_contexts']=contexts[:3]").replace("b['ast_contexts']=say", "b['ast_contexts']=say[:3]")
s=s.replace("e['verified_class']=cls;counts", "e.setdefault('ast_contexts',visible[:3]);e['verified_class']=cls;counts")
p.write_text(s,encoding='utf8')
p=Path(__file__).parent/'totals.py';s=p.read_text('utf-8-sig').replace("e.get('ast_contexts')","e.get('ast_contexts',[])[:1]");p.write_text(s,encoding='utf8')
