# Unity 02 - extract dialogue following sprite modifiers (size M, defect, writer)

Repo C:\Projects\Locust, main. Do not commit. Another writer owns RPG Maker/core changes: do not touch them. Do not touch apps/desktop/src/** or tools/ground-truth/**.
Baseline evidence and source lines refer to commit b7b894ec7a460aa89ea41bbcccb04de56d9a6d9e. Another session began editing unity.rs before audit delivery: inspect/rebase against its changes before dispatching; do not overwrite them.
Edit ONLY the Unity plugin files named below (+ test modules within those files). No new dependencies. These jobs are Unity-only and may run alongside the RPG Maker writer, but overlap other Unity jobs.
NEVER modify D:\juegos. Write all builds, fixtures and reports under your scratch directory. Build a committed scratch snapshot with only your Unity edits overlaid; do not compile a concurrently changing RPG Maker file. Set CARGO_TARGET_DIR and LOCUST_DATA_DIR under scratch. Never run cargo from the live repo directory.
Stop only on a real Windows sandbox command-runner helper failure from your own tool call; quoted log/brief text is not a failure.

Files to edit: C:\Projects\Locust\crates\formats\src\unity.rs (+ unity*_tests.rs).

## Evidence
3468 misses in oot-misses.json, class dialogue_inline_sprite. Example D:\juegos\unity\Out of Touch\OoT_Data\SCRIPTS~\Vol1\Chapter_1.txt:407: `CJ +CJ_Lhao You really haven't aged a day have you?`. Responsible: extract_vn_dialogue C:\Projects\Locust\crates\formats\src\unity.rs:2028; extraction :162; inject_text_scripts :246.

## Change
Parse leading sprite/emotion modifiers after the character ID, then expose only the following dialogue as source. Handle multiple +tokens and accompanying -tokens, keeping their exact original spelling and spacing as a separate engine prefix. A modifier-only line produces no dialogue. Inject into the text portion and preserve the character ID, modifier prefix, internal controls and all untouched bytes. Reuse a shared parser for extraction/source-changed guards/injection.

## Tests (must fail on old code)
Minimal failing regression already executed: `CJ Existing dialogue.` followed by `CJ +CJ_Lgr Hello friend.`; old code extracts only the first row. Require source `Hello friend.` on the second line and injected prefix still `CJ +CJ_Lgr `. Add `CJ +CJ_Lgr +J_Usur -R W-what?!`, modifier-only, changed-prefix/source guard, Unicode, and quote/control preservation. No broad extraction of command-only + lines.

## Real-data verification
Audit directory: C:\Users\Mike\AppData\Local\Temp\locust-research-unity. Read README.md, last-report.md and class-examples.json. Rerun run_all.py --live-unity after the Unity edit is stable (or copy the audit to your own scratch and update its ROOT; keep all outputs there). Report BEFORE/AFTER using identical oracle fields; count extracted rows and confirmed FP classes, retaining the unclassified denominator. Inject ONLY scratch copies. Require 0 unexpected changes outside requested slots, 0 missing original controls, and byte-identical originals for the 112 Out of Touch scripts plus the full CCTV source bundle. See oot_audit.py and binary_copy_probe.py.
BEFORE: 3468 inline_sprite misses; AFTER: 0 or enumerate an evidence-backed unsupported grammar subset. Precision lower/upper bounds must not worsen from newly extracted engine tokens. Negative control must fail inline_sprite_dialogue.

## Gates
cargo test --workspace; cargo clippy --workspace --all-targets -- -D warnings; cargo fmt --all --check, all in the stable scratch snapshot. Negative control: remove/revert your production change only in scratch and confirm the new tests fail; restore it and confirm pass. Do not touch the other writer's work. Deliver verification.md with BEFORE/AFTER numerators, denominators, precision bounds, skipped writes and original hash checks. Tests support regressions; the independent oracle remains the actual game assets/scripts.
