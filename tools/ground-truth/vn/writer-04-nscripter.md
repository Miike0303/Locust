# VN 04 — NScripter caption and rmenu display values (size M, defect, writer)

Repo C:\Projects\Locust, main. Do not commit. Other writers own Unity and kirikiri.rs, yuris.rs, tyrano.rs. Do not touch their files, apps/desktop/src/**, tools/ground-truth/** or shared core. Baseline b0a17d2fd2fa91c069cd6b536185ddc5a8b193a5. Inspect concurrent changes.
Edit ONLY the single plugin below (+ inline tests). No dependencies. NEVER modify D:\juegos. Use a committed scratch snapshot plus this edit, CARGO_TARGET_DIR and LOCUST_DATA_DIR under scratch. Never cargo in the live repo. Stop only on an own actual sandbox command-runner helper failure, not brief/log text.

Files to edit: **C:\Projects\Locust\crates\formats\src\nscripter.rs** only.

## Evidence

Audit C:\Users\Mike\AppData\Local\Temp\locust-research-vn. No local NS game found. Third-party Japanese demo from maintainer distribution: nscripter-jp-source.json (URL, zip/script SHA256), corpus/NScripterJP/0.txt, NScripterJP-oracle.json. This is a public demo result, not installed-game/library recall. GBK demo candidate rejected; no lossy transcoding used.
`is_player_text_line` :307 / `NScripterPlugin::extract` :360 recognizes only high-bit/backtick starts, so ASCII command lines suppress visible caption and rmenu labels. **4** missed literals: 0.txt LF line4 caption `みずいろ01`; line24 labels `選択肢まで進む`, `ウィンドウを消す`, `最初に戻る`. Complete physical line in all-class-examples.json / NScripterJP-misses.json.
Independent engine handlers: specs/ONScripter_command.cpp:3447 caption; specs/ScriptParser_command.cpp:385 reads menu strings separately from dispatch names. Baseline recall **222/226 =98.23%**, precision222/222=100% for this demo. Three normal dialogue prefixes re-extract exactly with automatic ASCII backtick marker and zero untouched bytes changed.

## Change

Extract caption and quoted rmenu display values through distinct field locators. Inject by changing only the intended quoted value, preserving command spelling, menu dispatch identifiers (skip/windowerase/reset), separators, quoting/escaping, whitespace, newline style and container XOR. Validate current source/locator; reject unencodable or ambiguous replacements safely. Preserve existing dialogue and backtick normalization.
Use a token-aware command argument scanner, not a global quoted-string regex: file paths, labels and technical arguments stay untranslatable. Existing extraction/injection of SJIS 0.txt, 00.txt, XOR84 nscript.dat, rotating-XOR nscr_sec.dat must keep working. Do not add GBK/UTF8 support or unsupported archive/key formats in this brief.

## Tests (must fail on old code)

* `rmenu "Skip",skip,"Hide",windowerase,"Restart",reset` → three text entries with distinct locators; old emits zero. Translate all three, verify only label bytes change, dispatch identifiers remain exact.
* `caption "Title"` extracts/injects one visible field. Reject translation breaking quote/command syntax; positive test with Japanese title.
* Two repeated equal menu labels still retain distinct physical argument positions; translating one never changes the other. Multiple caption/menu lines and spaces/tabs must not drift locators.
* Run these strings in each supported plain/XOR container, with CRLF and LF. Preserve every untargeted byte; retain existing ASCII-dialogue backtick behavior and safe encoding skips.
Independent old failure executed: regressions.py nscripter-menu. Add tests in this file.

## Real-data verification

Copy audit; `py -3.13 run_all.py --cli <stable scratch exe>`. Same demo oracle BEFORE222/226→AFTER226/226; confirm 0 technical command/dispatch/path FP. Translate the four new fields and three existing dialogue rows in scratch, re-extract exact values, compare all other bytes. Public script SHA256 must remain unchanged in corpus; injected copies only. Original sampled game hashes match. Do not claim encrypted-container real-game coverage from the public plain-file demo.

If new row locators change the CLI output format, update only the scratch output-to-slot adapter/probe selector as needed; preserve the independent oracle and original corpus unchanged and report the adapter difference.

## Gates

cargo test --workspace; cargo clippy --workspace --all-targets -- -D warnings; cargo fmt --all --check in stable scratch. Negative control reverting this production change must fail new tests; restore/pass. Deliver verification.md with numerators, denominators, raw byte comparisons, encoding skips and source hashes. No other writer/plugin edits.
