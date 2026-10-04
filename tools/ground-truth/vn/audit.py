from inventory import *
from readers import *
import subprocess,sqlite3,collections,re
EXE=Path(os.environ.get('LOCUST_AUDIT_CLI',str(ROOT/'target/debug/locust.exe')))
ENV=dict(os.environ,LOCUST_DATA_DIR=str(ROOT/'cli-data'),TEMP=str(ROOT),TMP=str(ROOT),PYTHONIOENCODING='utf-8')
def run(args,log):
    p=subprocess.run([str(EXE),*map(str,args)],cwd=ROOT,env=ENV,capture_output=True,encoding='utf-8',errors='replace',timeout=1800)
    (ROOT/log).write_text(p.stdout+p.stderr,encoding='utf-8');return p.returncode
def rows(db):
    c=sqlite3.connect(db);c.row_factory=sqlite3.Row;r=[dict(x) for x in c.execute('select * from strings')];c.close();return r
def norm(t):return ' '.join(t.split())
def yuris_truth(doc,commands):
    """Physical attribute units, independent of Locust's dense #arg ids.
    WORD, GOSUB ES.CHAR.NAME/ES.SEL.SET and DIALOG are definite display slots.
    Remaining expression strings are unclassified unless an immutable command
    target, comparison key or engine resource id can be established.
    """
    attrs=doc['attributes'];groups=collections.defaultdict(list)
    for a in attrs:groups[a['command']].append(a)
    out={}
    for ci,aa in groups.items():
        op=commands[aa[0]['opcode']]['name'];target=(aa[0]['text'] or '').upper()
        for a in aa:
            t=a['text'];kind=None;truth=None
            if t is None or not t:continue
            if op=='WORD' and a['arg']==0:truth=True;kind='word_control' if any(bytes.fromhex(a['raw']).find(x)>=0 for x in [b'\xef\xf0',b'\xef\xf2',b'\xef\xf3',b'\xef\xf5']) else 'word_plain'
            elif op=='CGACT' and a['id']==11 and any(x['id']==64 and x['raw']=='42010001' for x in aa):truth=True;kind='cgact_text'
            elif op=='DIALOG' and a['arg'] in {0,1}:truth=True;kind='dialog_ui'
            elif op=='GOSUB' and target in {'ES.CHAR.NAME','ES.SEL.SET'} and a['arg']>0:truth=True;kind='gosub_display'
            elif op=='GOSUB' and a['arg']==0 or op=='GO':truth=False;kind='dispatch_target'
            elif op in {'CG','CGACT','CGEND','CGINFO','SOUND','SOUNDINFO','SOUNDEND','MOVIE','FONT','F_INT','F_STR','S_STR','STR','INT','IF','ELSE','LOOP','LET','FILEACT','FILEINFO','SAVE','LOAD'}:
                # Content independent structural roles; assignment literals can
                # become visible later, so keep those outside precision claims.
                if op in {'IF','ELSE','LOOP'}:truth=False;kind='comparison_key'
                elif op in {'CG','CGACT','CGEND','CGINFO','SOUND','SOUNDINFO','SOUNDEND','MOVIE','FONT','FILEACT','FILEINFO','SAVE','LOAD'}:truth=False;kind='resource_or_config'
            out[a['index']]=dict(truth=truth,kind=kind or 'unclassified',opcode=op,text=t)
    return out
def rendered_raw(a):
    if a['type']!=0:return a['text']
    raw=bytes.fromhex(a['raw']);raw=raw.replace(b'\xef\xf0',b'\r\n').replace(b'\xef\xf2',b'\\p').replace(b'\xef\xf3',b'\\c').replace(b'\xef\xf5',b'\\u')
    return raw.decode('cp932',errors='replace')
def output_text(source):
    # CLI representation adapter only; oracle and rendered_raw stay unchanged.
    for token, value in [('F2', r'\p'), ('F3', r'\c'), ('F5', r'\u'),
                         ('CRLF', '\r\n'), ('CR', '\r'), ('LF', '\n')]:
        source=source.replace('{{yuris:'+token+'}}', value)
    source=re.sub(r'\{\{yuris:bytes:([0-9A-F]{2}(?:[0-9A-F]{2})?)\}\}', lambda m:bytes.fromhex(m[1]).decode('cp932',errors='replace'), source)
    return source

