# Pin the existing frontend test runner

## Intent and boundary
User authorized sequential pending-task completion and repeatedly requested continuation. Branch `build/pinned-frontend-test-runner`, base `b50875b`. Declare the already-used tsx4.23.15 as an exact development dependency and replace only `npx --yes tsx --test` with `tsx --test`; retain all38 test paths and their order. Source: `apps/desktop/package.json`, `package-lock.json`; parent ledger/task evidence only. No test-framework replacement, production dependency updates, audit-fix, Rust/UI/workflow edits, original games or releases.

## Tasks
- [x] RUN-1 — Original empty-cache runner RED observed; exact existing runner pinned, same38 files GREEN104/0/0 and build passed. Work unit `184ec7d0bbd828ecff2c31dc937f93681687c50b`.
- [x] RUN-2 — Clean owned fixture installed the exact locked graph; empty-cache suite104/0/0 and TypeScript/Vite build passed. Native medium/large-writer plan honored; no separate code adjudication. Evidence work unit `e03c4bee21ae23691b017d2d28926b0e46041482`.
- [x] RUN-3 — Published feature, observed exact hosted Linux/Windows CI and delivered main non-forced at `e03c4bee21ae23691b017d2d28926b0e46041482`. Remote ref confirmed; ledger/task evidence reconciled.

## Acceptance and risks
- Node24 compatibility: cached tsx4.23.15 supports Node>=18; baseline direct cached runner already passed104 tests. Existing suite no paths removed or filtered.
- New runner requires esbuild~0.28 versus Vite's0.25.12: generated lock may grow beyond400 lines for platform binaries. Surface generated-only review load; stop on unrelated dependency churn or broad source changes. Do not down-grade or choose a different untested runner simply to shrink the lock.
- Use existing cache/offline installation first. If required archives/metadata are missing, report precisely before any fallback fetch. No purchases or global installs. Normal generated node_modules and bounded tmp caches allowed; never delete source or user caches.
- RED must execute the original command under empty offline metadata cache and show ENOTCACHED/no tests; then GREEN runs normal local command under another empty cache, actual104/0/0 and build. Independent clean npm ci fixture checks manifest/lock completeness; hosted jobs also install cleanly.

## Evidence and next step
- Writer reproduced original REDexit1/ENOTCACHED with no tests and unchanged package files. Offline exact dev install succeeded from existing cache; no fallback network/global install/audit fix.
- GREEN with fresh empty cache:104/0/0, no npx or metadata cache creation; build passed. Same38 paths and their order byte-identical. All191 existing non-root lock records preserved;28 new records/504 generated additions only. Source manifest+2/-1. TSX4.23.15 uses nested esbuild0.28.2; Vite6.4.1 retains0.25.12.
- Initial artifact path/quoting probes failed before writes and were corrected; evidence retained under ignored `tmp/pinned-tsx-writer`. Stale Browserslist/mixed-import warnings untouched.
- Native assessment:medium configuration, large writer/runtime,525lines3paths, self-checks stand/no separate code verifier. RDDoff. reviewDue=true/slice_budget_reached; exact returned continuation relayed unchanged: `gentle-ai review status '--cwd=C:\Projects\Locust' --contract=gentle-ai.review-integration/v2 --next-transition=true`. No native review/switch changes.
- First clean fixture contains129 tracked frontend files, verified byte-identical. Offline npm ci exited1 because existing react-refresh0.17.0 archive was absent; no tests ran and source remained unchanged. Initial attempt preserved.
- Parent permitted necessary locked public-npm downloads only inside the owned temporary fixture. Clean npm ci then passed with ignore-scripts/no-audit/no-fund:114 installed packages and observed fetches matched219 integrity-bearing public-registry lock records. No global install, version churn or source edits.
- Functional verifier: actual native Windows invocation ran all38 files,104/0/0; empty cache remained empty with no_npx/_cacache or test/build fetches. An initial exit0 without results was not counted as GREEN. TypeScript5.9.3/Vite6.4.1 build passed; actual runner/Vite esbuild isolation confirmed.
- All129 fixture/source hashes and package-pair bytes match before/after install/tests/build; root only parent task doc modified. Setup quoting probe failed before writes; logs retained under host-temp `locust-pinned-tsx-verify`.
- [Hosted run37562413134](https://github.com/Miike0303/Locust/actions/runs/37562413134) succeeded at exact SHAe03c4bee21ae23691b017d2d28926b0e46041482: LinuxRust1993/0/19, Windows2003/0/19, frontend104/0/0 each. Both passed actual clean npmci, build, localtsx--test, fmt, strict Clippy and workspace Rust tests. Primary harnesses report19 ignored per OS; npm13 vulnerabilities remain untouched.
- Hosted parser corrections for escaped ANSI/interleaved Cargo headers failed twice then passed using the same saved logs; no CI rerun. Evidence: host-temp `locust-pinned-tsx-hosted` with explicit absolute artifact root.
- Fetched main, checked ancestry, fast-forwarded locally and pushed without force; remote exact successful SHA confirmed. Documentation-only closeout changes no package/code/workflow from that candidate.
- Publish passive closeout, then next bounded backlog unit. This reproducibility correction is complete; broader IO/parse localization, helpers, UI design and human-dependent runtime coverage remain separate.
