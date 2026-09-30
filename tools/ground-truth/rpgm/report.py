from audit import *
R=load(ROOT/'extraction-results.json');D=load(ROOT/'extraction-details.json');I=load(ROOT/'inventory.json')
R.sort(key=lambda r:r['slug'])
assert len({r['slug'] for r in R})==len(R)
def pct(a,b):return f'{a/b*100:.2f}%' if b else '—'
def ratio(a,b):return f'{a:,}/{b:,} ({pct(a,b)})' if b else '—'
tot={k:sum(r.get(k,0) for r in R) for k in ('extracted','visible_hit','visible_total','broad_hit','broad_total','diff_hit','diff_total','visible_diff_hit','visible_diff_total','fp_total')}
groups=collections.defaultdict(list)
for d in D:
    if d['kind']=='structural_miss' or d['kind']=='pair_miss' or d['kind']=='false_positive':groups[(d['kind'],d['classification'])].append(d)
examples=[]
for (kind,cl),items in sorted(groups.items()):
    # Prefer different files/texts instead of three identical repetitions.
    chosen=[];seen=set()
    preferred=[x for x in items if x.get('visible')] if kind=='structural_miss' else items
    for x in preferred or items:
        key=(x['file'],x['text'])
        if key not in seen:chosen.append(x);seen.add(key)
        if len(chosen)==3:break
    examples.append(dict(kind=kind,classification=cl,count=len(items),visible_count=sum(bool(x.get('visible')) for x in items),examples=chosen))
dump('class-examples.json',examples);dump('totals.json',tot)
injs=[load(p) for p in sorted(ROOT.glob('g*-injection-result.json'))]
dump('injection-results.json',injs)
out=[]
def add(x=''):out.append(x)
add('# Cycle 100 — RPG Maker ground-truth audit (main, research only)')
add()
add(f"Inventory: {len(I['games'])} deployed roots, {dict(collections.Counter(g['engine'] for g in I['games']))}; {sum(g['protected'] for g in I['games'])} protected JSON wrappers, {sum(bool(g['jsono_files']) for g in I['games'])} JSONo deployment; no System.rvdata2 or RGSS3A deployment found. Full paths, titles and counts: inventory.json (inventory.py).")
add('RPG Maker also lives under D:\\juegos\\renpy: CamXFamily -Eng + renpy\\1es\\CamXFamily -Eng-es (audited); This goddess corrupted our horny world 0.3 (additional deployment, main rpgm copy audited). LoQOO / Legend of Queen Opala use XP .rxdata rather than VX Ace and are outside this requested MV/MZ/VX Ace census. Empty juego ut holds a ZIP; juegout\\Project1 - Copy is MZ (g91).')
add(f"Audited {len(R)} source deployments, {sum(bool(r.get('pair')) for r in R)} original/translated pairs, {tot['extracted']:,} extracted DB rows. Plain JSON parsed independently with Python json; compressed JSONo parsed by independent lzbase64.py. No repo edits. Build: cargo build -p locust-cli succeeded.")
add()
add('Measurement definitions: recall counts physical nonempty string-field occurrences keyed by file + JSON path + original value; a merged 401/405 row covers each original line ONLY when DB.source exactly equals the independent assembled original. Corrupted mojibake sources are misses, even if the row ID exists. MZ encoded choices arrays are parsed independently, including twice-encoded objects; .@json marks decoding a JSON string before traversing its labels (a logical locator, not a literal outer JSON property). Strict structural denominator includes known display slots with literal language; control-only/lookup-only message/speaker slots are excluded. Broad denominator includes notes, editor MapInfos names, comments, scripts, plugin arguments and plugins.js parameter strings as candidate locations, so it is coverage of candidates, not visible-text recall. All plugins.js parameters are candidates, not automatically confirmed translatable UI. Raw pair diffs include technical strings too; visible pair subset is separately reported. Pairs have unknown translator provenance and often keep speakers unchanged: pair recall is an incomplete reference, not a substitute for the independent census. Raw string-field diff treats the whole encoded choices parameter as its own field; translating only its embedded labels does not count as extracting that entire JSON string.')
add('Pair alignment refuses unequal array lengths and changed event-command codes rather than attributing shifted strings to the wrong field. Excluded boundaries: '+str({k:sum(r['diff_alignment'].get(k,0) for r in R) for k in ('array_length_mismatch','command_mismatch','key_difference')})+'. Holding Hands EN 1.0.11 versus ES 1.0.9 is a version-mismatched pair; its diff is lower-confidence. All data files are examined; baseline nonstandard/empty JSON parse errors are preserved in extraction-results.json and excluded from the census.')
add('Precision-ish = (rows − confirmed nontext/composite-command rows)/rows. Composite D_TEXT rows contain visible prose plus command names/numeric font sizes: subtracting them measures unsafe provider input, not pure irrelevant prose. This uses confirmed contamination only, so it is an upper bound on true precision; unclassified rows are not proven visible text. Class counts and 3 full, real path/JSON-path/text examples (or all examples when fewer than 3 exist): class-examples.json, generated by report.py.')
add()
add('| Total metric | Measured |')
add('|---|---:|')
for name,hit,total in [('Pair raw diff recall','diff_hit','diff_total'),('Pair visible subset recall','visible_diff_hit','visible_diff_total'),('Independent visible structural recall','visible_hit','visible_total'),('Independent broad candidate coverage','broad_hit','broad_total')]:add(f'| {name} | {ratio(tot[hit],tot[total])} |')
add(f"| Precision-ish | {ratio(tot['extracted']-tot['fp_total'],tot['extracted'])} |")
add()
add('Per deployment: ★ = original/translated diff truth + structural truth; otherwise structural only. IDs resolve to complete paths below and inventory-rpgm.json. P = precision-ish. All values are extraction-results.json measurements from audit.py.')
add()
add('| ID / source deployment | Rows | Raw pair recall | Visible structural recall | Broad coverage | P |')
add('|---|---:|---:|---:|---:|---:|')
for r in R:
    name=Path(r['root']).name
    add(f"| {r['slug']} {name}{' ★' if r.get('pair') else ''} | {r['extracted']:,} | {ratio(r['diff_hit'],r['diff_total'])} | {ratio(r['visible_hit'],r['visible_total'])} | {ratio(r['broad_hit'],r['broad_total'])} | {pct(r['extracted']-r['fp_total'],r['extracted'])} |")
