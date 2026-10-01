# Unity ground-truth audit

Baseline: `b7b894ec7a460aa89ea41bbcccb04de56d9a6d9e`. All repository code line citations refer to that committed snapshot, preserved verbatim under `C:\Users\Mike\AppData\Local\Temp\locust-research-unity\workspace\crates\formats\src`. Other sessions changed HEAD and began editing unity.rs before final delivery; this is a baseline audit, not validation of those unbuilt changes (`provenance.json`). Rebase briefs before dispatching into the current working tree.

## Integrity first

- **3/3 targeted Out of Touch lines lost embedded formatting controls** (write/skip outcome: `oot-inject.log`). Baseline cause: source stripping + prefix/suffix-only reconstruction (C:\Projects\Locust\crates\formats\src\unity.rs:166, C:\Projects\Locust\crates\formats\src\unity.rs:247, C:\Projects\Locust\crates\formats\src\unity.rs:2049).
- **0 untargeted files changed** in the script-copy probe. 112 original script files SHA-256 checked before/after, 0 changes (`oot-original-hashes.json`; game: `D:\juegos\unity\Out of Touch\OoT_Data\SCRIPTS~`).
- **0 unexpected binary changes**: 13,726 CCTV object payloads checked; exactly 3 targeted objects changed, and only the requested slots changed (`binary-injection.json`; original: `D:\juegos\unity\CCTV_WINDOWS_1_3_FULL\CCTV_Data\data.unity3d`). Original bundle SHA-256 before/after: `736e847d91de8f7e14010d62eeed40dc824a5b6c8c1604f038c47bfc776e8f07`.
- Hash coverage is a sample of the entire game trees: every source script plus the complete original CCTV bundle used by the inject probes. Original executables, media and other games were never injected.

Complete inject examples:

- `D:\juegos\unity\Out of Touch\OoT_Data\SCRIPTS~\Vol3\Chapter_31_open.txt:1405` — BEFORE `"      L Why do I...I...\\ilike\\i Vickie? You hate your sisters."`; AFTER `"      L AUDIT Why do I...I...like Vickie? You hate your sisters."`.
- `D:\juegos\unity\Out of Touch\OoT_Data\SCRIPTS~\Vol1\Chapter_1.txt:239` — BEFORE `"      J That...\\ishouting?!"`; AFTER `"      J AUDIT That...shouting?!"`.
- `D:\juegos\unity\Out of Touch\OoT_Data\SCRIPTS~\Vol2\Chapter_26.txt:2290` — BEFORE `"    CJ Wait, uh sorry but you were a \\istudent\\i 35 years ago?"`; AFTER `"    CJ AUDIT Wait, uh sorry but you were a student 35 years ago?"`.

## Oracle and rejected candidates

