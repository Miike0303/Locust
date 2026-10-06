# Windows tag-only CI

## Intent and authorization
Add Windows checks alongside Ubuntu without changing `v*` tag/manual-only CI. User subsequently requested sequential completion with passing tests and push to `main` before the next task. After preflight showed 162 older unpushed commits, the user explicitly selected publishing the accumulated history plus this task, with feature-branch hosted validation before `main`.

Authorized: work-unit commits, full local checks, temporary feature-branch push, manual CI on that exact candidate, and non-forced fast-forward delivery to `main` only after all gates pass. Not authorized: tags/releases, destructive Git operations, skipping failed checks, original-game writes, unrelated repository edits or purchases.

## Scope and route
Implementation: `.github/workflows/ci.yml`; hosted-blocker regression: `crates/cli/tests/backup_recovery_tests.rs`; ledger: `docs/IMPROVEMENT-GOAL.md`; tracking: this file and Engram `odd/windows-tag-ci/tasks`. Branch `ci/windows-tag-checks`; original base `d6c1846`. Linux source verification uses an isolated scratch checkout; no concurrent cargo builds. Preserve Ubuntu job, concurrency and environment; no branch-push/PR triggers, dependencies, suppressions, artifacts or release edits.

Route: read-only explorer, bounded writer, independent verifier, parent delivery. Approximately 60 workflow/ledger diff lines; delivery strategy ask-on-risk. Prior accumulated history publication was explicitly accepted, not inferred from this small diff.

## Tasks
- [x] WCI-1 — Add and structurally check Windows CI. Delegated writer; local outcome verified.
- [x] WCI-2 — Independent local Windows gates. YAML assertions, frontend/Rust gates passed.
- [x] WCI-3 — Reconcile stale CI backlog. Current ledger explicitly distinguishes local implementation from hosted validation.
- [x] WCI-5 — Verify Linux prepush gates, commit candidate and publish the feature branch. Commit `3c8c851`; remote feature ref confirmed at the same full SHA after non-forced push.
- [ ] WCI-6 — Correct the hosted Windows backup-listing regression with deterministic RED/GREEN. **In progress.** Test-only scope; validate filesystem identity rather than exact displayed path spelling. Preserve restore/byte assertions.
- [ ] WCI-4 — Observe both GitHub CI jobs for the corrected candidate SHA, then deliver to `main`. **Blocked pending WCI-6.** Do not mark complete before hosted success and remote-ref confirmation.

## Acceptance
- Exactly original tag/manual triggers and unchanged Ubuntu job; independent `windows-latest` x64 MSVC job.
- Node 24 and Rust 1.94.0/rustfmt/clippy, npm and Rust caches; frontend `npm ci`, build and unit tests before Rust fmt, strict workspace Clippy and tests.
- Local Windows plus full Linux checks including Tauri/frontend. No meaningful hosted-job RED/GREEN is locally runnable; use structural and functional verification honestly.
- Hosted Linux and Windows jobs pass for the published candidate SHA. No forced pushes; recheck remote divergence before `main` delivery.

## Evidence
- WCI-1: workflow +43 lines; parsed YAML/HEAD comparison passed; original top-level contract and Ubuntu job unchanged; CRLF preserved. Diff check passed.
- WCI-2: Windows x64/MSVC, Node24.17.0/Rust1.94.0; frontend build passed, 98 unit tests passed/0 failed; fmt and strict Clippy passed; Rust1999 passed/0 failed/19 ignored. Logs in host temp `locust-windows-ci-verify`. Nonblocking Browserslist/import warnings only.
- WCI-3: ledger CI entry updated; tracked diff46 additions/1 deletion. No unrelated edits.
- Preflight: fetched origin/main successfully; 162 ahead/0 behind; remote main `7b367c9`; public repository `Miike0303/Locust`, default `main`, active CI workflow; gh authentication present; Linux Docker engine reachable. Reachability is not a Linux test pass.
- Native assessment unavailable due undeclared untracked task doc; conservative independent verifier used. RDD clone-local off; no native review started.
- Full isolated Linux gate: Debian12 Bookworm, Rust1.94.0/Node24.21.0; clean npm install, build and frontend98/98 passed; fmt/strictClippy passed; workspace including Tauri1989 passed/0 failed/19 ignored. Logs: host temp `locust-windows-ci-linux-verify/run-Vc9vAB0i`.
- Initial wrapper exit1 flagged generated `gen/schemas/desktop-schema.json` CRLF-to-LF rewrite. Separate incident exploration and independent byte/JSON comparison confirmed expected generated output only; all513 other baseline paths and host source hashes unchanged. Functional gates all exit0; original integrity exit1 retained and explained. No builds remain running.
- `actionlint` unavailable; parsed YAML is not full Actions schema validation. Initial hosted run `37545042216` for3c8c851: Ubuntu success (Rust1989/0/19, frontend98/98); Windows frontend/fmt/Clippy passed but Rust failed before complete workspace coverage. npm reported13 vulnerabilities; no dependency/audit changes authorized in this unit.
- Work-unit commit `3c8c851fcae5d9ff599a3cfcea73d554778caf9c` (`ci: check Windows on version tags and manual runs`) includes workflow, ledger and task document. Non-forced feature push succeeded; remote branch exact SHA confirmed. Remote main remains `7b367c9c041941675415a5e1152d7a151ed28daf`.
- Committed-slice assessment reports high risk (`shell_source` in CI YAML); RDD off. Required independent verifier already completed Windows, Linux and structural checks. No native review started.

- Hosted Windows blocker: `cli_can_list_and_restore_an_injection_backup` raw `game.display()` substring assertion at `crates/cli/tests/backup_recovery_tests.rs:198`. Listing intentionally uses presentation-normalized canonical paths; exact hosted fixture spelling was not logged. Separate read-only diagnosis proposes force canonical Windows fixture path for deterministic RED, then parse listing fields and compare canonical filesystem identity. No production path fix justified by current evidence.

## Next step
Run WCI-6 RED/GREEN and independent checks, commit correction on feature branch, then dispatch a new hosted CI run bound to its exact SHA. Keep main unchanged while any job fails. Do not skip assertions or suppress tests.
