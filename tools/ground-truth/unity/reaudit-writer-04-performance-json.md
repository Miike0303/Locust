# Unity 04 — remove PerformanceTestRun configuration rows (S, defect, writer)

Repo C:\Projects\Locust, main. Do not commit. Re-audited commit: 09eac768a315225c764091697d9fa8a54859158a; inspect/rebase if HEAD moves. Other writers own yuris.rs and tyrano.rs: do not touch them. Do not touch core, apps/desktop/src/**, or tools/ground-truth/**. Edit only the Unity files named below and their in-file tests. No new dependencies.
NEVER modify D:\juegos. Fixtures/reports/build outputs belong in a new scratch root, under 5 GB. Archive committed HEAD and overlay only your Unity changes. Never run cargo in the live repo. Use %LOCALAPPDATA%\locust-shared-target, CARGO_INCREMENTAL=0, CARGO_PROFILE_DEV_DEBUG=0; copy only the finished executable. Stop only for an actual Windows sandbox helper failure from your own tool call.
Evidence root: C:\Users\Mike\AppData\Local\Temp\locust-c113-unity-reaudit. This run could not recreate the pinned custom-schema oracle; do not treat supplementary precision/recall as the original full audit.


Files: crates/formats/src/unity.rs and in-file tests; unity_serialized.rs only if a shared predicate is actually needed.

## Evidence
6 confirmed current FP rows: two each in CCTV, USSR and Sunkissed (supplementary-fps.json). Three complete short examples: resources.assets objects 324 / 186 / 2449, source {"MeasurementCount":-1}. remaining_regressions.py performance-json returns that configuration as a translation row. Structural TextAsset path UnityPlugin::extract_strings_from_assets unity.rs:835/:837/:842 applies content/name filters that omit no PerformanceTestRun config.

## Change
Reject identified PerformanceTestRun/PerformanceTestConfig technical TextAssets using their owning asset name and recognizable measurement/config schema. Apply before locale/CSV/plain extraction; structural objects must remain covered by heuristic skip ranges, so rejected text cannot re-enter through the scanner. Preserve valid player-facing JSON localization tables and arbitrary dialogue containing braces. Do not add a generic reject-all-JSON gate.

## Tests (must fail on current HEAD)
A TextAsset named PerformanceTestRun with {"MeasurementCount":-1} must yield 0 translation rows. Also test the full measurement JSON captured in supplementary-fps.json, while JSON UI/dialogue positive controls continue to extract/inject. Reverting only the filter must restore the bad row.

## Real-data verification
Require 0/6 configuration FP rows after, same 443/449 BOXMAN independent table hits unless writer-02 is also merged, same script recall, 0 row collisions and unchanged originals. The other unknown binary classes require the full pinned schema oracle; do not infer zero FP outside these six.

## Gates
Same stable-snapshot cargo test/clippy/fmt gates as writer-01. Deliver verification.md with before=6, after count, positive controls, skipped rows and exact complete evidence.