- Game-authored Out of Touch `Characters.txt` declarations, dialogue and `button` records in 112 runtime script files. Identity = absolute file + physical line; speaker set comes from the game, not Locust. Citation: `D:\juegos\unity\Out of Touch\OoT_Data\SCRIPTS~\Characters.txt:4`, `Vol1\Chapter_0.txt:13`, `Vol1\Vol1_Menus.txt:6` under that same scripts directory.
- Independent parser: UnityPy **1.25.2** + TypeTreeGeneratorAPI **0.0.10**, installed only in scratch `venv/Lib/site-packages` (global Python 3.13 import failed). Custom schemas generated from each game’s shipped `*_Data/Managed/*.dll`; built-in fields/object tables parsed by UnityPy. Citation examples: `D:\juegos\unity\CCTV_WINDOWS_1_3_FULL\CCTV_Data\Managed\Assembly-CSharp.dll`, `Unity.TextMeshPro.dll` in the same folder; BOXMAN `Boxman_Data/Managed/Elringus.Naninovel.Runtime.dll`.
- Binary positives = successfully parsed nonempty `Text.m_Text`, `TextMeshPro[UGUI].m_text`, dropdown option text, tooltip text, confirmation label, ManagedTextProvider defaultValue; BOXMAN named UI ManagedText assets, CharacterNames and `_ITEMS` category/name cells. `binary-truth.json` preserves every object/path/value. These are serialized renderer/runtime localization fields, **not a claim that every prefab/debug/default value was observed on-screen**.
- BOXMAN runtime table coverage: **63/63 ManagedText UI values**, **380/380 item display cells**, **0/6 CharacterNames**. Data: `D:\juegos\unity\es\BOXMAN_v0.5.02_x64\Boxman_Data\resources.assets`, TextAssets `DefaultUI/START/CREDITS/LOAD/GalleryUI/_ITEMS/CharacterNames`; complete IDs/keys/values in `es-BOXMAN_v0.5.02_x64-textassets.json`.
- Type-tree limits: native generator uses 2022.3.32f1 base layout for Unity 6000 games, plus actual game DLL schemas. String fields only are scored; base references from generated schemas are not scored. Full object reads must succeed. Parser failures are **excluded**, not counted as Locust misses: CCTV 157, USSR 4, Sunkissed 292, BOXMAN 146, Out of Touch 0 (`*-trees.json`, errors carry game file and object ID). Naninovel Script SerializeReference objects were among unsupported BOXMAN objects; scenario recall is **unmeasured**.
- `D:\juegos\unity` inventory: CCTV_WINDOWS_1_3_FULL, CCTV_USSR_WINDOWS_FULL, Sunkissed_windows_full, Out of Touch; `es/` copies of all four and BOXMAN_v0.5.02_x64. BOXMAN exists only inside `es/`. Root also has five `*.locust.db` sets and unrelated Outlook/Payoneer files (`inventory.json`, 111,536 files).
- Rejected as oracle: Locust databases (self-produced); Spanish original/translated pairs (unknown provenance, potentially Locust-written); TMP line-breaking character tables and PerformanceTestRun JSON (not dialogue/localization). Existing `es/BOXMAN` is used as **current runtime data**, not as an independently authored translation pair.
- No AutoTranslator/BepInEx/dump path was found in the complete inventory (`inventory.json`; original game roots above). No loose `Localization`/`Languages` text tables were found; the useful tables are embedded TextAssets. UnityEngine.LocalizationModule.dll alone is not localization truth.

## Metrics

Recall = matched oracle field occurrences / oracle field occurrences. Binary matching requires file/node + object ID + exact source text, with duplicate occurrences consuming distinct rows. Scripts match file + line; source controls are evaluated separately by injection. Empty strings excluded; numeric/symbol labels and shipped placeholder text are retained and broken out below.

Precision (classified) = confirmed visible-field rows / (confirmed visible-field rows + confirmed technical rows). Unclassified rows are excluded from that denominator. Whole-output precision interval = [TP/extracted, (extracted−FP)/extracted]. **Classified precision is not a whole-game precision estimate.** Object names are classified only after independent MonoScript class resolution: renderer/font/tile/scenario/configuration asset IDs. Three unresolved BOXMAN asset names and three Sunkissed MemoryType names remain unclassified (`independent-headers.json`; original game file/object IDs included).

| Scope | Truth | Hits | Misses | Recall | Extracted | TP | FP | Unclassified | Classified precision | Whole-output precision interval | Game data |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|
| cctv binary | 1167 | 1000 | 167 | 85.69% | 2659 | 1000 | 131 | 1528 | 88.42% | 37.61–95.07% | `D:\juegos\unity\CCTV_WINDOWS_1_3_FULL\CCTV_Data\data.unity3d` |
| ussr binary | 750 | 620 | 130 | 82.67% | 4363 | 620 | 72 | 3671 | 89.60% | 14.21–98.35% | `D:\juegos\unity\CCTV_USSR_WINDOWS_FULL\CCTV - USSR_Data\data.unity3d` |
| sunkissed binary | 6040 | 5935 | 105 | 98.26% | 8127 | 5935 | 51 | 2141 | 99.15% | 73.03–99.37% | `D:\juegos\unity\Sunkissed_windows_full\Sunkissed_Data` |
| boxman binary | 1308 | 985 | 323 | 75.31% | 3193 | 985 | 538 | 1670 | 64.67% | 30.85–83.15% | `D:\juegos\unity\es\BOXMAN_v0.5.02_x64\Boxman_Data` |
| Out of Touch scripts | 75495 | 71886 | 3609 | 95.22% | 72161 | 71886 | 0 | 275 | 100.00% | 99.62–100.00% | `D:\juegos\unity\Out of Touch\OoT_Data\SCRIPTS~` |
| Out of Touch binary | 5340 | 0 | 5340 | 0.00% | 72161 combined script/binary rows | 0 | 0 | see merged denominator | not separately scored | — | `D:\juegos\unity\Out of Touch\OoT_Data` |

