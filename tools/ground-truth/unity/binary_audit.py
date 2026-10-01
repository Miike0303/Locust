from pathlib import Path
import UnityPy,json,collections,sqlite3,re,csv,io
ROOT=Path(__file__).resolve().parent
GAMES={'cctv':('CCTV_WINDOWS_1_3_FULL','cctv.db'),'ussr':('CCTV_USSR_WINDOWS_FULL','ussr.db'),'sunkissed':('Sunkissed_windows_full','sunkissed.db'),'boxman':('es/BOXMAN_v0.5.02_x64','boxman-alone.db'),'oot-binary':('Out of Touch','oot-alone.db')}
def dump(n,x):(ROOT/n).write_text(json.dumps(x,ensure_ascii=True,indent=2),encoding='utf-8')
def dbrows(p):
    with sqlite3.connect(p) as c:
        c.row_factory=sqlite3.Row;return [dict(r) for r in c.execute('select * from strings')]
results=[];misses=[];fps=[];unclassified=[];truthall=[]
for slug,(game,dbname) in GAMES.items():
    tp=ROOT/(game.replace('/','-')+'-trees.json')
    if not (ROOT/dbname).exists():continue
    trees=json.loads(tp.read_text());objects=trees['objects']
    records=[];byobj=collections.defaultdict(list)
    for h in json.loads((ROOT/'independent-headers.json').read_text()):
        if h['game']==game:byobj[(h['file'],h['path_id'])].append(('m_Name',h['name'],h.get('cls','Object')))
    for o in objects:
        file=str(Path(o['file'])/o['node']) if Path(o['file']).name=='data.unity3d' else o['file']
        for field,text in o['fields']:
            key=(file,o['path_id']);byobj[key].append((field,text,o['cls']))
            if field in ('m_text','m_Text') or (o['cls']=='TMP_Dropdown' and field.endswith('.m_Text')) or (o['cls']=='TooltipTargetUI' and field=='Text') or (o['cls']=='DialogButton' and field=='_warningWindowLabel') or (o['cls']=='ManagedTextProvider' and field=='defaultValue'):
                cl='managed_fallback_value' if field=='defaultValue' else 'tooltip_or_confirmation' if field in ('Text','_warningWindowLabel') else 'typed_UI_text'
                records.append(dict(file=file,path_id=o['path_id'],path=o['cls']+'.'+field,text=text,cls=cl))
    ta=ROOT/(game.replace('/','-')+'-textassets.json')
    if ta.exists():
        for o in json.loads(ta.read_text()):
            file=str(Path(o['file'])/o['node']) if Path(o['file']).name=='data.unity3d' else o['file']
            key=(file,o['path_id'])
            if o['name'] in ('START','CREDITS','LOAD','GalleryUI','DefaultUI','CharacterNames'):
                for n,line in enumerate(o['script'].splitlines(),1):
                    if ': ' in line:
                        k,v=line.split(': ',1)
                        if v:records.append(dict(file=file,path_id=o['path_id'],path=f"TextAsset[{o['name']}].m_Script.line:{n}.key:{k}",text=v,cls='managed_character_name' if o['name']=='CharacterNames' else 'managed_UI_text'))
            elif o['name']=='_ITEMS':
                tab=list(csv.reader(io.StringIO(o['script'])))
                for n,row in enumerate(tab[1:],1):
                    for col in ['ITEM_CATEGORY','ITEM_NAME']:
                        i=tab[0].index(col)
                        if row[i]:records.append(dict(file=file,path_id=o['path_id'],path=f"TextAsset[_ITEMS].m_Script.row:{n}.col:{i}({col})",text=row[i].strip(),cls='item_display_table'))
            elif o['name'].startswith('PerformanceTestRun'):
                byobj[key].append(('m_Script',o['script'],'PerformanceTestConfig'))
    rs=dbrows(ROOT/dbname);indexed=collections.defaultdict(list)
    for r in rs:
        m=json.loads(r['metadata']);indexed[(r['file_path'],m.get('path_id'),r['source'])].append(r)
    hit=[];missing=[]
    for t in records:
        # Field occurrences, with source equality; identical occurrences in one object
        # consume distinct extracted rows, avoiding duplicate-text recall inflation.
        found=indexed[(t['file'],t['path_id'],t['text'])]
        if found:hit.append((t,found.pop()))
        else:missing.append(t)
    hitids={r['id'] for t,r in hit};positive=len(hitids);negative=[];unknown=[]
    for t in missing:
        if t['cls']=='typed_UI_text':
            if slug=='oot-binary':t['cls']='binary_bypassed_by_scripts'
            elif 'TMP_Dropdown' in t['path']:t['cls']='typed_dropdown_option'
            elif 'lorem ipsum' in t['text'].lower() or t['text'].strip() in ('Author Name','New Text','Speaker','Dialogue'):t['cls']='typed_placeholder'
            elif not any(c.isalpha() for c in t['text']):t['cls']='typed_numeric_or_symbol'
            elif t['text'].strip().isupper():t['cls']='typed_uppercase_label'
            else:t['cls']='typed_other_text'
    for r in rs:
        if r['id'] in hitids:continue
        m=json.loads(r['metadata']);key=(r['file_path'],m.get('path_id'))
        fields=[(f,c) for f,v,c in byobj[key] if v==r['source']]
        if m.get('field_index')==0:
            # Independently verify m_Name instead of trusting Locust metadata.
            fields=fields or []
        neg=[(f,c) for f,c in fields if (f=='m_Name' and c not in ('unknown','Object','MemoryType')) or 'm_MethodName' in f or c=='PerformanceTestConfig' or (c=='ManagedTextProvider' and f in ('category','key')) or (c=='DialogButton' and f in ('TargetObjectBundle','TargetObjectInBundle'))]
        if neg and len(neg)==len(fields):
            f,c=neg[0];cl='object_name' if f=='m_Name' else 'event_method_name' if 'm_MethodName' in f else 'performance_test_JSON' if c=='PerformanceTestConfig' else 'resource_or_localization_key'
            negative.append(dict(file=r['file_path'],path_id=m.get('path_id'),path=c+'.'+f,text=r['source'],id=r['id'],cls=cl))
        else:unknown.append(dict(file=r['file_path'],id=r['id'],path_id=m.get('path_id'),text=r['source']))
    result=dict(game=slug,truth=len(records),hits=len(hit),misses=len(missing),extracted=len(rs),true_positive_rows=positive,false_positive_rows=len(negative),unclassified_rows=len(unknown),recall=len(hit)/len(records) if records else None,precision_classified=positive/(positive+len(negative)) if positive+len(negative) else None,precision_lower_bound=positive/len(rs),precision_upper_bound=(len(rs)-len(negative))/len(rs),parser_errors=len(trees['errors']),miss_classes=dict(collections.Counter(t['cls'] for t in missing)),fp_classes=dict(collections.Counter(t['cls'] for t in negative)))
    results.append(result);misses.extend(dict(game=slug,**t) for t in missing);fps.extend(dict(game=slug,**t) for t in negative);unclassified.extend(dict(game=slug,**t) for t in unknown);truthall.extend(dict(game=slug,**t) for t in records)
dump('binary-results.json',results);dump('binary-misses.json',misses);dump('binary-fps.json',fps);dump('binary-unclassified.json',unclassified);dump('binary-truth.json',truthall)
print(json.dumps(results,indent=2))
