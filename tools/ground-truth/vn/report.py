from inventory import *
REPO='C:/Projects/Locust/'
def load(name):return json.loads((ROOT/name).read_text(encoding='utf-8'))
def cite(file,line):return f'`{REPO}crates/formats/src/{file}:{line}`'
def artifact(name):return f'[{name}]({name})'
def main():
    stats=load('stats.json')+[load('NScripterJP-stats.json')];injs=load('injection-results.json');archives=load('archive-injection-results.json');ex=load('class-examples.json');ns=load('NScripterJP-misses.json')
    for x in ns:
        ex.setdefault(x['defect'],[])
        if len(ex[x['defect']])<3:ex[x['defect']].append(x)
    counts=collections.Counter()
    for st in stats:counts.update(st['miss_classes']);counts.update(st['fp_classes'])
    citations={
        'kirikiri_miss_attribute_name_text':cite('kirikiri.rs',531)+'; '+cite('kirikiri.rs',655),
        'kirikiri_miss_attribute_name_w_n':cite('kirikiri.rs',531)+'; game `D:/juegos/VN/Taimanin Asagi Premium Box/unencrypted/name.ks:783` / `:799` (macro renders %n)',
        'kirikiri_miss_bare_cr_message':cite('kirikiri.rs',653)+'; '+cite('kirikiri.rs',486),
        'kirikiri_miss_punctuation_message':cite('kirikiri.rs',537)+'; '+cite('kirikiri.rs',587)+' (deliberate filler filter; visible literal coverage, not a required prose translation)',
        'kirikiri_fp_script_body':cite('kirikiri.rs',506)+'; '+cite('kirikiri.rs',653)+'; engine `specs/KAGParser.cpp:1134`',
        'yuris_miss_word_control':cite('yuris.rs',473)+'; '+cite('yuris.rs',515)+'; independent `specs/YurisScenarioScript.cs:451`',
        'yuris_miss_display_literal':cite('yuris.rs',506)+'; '+cite('yuris.rs',769)+'; caption `specs/YurisConfigScript.cs:12`',
        'yuris_fp_technical_attribute':cite('yuris.rs',794)+'; '+cite('yuris.rs',504)+'; argument definitions `D:/juegos/VN/Injuu Kangoku RE/res/ysc.ybn` decoded in `yuris-commands.json`',
        'tyrano_miss_visible_attribute':cite('tyrano.rs',367)+'; '+cite('tyrano.rs',433)+'; engine `specs/kag.tag.js:7168`, `specs/kag.tag_ext.js:2056`, `specs/kag.tag.js:6290`',
        'tyrano_fp_script_body':cite('tyrano.rs',380)+'; engine `specs/kag.parser.js:180` / `:484`',
        'tyrano_fp_quoted_bracket_tag':cite('tyrano.rs',345)+'; engine quote parser `specs/kag.parser.js:169` and `:309`',
        'nscripter_miss_display_attribute':cite('nscripter.rs',308)+'; '+cite('nscripter.rs',394)+'; engine `specs/ONScripter_command.cpp:3447`, `specs/ScriptParser_command.cpp:385`',
    }
    out=['# VN ground-truth audit — HEAD b0a17d2fd2fa91c069cd6b536185ddc5a8b193a5','',
    '## 1. Data integrity first','',
    '| Probe | Requested | Result | Evidence |','|---|---:|---|---|']
    for d in archives:
        if d['engine']=='kirikiri':result=f"**172/172 pre-existing patch.xp3 members deleted**; patch.xp3 172 → 1 member. patch2.xp3 54 → 54, unchanged. Locust reports 3 writes; 0/3 requested translations re-extract. Runtime was not launched."
        else:result='Direct aborted (exit 1): untracked `.locust-stage-*/previous`. 572/572 installed members byte-identical; 0 writes installed; 3 requested rows remain original. Private staging retained. Safe installation failure, not a passing round-trip.'
        out.append(f"| {d['label']} | {d['requested']} | {result} | {artifact(d['label']+'-injection.json')}; {artifact(d['label']+'-inject.log')} |")
    for d in injs:
        st=d['stats'];result=f"exit {d['inject_exit']}; admitted rows failing exact re-extract {st.get('admitted_rows_not_exact_on_reextract',0)}/{d['requested']}; untouched row locator/source changes {st.get('untouched_rows_missing_or_changed_on_reextract',0)}"
        if d['label']=='kag-mixed-newlines':result+='; **5,381 bare CR → LF**; LF count 18 → 5,398; extracted rows 18 → 2,022. Mixed delimiter normalization also changes how continuation tags are tokenized.'
        if st.get('safely_rejected_requested_rows'):result+='; **3/3 unsafe control-removal requests safely rejected, game files unchanged**'
        if d['label']=='yuris-prefix':result+='; 0 untouched attribute payload/instruction/line-number/tail mutations; includes a FONT.NAME lookup translated to AUDIT MS Gothic (technical-data false positive)'
        if d['label']=='kag-prefix':result+='; extracted TJS body accepted for translation (see rows #27/#29/#30); no unrelated bytes changed'
        if d['label']=='tyrano-prefix':result+='; includes JS/control-only rows accepted for translation; no unrelated bytes changed'
        out.append(f"| {d['label']} | {d['requested']} | {result} | {artifact(d['label']+'-injection.json')}; {artifact(d['label']+'-inject.log')} |")
    out+=['',f"Responsible XP3 write: `KirikiriPlugin::inject` {cite('kirikiri.rs',1145)} / :1146 overwrites fixed `patch.xp3`; selection ranks `patch2` higher at {cite('kirikiri.rs',767)}. Newline loss: `normalize_newlines` {cite('kirikiri.rs',486)}, called by `encode_ks_bytes` :432 and `apply_translations` :668.",
    '',f"YPF abort crosses plugin/shared infrastructure: `YurisPlugin::inject_under_lock` {cite('yuris.rs',1322)} → `archive_replace::replace_files` {cite('archive_replace.rs',81)}; retained `previous` stage :139 rejected by `C:/Projects/Locust/crates/core/src/injection_transaction.rs:859`. No shared-core fix is assigned in these plugin-only briefs.",
    '', '## 2. Inventory and rejected candidates','', '| Local VN folder | Engine / evidence | Audited scope |','|---|---|---|']
    for g in load('inventory.json'):
        if '/VN/' not in g['root'].replace('\\','/'):continue
        name=Path(g['root']).name;engine=g['engine'];evidence=''
        if 'KiriKiri' in engine:evidence='XP3 magic/index and shipped .ks; `'+g['root']+'/data.xp3`'
        elif engine=='YU-RIS':evidence='`'+g['root']+'/res/ysc.ybn` YSCM and `res/yst00182.ybn` YSTB'
        elif 'Liar' in engine:evidence='`'+g['root']+'/scr.xfl` LB 01 00; independent `specs/ArcXFL.cs:46`'
        elif 'GsPack' in engine:evidence='`'+g['root']+'/System.dat` DataPack5; independent `specs/ArcGsPack.cs:61`'
        else:evidence='`'+g['root']+'/renpy/`'
        scope='out of these four plugins'
        if name.startswith('Motto'):scope='6 loose initialization .ks, only 1 literal; **not whole-game recall**; hashed/extensionless archive names; gameplay payloads unexamined'
        elif name.startswith('My'):scope='32 loose scenario scripts; 101 data.xp3 .ks members not included'
        elif name.startswith('Ochiru'):scope='49 readable .ks from patch2.xp3; independent XP3 transport; archive injection also checks patch.xp3'
        elif name.startswith('Taimanin'):scope='9 loose .ks, with mixed/bare-CR files; archive .ks not included (data:130, patch2:10)'
        elif name.startswith('Injuu'):scope='571 loose .ybn (YSTB oracle + YSCF title); real ysbin.ypf injection separate'
        out.append(f'| {name} | {engine}; {evidence} | {scope} |')
    out+=['', '| Other location / candidate | Disposition and evidence |','|---|---|',
    '| `D:/juegos/parches/locust-tests/` | Rejected as independent truth: Locust-named experiment trees (`mditzy_*`, `taimanin_*`, `ochiru_*`, `_rt*`); inventoried only. |',
    '| `D:/juegos/parches/Erospanish-ParcheLustEpidemic/`, `Traducción Español The Genesis Order/` and sibling RARs | Loose folders empty in inventory; title suggests unrelated game patches, engine unverified; no .ks/.ybn/NS container. RARs not treated as a VN oracle. |',
    '| `D:/juegos/otro/fall.out/`, `The_Nun_v0.1.2_base/` | Engine unverified from this inventory; no target script/container marker. Excluded. |',
    '| `D:/juegos/otro/marniegame 041.pck` and exe/ZIP copies | Godot PCK candidate, outside scope. |',
    '| Existing KiriKiri `patch.xp3`, `patch2.xp3` and `.bak`/`.orig` | Used actual bytes for archive integrity; **not assumed to be translator-authored original/translation pairs**. Previous edits/provenance unknown. |',
    '| Injuu `output/` vs `output.ja.bak/` | 412 paired files, same array lengths, 37,616 nonempty name/message fields, 25,448 changed. `.ja.bak` contains **English**, not Japanese. Provenance clues: `VNTranslationTools/run_extract.bat:2` names Dazed workspace; `gameupdate/patch-config.txt:1` names dazed-translations. Separate lexical cross-check, not asserted human translation or primary opcode oracle. See `translation-pairs.json`. |',
    '| Python parser packages | `py -3.13` import checks: no krkr, xp3, yuris, pylzss, construct, chardet, asar, zstandard; charset_normalizer available but not a format oracle. No installs. |',
    '| VNTextPatch executable | Copied third-party exe/DLLs into `vntp/`; process launch returned application loader 0xc0000142. Not claimed executed. Public parser source + independent readers used instead. |',
    '| Public GBK NS example `corpus/NScripterPublic/0.txt` | Rejected: Chinese GBK demo from wcwac/em-onscripter; strict CP932 decode fails byte 177. No encoding-loss conversion used. See `nscripter-public-source.json`. |',
    '| Tyrano / NS installed games | None identified in listed local trees. **Public third-party samples are reported separately**; see manifests below. |','',
    '## 3. Ground truth, definitions, denominators','',
    '| Oracle | Why independent / trustworthy | Scope and limits |','|---|---|---|',
    '| KAG | Game scripts produced before audit, parsed by scratch `readers.py` from KiriKiri runtime grammar (`specs/KAGParser.cpp:1134` iscript, :1507 text, :1514 escaped bracket). NAME_W macro renders `%n` in actual `Taimanin.../unencrypted/name.ks:799`. | Static literal spans and selected known display attributes; no execution/reachability claim. My `[name text]` role inferred from repeated speaker/dialogue use (`00_000.ks:12`, :37, :45), not from a recovered custom macro implementation. Conditional macros and other display commands are not exhaustively evaluated. |',
    '| YU-RIS | Actual `res/ysc.ybn` opcode/argument table + independent YSTB binary reader. Public `specs/YurisScenarioScript.cs:226` WORD and :236 ES.CHAR.NAME/ES.SEL.SET; :451 EF F0 newline; `YurisConfigScript.cs:12` caption. CGACT SETSTR with TEXT=1 recognized using descriptor parameter ids from `ysc.ybn`. | Physical displayed attribute occurrences, including punctuation literals; assignments and unproven indirect display roles remain unclassified. No Locust tests/database define the oracle. |',
    '| Tyrano | 8 engine-author sample .ks files, URLs and SHA256 in `tyrano-public-sources.json`; own parser `specs/kag.parser.js:180` separates script bodies; glink handler `specs/kag.tag.js:7168`, ruby :6290, chara name `specs/kag.tag_ext.js:2056`. | Public sample, not local commercial-game recall. 5 ruby readings are inside opaque rich rows but not separately addressable as text attributes; strict typed-slot metric below counts them missing. |',
    '| NScripter | Japanese demo `onscripter_jp_test/0.txt` distributed by external maintainer; SHA256/URL in `nscripter-jp-source.json`. Runtime `specs/ScriptHandler.cpp:202` text routing, `ONScripter_command.cpp:3447` caption, `ScriptParser_command.cpp:385` rmenu labels. | One public SJIS demo; no encrypted-container or whole-library inference. Raw script preserved; only relevant bytes extracted from ZIP. |','',
    '- **Recall** = oracle literal slots covered / oracle literal slots. Each slot is (file, physical span/attribute), duplicates count. Message spans may be carried in a rich whole-line row; tag attributes must be separately addressable source values. This is **strict typed-slot recall**, not raw substring discovery. No whitespace-only slots. Visible punctuation is counted and reported separately.',
    '- **Precision bounds** = confirmed rows containing player text / all extracted rows, through (all rows − confirmed technical rows) / all rows. Unclassified rows remain in the denominator; KAG/Tyrano rich messages carrying controls are accepted as text-bearing rows. This is row precision, not token purity.',
    '- CLI databases are measured output only. Oracle scripts never import Locust code/tests or use its database as truth. IDs only map output to independent physical slots; Yuris mapping uses text/order and has 0 unmapped baseline rows.',
    '- KAG logical locations count CR/CRLF/LF; CLI IDs count LF only. Both are retained in evidence; physical script bytes remain the source of truth. Public URLs are backed by cached files and SHA256 (`specs/`, source manifests, especially `final-spec-sources.json`).','',
    '| Scope | Files | Oracle | Hit | Miss | Recall | Rows | Confirmed TP | FP | Unclassified | Precision lower–upper |','|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|']
    for s in stats:
        out.append(f"| {s['slug']} | {s['files']} | {s['oracle']:,} | {s['hits']:,} | {s['miss']:,} | {s['recall']:.2%} | {s['rows']:,} | {s['tp_rows']:,} | {s['fp']} | {s['unknown']:,} | {s['precision_lower']:.2%}–{s['precision_upper']:.2%} |")
    pair=load('translation-pairs.json');out+=['',f"Independent shipped-JSON lexical cross-check: **{pair['found']:,}/{pair['unique_file_field_text']:,} = {pair['recall']:.2%}** unique (file, field, normalized value) found as a substring in matching-file Locust sources. Not occurrence recall and not precision. Full pair paths/values: `translation-pairs.json`.",
    '', '## 4. Miss / false-positive classes — complete real examples','', '| Class | Count | Source / rationale |','|---|---:|---|']
    for cl,n in counts.items():out.append(f'| {cl} | {n:,} | {citations.get(cl,"See source/evidence files")} |')
    for cl,n in counts.items():
        out+=['',f'### {cl} — {n:,}', '']
        for i,x in enumerate(ex.get(cl,[])[:3],1):
            path=x['file'];rel=x.get('relative');local=''
            if path.startswith('http') and rel:local=f"; cached `{ROOT/'corpus/TyranoOfficial'/rel}`"
            out.append(f"{i}. `{path}` — {x['location']}{local}")
            out+=['','```text',x['text'],'```','']
            if x.get('physical') and x['physical']!=x['text']:out+=['Complete physical line:','```text',x['physical'],'```','']
        assert len(ex.get(cl,[]))>=3,('need three full examples',cl)
    out+=['', '## 5. Ranked writer work — disjoint plugin ownership','', '| Rank | Size | Single file | Defect cluster / exact symbols | Baseline / minimum failing regression | Brief |','|---|---|---|---|---|---|',
    f"| 1 | L | **kirikiri.rs** | `KirikiriPlugin::inject` :1145; `normalize_newlines` :485; `extract_lines_from_text` :651; `is_non_text_line` :506 | XP3 deletes 172 untouched members and 0/3 round-trip; mixed CR 5,381 conversions; 20,247 bare-CR literal misses; 11,345 display-attribute misses; 17 TJS FP. Minimal executable fixtures: `regressions/kag-xp3-integrity` loses keep.bin and hides translated story; `regressions/kag-newline-integrity` adds BOM, changes unrelated CRs and drops final separator; `;comment\\rHello.\\rGoodbye.\\r` extracts 0/2. | {artifact('writer-01-kirikiri.md')} |",
    f"| 2 | L | **yuris.rs** | `decode_attr_value` :461; `looks_player_visible` :504; `load_ystb` :764; `serialize_attr_value` :723 | 6,615 control-bearing WORD misses; 178 other visible literal misses incl title; 10 confirmed technical FP. Minimal WORD raw `Hello EF F0 world` extracts 0/1; read `regressions/yuris-word-control/story.ybn`. | {artifact('writer-02-yuris.md')} |",
    f"| 3 | M | **tyrano.rs** | `classify_lines` :380; `is_pure_tag_line` :331; `entries_from_ks_bytes` :433 | 156 JS + 85 pure tag FP; 15 strict attribute misses (8 choices, 2 jname, 5 ruby); precision 147/388. Minimal iscript+JS+glink emits JS but no choice; eval exp with nested bracket emitted as text. | {artifact('writer-03-tyrano.md')} |",
    f"| 4 | M | **nscripter.rs** | `is_player_text_line` :307; `NScripterPlugin::extract` :360; `NScripterPlugin::inject` :408 | 4 caption/rmenu literal misses, 222/226 recall; minimal `rmenu \"Skip\",skip,\"Hide\",windowerase,\"Restart\",reset` yields 0/3 labels. | {artifact('writer-04-nscripter.md')} |",'',
    'No briefs edit unity.rs, unity_serialized.rs, shared core, archive_replace.rs, or archive reader files. Each writer owns one plugin file. Shared YPF staging failure is recorded for coordinator routing; it is not disguised as a successful archive injection.','',
    '## 6. Reproduction / verification','',
    '- Integrity regressions: `regression-results.json`, `kag-xp3-integrity-regression-inject.log`, `kag-newline-integrity-regression-inject.log`. Minimal synthetic XP3 uses public `specs/ArcXP3.cs:97/:111/:150/:216`; 1 untouched member deleted, 0/1 effective translation. Minimal mixed-newline fixture preserves an LF target but has two unrelated CR separators: old inject also adds an UTF8 BOM and drops the final separator. This supplements, and does not replace, real-game integrity evidence above.',
    '- One command: `py -3.13 run_all.py`. Writer CLI: `py -3.13 run_all.py --cli C:/.../locust.exe`. `regressions.py --require-fixed` is a post-fix gate; baseline records eight expected failing checks without stopping the audit.',
    '- Build: `cargo build --locked -p locust-cli` ran only in scratch `snapshot/`, produced `target/debug/locust.exe`; source from `git archive HEAD`. CARGO_TARGET_DIR/TEMP/TMP/LOCUST_DATA_DIR stay under this audit directory. No cache copied or written. No repo build/commit/edit.',
    '- `original-hashes-before.json` / `original-hashes-check.json`: **3,310 sampled original files, 0 SHA256 changes**. Scope: all audited loose .ks/.ybn/.json/.tjs plus selected data/patch/patch2/scn XP3 and ysbin YPF(+bak), excluding huge media archives. This is a sample of game trees, not every game byte.',
    '- `source-hashes.json`: audited eight plugin sources match pinned HEAD after LF normalization; checkout CRLF bytes differ from git-archive LF. `repo-status-after.txt` has only the two pre-existing Unity writer files modified. No other tracked changes.',
    '- Independent hand checks: `hand-checks.md`; full machine evidence: `misses.json`, `false-positives.json`, `*-oracle.json`, `*-injection.json`, extraction/inject logs, `translation-pairs.json`, `regression-results.json`. Installed game runtimes were not launched.',
    '- Archive denominator: XP3 original member loss is independently decoded; YPF failure preserves 572 members. Successful loose Yuris injection compares every untouched attribute payload and instructions/line-number/tail bytes, not just write counts. Failed control requests are distinguished from admitted round-trip failures.',
    '- No real Windows sandbox command-runner helper failure occurred. PowerShell profile warnings on initial reads and VNTextPatch application-loader failure were not treated as sandbox-helper failures.']
    (ROOT/'last-report.md').write_text('\n'.join(out)+'\n',encoding='utf-8')
    dump('all-stats.json',stats);dump('all-class-examples.json',ex)
    print('REPORT',ROOT/'last-report.md',len(out),'sections/lines',flush=True)
if __name__=='__main__':main()
