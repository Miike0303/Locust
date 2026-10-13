# Trustworthy Japanese -> English -> Spanish game pipeline

## Goal and authorization
The owner requested renewed work until the JA -> EN -> ES -> patch -> easy automatic installation pipeline is reliable for Japanese visual novels, Ren'Py and RPG Maker. Universal linguistic correctness is not a testable guarantee. Guarantee only observed completeness, immutable source/control preservation, safe isolated installation and truthful unsupported/failed states.

## Scope and constraints
- Main baseline: `01c9561`; feature branch: `feat/ja-en-es-pipeline`.
- Preserve the three pre-existing dirty `.claude` tooling files and the separate verified UI branch `fix/desktop-audit-findings` (`34b05b2`). Do not include either in this feature.
- Never modify original games, read credentials, spend money, delete existing backups, or weaken unsupported/encrypted-format guards.
- Install/inject/apply testing targets only operation-owned copies. Subscription/free provider checks only where already registered, no purchases or account changes.
- Existing partial editing workflows remain supported. Add strict completeness to the automated release path rather than silently dropping rows.
- Single writer; applicable deterministic behavior changes use observed RED then GREEN.
- RDD is clone-local off; ordinary functional checks remain required.
- Delivery: verified default/main branch chain, one coherent unit with full checks and manual exact-SHA hosted CI before non-force main delivery. YOLO supplies ordinary delivery standing permission, never destructive authorization.
- Forecast: 3 bounded units, approximately 600-1000 authored diff lines total; slice by behavior, not file type. Unit 1 forecast 200-350 lines. Do not shrink tests or code-golf for line counts.

## Tasks
- [ ] PIPE-1 (in progress; delegated): Add a strict complete pivot entry point and CLI flag. Reject absent/blank/not-ready rows before output creation; retain current provenance and protected-control validation and legacy partial-pivot behavior. Deterministic core/CLI tests, unchanged source and absent output on refusal, full checks. Work-unit commit pending.
- [ ] PIPE-2 (pending; delegated): Characterize JA -> EN -> ES -> inject -> package -> apply -> re-extract -> rollback for Ren'Py and RPG Maker plus supported VN fixtures using scripted providers. Cover generic speaker titles, placeholders, duplicate strings and missing/corrupt answers. Confirm original hashes unchanged and exact install/rollback artifacts. Derive exact edit surfaces before dispatch. Commit evidence pending.
- [ ] PIPE-3 (pending; delegated): Map then implement the smallest safe repeatable orchestration interface using existing commands and strict gates, producing isolated translated copies/patches with explicit reports and resumable failure states. No original-game unattended writes. Audit real available games read-only plus isolated copies, using free/subscription providers only; report linguistic limitations and unsupported formats honestly. Commit evidence pending.

## Acceptance
- No release-success claim when inventory shrank through missing translations, stale outputs or damaged controls.
- Strict pivot preserves every eligible extracted row; failure produces no output DB and changes no source rows.
- Successful synthetic Ren'Py/RPG Maker end-to-end outputs preserve semantic controls, apply to pristine copies, re-extract correctly, and roll back byte-identically.
- Automated interface does not inject into the original root and refuses incomplete stages, unsafe/occupied output locations and unsupported formats.
- Reports bind observed checks to commits, distinguish real/scripted providers, and do not claim native/runtime/linguistic checks that were not run.

## Exploration evidence
- `crates/core/src/database.rs:1130-1139,1161-1238`: legacy pivot selects only nonempty translations and skips untranslated inventory.
- `crates/cli/src/main.rs:1842-1856`: translation can return success while pending rows remain.
- Existing tests explicitly permit partial pivot (`database.rs:test_pivot_to_uses_translations_as_sources_and_skips_pending`, CLI `cmd_pivot_skips_pending_and_refuses_overwrite`). Preserve compatibility.
- Name-box controls already tested for English pivots; characterize full title translation before changing masking.
- Existing ground-truth and saved-matrix tools are reusable but contain historical hard-coded paths; review before executing.

## Verification and progress
PIPE-1 implemented and independently checked. Writer observed RED (core 4 failures; CLI 4 failures) then GREEN (core 24, CLI 9). Independent full workspace: 2275 passed / 0 failed / 21 ignored; frontend 439 / 0 and production build passed; fmt and whitespace passed. Strict Clippy first failed on the new negated Option predicate; equivalent `is_none_or` correction cleared Clippy, with independent CLI 9 / 0 spot check and fmt passing. `.test-tmp` contains only `.gitignore`. No native UI, real-game/provider or linguistic check yet. Authored PIPE-1 diff is 440 lines excluding task tracking: one coherent gate plus readable core/CLI coverage, kept intact despite the advisory 400-line forecast. Native assessment was unavailable because of undeclared untracked files; treated as high and separate verifier used. RDD remains off. Work-unit commit and hosted exact-SHA CI are next.

## Next step
Delegate PIPE-1 with exact edit surfaces and focused checks; assess the resulting diff, run required independent/full verification, commit and verify the work-unit candidate before beginning the next unit.
