# Unity 01 - preserve embedded VN controls (size M, defect, writer)

Repo C:\Projects\Locust, main. Do not commit. Another writer owns RPG Maker/core changes: do not touch them. Do not touch apps/desktop/src/** or tools/ground-truth/**.
Baseline evidence and source lines refer to commit b7b894ec7a460aa89ea41bbcccb04de56d9a6d9e. Another session began editing unity.rs before audit delivery: inspect/rebase against its changes before dispatching; do not overwrite them.
Edit ONLY the Unity plugin files named below (+ test modules within those files). No new dependencies. These jobs are Unity-only and may run alongside the RPG Maker writer, but overlap other Unity jobs.
NEVER modify D:\juegos. Write all builds, fixtures and reports under your scratch directory. Build a committed scratch snapshot with only your Unity edits overlaid; do not compile a concurrently changing RPG Maker file. Set CARGO_TARGET_DIR and LOCUST_DATA_DIR under scratch. Never run cargo from the live repo directory.
Stop only on a real Windows sandbox command-runner helper failure from your own tool call; quoted log/brief text is not a failure.

Files to edit: C:\Projects\Locust\crates\formats\src\unity.rs (+ unity*_tests.rs if needed).

## Evidence
3/3 real copied lines lost all internal \i controls. Game paths and full BEFORE/AFTER strings: oot-injection.json. Responsible: UnityPlugin::extract_text_scripts C:\Projects\Locust\crates\formats\src\unity.rs:166; inject_text_scripts :247/:265; strip_vn_format_codes :2037; split_format_codes :2049.

## Change
Keep engine controls inside the dialogue representation, so translated text carries the original protected tokens. Do not reconstruct only leading/trailing codes. Preserve inline italic/bold/link boundaries, explicit breaks and any non-text prefix. Validate/reject unsafe missing controls before writing; a skip must not report a successful write. Maintain original CRLF, trailing spaces and final newline. Existing plain-dialogue translation remains supported; any source-schema change must not silently write old stripped entries over rich text.

## Tests (must fail on old code)
Minimal failing regression (already executed in regressions.py): a scripts-only game with `CJ Hello \bworld\b.`; translate its extracted row and inject. Old code produces `CJ AUDIT Hello world.`. New tests require both embedded \b controls to survive, proper pairing, and identical untouched lines. Add \i, \p<link=...>, \n/\> continuation and non-ASCII text cases. Either round-trip a supplied translation carrying controls or reject its removal with 0 writes; never claim success while deleting them.

## Real-data verification
Audit directory: C:\Users\Mike\AppData\Local\Temp\locust-research-unity. Read README.md, last-report.md and class-examples.json. Rerun run_all.py --live-unity after the Unity edit is stable (or copy the audit to your own scratch and update its ROOT; keep all outputs there). Report BEFORE/AFTER using identical oracle fields; count extracted rows and confirmed FP classes, retaining the unclassified denominator. Inject ONLY scratch copies. Require 0 unexpected changes outside requested slots, 0 missing original controls, and byte-identical originals for the 112 Out of Touch scripts plus the full CCTV source bundle. See oot_audit.py and binary_copy_probe.py.
BEFORE: 3/3 real control-loss probes; AFTER: 0/3 loss, same requested writes or explicit safe skips, no changes to untouched lines. Negative control must fail preserve_internal_controls. Review oot-injection.json after every run; do not compare only reported write totals.

## Gates
cargo test --workspace; cargo clippy --workspace --all-targets -- -D warnings; cargo fmt --all --check, all in the stable scratch snapshot. Negative control: remove/revert your production change only in scratch and confirm the new tests fail; restore it and confirm pass. Do not touch the other writer's work. Deliver verification.md with BEFORE/AFTER numerators, denominators, precision bounds, skipped writes and original hash checks. Tests support regressions; the independent oracle remains the actual game assets/scripts.
