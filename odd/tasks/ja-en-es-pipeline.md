# Trustworthy Japanese -> English -> Spanish game pipeline

## Goal
Reliable JA -> EN -> ES -> patch -> easy automatic installation for Japanese visual novels, Ren'Py and RPG Maker. Universal linguistic correctness is not a testable guarantee; report only observed completeness, source/control preservation, safe isolated installation and truthful failures/unsupported states.

## Scope and constraints
- Baseline `01c9561`; branch `feat/ja-en-es-pipeline`.
- Preserve three dirty `.claude` tooling files and separate verified UI branch `fix/desktop-audit-findings` (`34b05b2`); exclude them from this feature.
- Never modify original games, read credentials, spend money, delete existing backups, or weaken unsupported/encrypted-format guards. Testing installs only into operation-owned copies.
- Free/subscription providers only where already registered; no purchases/account changes. Deterministic local providers use dummy keys and loopback.
- Partial editing workflows remain supported; automated releases use strict completeness.
- One delegated writer at a time. Applicable tests: observed RED then GREEN. RDD off; native assessment failures receive independent verification.
- Delivery: verified default/main branch chain, each work unit with full checks and manual exact-SHA hosted CI before non-force main delivery. YOLO supplies ordinary delivery permission, not destructive authorization.
- Forecast 600-1000 authored diff lines in three coherent units; actual PIPE-1 440 excluding task tracking. Advisory size heuristic only; no code golf or removal of tests.

## Tasks
- [x] PIPE-1 (done; delegated worker + verifier): Strict complete pivot API/CLI flag; refuse absent/Unicode-blank/Pending/Error rows before artifacts, retain provenance/control checks and partial compatibility. Commit `4afefbda8ff8b730b1754b816c49b4666054bcaf`, exact-SHA CI passed and main fast-forwarded/pushed.
- [ ] PIPE-2 (in progress; delegated): Deterministic CLI JA -> EN -> ES -> inject -> package -> apply -> re-extract -> rollback for Ren'Py, RPG Maker MV and loose KiriKiri. Use a local scripted OpenAI-compatible endpoint, not SQL translation seeding. Cover titles, controls, duplicate slots, missing/corrupt answers and original hashes. Edit surfaces: `crates/cli/tests/ja_en_es_pipeline.rs`, `crates/cli/tests/support/scripted_openai.rs`. Work-unit commit pending.
- [ ] PIPE-3 (pending; delegated): Safe repeatable stdlib Python orchestration around existing commands with strict gates, isolated translated/install copies, reports and fail-closed resumability. Proposed surfaces `tools/ja_en_es_pipeline.py`, `tools/test_ja_en_es_pipeline.py`, `tools/ja_en_es_pipeline.md`; map details before dispatch. Audit real available games read-only/owned copies with free/subscription providers; explicitly report language/runtime/unsupported limitations. Commit pending.

## Acceptance
- No release-success when inventory shrinks, translations are absent/stale or controls damaged.
- Strict pivot preserves every row; refusal creates no output DB/sidecar and changes no source rows.
- Synthetic outputs preserve controls, strictly install on pristine copies, re-extract exact expected text, rollback asset bytes, and leave originals unchanged.
- Automation never injects into originals; rejects incomplete extraction/stages, unsafe or occupied roots and unsupported formats. Exit status alone is not a completeness gate.
- Evidence binds checks to commits and distinguishes scripted/real providers and technical/linguistic/runtime claims.

## Evidence
### PIPE-1
- RED: four core failures and four CLI failures before implementation. GREEN: core pivot 24/0; CLI pivot 9/0.
- Independent local workspace 2275 passed / 0 failed / 21 ignored; frontend 439/0 and production build; fmt/whitespace passed.
- Initial Clippy nonminimal_bool failure corrected with equivalent `is_none_or`; independently rerun full strict Clippy, fmt and CLI9/0 passed. `.test-tmp` only `.gitignore`.
- Assessment unavailable due undeclared untracked files; conservatively treated high, separate verifier used. No native RDD because off.
- Hosted [38453099882](https://github.com/Miike0303/Locust/actions/runs/38453099882) exact `4afefbd`: Ubuntu Rust2264/0/21; Windows2275/0/21; frontend439/0/0 each; npm ci/build, fmt, strict Clippy and workspace including Tauri pass. Ignored tests not run. No real linguistic/game/native-UI claim.
### PIPE-2 map
- Unknown configured providers with base_url register OpenAI-compatible transport; existing seam can test real translate CLI twice with masked source-keyed local answers.
- Direct-injected root cannot be refreshed normally: re-extract separately patch-applied pristine copy into a new DB.
- CLI factory creates a profile per command by default; override one operation-owned profile per case.
- Extraction warnings persist as `project_metadata.extraction_warnings` and pivot copies metadata. Strict pivot is a row gate, not an extraction completeness gate.

### PIPE-2 implementation
- Added scripted real-transport CLI round trips for Ren'Py, MV and loose KiriKiri (six resource slots each), exact JA/EN/ES literals, duplicated slots, names/controls, ZIP payload/manifest hashes, strict receipts, unchanged original assets and complete file-byte rollback.
- Six negative missing-JA-answer / corrupted-ES-placeholder scenarios actually run translation and observe rejected pending rows plus strict pivot refusal and no DB sidecars/artifacts. Extraction-warning test is metadata carry-forward only, not a completeness gate.
- Initial characterization exception: no product-change RED applicable. Initial focused5/0 and independent full workspace2280/0/21, frontend439/0+build, strictClippy/fmt passed.
- Independent reviewer found fixture endpoint teardown unbounded under slow trickle. Added deadline/cancellation tests: observed RED5passed/2failed, corrected 50ms polls/shared2sec deadline, then writer two focused8/0 runs and independent8/0/fmt passed. Writer strictClippy passed. `.test-tmp` only `.gitignore`.
- Total tests/helpers 1384 new lines: self-contained bounded HTTP transport plus three engine oracles; exceeds forecast, kept as one test-only behavior slice without mixing production changes or compressing coverage. Hosted corrected-candidate CI pending.
- Follow-up limitation: generic placeholder validator does not protect argument-bearing KAG tags, while KiriKiri injection rejects altered tag inventory. No malformed-tag product CLI reproduction yet; automation must reject injection warnings/skips, not label a partial patch complete.

## Next step
Commit and hosted-verify PIPE-2, then implement PIPE-3 safety gates and quality audits. No source production changes until a deterministic failing behavior justifies a separate bounded fix.