def output_attr(row, attributes):
    index=int(row['id'].rsplit('#attr',1)[1])
    a=attributes[index]
    assert output_text(row['source']) in (a['text'], rendered_raw(a)), row['id']
    return a

def main():
    cases=json.loads((ROOT/'cases.json').read_text(encoding='utf-8'));stats=[];examples=collections.defaultdict(list);allmiss=[];allfp=[];details={};commands=yscm(Path('D:/juegos/VN/Injuu Kangoku RE/res/ysc.ybn').read_bytes())
    for case in cases:
        slug=case['slug'];root=Path(case['root']);db=ROOT/(slug+'.db')
        if db.exists():db.unlink()
        rc=run(['extract',root,'-f',case['engine'],'-o',db],slug+'-extract.log');assert rc==0,(slug,rc)
        rs=rows(db);dump(slug+'-rows.json',rs);miss=[];fp=[];oracle=[];tp=set();unknown=0;locs={};line_rows=collections.defaultdict(list)
        if case['engine'] in {'kirikiri','tyrano'}:
            for r in rs:
                rel,locator=r['id'].split('#',1);m=re.match(r'(?:kag:)?(\d+)(?:#|:|$)',locator)
                if not m:raise ValueError('adapt output locator mapper without changing oracle: '+r['id'])
                line_rows[(rel,int(m.group(1)))].append(r)
            for rel,original in case['mapping'].items():
                text=decode_ks((root/rel).read_bytes());slots,technical=kag_slots(text,case['engine']=='tyrano')
                if case['engine']=='tyrano':
                    # Tyrano's own #name syntax and glink/button attributes.
                    for n,line in enumerate(text.splitlines(),1):
                        if line.strip().startswith('#'):
                            slots=[s for s in slots if s['line']!=n];name=line.strip()[1:].split(':',1)[0]
                            if name:slots.append(dict(line=n,kind='speaker_reference',text=name,physical=line))
                    # Bare ASCII # ids reference chara declarations; literal
                    # Japanese names and '?' are the displayed name itself.
                    slots=[dict(s,kind='speaker_literal') if s['kind']=='speaker_reference' else s for s in slots if s['kind']!='speaker_reference' or not re.fullmatch(r'[A-Za-z_][A-Za-z0-9_-]*',s['text'])]
                physical_lines=text.splitlines(keepends=True);cli_lines={};start=0
                cli_no=1
                # KiriKiri kag:N counts CR, LF and CRLF alike (cycle 111);
                # Tyrano's #N counts LF only.
                for logical,physical in enumerate(physical_lines,1):
                    cli_lines[logical]=logical if case['engine']=='kirikiri' else cli_no;cli_no+=physical.count('\n')
                byline=collections.defaultdict(list)
                for s in slots:byline[cli_lines[s['line']]].append(s)
                technical_byline={n:(kind,line) for n,kind,line in technical}
                for s in slots:
                    q=dict(file=original,relative=rel,location='line '+str(s['line']),**s);oracle.append(q)
                    # Raw whole-line extraction covers message text but tag
                    # attributes are unsafe conflations, not isolated slots.
                    candidates=line_rows.get((rel,cli_lines[s['line']]),[])
                    covered=any((s['kind']=='message_text' and s['text'] in r['source']) or r['source']==s['text'] for r in candidates)
                    if covered:
                        for r in candidates:tp.add(r['id'])
                    else:
                        cl=s['kind']
                        if case['engine']=='tyrano' and cl.startswith('attribute_'):cl='visible_attribute'
                        if cl=='message_text':
                            cl='bare_cr_message' if '\r' in text.replace('\r\n','') else 'punctuation_message' if not any(c.isalpha() for c in s['text']) else cl
                        miss.append(dict(**q,defect=case['engine']+'_miss_'+cl))
                for n,line in enumerate(text.split('\n'),1):
                    for r in line_rows.get((rel,n),[]):
                        actual=[(ln,k,v) for ln,(k,v) in technical_byline.items() if cli_lines[ln]==n and v.strip()==r['source'].strip()]
                        if actual:fp.append(dict(file=original,relative=rel,line=actual[0][0],location='logical line '+str(actual[0][0])+' CLI line '+str(n),text=r['source'],id=r['id'],defect=case['engine']+'_fp_script_body'));tp.discard(r['id'])
                        elif case['engine']=='tyrano' and not byline[n] and all(k=='tag' or not s.strip() for k,a,b,s in tokens(line)):
                            fp.append(dict(file=original,relative=rel,line=n,location='line '+str(n),text=r['source'],id=r['id'],defect='tyrano_fp_quoted_bracket_tag'));tp.discard(r['id'])
                        elif n not in byline:unknown+=1
            counts=collections.Counter(x['kind'] for x in oracle)
            # Recall counts oracle slots; precision counts extracted rows. A
            # row with two literal spans covers two slots, only one output row.
            hits=len(oracle)-len(miss);unmapped=[]
        else:
            byfile=collections.defaultdict(list)
            for r in rs:byfile[Path(r['file_path']).name].append(r)
            counts=collections.Counter();hits=0;unmapped=[]
            for rel,original in case['mapping'].items():
                b=(root/rel).read_bytes()
                if b[:4]==b'YSCF':
                    # Header offset from the shipped caption length+string;
                    # VNTextPatch YurisConfigScript reads v>=0x1C2 at 0x4C.
                    off=0x4c;n=struct.unpack_from('<H',b,off)[0];title=b[off+2:off+2+n].decode('cp932')
                    o=dict(file=original,relative=rel,location='caption length at 0x4c, bytes 0x4e..',text=title,kind='project_caption',attr=-1)
                    oracle.append(o);counts['project_caption']+=1
                    matching=[r for r in byfile.get(rel,[]) if norm(r['source'])==norm(title)]
                    if matching:hits+=1;tp.update(r['id'] for r in matching)
                    else:miss.append(dict(**o,defect='yuris_miss_display_literal'))
                    continue
                if b[:4]!=b'YSTB':continue
                doc=ystb(b);truth=yuris_truth(doc,commands);aa=doc['attributes'];mapped={};cursor=0
                for r in sorted(byfile.get(rel,[]),key=lambda x:int(x['id'].rsplit('#attr',1)[1])):
                    a=output_attr(r, aa);mapped[a['index']]=r;locs[r['id']]=a['index']
                    q=truth.get(a['index'])
                    if not q or q['truth'] is None:unknown+=1
                    elif q['truth']:tp.add(r['id'])
                    else:fp.append(dict(file=original,relative=rel,location=f"instruction {a['command']} opcode {q['opcode']} argument {a['arg']} attr {a['index']}",text=r['source'],id=r['id'],attr=a['index'],subclass=q['kind'],defect='yuris_fp_technical_attribute'))
                for i,q in truth.items():
                    if q['truth'] is not True:continue
                    a=aa[i];o=dict(file=original,relative=rel,location=f"instruction {a['command']} opcode {q['opcode']} argument {a['arg']} attr {i}",text=rendered_raw(a),raw_hex=a['raw'],kind=q['kind'],attr=i)
                    oracle.append(o);counts[q['kind']]+=1
                    if i in mapped:hits+=1
                    else:miss.append(dict(**o,defect='yuris_miss_word_control' if q['kind']=='word_control' else 'yuris_miss_display_literal'))
            dump('yuris-row-attributes.json',locs)
        for x in miss+fp:
            if len(examples[x['defect']])<3:examples[x['defect']].append(x)
        allmiss.extend(miss);allfp.extend(fp)
        st=dict(slug=slug,engine=case['engine'],files=len(case['mapping']),rows=len(rs),oracle=len(oracle),hits=hits,miss=len(miss),tp_rows=len(tp),fp=len(fp),unknown=unknown,unmapped=len(unmapped),recall=hits/len(oracle) if oracle else None,precision_lower=len(tp)/len(rs) if rs else None,precision_upper=(len(rs)-len(fp))/len(rs) if rs else None,oracle_classes=dict(counts),miss_classes=dict(collections.Counter(x['defect'] for x in miss)),fp_classes=dict(collections.Counter(x['defect'] for x in fp)))
        stats.append(st);dump(slug+'-oracle.json',oracle);dump(slug+'-unmapped.json',unmapped);print(json.dumps(st,ensure_ascii=False),flush=True)
    dump('stats.json',stats);dump('class-examples.json',dict(examples));dump('misses.json',allmiss);dump('false-positives.json',allfp)
if __name__=='__main__':main()
