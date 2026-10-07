# Pin the existing frontend test runner

## Intent and boundary
User authorized sequential pending-task completion and repeatedly requested continuation. Branch `build/pinned-frontend-test-runner`, base `b50875b`. Declare the already-used tsx4.23.15 as an exact development dependency and replace only `npx --yes tsx --test` with `tsx --test`; retain all38 test paths and their order. Source: `apps/desktop/package.json`, `package-lock.json`; parent ledger/task evidence only. No test-framework replacement, production dependency updates, audit-fix, Rust/UI/workflow edits, original games or releases.

## Tasks
- [ ] RUN-1 — Reproduce runner RED with empty offline cache, pin the existing runner and observe GREEN with the same38 files. **In progress.** Bounded writer; config behavior is testable by exit status and actual execution, not a contrived compile failure.
- [ ] RUN-2 — Independently verify dependency/version scope, clean installation, empty-cache tests and build. Follow native assessment; no implicit review switch changes.
- [ ] RUN-3 — Commit, publish feature, observe exact hosted Linux/Windows CI and deliver main before another unit; reconcile ledger/task evidence.

## Acceptance and risks
- Node24 compatibility: cached tsx4.23.15 supports Node>=18; baseline direct cached runner already passed104 tests. Existing suite no paths removed or filtered.
- New runner requires esbuild~0.28 versus Vite's0.25.12: generated lock may grow beyond400 lines for platform binaries. Surface generated-only review load; stop on unrelated dependency churn or broad source changes. Do not down-grade or choose a different untested runner simply to shrink the lock.
- Use existing cache/offline installation first. If required archives/metadata are missing, report precisely before any fallback fetch. No purchases or global installs. Normal generated node_modules and bounded tmp caches allowed; never delete source or user caches.
- RED must execute the original command under empty offline metadata cache and show ENOTCACHED/no tests; then GREEN runs normal local command under another empty cache, actual104/0/0 and build. Independent clean npm ci fixture checks manifest/lock completeness; hosted jobs also install cleanly.

## Evidence and next step
Current package/lock are unchanged from prior ENOTCACHED observation. Explorer found no declared tsx dependency, exact cached4.23.15, and existing Vite/esbuild0.25.12 incompatibility with new runner range. No configuration edits or new RED/GREEN yet. Next: writer with explicit generated-artifact surfaces; parent owns scope decisions and delivery.