Combined physical-field recall: **80426/90100 = 89.26%**. Unique extracted-row denominator: **90503** (Out of Touch counted once); TP **80426**, FP **792**, unclassified **9285**. Classified precision **99.02%**; whole-output interval **88.87–99.12%**. Source paths: all oracle/game records in `oot-truth.json` and `binary-truth.json`; counts in `oot-result.json`, `binary-results.json`.

## Miss and false-positive classes

| Kind | Class | Count | Responsible source |
|---|---|---:|---|
| miss | binary_bypassed_by_scripts | 5340 | `C:\Projects\Locust\crates\formats\src\unity.rs:2493` |
| miss | dialogue_inline_sprite | 3468 | `C:\Projects\Locust\crates\formats\src\unity.rs:2028` |
| miss | typed_numeric_or_symbol | 306 | `C:\Projects\Locust\crates\formats\src\unity_serialized.rs:1106` |
| miss | typed_uppercase_label | 262 | `C:\Projects\Locust\crates\formats\src\unity_serialized.rs:1424` |
| miss | character_display_name | 117 | `C:\Projects\Locust\crates\formats\src\unity.rs:146` |
| miss | managed_fallback_value | 67 | `C:\Projects\Locust\crates\formats\src\unity_serialized.rs:1409` |
| miss | typed_dropdown_option | 45 | `C:\Projects\Locust\crates\formats\src\unity_serialized.rs:757` |
| miss | typed_other_text | 24 | `C:\Projects\Locust\crates\formats\src\unity_serialized.rs:1409` |
| miss | dialogue_single_character | 22 | `C:\Projects\Locust\crates\formats\src\unity.rs:164` |
| miss | typed_placeholder | 15 | `C:\Projects\Locust\crates\formats\src\unity_serialized.rs:1151` |
| miss | managed_character_name | 6 | `C:\Projects\Locust\crates\formats\src\unity.rs:1559` |
| miss | dialogue_other | 2 | `C:\Projects\Locust\crates\formats\src\unity.rs:167` |
| FP | object_name | 664 | `C:\Projects\Locust\crates\formats\src\unity_serialized.rs:740` |
| FP | resource_or_localization_key | 88 | `C:\Projects\Locust\crates\formats\src\unity_serialized.rs:1409` |
| FP | event_method_name | 34 | `C:\Projects\Locust\crates\formats\src\unity_serialized.rs:1409` |
| FP | performance_test_JSON | 6 | `C:\Projects\Locust\crates\formats\src\unity.rs:803` |

Numeric/symbol fields and designer placeholders are renderer fields, but not all need translation. They are not counted as prose bugs. 275 script rows outside the declared speaker set remain unclassified (some aliases and bilingual continuations); no false-positive claim is made for them. Data: `oot-unclassified.json`, each record includes the source game file and physical line.

## Complete examples (three per class where available)

**miss: binary_bypassed_by_scripts** (5340 total; 3 shown; all shown if fewer than three)

- `D:\juegos\unity\Out of Touch\OoT_Data\level0 :: object 13033 :: TextMeshProUGUI.m_text` — `"Return"`
- `D:\juegos\unity\Out of Touch\OoT_Data\level0 :: object 13034 :: TextMeshProUGUI.m_text` — `"Exit"`
- `D:\juegos\unity\Out of Touch\OoT_Data\level0 :: object 13035 :: TextMeshProUGUI.m_text` — `"Settings"`

**miss: character_display_name** (117 total; 3 shown; all shown if fewer than three)

- `D:\juegos\unity\Out of Touch\OoT_Data\SCRIPTS~\Characters.txt:4` — `"CJ"`
- `D:\juegos\unity\Out of Touch\OoT_Data\SCRIPTS~\Characters.txt:5` — `"Victoria"`
- `D:\juegos\unity\Out of Touch\OoT_Data\SCRIPTS~\Characters.txt:6` — `"Sarah"`

**miss: dialogue_inline_sprite** (3468 total; 3 shown; all shown if fewer than three)

- `D:\juegos\unity\Out of Touch\OoT_Data\SCRIPTS~\Vol1\Chapter_1.txt:407` — `"+CJ_Lhao You really haven't aged a day have you?"`
- `D:\juegos\unity\Out of Touch\OoT_Data\SCRIPTS~\Vol1\Chapter_1.txt:410` — `"+CJ_Ll What are you, 5'6? 7? Never thought you'd be a manlet!"`
- `D:\juegos\unity\Out of Touch\OoT_Data\SCRIPTS~\Vol1\Chapter_1.txt:413` — `"+CJ_Lha +J_Usur W-what?!"`

