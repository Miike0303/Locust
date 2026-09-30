import json,collections,re
from pathlib import Path
G=Path(__file__).parent
S=json.loads((G/'summary.json').read_text());M=[json.loads(l) for l in (G/'classified-misses.jsonl').read_text('utf8').splitlines()];E=[json.loads(l) for l in (G/'classified-extras.jsonl').read_text('utf8').splitlines()];A=[json.loads(l) for l in (G/'ast-misses.jsonl').read_text('utf8').splitlines()];MENU=json.loads((G/'menu-summary.json').read_text())
def examples(xs,key):
 out=[];seen=set()
 preferred=sorted(xs,key=lambda x:(not 12<=len(x.get('statement',x.get('source','')))<=130,len(x.get('statement',x.get('source','')))))
 for b in preferred:
  game=b['game']
  if game not in seen:out.append(b);seen.add(game)
  if len(out)==3:return out
 for b in preferred:
  if b not in out:out.append(b)
  if len(out)==3:break
 return out
lines=[];index={}
def emit(s=''):lines.append(s)
def section(title,rows,key):
 index[title]=len(lines)+1;emit(f'{title} — {len(rows):,}')
 ex=examples(rows,key)
 for b in ex:
  if 'statement' in b:
   ref=b['ref'];origin=b.get('origin') or str(Path(b['tl']).parents[2])+'/[compiled source]/'+ref[0]
   emit(f"  {b['game']}: {origin}:{ref[1]} | {b['statement']}")
   emit(f"  Ground truth: {b['tl']}:{b.get('comment_line',b.get('tl_line'))} | block {b.get('id','strings/old')}")
  else:
   ref=b.get('ref');contexts=b.get('ast_contexts',[])
   if contexts and contexts[0].get('location'):
    f,n=contexts[0]['location'];where=str(Path(r'D:\juegos\renpy')/b['game'])+'/#'+str(f)+':'+str(n)
   elif ref:where=b['file_path']+':'+str(ref[1])
   else:where=b['file_path']+'#'+b['id']
   emit(f"  {b['game']}: {where} | {json.dumps(b['source'],ensure_ascii=False)}")
   emit(f"  DB row: {b['id']} | tags {b['tags']} | AST {json.dumps(contexts[:1],ensure_ascii=False)} | current {b.get('current','')}")
 if len(ex)<3:emit(f'  Only {len(ex)} cases exist; no additional examples invented.')
 emit()
for cls in collections.Counter(x['class'] for x in M):section('MISSED: '+cls,[x for x in M if x['class']==cls],'class')
for cls in collections.Counter(x['verified_class'] for x in E):section('UNMATCHED EXTRACT: '+cls,[x for x in E if x['verified_class']==cls],'verified_class')
for cls in ['compiled Say.what omitted by heuristic','compiled TL mismatch/unresolved']:
 section('AST VERIFICATION: '+cls,[x for x in A if x['verified_class']==cls],'verified_class')
mm=[]
for r in MENU:
 for b in r['misses']:b['game']=r['game'];b['class']='conditional menu choice' if ' if ' in b['statement'] else 'menu choice with argument clause';mm.append(b)
