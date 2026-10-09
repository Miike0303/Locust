# Resume verified Direct-injected projects

## Intent
User selected `resume_verified_project`: when opening a folder with a verifiable Locust Direct injection and matching saved database, offer resume as the default; keep refresh explicit. Resume preserves currently saved sources, translations and approvals, not reconstruction of already overwritten sources.

User selected delivery `auto-chain`, `chain_strategy=feature-branch-chain`: core verification first, then transport/shared desktop UI, on one feature branch. No PR is created by this choice. Base/main `68c4392806b8f4a1cd1ff6e3e207b95a410d74af`; branch `feat/direct-project-resume`. Main remains intact while both units are built. Any publication follows separately authorized repository policy, never this shape selection alone.

## Boundary and workload
Initial forecast 550-850 authored diff lines; core alone observed 825 (795 additions/30 deletions, about 500 test lines), so forecast revised to 1150-1600 for both slices. Keep the selected feature-branch chain and two cohesive units; do not omit tests or cosmetically shrink. Track actual counts per commit. No original-game edits, credentials, live uploads, native desktop control, purchases, releases, or unrelated repo changes. One foreground writer; English artifacts and neutral Spanish UI.

Do not suppress ordinary source-change detection or stale markers. Explicit DB/recent opening keeps existing semantics. Queue must not translate on canceled choice. Unverifiable/ambiguous evidence stays visible and never silently falls back to extraction. No complete pristine-backup integrity requirement for saved-data resume; later injection retains its independent checks.

## Evidence and design
Fresh independent synthetic reproduction on base: approved Japanese actor source + English translation survives saved DB/recent opening. Ordinary folder open extracts injected English as source, resets pending/stale, retains translation/provider. Reinjection refuses; game/backup/recording hashes unchanged. Evidence host-temp locust-direct-reopen-investigation-ma0205cc.

Existing candidate enumeration and read-only DB probe in core project.rs; get_injection validates recordings, but lacks mode/format. Committed generation ownership provides mode/format/path/hash/size/language. Revised Direct recordings may span active generations. Preflight is advisory; verified resume revalidates under the existing source lock. Avoid treating status metadata alone as proof; verify recorded physical members. Read-only SQLite may create sidecars; do not use immutable mode that ignores active WAL. Only Direct with exact matching DB/root/format, output membership and bytes qualifies; no Add/Replace/legacy inference.

## Tasks
- [x] RES-1 — Core preflight and locked verified resume with deterministic tests; ordinary folder behavior unchanged. Work unit `b351eb4b0f1af49ab6cf5dd8d488694337f9c9dd`. Delegated writer and independent safety verifier; source/test825 lines plus task document38. Review slice1.
- [x] RES-2 — HTTP/Tauri adapters and shared accessible Resume/Refresh/Cancel choice before extraction; Welcome, hotkey and queue use common flow, explicit saved/recent unchanged. Delegated writer (multi-file); pure flow/transport tests and browser functional checks. Review slice2.
- [ ] RES-3 — Verify whole feature, current evidence/docs/ledger and authorized delivery. Delegated command verifier/ledger writer; parent Git. Applicable full tests/builds, exact hosted candidate before main if publication authorization applies.

## Acceptance and checks
- Verified Direct root with sole valid matching DB and committed nonpending ownership offers resume. Resume retains stored row source/status/provider/warnings and returns zero merge counters without extraction.
- Reject foreign roots, distinct DB candidates, malformed provenance, pending/restored/unknown/Add/Replace ownership, missing/modified/unsafe/duplicate outputs, and mismatched format. Preserve alias dedup and revised generation unions.
- Recheck current files on confirmation; drift after preflight refuses without extraction or DB mutation.
- Resume is initial focus/default; Refresh explicitly explains reads current files and may reset source approval; Escape/Cancel changes nothing and aborts queued translation.
- Both transports serialize same core result and update current-project/config consistently. Existing APIs remain compatible.
- Test-first meaningful RED before behavior; GREEN plus alternate cases. Passive docs no meaningful RED, structural checks instead. No lifecycle evidence invented.
- Scoped unit checks first, then workspace fmt/strict Clippy/Rust tests, frontend unit/type/build, headless browser with synthetic/mock seams clearly distinguished. Native Tauri and real games not claimed.

## Progress
RES-1 implemented, independently checked and committed as b351eb4; main remains68c4392. Writer observed meaningful runnable RED0/2 (preflight returned Extract), then integration GREEN6/0 with20negative cases. Writer project28/0, extraction70/0/1ignored, transaction39/0/1ignored, fmt/Clippy green. Independent integration6/0 and core library601/0/2ignored, fmt/Clippy/diff checks green; no causal source defect found. Native assess medium/large/RDDoff, writer self-verification stands; independent safety spot check performed voluntarily.

Verifier incident: existing indexed_path_union_preserves_alias_identity_and_order uses tempdir_in(current_dir), briefly creating repository-local fixture outside delegated output bounds. Parent inspected unchanged staged scope/no unstaged changes and zero retained .tmp dirs in repo/core; no cleanup or source mutation, incident closed. Future suite authorization must include operation-owned ephemeral test fixtures. Coverage limits: replace_copy negative equals foreign-root mutation, not actual Replace flow; no dedicated new symlink/single-file resume integration case, existing guard tests pass. No full workspace/transport/UI/native/live-game checks yet.

RES-2 implemented by a delegated writer (18 files, +1199/-38): POST /api/project/preflight and /api/project/resume, equivalent Tauri commands, shared successful-open state/config path, app-wide ResumeProjectDialog (Resume initial focus, Refresh explicit, Escape/Cancel no change), Welcome/hotkey/queue through one folder flow; needs_attention evidence offers only Refresh/Cancel. Writer RED: HTTP 404 on 4 tests, 8 flow and 6 queue tests failing before behavior; GREEN. Parent rerun: fmt, strict Clippy, locust-server + locust-desktop all green, core direct_project_resume 6/0, tsc, frontend 148/0, build. Not verified: native Tauri focus, live-backend browser, real games.

## Next step
RES-3: full workspace tests, push branch, hosted CI on the exact SHA, fast-forward main only if green.