**miss: dialogue_other** (2 total; 2 shown; all shown if fewer than three)

- `D:\juegos\unity\Out of Touch\OoT_Data\SCRIPTS~\Vol2\Chapter_22.txt:15367` — `"\\bI"`
- `D:\juegos\unity\Out of Touch\OoT_Data\SCRIPTS~\Vol2\Chapter_25.txt:1231` — `"\\bI"`

**miss: dialogue_single_character** (22 total; 3 shown; all shown if fewer than three)

- `D:\juegos\unity\Out of Touch\OoT_Data\SCRIPTS~\SideStories\Katie_1.txt:143` — `"!"`
- `D:\juegos\unity\Out of Touch\OoT_Data\SCRIPTS~\SideStories\Teresa_2.txt:343` — `"?"`
- `D:\juegos\unity\Out of Touch\OoT_Data\SCRIPTS~\Vol1\Chapter_11.txt:435` — `"!"`

**miss: managed_character_name** (6 total; 3 shown; all shown if fewer than three)

- `D:\juegos\unity\es\BOXMAN_v0.5.02_x64\Boxman_Data\resources.assets :: object 824 :: TextAsset[CharacterNames].m_Script.line:1.key:Carter` — `"Carter"`
- `D:\juegos\unity\es\BOXMAN_v0.5.02_x64\Boxman_Data\resources.assets :: object 824 :: TextAsset[CharacterNames].m_Script.line:2.key:Emily` — `"Emily"`
- `D:\juegos\unity\es\BOXMAN_v0.5.02_x64\Boxman_Data\resources.assets :: object 824 :: TextAsset[CharacterNames].m_Script.line:3.key:Jake` — `"Jake"`

**miss: managed_fallback_value** (67 total; 3 shown; all shown if fewer than three)

- `D:\juegos\unity\es\BOXMAN_v0.5.02_x64\Boxman_Data\resources.assets :: object 10413 :: ManagedTextProvider.defaultValue` — `"BGM"`
- `D:\juegos\unity\es\BOXMAN_v0.5.02_x64\Boxman_Data\resources.assets :: object 10430 :: ManagedTextProvider.defaultValue` — `"CONTINUE"`
- `D:\juegos\unity\es\BOXMAN_v0.5.02_x64\Boxman_Data\resources.assets :: object 10481 :: ManagedTextProvider.defaultValue` — `"TIPS"`

**miss: typed_dropdown_option** (45 total; 3 shown; all shown if fewer than three)

- `D:\juegos\unity\CCTV_WINDOWS_1_3_FULL\CCTV_Data\data.unity3d\level0 :: object 2090 :: TMP_Dropdown.m_Options.m_Options.0.m_Text` — `"Low"`
- `D:\juegos\unity\CCTV_WINDOWS_1_3_FULL\CCTV_Data\data.unity3d\level0 :: object 2090 :: TMP_Dropdown.m_Options.m_Options.1.m_Text` — `"Medium"`
- `D:\juegos\unity\CCTV_WINDOWS_1_3_FULL\CCTV_Data\data.unity3d\level0 :: object 2090 :: TMP_Dropdown.m_Options.m_Options.2.m_Text` — `"High"`

**miss: typed_numeric_or_symbol** (306 total; 3 shown; all shown if fewer than three)

- `D:\juegos\unity\CCTV_WINDOWS_1_3_FULL\CCTV_Data\data.unity3d\level0 :: object 1749 :: TextMeshPro.m_text` — `"?"`
- `D:\juegos\unity\CCTV_WINDOWS_1_3_FULL\CCTV_Data\data.unity3d\level0 :: object 1764 :: TextMeshPro.m_text` — `"?"`
- `D:\juegos\unity\CCTV_WINDOWS_1_3_FULL\CCTV_Data\data.unity3d\level0 :: object 1791 :: TextMeshProUGUI.m_text` — `"16:11"`

**miss: typed_other_text** (24 total; 3 shown; all shown if fewer than three)

