# VN 01 — lossless KAG edits and existing XP3 patches (size L, defect, writer)

Repo C:\Projects\Locust, main. Do not commit. Two Unity writers own unity.rs and unity_serialized.rs. Other VN writers own yuris.rs, tyrano.rs, nscripter.rs. Do not touch their files, apps/desktop/src/**, tools/ground-truth/** or shared core.
Baseline commit: b0a17d2fd2fa91c069cd6b536185ddc5a8b193a5. Inspect current changes before dispatch; never overwrite another writer's work.
Edit ONLY the single plugin file below, including its inline test module. No dependencies. NEVER modify D:\juegos. Build a committed scratch snapshot with only your edit overlaid; set CARGO_TARGET_DIR and LOCUST_DATA_DIR under scratch. Never cargo-build the live repo. Stop only on your own real Windows sandbox command-runner helper failure, not quoted logs.

Files to edit: **C:\Projects\Locust\crates\formats\src\kirikiri.rs** only. This file is exclusively owned by this brief; all KAG subissues are grouped here to avoid overlapping writers.

## Evidence

Audit: C:\Users\Mike\AppData\Local\Temp\locust-research-vn. Read last-report.md, README.md, xp3-real-injection.json, kag-mixed-newlines-injection.json, class-examples.json.
1. `KirikiriPlugin::inject` :1145/:1146 replaces existing patch.xp3 with only new payloads: Ochiru copy loses **172/172 unrelated members**; reports 3 writes, then 0/3 translations re-extract because original patch2.xp3 still outranks generated patch.xp3. `script_source_rank` :767. Original game path D:\juegos\VN\Ochiru Hitozuma\patch.xp3; exact lost names/sizes in xp3-real-injection.json.
2. `normalize_newlines` :485/:486, `encode_ks_bytes` :432, `apply_translations` :668: Taimanin newgame01.ks has 5,381 bare CR and 18 LF; translating 3 extracted rows produces 5,398 LF, 15 untouched rows change locator/source, row count 18→2,022.
3. `extract_lines_from_text` :651/:653 only splits LF: **20,247** logical message slots missed in bare-CR scripts. `is_non_text_line` :506 and `is_pure_tag_line` :605 skip **953** `[name text]` and **10,392** `[NAME_W n]` values. The shipped NAME_W macro actually renders `%n`: Taimanin unencrypted/name.ks:783/:799. My Ditzy Mom role inferred from speaker/dialogue use, see hand-checks.md.
4. Line heuristics admit **17** TJS body rows inside iscript, e.g. My unencrypted/scenario/_first.ks:27/:29/:30. Ordinary prefix injection accepts them. Current core rejects translations that remove protected controls: preserve that protection.

## Change

Make edits lossless across supported line delimiters/encodings/BOM/cipher wrappers. Retain each original separator and every untouched byte; change addressed text spans only. Parse iscript/endscript state rather than declaring TJS-like lines safe by keyword heuristics. Extract/inject known display attributes through explicit locators, preserving tag names, other attributes, escaping, whitespace and continuation controls.
Preserve old locator compatibility safely: validate the addressed current source before writing; refuse stale/ambiguous rows. Do not translate arbitrary tag attributes or macro/storage ids. Keep visible punctuation policy explicit; the 207 Ochiru filler misses are documented deliberate behavior, not the priority fix.
For archive edits, preserve all existing patch entries, metadata/payloads and active precedence. Choose a safe effective patch generation or explicitly refuse without claiming writes when precedence/merged encrypted entries cannot be preserved. Never replace an existing patch with only the requested entries. Only report effective writes after successful output. Keep the change in this plugin file; use existing archive APIs or safe refusal instead of changing kirikiri_xp3.rs.

## Tests (must fail on old code)

* Minimal archive: pre-existing patch.xp3 contains an unrelated `keep.bin` and patch2.xp3 contains `story.ks`; translate a story row. Old code loses keep.bin and the translated row remains hidden. Require original member retention and effective re-extraction or explicit 0-write refusal. Test existing script overrides and unrelated non-KS members.
* `;comment\rHello.\rGoodbye.\r` → two dialogue slots; old extraction emits zero. Mixed CR/LF/CRLF fixture: prefix one row, require identical untargeted bytes/separators and no locator drift; test UTF8 BOM/no BOM, UTF16 and supported cipher wrappers.
* `[iscript]\nSystem.exit();\n[endscript]\nHello.\n` emits only Hello. Old emits System.exit(). Include quoted strings/indented iscript bodies.
* `[NAME_W n="Asagi"]\` / `[name text="Mitsuki"]`: extract and change name only; every other byte/tag survives. Include quoted bracket attributes and adjacent inline tags; reject missing controls or stale locators.
The independent regressions.py confirms kag-bare-cr, kag-script-block, kag-xp3-integrity and kag-newline-integrity fail. The last two are executable minimal injection fixtures: keep.bin is deleted and story remains untranslated; one LF message edit also mutates unrelated CR separators, adds a UTF8 BOM and drops the trailing separator. Exact byte hex is in regression-results.json. Add plugin unit tests with the same observable behavior.

## Real-data verification

Copy audit to your scratch. Run `py -3.13 run_all.py --cli <your stable scratch locust.exe>` with LOCUST_DATA_DIR inside that audit copy. Report identical-oracle BEFORE/AFTER: XP3 172 lost→0; effective round-trip 0/3→3/3 **or explicit safe refusal with no member loss**; bare-CR misses 20,247→0; untouched newline/row drift→0; script FP 17→0; known attribute misses 11,345→0. Explain any safe skips, including encrypted existing patch entries. All sampled original hashes must match. Do not substitute unit tests for real scripts/member payload comparisons.

If new row locators change the CLI output format, update only the scratch output-to-slot adapter/probe selector as needed; preserve the independent oracle and original corpus unchanged and report the adapter difference.

## Gates

cargo test --workspace; cargo clippy --workspace --all-targets -- -D warnings; cargo fmt --all --check, all in stable scratch. Negative control: revert only your production change in scratch and prove new tests fail, then restore/pass. No live-repo build and no other writer edits. Deliver verification.md with numerators/denominators, byte changes, effective writes, safe skips and hashes. No AFTER numbers asserted by the researcher.
