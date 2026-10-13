# Trustworthy Japanese -> English -> Spanish game pipeline

## Goal and constraints
Reliable JA -> EN -> ES -> patch -> easy installation for Japanese VN, Ren'Py and RPG Maker. Universal linguistic correctness is not promised. Never modify original games, read owner credentials, spend money, delete existing backups, weaken unsupported/encrypted guards, or claim unobserved runtime/native/linguistic checks.
Baseline `01c9561`, feature `feat/ja-en-es-pipeline`. Preserve three dirty `.claude` files and separate UI branch `34b05b2`. One writer, deterministic behavior changes use observed RED/GREEN. RDD off; unavailable assessment => independent verifier. Free/subscription or isolated dummy-key loopback providers only.
Delivery: verified main chain; full checks and manual exact-SHA hosted CI before non-force main delivery, YOLO ordinary permission. Size heuristic advisory: keep readable coherent slices. Running authored code/tests approximately1824 lines; growth reflects self-contained transport/engine oracles, not production breadth.

## Tasks
- [x] PIPE-1 (delegated): Strict complete pivot API/CLI. Commit `4afefbda8ff8b730b1754b816c49b4666054bcaf`, hosted verified and main delivered.
- [x] PIPE-2 (delegated): Scripted real-transport CLI round trips for Ren'Py/MV/loose KiriKiri, duplicate slots, titles/controls, strict patch/receipts, re-extract and exact asset rollback. Commit `7f8d5b3bb2a4d0b2d5e980d8e88be46db1172beb`, hosted verified and main delivered.
- [ ] PIPE-3A (in progress; delegated): Machine-readable CLI injection report, keeping human output compatible, so automation cannot silently miss warnings/skips. Derive/test exact supported contract. Surfaces `crates/cli/src/main.rs`, `crates/cli/tests/inject_json.rs`, small README usage. Commit pending.
- [ ] PIPE-3B (pending; delegated): Safe stdlib Python orchestration around existing commands, strict row/extraction gates, isolated pristine/work/install copies, machine-readable injection evidence, artifact/re-extract/rollback checks, reports and conservative resumability. Surfaces `tools/ja_en_es_pipeline.py`, `tools/test_ja_en_es_pipeline.py`, `tools/ja_en_es_pipeline.md`. Commit pending.
- [ ] PIPE-4 (pending; delegated): Audit real available Japanese games and saved JA/EN/ES text read-only; translate small representative isolated copies with already available free/subscription providers, evaluate names/titles/control codes/line layouts and untranslated/degenerate text. Fix deterministic causes in separately scoped units; report linguistic/runtime/unsupported limits and require no owner-game writes. Commit evidence pending.

## Acceptance
No release-success for shrinking inventory, warnings of incomplete extraction, missing/stale translations, damaged controls or partial injection. Partial editing remains supported. Strict pivot refusal creates no output/sidecar and changes no source. Automation refuses unsafe/occupied/overlapping roots, never injects originals, strictly installs expected payloads, re-extracts correct slots and proves rollback against immutable pristine assets. Evidence distinguishes scripted/real providers and technical/linguistic/runtime guarantees.

## Evidence
### PIPE-1
RED core4 + CLI4 failures; GREEN core24/CLI9. Independent workspace2275/0/21, frontend439/0+build; fmt/whitespace pass. Clippy initial nonminimal_bool failure corrected equivalently; independent strictClippy/fmt/CLI9 pass. `.test-tmp` only `.gitignore`. Assessment unavailable due untracked declarations, conservative high path used.
Hosted [38453099882](https://github.com/Miike0303/Locust/actions/runs/38453099882), exact4afefbd: Ubuntu2264/0/21, Windows2275/0/21, frontend439/0 each. Full CI gates pass, ignored not run.
### PIPE-2
Six resource slots/engine, literal JA/EN/ES expectations, both real CLI provider requests, complete inventory/provenance, ZIP hashes, strict pristine receipt, original asset hashes, exact file-byte rollback. Six broken-answer scenarios observe rejected pending rows and strict pivot refusal. Seeded extraction warning only characterizes metadata carry-forward, not actual extraction warning prevention.
Characterization exception: no product behavior changed; no product RED. Initial5/0; separate reviewer found slow-trickle endpoint shutdown hazard. Regression RED5pass2fail, then 50ms poll/shared2sec deadline + cancellation corrected; writer twice8/0 and independent8/0/fmt pass. Initial full workspace2280/0/21, frontend439/0+build/strictClippy pass; corrected full hosted verification follows.
Hosted [38459104899](https://github.com/Miike0303/Locust/actions/runs/38459104899), exact7f8d5b3: Ubuntu2272/0/21, Windows2283/0/21, frontend439/0 each; all gates pass. Main delivered. No real-game/linguistic/native claims.
### Follow-up map
Injection currently prints tables and can return success with warnings/skips; serializable core reports already exist. Add JSON seam before automation instead of parsing tables. Generic control checker does not protect argument-bearing KAG tags; injector rejects altered tag inventory. Characterize malicious tag answers and ensure automation never accepts partial results. Preserve proper names/unchanged source carefully: a skip is not automatically corruption, but requires explicit reason and exact artifact equivalence.

### PIPE-3A implementation
CLI `inject --json` emits exactly one existing DirectInjectReport or MultiLangReport, with mode discriminator, warnings, skip reasons, recording outcomes and backups. Human output unchanged; JSON-mode diagnostics/tracing go stderr; errors retain nonzero exit. RED1pass6fail (flag missing); GREEN7/0 injectJSON +8/0 pipelines. Independent full workspace2290/0/21, frontend439/0+build, strictClippy/fmt/whitespace pass. `.test-tmp` only `.gitignore`. No-op counter semantics verified: staged strings can be nonzero even when no physical paths changed, so automation must not infer completeness from counts alone. Assessment unavailable/untracked, independent conservative path used. Commit/hosted CI pending.

## Next step
Commit and hosted-verify PIPE-3A, then implement safe orchestration and execute real quality audits without treating scripted results as linguistic proof.
