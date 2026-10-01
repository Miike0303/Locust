# Unity 04 - exclude technical MonoBehaviour Object.m_Name (size M, defect, writer)

Repo C:\Projects\Locust, main. Do not commit. Another writer owns RPG Maker/core changes: do not touch them. Do not touch apps/desktop/src/** or tools/ground-truth/**.
Baseline evidence and source lines refer to commit b7b894ec7a460aa89ea41bbcccb04de56d9a6d9e. Another session began editing unity.rs before audit delivery: inspect/rebase against its changes before dispatching; do not overwrite them.
Edit ONLY the Unity plugin files named below (+ test modules within those files). No new dependencies. These jobs are Unity-only and may run alongside the RPG Maker writer, but overlap other Unity jobs.
NEVER modify D:\juegos. Write all builds, fixtures and reports under your scratch directory. Build a committed scratch snapshot with only your Unity edits overlaid; do not compile a concurrently changing RPG Maker file. Set CARGO_TARGET_DIR and LOCUST_DATA_DIR under scratch. Never run cargo from the live repo directory.
Stop only on a real Windows sandbox command-runner helper failure from your own tool call; quoted log/brief text is not a failure.

Files to edit: C:\Projects\Locust\crates\formats\src\unity_serialized.rs, C:\Projects\Locust\crates\formats\src\unity.rs, test modules inside unity*.rs only.

## Evidence
Independent UnityPy Object.m_Name headers/class resolution confirm 664 FP rows: CCTV48, USSR44, Sunkissed34, BOXMAN538. Three Sunkissed MemoryType names and three unresolved BOXMAN names remain unclassified. Complete examples binary-fps.json, class object_name, and independent-headers.json. Responsible: SerializedFile::read_mono_strings_object C:\Projects\Locust\crates\formats\src\unity_serialized.rs:740-748; mono_name_worth_extracting :1088. Renderer/configuration/Tile/Cubism/scenario/font asset IDs such as LiftGammaGain and LensDistortion are not renderer m_text.

## Change
Exclude base Object.m_Name for identified technical renderer/configuration/Tile/Cubism/scenario/font assets. Identify the owning script type; do not blindly drop names from unknown custom ScriptableObjects (MemoryType may expose its name as a player-facing label). Preserve technical names exactly during all injections, including legacy entries targeting field_index=0. Continue to extract actual script string fields, including first visible field following m_Name, duplicate UI instances, and single CJK labels. Update tests deliberately expecting technical m_Name rows and keep positive tests for actual renderer/custom text fields. Do not replace this with a broader token blacklist.

## Tests (must fail on old code)
Minimal failing regression already executed: MonoBehaviour name LiftGammaGain with string field Hello traveler!; old code extracts both. Require only the visible script field. Inject its translation and assert the name bytes/object header/other object bytes remain identical. Add TextMeshPro-like m_Name MENU with actual m_text MENU: only the display field is eligible.

## Real-data verification
Audit directory: C:\Users\Mike\AppData\Local\Temp\locust-research-unity. Read README.md, last-report.md and class-examples.json. Rerun run_all.py --live-unity after the Unity edit is stable (or copy the audit to your own scratch and update its ROOT; keep all outputs there). Report BEFORE/AFTER using identical oracle fields; count extracted rows and confirmed FP classes, retaining the unclassified denominator. Inject ONLY scratch copies. Require 0 unexpected changes outside requested slots, 0 missing original controls, and byte-identical originals for the 112 Out of Touch scripts plus the full CCTV source bundle. See oot_audit.py and binary_copy_probe.py.
BEFORE: object_name FP664; AFTER: 0, renderer recall unchanged except explicitly justified name-only fixtures. Preserve the six unclassified name rows until runtime evidence resolves them. Negative control must fail do_not_extract_object_name; a fixture lacking type identity must be made explicitly technical by adding MonoScript/type data. Recheck independent-headers.json; do not count unclassified rows as technical.

## Gates
cargo test --workspace; cargo clippy --workspace --all-targets -- -D warnings; cargo fmt --all --check, all in the stable scratch snapshot. Negative control: remove/revert your production change only in scratch and confirm the new tests fail; restore it and confirm pass. Do not touch the other writer's work. Deliver verification.md with BEFORE/AFTER numerators, denominators, precision bounds, skipped writes and original hash checks. Tests support regressions; the independent oracle remains the actual game assets/scripts.