- `D:\juegos\unity\CCTV_WINDOWS_1_3_FULL\CCTV_Data\data.unity3d\resources.assets :: object 9646 :: Text.m_Text` — `"Vector4\n"`
- `D:\juegos\unity\CCTV_WINDOWS_1_3_FULL\CCTV_Data\data.unity3d\resources.assets :: object 9675 :: Text.m_Text` — `"Vector2\n"`
- `D:\juegos\unity\CCTV_WINDOWS_1_3_FULL\CCTV_Data\data.unity3d\resources.assets :: object 9750 :: Text.m_Text` — `"Vector4\n"`

**miss: typed_placeholder** (15 total; 3 shown; all shown if fewer than three)

- `D:\juegos\unity\es\BOXMAN_v0.5.02_x64\Boxman_Data\resources.assets :: object 10015 :: Text.m_Text` — `"Author Name"`
- `D:\juegos\unity\es\BOXMAN_v0.5.02_x64\Boxman_Data\resources.assets :: object 10016 :: Text.m_Text` — `"Author Name"`
- `D:\juegos\unity\es\BOXMAN_v0.5.02_x64\Boxman_Data\resources.assets :: object 10017 :: Text.m_Text` — `"Author Name"`

**miss: typed_uppercase_label** (262 total; 3 shown; all shown if fewer than three)

- `D:\juegos\unity\CCTV_WINDOWS_1_3_FULL\CCTV_Data\data.unity3d\level0 :: object 1802 :: TextMeshProUGUI.m_text` — `"MENU"`
- `D:\juegos\unity\CCTV_WINDOWS_1_3_FULL\CCTV_Data\data.unity3d\level0 :: object 1810 :: TextMeshProUGUI.m_text` — `"E"`
- `D:\juegos\unity\CCTV_WINDOWS_1_3_FULL\CCTV_Data\data.unity3d\level0 :: object 1818 :: TextMeshProUGUI.m_text` — `"HDR"`

**FP: event_method_name** (34 total; 3 shown; all shown if fewer than three)

- `D:\juegos\unity\CCTV_WINDOWS_1_3_FULL\CCTV_Data\data.unity3d\resources.assets :: object 12660 :: DialogButton.OnClickAlways.m_PersistentCalls.m_Calls.0.m_MethodName` — `"Show"`
- `D:\juegos\unity\CCTV_WINDOWS_1_3_FULL\CCTV_Data\data.unity3d\resources.assets :: object 11339 :: DialogButton.OnClickAlways.m_PersistentCalls.m_Calls.0.m_MethodName` — `"Show"`
- `D:\juegos\unity\CCTV_WINDOWS_1_3_FULL\CCTV_Data\data.unity3d\resources.assets :: object 9923 :: DialogButton.OnClickAlways.m_PersistentCalls.m_Calls.0.m_MethodName` — `"Show"`

**FP: object_name** (664 total; 3 shown; all shown if fewer than three)

- `D:\juegos\unity\CCTV_WINDOWS_1_3_FULL\CCTV_Data\data.unity3d\globalgamemanagers.assets :: object 2323 :: LiftGammaGain.m_Name` — `"LiftGammaGain"`
- `D:\juegos\unity\CCTV_WINDOWS_1_3_FULL\CCTV_Data\data.unity3d\sharedassets0.assets :: object 475 :: LensDistortion.m_Name` — `"LensDistortion"`
- `D:\juegos\unity\CCTV_WINDOWS_1_3_FULL\CCTV_Data\data.unity3d\sharedassets0.assets :: object 476 :: ChromaticAberration.m_Name` — `"ChromaticAberration"`

**FP: performance_test_JSON** (6 total; 3 shown; all shown if fewer than three)

- `D:\juegos\unity\CCTV_WINDOWS_1_3_FULL\CCTV_Data\data.unity3d\resources.assets :: object 324 :: PerformanceTestConfig.m_Script` — `"{\"MeasurementCount\":-1}"`
- `D:\juegos\unity\CCTV_USSR_WINDOWS_FULL\CCTV - USSR_Data\data.unity3d\resources.assets :: object 186 :: PerformanceTestConfig.m_Script` — `"{\"MeasurementCount\":-1}"`
- `D:\juegos\unity\Sunkissed_windows_full\Sunkissed_Data\resources.assets :: object 2449 :: PerformanceTestConfig.m_Script` — `"{\"MeasurementCount\":-1}"`

**FP: resource_or_localization_key** (88 total; 3 shown; all shown if fewer than three)

