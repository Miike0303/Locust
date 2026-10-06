# Windows tag-only CI

## Outcome and authorization
**Complete: implemented, verified on both hosted platforms, and delivered to main.** User requested sequential task completion with passing tests and main delivery. Publication of 162 older local commits plus this unit was explicitly confirmed after preflight.

Authorized delivery used work-unit commits, a feature-branch push, manual CI on the exact candidate, and a non-forced fast-forward to main. No tags, releases, skipped failing tests, destructive Git operations, original-game writes, or unrelated repository edits were performed.

## Scope and routing
- Workflow: `.github/workflows/ci.yml`.
- Hosted-blocker regression: `crates/cli/tests/backup_recovery_tests.rs` (test only).
- Evidence: `docs/IMPROVEMENT-GOAL.md`, this file, Engram `odd/windows-tag-ci/tasks`.
- Feature branch: `ci/windows-tag-checks`; original base: `d6c1846`.
- Explorers mapped prerequisites and incidents; bounded writers implemented; independent verifiers ran checks; parent owned delivery. Writes/builds were serial. The feature remained below the 400-line review heuristic; accumulated-history publication was separately authorized.

## Tasks
- [x] WCI-1 — Add independent Windows CI without altering Linux or triggers. Work unit `3c8c851`.
- [x] WCI-2 — Independently validate YAML and local Windows gates. Evidence included with `3c8c851`.
- [x] WCI-3 — Reconcile the stale backlog and final delivery evidence. Work units `3c8c851`, `b7feef9`, and documentation closeout.
- [x] WCI-5 — Pass the full Linux prepush gate and publish the feature branch. Commit/ref `3c8c851` verified.
- [x] WCI-6 — Reproduce and correct the Windows backup-listing test. Work unit `b7feef9`; deterministic RED/GREEN and independent Windows/Linux checks passed.
- [x] WCI-4 — Observe both hosted jobs for exact candidate `027b82e84849f37f53fdb4447a27b4158dad4ecb`, then deliver that SHA to main. Both jobs and remote-ref confirmation succeeded.

## Acceptance and observed evidence
- CI remains `v*` tag/manual only; Ubuntu job, concurrency and global environment preserved. Independent Windows x64 MSVC job uses Node 24, Rust 1.94.0 with rustfmt/clippy and caches. Frontend install/build/tests precede Rust checks, including Tauri.
- Parsed YAML assertions passed 10/10. Configuration has no locally runnable hosted-job RED/GREEN; structural and functional evidence was used instead. `actionlint` was unavailable; actual hosted execution covers the executable workflow.
- Local Windows: frontend build and 98 tests passed; Rust fmt/strict Clippy passed; 1999 Rust passed, 0 failed, 19 ignored.
- Full isolated Debian 12 Linux gate: clean npm install/build and 98 frontend tests passed; fmt/strict Clippy passed; 1989 Rust passed, 0 failed, 19 ignored, including Tauri. Initial wrapper exit 1 flagged a generated Tauri schema CRLF-to-LF rewrite; independent byte-normalized and JSON checks confirmed identical content and all 513 other inputs unchanged. Original diagnostic evidence retained.
- Initial hosted run `37545042216` at `3c8c851` passed Ubuntu but failed Windows on raw backup-path spelling. A canonical fixture reproduced RED (exit 101). Correction parses exactly one four-field nonblank row, verifies exact backup ID, prefix-free display and canonical directory identity. Restore/original-byte assertions remain; production unchanged. Independent Windows focused 7/7 and full 1999/0/19, Linux focused 7/7 passed.
- Corrected hosted run [37547390292](https://github.com/Miike0303/Locust/actions/runs/37547390292), exact SHA `027b82e84849f37f53fdb4447a27b4158dad4ecb`: Ubuntu **1989/0/19**, Windows **1999/0/19**, frontend **98/98 on each**. Both completed npm ci/build/unit, fmt, strict workspace Clippy and workspace tests successfully. Ignored tests were not executed.
- Fetched remote main, verified ancestry, fast-forwarded locally and pushed without force. Remote main confirmed at the exact successful candidate SHA. Documentation-only closeout does not change workflow or code from that tested candidate.

## Limits and incidents
- npm reported 13 vulnerabilities; no dependency/audit fix was bundled. Nonblocking frontend/action-runtime warnings remain.
- A verifier's process-local exact toolchain alias triggered an unintended host rustup install. Diagnosis confirmed default stable unchanged and identical compiler manifests; installation retained, no destructive rollback. Future host checks use the existing active toolchain.
- RDD remained clone-local off. CI assessment requested independent verification; completed. No native review started.
- Logs are retained under host-temp `locust-windows-ci-verify`, `locust-windows-ci-linux-verify`, `locust-windows-ci-backup-path`, `locust-windows-ci-backup-verify`, and `locust-windows-ci-hosted-verify`.

## Next step
Publish the documentation-only closeout, then start the next bounded Locust backlog unit. No remaining Windows CI implementation or hosted-validation task is pending.
