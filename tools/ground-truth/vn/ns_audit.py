from audit import *
def main():
    root=ROOT/'corpus/NScripterJP';p=root/'0.txt';b=p.read_bytes();text=b.decode('cp932');db=ROOT/'NScripterJP.db'
    if db.exists():db.unlink()
    rc=run(['extract',root,'-f','nscripter','-o',db],'NScripterJP-extract.log');assert rc==0
    rs=rows(db);byline={int(r['id'].rsplit('#',1)[1]):r for r in rs};oracle=[];miss=[];tp=set()
    for n,line in enumerate(text.split('\n'),1):
        line=line.rstrip('\r');s=line.lstrip(' \t')
        if not s or s[0] in ';*':continue
        if not s[0].isascii() or s[0]=='`':
            o=dict(file=str(p),location='LF line '+str(n),line=n,kind='message',text=line);oracle.append(o)
            if n in byline:tp.add(byline[n]['id'])
            else:miss.append(dict(**o,defect='nscripter_miss_message'))
        if s.startswith(('caption ','rmenu ')):
            for value in re.findall(r'"([^"]*)"',s):
                o=dict(file=str(p),location='LF line '+str(n),line=n,kind='caption_or_menu',text=value,physical=line);oracle.append(o);miss.append(dict(**o,defect='nscripter_miss_display_attribute'))
    stats=dict(slug='NScripterJP',engine='nscripter',files=1,rows=len(rs),oracle=len(oracle),hits=len(oracle)-len(miss),miss=len(miss),tp_rows=len(tp),fp=0,unknown=len(rs)-len(tp),unmapped=0,recall=(len(oracle)-len(miss))/len(oracle),precision_lower=len(tp)/len(rs),precision_upper=1,oracle_classes=dict(collections.Counter(x['kind'] for x in oracle)),miss_classes=dict(collections.Counter(x['defect'] for x in miss)),fp_classes={})
    dump('NScripterJP-stats.json',stats);dump('NScripterJP-rows.json',rs);dump('NScripterJP-oracle.json',oracle);dump('NScripterJP-misses.json',miss)
    print(json.dumps(stats),flush=True)
if __name__=='__main__':main()
