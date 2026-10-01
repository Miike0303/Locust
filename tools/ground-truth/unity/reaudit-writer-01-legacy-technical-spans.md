# Unity 01 — reject legacy heuristic technical-name writes (M, defect, writer)

Repo C:\Projects\Locust, main. Do not commit. Re-audited commit: 09eac768a315225c764091697d9fa8a54859158a; inspect/rebase if HEAD moves. Other writers own yuris.rs and tyrano.rs: do not touch them. Do not touch core, apps/desktop/src/**, or tools/ground-truth/**. Edit only the Unity files named below and their in-file tests. No new dependencies.
NEVER modify D:\juegos. Fixtures/reports/build outputs belong in a new scratch root, under 5 GB. Archive committed HEAD and overlay only your Unity changes. Never run cargo in the live repo. Use %LOCALAPPDATA%\locust-shared-target, CARGO_INCREMENTAL=0, CARGO_PROFILE_DEV_DEBUG=0; copy only the finished executable. Stop only for an actual Windows sandbox helper failure from your own tool call.
Evidence root: C:\Users\Mike\AppData\Local\Temp\locust-c113-unity-reaudit. This run could not recreate the pinned custom-schema oracle; do not treat supplementary precision/recall as the original full audit.


Files: crates/formats/src/unity.rs AND crates/formats/src/unity_serialized.rs.

## Evidence
Normal CLI reproduces the unsafe write: legacy-gap-cli.json and legacy-gap-cli-inject.log. CCTV globalgamemanagers.assets object 2323, length-prefix offset 813132: LiftGammaGain → AUDIT, 1 object changed, 14 byte differences, 0 skip, original bundle unchanged. Payload length is 13, not 14. The earlier handoff's 14 matches changed bytes.
UnityPlugin::inject_serialized_bytes unity.rs:401/:408/:415 builds technical_ranges from whole noise-class objects only; heuristic write guard :760 misses class-114 base names and font aliases. Extraction and fixed-slot rewrite already use script_identity unity_serialized.rs:579, MonoScriptIdentity::has_technical_object_name :242, technical_font_name_range :1745, and rewrite_text_asset_script_inplace :2226/:2313.

## Change
unity_serialized.rs: add a pub(crate) SerializedFile method returning validated forbidden byte spans for injection. Resolve each MonoBehaviour's real local/external/bundle MonoScript via the existing script_identity implementation, read the bounded version/endian-aware base, and return technical m_Name length-prefix + payload spans plus recognized TMP_FontAsset FaceInfo.m_FamilyName alias spans. Include existing noise-class object spans. Reuse has_technical_object_name and technical_font_name_range; do not invent a class-name string blacklist in unity.rs, expose private identity structs, or exclude whole renderers that contain legitimate m_text. Preserve unknown/custom names and equal-looking text in actual display fields. Use checked bounds and deterministic sorted/merged ranges. Keep extraction's broader structural heuristic-skip ranges separate.
unity.rs: replace the noise-class-only construction in inject_serialized_bytes with this shared SerializedFile API before any writes. Apply the resulting forbidden-span overlap check to both explicit binary_offset and searched legacy heuristic targets, including the 4-byte prefix, before changing bytes. Reject as invalid_target with 0 strings_written and unchanged file bytes. Keep valid structural renderer/table writes and current namespace IDs unchanged.

## Tests (must fail on current HEAD)
Minimal real regression: run legacy_gap_cli_probe.py against a scratch snapshot; legacy-gap-rows.json documents the exact source/metadata. After the fix require unchanged LiftGammaGain, identical file bytes, strings_written=0, invalid_target=1. The probe currently records a failing desired invariant (reproducible=true).
Add self-contained fixtures with actual local/external MonoScript identities: class-114 UnityEngine.Rendering.LiftGammaGain technical m_Name; TMP_FontAsset 1.1.0 matching m_Name/m_FamilyName alias; local and UnityFS external resolution; LE/BE layouts. Exercise explicit offsets and offset-less old rows. Positive controls: same text in a supported Text.m_Text/TextMeshPro.m_text and an unresolved custom Object.m_Name still writes. Existing regressions.py ObjectNames fixture has no MonoScript identity and is NOT a valid technical-name negative control.

## Real-data verification
Repeat all five reader-versus-DB checks; require 0 collisions/lost IDs, all 9 BOXMAN level2 rows and safe injection, 0 unexpected script/binary slot changes, unchanged 112 original scripts + CCTV bundle + BOXMAN sample. Reproduce the forbidden write before and safe rejection after. Preserve 3/3 VN controls and current MENU extraction. Run the full pinned oracle when available; report skipped writes and unclassified counts separately.

## Gates
cargo test --workspace; cargo clippy --workspace --all-targets -- -D warnings; cargo fmt --all --check in the stable scratch snapshot. Revert only your production change in scratch and require the new technical-name/alias tests to fail; restore and pass. Deliver verification.md with source commit, BEFORE/AFTER, exact offsets, write/skip reasons, hashes and disk usage.
