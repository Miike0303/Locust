# Windows tag-only CI

## Intent and authorization
Add Windows checks alongside Ubuntu while preserving `v*` tag/manual-only CI. User requested sequential completion, passing tests and push to `main` before starting another task. After preflight showed162 older unpushed commits, user explicitly authorized publishing accumulated history plus this task through feature-branch hosted validation first.

Authorized: work-unit commits, local gates, feature-branch push, manual CI bound to candidate SHA, non-forced fast-forward main only after both hosted jobs pass. No tags/releases, destructive Git operations, skipped failed checks, originals-game writes, unrelated repositories or purchases.

## Scope and route
Workflow `.github/workflows/ci.yml`; test-only hosted blocker `crates/cli/tests/backup_recovery_tests.rs`; ledger `docs/IMPROVEMENT-GOAL.md`; this document/Engram `odd/windows-tag-ci/tasks`. Branch `ci/windows-tag-checks`, base `d6c1846`. Preserve Linux job, triggers, concurrency, environment and test coverage; no dependencies or release edits. Read-only explorers, bounded writers, independent verifiers, parent delivery; writes/builds serial. Feature diff below400-line heuristic; publication of older history explicitly accepted.

## Tasks
- [x] WCI-1 — Add Windows job; structural YAML/HEAD checks passed. Work unit `3c8c851`.
- [x] WCI-2 — Independent Windows local gates passed. Work unit `3c8c851`.
- [x] WCI-3 — Reconcile stale CI ledger without claiming hosted completion. Work units `3c8c851`, `b7feef9`.
- [x] WCI-5 — Full Linux prepush gate passed; publish feature branch. Remote ref confirmed at `3c8c851`.
- [x] WCI-6 — Reproduce and correct backup-listing test identity mismatch. Work unit `b7feef9`; RED/GREEN and independent Windows/Linux checks observed.
- [ ] WCI-4 — Observe both hosted jobs for corrected published SHA and deliver to main. **In progress.** Initial run failed Windows; main remains unchanged until corrected run passes.

## Acceptance
- Independent windows-latest x64 MSVC job, Node24/Rust1.94.0/rustfmt/clippy and caches; frontend npm ci/build/unit before Rustfmt/strictworkspaceClippy/workspace tests including Tauri.
- Original Ubuntu/top-level contract unchanged. Config-only change has no locally runnable hosted-job RED/GREEN; use structural/local/hosted evidence. Test correction has deterministic RED/GREEN; do not remove assertions or ignore tests.
- Both hosted jobs successful for exact candidate; recheck remote divergence before non-forced main update.

## Evidence
- Structural YAML10/10, workflow+43lines, diff check passed; CRLF preserved. Actionlint unavailable (no full Actions schema validation).
- Initial local Windows x64MSVC Node24.17/Rust1.94: frontend build/unit98/98, fmt/strictClippy, Rust1999/0/19 passed. Logs host temp `locust-windows-ci-verify`.
- Full Docker Linux Debian12/Rust1.94/Node24.21: clean npm ci/build/unit98/98, fmt/strictClippy, Rust1989/0/19 including Tauri passed. Logs `locust-windows-ci-linux-verify/run-Vc9vAB0i`. Wrapper initially exit1 due generated schemaCRLF->LF; independent byte-normalized+parsedJSON checks proved expected output only, all513 other inputs/host source unchanged. Original wrapper evidence retained, no hidden failure.
- Initial feature commit `3c8c851fcae5d9ff599a3cfcea73d554778caf9c` published non-forced. Hosted run `37545042216` exact SHA: Ubuntu all gates success(Rust1989/0/19,frontend98/98); Windows frontend/fmt/Clippy success, Rust stopped at backup recovery raw path assertion. Expected hosted operand not logged.
- WCI-6 force canonical fixture and old raw assertion produced local REDexit101; GREEN compares exactly one4-fieldnonblank row, exact backupid, prefixfreepath and canonical filesystemidentity. Restore/originalbyte assertions preserved. Production unchanged; correction commit `b7feef963080fe7dc1f09d5661bc4ee6d03c0ba5`.
- Independent correction gates: Windows backup7/7, fmt/strictworkspaceClippy and fullRust1999/0/19; isolatedLinux backup7/7. Logs `locust-windows-ci-backup-path`, `locust-windows-ci-backup-verify/run-NFSJvU`.
- Verifier incident: process-local explicit RUSTUP_TOOLCHAIN1.94 installed versioned alias on host unexpectedly. Read-only diagnosis confirmed defaultstable unchanged and compiler manifests identical; installation preserved, no automatic rollback. Functional exits remain0; future host checks use existing active toolchain.
- RDD clone-local off. Original committed CI slice assessed high; independentverification completed. Test correction assessed medium/largewriter; independent spot additionally executed.
- npm13 vulnerabilities and nonblocking frontend warnings observed; no audit/dependency changes bundled.
- Remote main last confirmed `7b367c9c041941675415a5e1152d7a151ed28daf`. Corrected hosted run/publication/main delivery still pending. No tags/releases.

## Next step
Commit bookkeeping evidence, publish corrected feature branch non-forced, dispatch CI once and bind to exact new HEAD. Keep main unchanged until both hosted jobs pass; resolve any new blocker within this task before starting another.
