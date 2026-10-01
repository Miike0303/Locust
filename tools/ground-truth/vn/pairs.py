from audit import *
def main():
    g=Path('D:/juegos/VN/Injuu Kangoku RE');rs=json.loads((ROOT/'Injuu-rows.json').read_text(encoding='utf-8'));by=collections.defaultdict(list)
    for r in rs:by[r['id'].rsplit('#arg',1)[0]].append(r['source'])
    counts=collections.Counter();examples=[];oracle=set();hits=set();missing=[];mismatch=[]
    for p in sorted((g/'output').glob('*.json')):
        q=g/'output.ja.bak'/p.name
        if not q.exists():continue
        a=json.loads(q.read_text(encoding='utf-8-sig'));b=json.loads(p.read_text(encoding='utf-8-sig'));counts['paired_files']+=1
        if len(a)!=len(b):counts['array_length_mismatch']+=1;mismatch.append(dict(file=p.name,before=len(a),after=len(b)));continue
        for i,(old,new) in enumerate(zip(a,b)):
            for field,value in new.items():
                if field not in {'message','name'} or not isinstance(value,str) or not value.strip():continue
                counts['paired_nonempty_fields']+=1
                if old.get(field)!=value:
                    counts['changed_fields']+=1
                    if len(examples)<3:examples.append(dict(before_file=str(q),after_file=str(p),location=f'$[{i}].{field}',before=old.get(field),after=value))
                key=(p.stem+'.ybn',field,norm(value));oracle.add(key)
                if any(norm(value) in norm(s) for s in by[key[0]]):hits.add(key)
                elif len(missing)<3:missing.append(dict(file=str(p),location=f'$[{i}].{field}',text=value))
    result=dict(counts=dict(counts),unique_file_field_text=len(oracle),found=len(hits),recall=len(hits)/len(oracle),examples=examples,missing_examples=missing,mismatches=mismatch,definition='unique (file, name/message field, whitespace-normalized value) from existing translated JSON; found when contained in any Locust source from matching compiled file; lexical coverage only, not per-occurrence or precision')
    dump('translation-pairs.json',result);print(json.dumps({k:v for k,v in result.items() if k not in {'examples','missing_examples','mismatches'}},ensure_ascii=False),flush=True)
if __name__=='__main__':main()