- `D:\juegos\unity\CCTV_WINDOWS_1_3_FULL\CCTV_Data\data.unity3d\resources.assets :: object 12644 :: DialogButton.TargetObjectInBundle` — `"morning"`
- `D:\juegos\unity\CCTV_WINDOWS_1_3_FULL\CCTV_Data\data.unity3d\resources.assets :: object 13708 :: DialogButton.TargetObjectBundle` — `"Morning"`
- `D:\juegos\unity\CCTV_WINDOWS_1_3_FULL\CCTV_Data\data.unity3d\resources.assets :: object 11356 :: DialogButton.TargetObjectInBundle` — `"morning"`

## Ranked defects (five; all Unity-only)

| Rank | Size | Defect | Measured footprint | Exact symbols/lines | Writer brief |
|---:|---|---|---|---|---|
| 1 | M | Inject loses embedded VN controls | 3/3 real-copy probes | `UnityPlugin::inject_text_scripts` `C:\Projects\Locust\crates\formats\src\unity.rs:247`; `strip_vn_format_codes` :2037; `split_format_codes` :2049 | `writer-01-controls.md` |
| 2 | M | Leading sprite modifiers hide dialogue | 3468 misses | `extract_vn_dialogue` `C:\Projects\Locust\crates\formats\src\unity.rs:2028`; `extract_text_scripts` :162; `inject_text_scripts` :246 | `writer-02-inline-sprites.md` |
| 3 | S | Scripts suppress all binary extraction | 5340 renderer fields; includes prefab defaults | `UnityPlugin::extract` `C:\Projects\Locust\crates\formats\src\unity.rs:2488 / early return :2493 | `writer-03-mixed-discovery.md` |
| 4 | M | Technical Object.m_Name enters translation queue | 664 confirmed FP rows | `SerializedFile::read_mono_strings_object` `C:\Projects\Locust\crates\formats\src\unity_serialized.rs:740`; `mono_name_worth_extracting` :1088 | `writer-04-object-names.md` |
| 5 | L | Untyped MonoBehaviour scanning confuses UI with control fields | 262 uppercase UI misses; 67 fallback misses (3 Lorem); 45 option misses; 34 callback FP; 88 lookup FP | `mono_script_field_worth_extracting` `C:\Projects\Locust\crates\formats\src\unity_serialized.rs:1409 / uppercase gate :1424; `read_mono_strings_object` :757; `is_mono_engine_noise` :1112 | `writer-05-typed-fields.md` |

All five briefs authorize edits only to `crates/formats/src/unity*.rs` (+ tests inside those files), no dependencies/core/RPG Maker edits. **They can run alongside the RPG Maker writer; they overlap each other and must be serialized or assigned to one Unity writer.** Existing injection routing already supports mixed text/binary batches (`C:\Projects\Locust\crates\formats\src\unity.rs:2552`).

Additional measured backlog: 117 script character display names + 6 ManagedText CharacterNames omitted; 24 one-character dialogue bodies omitted. Scope/cause citations are in the class table; these are not extra dispatch briefs.

## Reproducibility and read-only boundary

- One command: `py -3.13 C:\Users\Mike\AppData\Local\Temp\locust-research-unity\run_all.py` (details in README.md). Baseline committed snapshot: `b7b894ec7a460aa89ea41bbcccb04de56d9a6d9e`; all nine audited Unity files match that commit after newline normalization (`provenance.json`). Current HEAD/status are recorded separately because another session worked in this repo during the audit.
- CLI built offline in scratch; `CARGO_TARGET_DIR` and `LOCUST_DATA_DIR` both under this audit directory. No cargo command ran in the repo. Five regressions **fail on the old code** (`regression-results.json`); these validate defect triggers, not the oracle.
- No real Windows command-runner sandbox-helper error occurred. PowerShell profile startup and Python/pip temporary-directory permission errors were process errors, not helper failures. Later shell calls used no profile; dependency installer kept its temporary files under scratch.
- **Write-boundary exception:** initial CLI runs used standard persistent lock files outside the requested scratch root (`C:\Projects\Locust\crates\core\src\patch\lock.rs:30`, `C:\Users\Mike\AppData\Local\locust-patch-locks-v1`). Initial shell/pip bootstrap also attempted default profile/Temp locations and failed. The final runner redirects locks using a documented scratch-only core shim (`isolate_cli.py`); the audited Unity source is unchanged. The initial runs did not fully meet the write-only-root constraint.
- No repo edits/commits or game edits were made by this researcher. `provenance.json` contains initial/final observed git status, Unity source hashes, and original-data hash checks. Concurrent session changes are external to this audit.