add()
add('| Confirmed loss / provider contamination class | Count | Consequence |')
add('|---|---:|---|')
for cl,desc in [('speaker_101','MZ displayed speaker names have context only; no translation rows'),('plugin_command_357_display','Visible English plugin message/quest text rejected by CJK gate'),('plugin_command_357_nested_choice_label','Embedded choice label rejected by CJK gate'),('plugin_command_356_display','Visible Latin D_TEXT text rejected by CJK gate'),('database_nickname','Actor nicknames omitted'),('system_currencyUnit','Currency unit omitted'),('actor_event_324','Change-nickname command omitted')]:
    add(f"| {cl} | {sum(x['visible'] for x in groups[('structural_miss',cl)]):,} | {desc} |")
for cl,desc in [('troop_editor_name','Editor-only troop labels sent as names'),('control_or_lookup_only','Only escape codes/lookup keys; no literal prose'),('MV_command_prefix_sent_as_text','Command verb + numeric arguments sent to provider')]:
    add(f"| {cl} | {len(groups[('false_positive',cl)]):,} | {desc} |")
source_bad=load(ROOT/'source-value-mismatches.json')
add(f"| Extracted source-value corruption | {len(source_bad):,} rows | Exact ID exists, but original text != DB.source; visible field occurrences are deducted from recall |")
add('Source corruption examples (verify_sources.py; source-value-mismatches.json):')
for d in source_bad[:3]:add('- `'+d['file']+'` `'+d['path']+'`: '+repr(d['original'])+' → DB '+repr(d['extracted'])+'.')
add()
add('Injection used full scratch copies and direct mode (same RpgMakerMvPlugin::inject replace writer), with translation = "AUDIT " + exact source saved into scratch SQLite. Initial [T] prefix was correctly rejected as an extra protected bracket control before writing; it is not an RPG Maker defect. g09 first attempt hit the harness 180 s timeout; inject-status then reported pending:null. Retried with a 1,800 s harness timeout; final measurement below. No original D:\\juegos path was passed to inject.')
add()
add('| Copy | Before → re-extracted rows | JSON valid | Unexpected structure/nontext changes | Control changes | Lost / extra roundtrip rows | Byte-identical original files | Rewrapped groups |')
add('|---|---:|---:|---:|---:|---:|---:|---:|')
for r in injs:
    s=r['stats'];add(f"| {r['game']} | {r['rows_before']:,} → {r['rows_after']:,} | {r['json_files_checked']-len(r['json_parse_errors'])}/{r['json_files_checked']} | {s.get('unexpected_structural_changes_vs_targets',0)} / {s.get('nonstring_changes',0)} | {s.get('control_mutation',0)} | {s.get('roundtrip_missing',0)} / {s.get('roundtrip_extra',0)} | {r['byte_identical_files']:,} | {s.get('rewrapped_message_groups',0):,} |")
