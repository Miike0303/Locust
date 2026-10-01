# VN 02 — opcode-aware YSTB display text and controls (size L, defect, writer)

Repo C:\Projects\Locust, main. Do not commit. Other writers own Unity and kirikiri.rs, tyrano.rs, nscripter.rs. Do not touch apps/desktop/src/**, tools/ground-truth/**, shared core or archive_replace.rs.
Baseline commit b0a17d2fd2fa91c069cd6b536185ddc5a8b193a5. Inspect concurrent changes before dispatch. Edit ONLY the single plugin below (+ inline tests), no dependencies. NEVER modify D:\juegos. Build a committed scratch snapshot with only your edit overlaid, CARGO_TARGET_DIR and LOCUST_DATA_DIR under scratch; never cargo in live repo. Stop only on an own real sandbox command-runner helper failure, not text in logs.

Files to edit: **C:\Projects\Locust\crates\formats\src\yuris.rs** only.

## Evidence

Audit C:\Users\Mike\AppData\Local\Temp\locust-research-vn: last-report.md, Injuu-oracle.json, misses.json, false-positives.json, yuris-prefix-injection.json, yuris-commands.json.
`decode_attr_value` :461/:473 decodes raw WORD bytes as SJIS without mapping EF F0/F2/F3/F5; `looks_player_visible` :504/:515 rejects resulting replacement characters. **6,615/6,615** control-bearing visible WORD attributes missed. Real example D:\juegos\VN\Injuu Kangoku RE\res\yst00182.ybn instruction 30 attr 100; text/newline and full bytes in class-examples.json.
`looks_player_visible` :506 and token filters skip **177** other definite display literals (short names/visible punctuation etc). `load_ystb` :769 skips YSCF: project caption `yscfg.ybn` at 0x4C/0x4E, also one miss.
`load_ystb` :794 iterates every attribute without command roles: **10** confirmed technical rows, including FONT.NAME MS Gothic, FILEACT IExplore, GOSUB target and Japanese asset paths. Do not classify CGACT SETSTR as an asset when TEXT=1: two actual UI values are already included in the oracle.
Baseline 20,271/27,064 oracle slots = 74.90% recall; 20,271 confirmed TP / 22,506 output rows, 10 FP, 2,225 unclassified → precision 90.07–99.96%. Existing translated JSON independently gives lexical coverage 21,302/28,126 =75.74%; definitions differ.

## Change

Interpret instruction/attribute associations using the shipped YSCM command/parameter definitions or validated version-specific command mappings. Admit WORD and verified display/choice/name/title fields regardless of ASCII/CJK heuristics; preserve unknown indirect-role cases conservatively. Exclude definite dispatch/resource/font lookup attributes. Decode engine control pairs to protected reversible tokens/newlines and serialize them back to the correct raw bytes, preserving their order and type. Do not write CRLF bytes where EF F0 is required. Add supported YSCF caption read/write without rewriting unrelated header bytes.
Maintain locator compatibility or safe source validation: filtered dense #arg indexes change when recall improves or technical rows disappear. Verify physical/current source matches the intended row; never let old rows translate a different attribute after indexing shifts. Preserve every untouched instruction, descriptor identity/type, payload, line-number section and tail. Unknown source layouts must be safe skips.

## Tests (must fail on old code)

* Minimal real-layout YSTB: version0x22B, one WORD108, raw0 attr containing `Hello EF F0 world`. Old extracts zero. New must extract one reversible display value; prefix/inject/re-extract and retain EF F0. Include EF F2/F3/F5, controls adjacent to SJIS multibyte text, multiple protected tokens, encrypted section keys.
* WORD `...`, one/two-character names and ES.CHAR.NAME/ES.SEL.SET strings survive. FONT.NAME MS Gothic and GOSUB target remain technical. Include CGACT TEXT=1 SETSTR as a positive control.
* YSCF header+caption at 0x4C: extract then change caption only, preserve header. Old emits none.
* Removed/added visible attribute before an existing locator: old project rows cannot overwrite a new attribute. Require explicit skip/error or validated stable addressing; no corrupt successful writes.
* One changed attr leaves all untouched raw payloads, descriptor identities/types, instructions/line numbers/tail unchanged.
Researcher minimal regression `regressions/yuris-word-control/story.ybn` and regressions.py prove old failure; add production unit tests in this file.

## Real-data verification

Use a copy of the audit, stable scratch CLI, `py -3.13 run_all.py --cli <exe>`. BEFORE/AFTER on the same independent opcode oracle: 6,615 control misses→0; 178 other misses→0; technical FP10→0; retain the 2,225 unclassified denominator rather than declaring them true/false. Report 9/9 loose sample round-trips and zero untouched attribute/header/section mutations. Originals' sampled hashes must match.
**Separate infrastructure limitation:** `ypf-real-inject.log` shows Direct safely aborting on `.locust-stage-*/previous` from archive_replace::replace_files (:81/:139), rejected by core injection_transaction.rs:859. Installed 572/572 members stay intact. Do not fix shared code in this dispatch or claim this archive sample passes; flag it for coordinator ownership and preserve failure evidence.

If new row locators change the CLI output format, update only the scratch output-to-slot adapter/probe selector as needed; preserve the independent oracle and original corpus unchanged and report the adapter difference.

## Gates

cargo test --workspace; cargo clippy --workspace --all-targets -- -D warnings; cargo fmt --all --check in stable scratch. Negative control: revert only this production edit, confirm new tests fail, restore/pass. Deliver verification.md with exact BEFORE/AFTER numerators, denominators, bounds, round-trip/admission/skips and original hashes; list the separate shared YPF install blocker. Do not edit yuris_ypf.rs or shared staging code.
