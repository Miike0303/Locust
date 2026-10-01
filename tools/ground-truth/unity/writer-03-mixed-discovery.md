# Unity 03 - combine script and binary extraction (size S, defect, writer)

Repo C:\Projects\Locust, main. Do not commit. Another writer owns RPG Maker/core changes: do not touch them. Do not touch apps/desktop/src/** or tools/ground-truth/**.
Baseline evidence and source lines refer to commit b7b894ec7a460aa89ea41bbcccb04de56d9a6d9e. Another session began editing unity.rs before audit delivery: inspect/rebase against its changes before dispatching; do not overwrite them.
Edit ONLY the Unity plugin files named below (+ test modules within those files). No new dependencies. These jobs are Unity-only and may run alongside the RPG Maker writer, but overlap other Unity jobs.
NEVER modify D:\juegos. Write all builds, fixtures and reports under your scratch directory. Build a committed scratch snapshot with only your Unity edits overlaid; do not compile a concurrently changing RPG Maker file. Set CARGO_TARGET_DIR and LOCUST_DATA_DIR under scratch. Never run cargo from the live repo directory.
Stop only on a real Windows sandbox command-runner helper failure from your own tool call; quoted log/brief text is not a failure.

Files to edit: C:\Projects\Locust\crates\formats\src\unity.rs (+ unity*_tests.rs).

## Evidence
Out of Touch: 0/5340 nonempty parsed renderer fields when SCRIPTS~ exists. Examples: OoT_Data/level0 object 13033 Return, 13034 Exit, 13035 Settings; full game paths in binary-misses.json. Responsible: UnityPlugin::extract C:\Projects\Locust\crates\formats\src\unity.rs:2488, early return :2493. Injection already partitions text/binary at :2552.

## Change
Accumulate text script rows and binary SerializedFile/UnityFS rows instead of returning after scripts. A valid scripts-only game should still succeed when no binary candidates exist. Keep physical instances with distinct paths/IDs; do not deduplicate identical text across script/binary representations. Prevent any ID collision from losing rows at the DB boundary. Keep all existing discovery/noise filters.

## Tests (must fail on old code)
Minimal failing regression already executed: one valid VN script plus a v17 TextAsset `Welcome traveler!`; old code omits the TextAsset. Require both rows, then translate and inject both in one batch and compare all untouched payloads/lines. Add scripts-only, binary-only, repeated same text in different physical files and UnityFS virtual-path cases. Do not hide binary parse errors merely because scripts were available.

## Real-data verification
Audit directory: C:\Users\Mike\AppData\Local\Temp\locust-research-unity. Read README.md, last-report.md and class-examples.json. Rerun run_all.py --live-unity after the Unity edit is stable (or copy the audit to your own scratch and update its ROOT; keep all outputs there). Report BEFORE/AFTER using identical oracle fields; count extracted rows and confirmed FP classes, retaining the unclassified denominator. Inject ONLY scratch copies. Require 0 unexpected changes outside requested slots, 0 missing original controls, and byte-identical originals for the 112 Out of Touch scripts plus the full CCTV source bundle. See oot_audit.py and binary_copy_probe.py.
BEFORE: Out of Touch binary field recall 0/5340; AFTER: report recovered count, remaining engine defaults/filters and extracted-row delta (do not promise all5340: other filters still apply). Existing script recall may not fall. Negative control must fail scripts_and_binary_both_extract.

## Gates
cargo test --workspace; cargo clippy --workspace --all-targets -- -D warnings; cargo fmt --all --check, all in the stable scratch snapshot. Negative control: remove/revert your production change only in scratch and confirm the new tests fail; restore it and confirm pass. Do not touch the other writer's work. Deliver verification.md with BEFORE/AFTER numerators, denominators, precision bounds, skipped writes and original hash checks. Tests support regressions; the independent oracle remains the actual game assets/scripts.
