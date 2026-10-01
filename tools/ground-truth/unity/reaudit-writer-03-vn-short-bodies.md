# Unity 03 — recover single-character VN dialogue (S, defect, writer)

Repo C:\Projects\Locust, main. Do not commit. Re-audited commit: 09eac768a315225c764091697d9fa8a54859158a; inspect/rebase if HEAD moves. Other writers own yuris.rs and tyrano.rs: do not touch them. Do not touch core, apps/desktop/src/**, or tools/ground-truth/**. Edit only the Unity files named below and their in-file tests. No new dependencies.
NEVER modify D:\juegos. Fixtures/reports/build outputs belong in a new scratch root, under 5 GB. Archive committed HEAD and overlay only your Unity changes. Never run cargo in the live repo. Use %LOCALAPPDATA%\locust-shared-target, CARGO_INCREMENTAL=0, CARGO_PROFILE_DEV_DEBUG=0; copy only the finished executable. Stop only for an actual Windows sandbox helper failure from your own tool call.
Evidence root: C:\Users\Mike\AppData\Local\Temp\locust-c113-unity-reaudit. This run could not recreate the pinned custom-schema oracle; do not treat supplementary precision/recall as the original full audit.


Files: crates/formats/src/unity.rs and in-file tests.

## Evidence
24 physical dialogue misses: 22 one-character bodies + 2 control-bearing \bI bodies (oot-misses.json). extract_text_scripts unity.rs:166/:176 uses minimum_len=2 for ordinary dialogue; vn_visible_text removes controls before the minimum check. remaining-regressions.json one-character fixture reproduces the omission.

## Change
Once a valid dialogue speaker/body record is recognized, accept a nonempty visible body even when it contains one character. Preserve all controls and exact engine prefix. Do not broaden generic binary heuristics or accept empty/control-only/directive records. Keep injection's control checks and original line endings.

## Tests (must fail on current HEAD)
A scripts-only fixture with CJ Existing dialogue., CJ !, CJ ?, CJ I and CJ \bI. Require every nonempty visible dialogue row, while CJ plus controls only remains absent. Inject a single-character translation with intact controls and require every untargeted line byte-identical. Test one CJK character, a multibyte symbol, paired controls and unsafe missing controls. Negative control: restore the minimum_len=2 branch and require failures.

## Real-data verification
Recover all 24 fields without changing 275 unclassified script rows by assertion. Script declaration names belong to writer-02. Three glued-sprite records remain a separate measured limitation (see report, extract_vn_dialogue :2068/:2076); do not claim this change fixes them. All existing control and sprite regressions must remain valid.

## Gates
Same stable-snapshot cargo test/clippy/fmt gates as writer-01. Deliver verification.md with 22+2 denominators, requested writes/skips, actual line diffs and 112 original script hash checks.
