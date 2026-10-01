# VN 03 — Tyrano script state, quoted brackets and display attributes (size M, defect, writer)

Repo C:\Projects\Locust, main. Do not commit. Other writers own Unity, kirikiri.rs, yuris.rs, nscripter.rs. Do not touch their files, apps/desktop/src/**, tools/ground-truth/** or shared core. Baseline b0a17d2fd2fa91c069cd6b536185ddc5a8b193a5; inspect/rebase concurrent changes.
Edit ONLY the single plugin below (+ inline tests), no dependencies. NEVER modify D:\juegos. Build committed snapshot plus only your edit under scratch; CARGO_TARGET_DIR and LOCUST_DATA_DIR there. Never cargo in live repo. Stop only on own actual sandbox command-runner helper failure.

Files to edit: **C:\Projects\Locust\crates\formats\src\tyrano.rs** only. Do not edit tyrano_asar.rs/tyrano_nw.rs.

## Evidence

Audit C:\Users\Mike\AppData\Local\Temp\locust-research-vn. Eight engine-author scenario files, independent runtime parser/tag handlers and SHA256/URLs: tyrano-public-sources.json, specs/kag.parser.js, kag.tag.js, kag.tag_ext.js. Public-sample numbers are not commercial-game recall.
`classify_lines` :380 tracks only block comments, not iscript: **156** JS rows treated as dialogue. Real cached cg.ks:18 (`tf.page = 0;`), :19, :32. `is_pure_tag_line` :331/:345 closes at first `]` even inside quoted expressions: **85** whole engine tag rows with array subscripts falsely emitted. Full examples in class-examples.json.
`entries_from_ks_bytes` :433 / `is_structural_non_text` :367 drop **15 strict display attributes**: 8 glink choices, 2 chara_new.jname, 5 ruby readings. Five ruby values are present within opaque rich rows, so distinguish addressable text extraction from raw substring presence.
Baseline 153/168 strict literal slots =91.07% recall; precision **147/388 =37.89%** (156+85 definite technical rows). Direct prefix accepts JS/control-only rows as translations; missing protected controls are already rejected safely by core. Normal prefix sample preserves untouched bytes.

## Change

Match the engine parser's iscript/endscript state and quote-aware inline tag scanning. Suppress script bodies and structural tags, including quoted bracket expressions; retain actual player text outside tags. Handle known glink text, chara_new jname and ruby reading attributes with explicit text locators and targeted injection; preserve chara ids, face ids, targets, storage, expressions and all other syntax. Keep inline rich-dialogue controls protected.
Validate stale/ambiguous old line locators before writing. Preserve all unrelated bytes, original delimiters/BOM/final newline. Keep bare #chara ids as references; translate literals/display definitions rather than changing character lookup keys. Do not translate arbitrary engine attributes or HTML/JS wholesale. Use this file's common payload path so loose/ASAR/NW readers receive identical semantics without archive-file edits.

## Tests (must fail on old code)

* `[iscript]\nwindow.flag=1;\n[endscript]\n[glink text="Continue" target="*next"]\n`: old emits JS, omits Continue. Require only the visible choice, then inject only text value; JS and target remain exact.
* `[eval exp="f.x[0]=1"]\nHello.[p]\n`: old extracts eval line. Require only Hello.[p]; include single/double quotes, adjacent tags, escaped literal bracket, full-line @ syntax.
* chara_new name="akane" jname="あかね", #akane, #あかね: display definition/literal translated safely; reference id and face remain unchanged.
* `[ruby text="かん"]漢`: translate reading and base-text spans independently without dropping ruby control or unrelated attributes.
* Unsafe missing controls/stale locators are rejected, not successful writes. Same original bytes unchanged outside requested slots.
Old failures already executed: regressions.py tyrano-code-choice, tyrano-bracket-tag. Add unit regressions in this file.

## Real-data verification

Copy audit; `py -3.13 run_all.py --cli <stable scratch locust.exe>`. BEFORE/AFTER same runtime-derived oracle: JS FP156→0, quoted-tag FP85→0, strict attr misses15→0 (record raw-carried ruby5 separately), recall153/168→168/168 under this scope. New rows must inject/re-extract exact values without changing command names/ids/target/storage/expression/whitespace. Require zero unrelated byte mutations and retained safe control rejection. Originals' hashes match; public scripts' cached hashes remain unchanged.

If new row locators change the CLI output format, update only the scratch output-to-slot adapter/probe selector as needed; preserve the independent oracle and original corpus unchanged and report the adapter difference.

## Gates

cargo test --workspace; cargo clippy --workspace --all-targets -- -D warnings; cargo fmt --all --check in stable scratch. Negative control revert only this production change to prove new tests fail; restore/pass. Deliver verification.md with exact before/after counts, source manifest hashes, admitted/skipped writes and byte comparisons. No production archive or shared-core edits.