for cls in sorted({b['class'] for b in mm}):section('SUPPLEMENTAL: '+cls,[x for x in mm if x['class']==cls],'class')
(G/'evidence.txt').write_text('\n'.join(lines),encoding='utf8');(G/'evidence-index.json').write_text(json.dumps(index,indent=2))
T={k:sum(r[k] for r in S) for k in ['unique_tl_statements','matched','dialogue_rows','not_in_tl','raw_statements_all_languages','tl_headers_all_languages','exact_ref_and_text','live_at_ref']}
T['recall_pct']=T['matched']/T['unique_tl_statements']*100;T['precision_ish_pct']=(T['dialogue_rows']-T['not_in_tl'])/T['dialogue_rows']*100
T['live_ref_recall_pct']=T['exact_ref_and_text']/T['live_at_ref']*100
T['miss_classes']=dict(collections.Counter(x['class'] for x in M));T['extra_classes']=dict(collections.Counter(x['verified_class'] for x in E));T['supplemental_menu_total']=sum(r['live_tl_menu_choices'] for r in MENU);T['supplemental_menu_misses']=len(mm)
assert len(M)==T['unique_tl_statements']-T['matched'];assert len(E)==T['not_in_tl']
(G/'totals.json').write_text(json.dumps(T,indent=2),encoding='utf8')
report=['Cycle 96: read-only RenPy extractor audit',f"16 games; {T['raw_statements_all_languages']:,} raw tl statements, {T['unique_tl_statements']:,} unique (file, line, who, canonical text) after de-duplicating languages.",f"Recall {T['recall_pct']:.2f}% ({T['matched']:,}/{T['unique_tl_statements']:,}); precision-ish {T['precision_ish_pct']:.2f}% ({T['dialogue_rows']-T['not_in_tl']:,}/{T['dialogue_rows']:,}); live exact-reference subset {T['live_ref_recall_pct']:.2f}% ({T['exact_ref_and_text']:,}/{T['live_at_ref']:,}).",'Match: exact normalized (relative file, physical line) plus canonical text when current source agrees with the tl literal; when references drift or source is compiled, fallback to canonical text with speaker/context when available. This is text coverage for fallback matches, not proof every repeated occurrence survives.',"Precision-ish accepts either a tl dialogue literal or an old string from any language. Unmatched live dialogue is not automatically a false positive. Confirmed false positives are compiled non-text AST fields or dialogue-tagged literals inside Python blocks.",'','game | unique tl | matched | extracted dialogue rows | not in tl | recall % | precision-ish %']
for r in S:report.append(f"{r['name']} | {r['unique_tl_statements']} | {r['matched']} | {r['dialogue_rows']} | {r['not_in_tl']} | {r['recall']:.2f} | {r['precision_ish']:.2f}")
report+=['','All 16 auto-detected RenPy and exited 0; no selected game was skipped for detection. 1es copies and the other eligible inventory games were not sampled. Loose .rpy and archived .rpy/.rpyc were included. Standalone .rpyc is not traversed by directory extract (renpy.rs:2258,2296).', '','MISS CLASSES: '+json.dumps(T['miss_classes']), 'UNMATCHED CLASSES: '+json.dumps(T['extra_classes']),f"Supplemental menu strings: {T['supplemental_menu_total']:,} current choices located from old-string text in the referenced source file; {T['supplemental_menu_misses']:,} absent from all extracted sources (917 conditional; 7 argument-clause choices). The strings reference often points at menu: rather than the choice; this is reported separately from dialogue-block recall.",'','WINNER: menu state suppresses character say statements inside branches. 165,910 current tl statements lost; for example menu prompt/branch lines remain in scripts but produce no database row. menu recognized at renpy.rs:657; exit guesses literal 8-space/2-tab indentation at :679; ordinary say only runs if !in_menu at :775. Narration is accepted as menu at :1267-1273 and omits label/context via :667.','Minimal executable regression: regression.py / regression/game/script.rpy. Actual CLI exits 0; assertion fails for A visible branch line. and Another visible branch line. Outside say and narration are present; branch narration is mistagged menu without label.','Suggested change only: crates/formats/src/renpy.rs, RenPyPlugin::extract_content and extract_menu_choice; replace boolean/fixed indentation with actual menu-header/choice/branch scopes, extract say inside branches, keep prompts/narration tagged dialogue with label. Size M, localized parser/state plus nested/tabbed/conditional menu regression tests. No implementation performed.','Runner-up 1: typed compiled AST extraction, not every Unicode pickle string: 35,055 confirmed non-text extracted rows; 8,740 confirmed Say.what misses (5,214 slash/backslash incl text tags; 2,308 single-word period/underscore; 1,218 no alphabetic/too short). renpy.rs:1605-1609,1715-1738; harvest_rpyc_strings, scan_pickle_strings, is_renpy_dialogue_like. M/L. Compiled injection uses runtime text filter, not direct code substitution (renpy.rs:1550-1552).','Runner-up 2: loose-dialogue file/path heuristic drops 1,573 markup/text lines and 21 escaped no-space lines. renpy.rs:913,942,1219-1256; extract_say_statement, is_file_reference. S. Examples include {i}tap-tap-tap!{/i} and {size=+50}THIS!!!{/size}.','Lower priority: 265 dialogue-tagged Python literals from unrecognized python headers (e.g. init 1000 python hide:, recognized headers renpy.rs:632-635); 917 conditional choice misses at renpy.rs:1271-1273.','All classifications and three complete path:line examples per class are in evidence.txt (two examples for single-quoted say because only two were found). Raw data are summary.json, misses.jsonl, classified-misses.jsonl, extras.jsonl, classified-extras.jsonl, ast-{summary,misses,extras}, menu-summary.json, *.locust.db and *.extract.log.','Ground-truth parser exclusions: four nvl clear blocks without text, four translate <lang> python sections, one Portuguese first block with no source ref (see unparsed.json). Only quote-bearing original comments from active translate blocks count; nested commented-out blocks and Locust-generated files are excluded. Canonicalization follows renpy.rs:1098-1139.','Rerun: python "'+str(G/'run_all.py')+'" (regression.py intentionally exits 1).','Build used: cargo build -p locust-cli (successful); git status --short checked empty; no repo source changes.']
(G/'report.txt').write_text('\n'.join(report)+'\n',encoding='utf8')
print(json.dumps(T,indent=2));print('EVIDENCE INDEX',json.dumps(index))