add('Structural comparison groups consecutive 401/405 commands and normalizes only wrapping whitespace; all other keys, array sizes, numeric values, command codes, parameters and non-target strings must be unchanged. Raw message-line arrays intentionally grow/shrink (counts in per-game results); there is no claim of raw command-array length equality. Literal byte-exact text roundtrip differs due wrapping; normalized text multisets plus target-by-target grouped structural diff check 1:1 identity. New .locust-injections recovery-store files are expected and inventoried; no original file was deleted. Existing unmodified files were SHA-256 byte-identical. Injection baseline/final documents are read from copies; all *JSON files* including transaction JSON were parsed.')
additional=load(ROOT/'additional-injection-checks.json')
add('Additional raw-control/source checks (verify_additional.py): '+str({k:sum(r['stats'].get(k,0) for r in additional) for k in ('raw_control_mutation','blank_line_collapsed','scroll_groups','scroll_group_line_count_changed','original_files_checked','original_byte_changes')})+'. Full raw before/after strings with physical JSON paths are in additional-injection-checks.json. Blank-line collapse is a layout loss hidden by normalized text roundtrip; g09 examples CommonEvents.json $[4].list[15] loses the blank before -Hizor, Map042.json $.events[182].pages[1].list[1] loses the separator before "You\'ll never be able to return to this realm", and Map043.json $.events[317].pages[2].list[18] loses its blank line. This is the unconditional split_whitespace flatten at rpgmaker_mv.rs:1226, also :1431 and :1438. There is 1 scrolling-text group in g01 whose line count changes; preserving paragraphs separately from display-width wrapping needs a regression.')
encoding=[x for x in load(ROOT/'g09-injection-details.json') if x['kind']=='unexpected_text_mutation']
add('Additional confirmed integrity defect: 6 untouched code657 annotation fields in g09 become mojibake (… → â€¦), despite valid original UTF-8 and no translations targeting them. read_data_json at rpgmaker_mv.rs:1815-1818 uses EncodingDetector::read_file_auto; encoding.rs:18-26 trusts a confident non-UTF8 detector before validating UTF-8; write_data_file (:1796-1811) serializes that corrupted full document as UTF-8. This fails the ONLY-intended-fields condition; roundtrip rows alone miss it. Three physical source examples (all six in g09-injection-details.json):')
for d in encoding[:3]:add('- `'+d['before_file']+'` `'+', '.join(d['before_paths'])+'`: '+repr(d['before'])+' → '+repr(d['after'])+'.')
add('Confirmed corruption: g01 loses 109 D_TEXT rows; command first token D_TEXT becomes AUDIT. Eg www/data/Map021.json $.events[2].pages[1].list[407].parameters[0]: D_TEXT 自由部屋でした妄想 32 → AUDIT D_TEXT 自由部屋でした妄想 32. Same trigger in Map023.json $.events[1].pages[1].list[320].parameters[0] and Map021.json $.events[2].pages[1].list[358].parameters[0] (full strings in g01-injection-details.json). Plugin dispatch explicitly switches on D_TEXT in copies/g01/www/js/plugins/DTextPicture.js:343-350, and MV command356 splits the first token as command (copies/g01/www/js/rpg_objects.js:10505-10508). JSON validity and escape preservation do not prevent this semantic corruption.')
add()
add('Winner: displayed MZ speaker names omitted from extraction. Exact trigger: nonempty literal name in code 101 parameters[4] on a map, common event or troop page. rpgmaker_mv.rs:429-434 only updates speaker context; :468 assigns it to dialogue context without a StringEntry. Responsible symbols: extract_event_commands, apply_map_translation (:904), apply_common_event_translation (:1046), apply_troops_translation (:1113). Proposed change: create a speaker slot ID and actor_name/speaker row, teach all three event writers to write ONLY parameters[4]; keep parameters[0] face assets, numeric header arguments and dialogue context intact. Skip purely dynamic control-only name slots. Size M, approximately 80–150 production/test lines in rpgmaker_mv.rs; no rpgmaker_lang.rs change required. This has by far the largest confirmed visible-text occurrence count; it is not a re-proposal of Done cycle60 (registration atomicity) or cycle32 (%N protection).')
add('Minimal executable failing regression: regression_speaker.py writes System.json + Map001.json in scratch with [{code:101,parameters:["",0,0,2,"Alice"]},{code:401,parameters:["Hello!"]},{code:0,parameters:[]}], extracts with real locust.exe, and asserts any source == "Alice". Measured current main: sources ["Hello!", "Test"], contexts ["Alice", null], AssertionError; exit 1. Post-fix acceptance extends this fixture with translation Alice→Alicia, injects a scratch copy, and asserts header parameters == ["",0,0,2,"Alicia"] with unchanged dialogue/face/numbers. The failing run is saved in regression-speaker-test.log plus the output DB. Repo remains untouched; regression script intentionally fails. The corpus has 151,299 literal speaker occurrences / 1,056 distinct name strings; dynamic control-only name slots are excluded. Runtime confirmation: copies/g09/js/rmmz_objects.js:9793-9800 passes parameters[4] to setSpeakerName.')
add('Runner-up 1: MV D_TEXT composite command injection damages dispatcher (127 provider-contaminated corpus rows; 109 confirmed broken in injected g01). rpgmaker_mv.rs:53-68 returns the entire command; :504-510 creates the provider row; :978-981 writes translation into parameters[0], also :1107-1108/:1183-1184 for common events/troops. Parse exact command token and supported argument grammar; translate only the body; preserve prefix and final font-size argument. Size M. Add a fixture D_TEXT テスト 32 → D_TEXT Prueba 32, assert first token and size survive, and 1 row re-extracts.')
add('Runner-up 2: non-CJK visible plugin texts omitted (167 regular MZ fields + 170 nested MZ choice labels + 7 MV display commands = 344 confirmed literal misses; an additional 106 nested candidate labels contain only controls/lookups). rpgmaker_mv.rs:113-122 (regular MZ fields), :103-107 (nested choice labels), :60-66 (MV) gate on CJK. Known descriptions include quests, e.g Branded to Fall Map044.json $.events[73].pages[1].list[271].parameters[3].description = "Meet Jade at the market at night." (plus 3 complete examples in class-examples.json). Size S/M; remove language-dependent gates only for structurally confirmed text slots, keep IDs/assets/eval arguments out. Test a code357 args.description = "Meet Jade" and assert it extracts and writes back while command/plugin ids remain unchanged.')
add('VX Ace: no live deployment found, so no measured VX Ace recall or Marshal integrity claim. rpgmaker_vxa.rs:208-211 replaces object references with Unsupported; :403 writer serializes Unsupported as nil — source-level risk requiring a real Ace fixture before ranking. rpgmaker_lang.rs:33-104 read: language registration is a separate operation, not invoked by extract/direct inject; its completed cycle60 defects were not re-proposed.')
add()
add('Complete source/pair paths (all rooted in D:\\juegos):')
for r in R:add(f"- {r['slug']}: `{r['root']}`"+(f" → `{r['pair']}`" if r.get('pair') else ' (structural only)'))
add()
add('Re-run in PowerShell from C:\\Projects\\Locust:')
add('```powershell')
add(f"$s = '{ROOT}'")
add('cargo build -p locust-cli')
add(r'python "$s\inventory.py"')
add(r'python "$s\inventory.py" "D:\juegos\rpgm"')
add(r'python "$s\audit.py"  # --reuse skips repeated CLI extract; recounts all JSON')
add(r'python "$s\verify_sources.py"')
add(r'python "$s\verify_additional.py"')
add(r'python "$s\enrich_injection_details.py"')
add(r'python "$s\report.py"')
add(r'python "$s\regression_speaker.py"  # EXPECTED FAIL on current main')
add('```')
add('Injection rerun: injection.py defaults to 1 9 92 and refuses a previously modified copy. To rerun fully without deleting evidence, copy audit.py, injection.py, lzbase64.py, inventory-rpgm.json into a NEW subdirectory under this same ground_rpgm scratch root, then run python <new-subdir>\\injection.py 1 9 92. Its ROOT is its script directory; ENV always sets LOCUST_DATA_DIR=<new-subdir>\\cli-data. This creates fresh full copies and DBs. Existing per-game measurement verification is rerunnable with report.py. All scratch outputs remain under the user-designated ground_rpgm root.')
(ROOT/'report.md').write_text('\n'.join(out)+'\n',encoding='utf-8')
print('TOTALS',json.dumps(tot));print('VISIBLE RECALL',pct(tot['visible_hit'],tot['visible_total']),'PAIR',pct(tot['diff_hit'],tot['diff_total']),'PRECISION',pct(tot['extracted']-tot['fp_total'],tot['extracted']));print('INJECTION GAMES',[r['game'] for r in injs]);print('REPORT',ROOT/'report.md')
