# B2 Router compatibility results

D1 upgrades `react-router-dom` and `react-router` from installed **6.30.6 to exactly 7.18.4**, preserving the declarative `BrowserRouter`, DOM imports and application source unchanged. Production npm audit is clean. The overall dependency audit remains partial: the existing development-only Typography/selector-parser finding belongs to D2; Rust scanning belongs to D3. Neither is repaired or claimed here.

## Review path

1. Review the exact dependency pin and explicit test allowlist in `apps/desktop/package.json`, then the bounded Router closure in `package-lock.json`.
2. Review `apps/desktop/src/lib/routerCompatibility.test.tsx`: 15 real-router SSR cases using the existing React/tsx stack, no DOM dependency or mocked router hooks.
3. Inspect the retained production browser differential and its route/history snapshots below. SSR alone does not prove effects, browser history or lazy error recovery.

Baseline: `a8399c3754faaed9733c0d835bbe006da24a2f10`, following permission source `d7e36a0`. Work branch: `deps/b2-router-compatibility`. No commit, ledger/index edit, CI change, Rust build/scan or publication was performed by this writer. Pre-existing ledger changes were preserved.

## Compatibility evidence (S1–S2)

All evidence paths below are relative to **`.test-tmp/b2-dependencies-evidence/router/`**, retained in the parent checkout rather than child-runtime TEMP.

| Check | Router 6 baseline | Router 7 candidate |
|---|---|---|
| Focused real-router characterization | 15 passed, 0 failed/skipped | 15 passed, 0 failed/skipped |
| Full frontend unit suite | 774 passed, 0 failed/skipped | 774 passed, 0 failed/skipped, after `npm ci` |
| TypeScript + production Vite build | Exit 0, 1,922 modules | Exit 0, 1,930 modules, after `npm ci` |
| Production Chromium flow | 37 checks, 36 captured states | Same 37 checks, all 36 states identical |
| Unexpected browser requests / uncaught page errors | 0 / 0 | 0 / 0 |
| Owned browser and HTTP server cleanup | Closed/stopped | Closed/stopped |

The application has **eight explicit path routes plus the wildcard** (not nine explicit paths): `/`, `/dashboard`, `/editor`, `/review`, `/qa`, `/speakers`, `/memory`, `/settings`, `*`. Both real-router SSR and production Chromium cover these destinations and sidebar link order. Home is active only at `/`; the nested unknown path leaves every sidebar link inactive.

The identical browser flow additionally proves:

- A held Editor chunk displays the existing Suspense loading status, then resolves normally.
- Settings normalizes missing/invalid sections to `providers`; all five other section buttons replace the current entry without changing history length/index. Back skips those tabs; Forward restores the last selected tab.
- Direct document deep links and reloads work for QA, Editor, Settings with a section query, and a nested wildcard.
- A **labelled synthetic browser-history handoff** supplies `formatPickerPath` to Welcome. Its actual effect opens the picker, clears location state by replacement, and does not replay the dialog after Back/Forward. This is not an actual extraction failure or game operation.
- An injected Memory lazy-chunk rejection displays the actual route error boundary. Sidebar navigation recovers another page; revisiting the rejected lazy module still shows the boundary; its actual Reload button recovers Memory once the fault is removed.

`browser.py` runs the unchanged production flow against `baseline-dist/` and `candidate-dist/`. The baseline's 26 production files were copied **before dependency migration and before the candidate build**; each copy has a SHA-256 manifest. `differential.json` compares route, headings, sidebar destinations/active state, history length/index/user state, selected Settings section and dialog content—not merely screenshot presence. Production CSS is byte-identical.

### Evidence entry points

| Artifact | Path |
|---|---|
| Baseline/candidate public states, request records, errors and shutdown | `baseline-browser-3/results.json`, `candidate-browser/results.json` |
| Each state's screenshot and full DOM, browser trace, HTTP server log | `baseline-browser-3/`, `candidate-browser/` |
| Actual baseline/candidate differential | `differential.json`, `browser-differential.stdout.log` |
| Retained production builds and input packages | `baseline-dist/`, `candidate-dist/`, `{baseline,candidate}-package*.json` |
| Build hashes | `{baseline,candidate}-dist-sha256.json` |
| Exact command arguments, cwd and exit code | Each `*.command.json` |
| Raw command stdout/stderr | Each `*.stdout.log`, `*.stderr.log` |
| Reproducible bounded probes | `run.py`, `snapshot.py`, `browser.py`, `graph.py`, `differential.py`, `selfcheck.py` |

Browser: installed Python 3.13.12 Playwright, Chromium **145.0.7632.6**, isolated evidence-owned profiles, English locale, 1440×1000 viewport. Node **24.17.0**, npm **11.13.0**. Browser API GET responses are explicitly synthetic empty/no-project fixtures; all unknown requests and every write/provider request are blocked and fail acceptance. No real backend, provider, game or personal browser profile was used. Expected console errors are retained: synthetic current-project HTTP404 and deliberately failed lazy-chunk fetches. This is production HTTP frontend evidence, **not native Tauri, populated-project, real-game, provider or human usability certification**.

## Dependency and audit evidence (S3)

| Command | Baseline | Candidate |
|---|---|---|
| `npm --prefix apps/desktop audit --omit=dev --json` | Exit **1**, 2 moderate affected package nodes | Exit **0**, 0 advisories |
| `npm --prefix apps/desktop audit --json` | Exit **1**, 4 moderate affected package nodes | Exit **1**, 2 moderate affected package nodes |
| `npm --prefix apps/desktop ci` | Not repeated before migration | Exit **0**, lock SHA-256 unchanged |
| `npm --prefix apps/desktop ls react-router-dom react-router --json` | Installed baseline captured in lock | Exit **0**, both exactly **7.18.4**, no overrides |

Raw audits: `{baseline,candidate}-audit-{prod,full}.stdout.log`; each matching `*.command.json` retains the actual audit exit rather than masking it with a successful wrapper exit.

The Router upgrade removes findings GHSA-wrjc-x8rr-h8h6 and GHSA-337j-9hxr-rhxg from the audit. Full audit still reports GHSA-rj75-hqrm-r3gf through unchanged `@tailwindcss/typography` **0.5.20** → `postcss-selector-parser` **6.0.10**. No force fix, Typography downgrade, parser override, unrelated refresh or standalone tool installation was performed. The original B2 audit is not closed.

`graph-before-ci.json` and `graph-after-ci.json` verify **216 package records unchanged**. The only changed lock records are the root Router requirement, `react-router-dom`, `react-router`, removed `@remix-run/router` 1.23.4, and newly required `cookie` 1.1.1 / `set-cookie-parser` 2.7.2. Router 7 requires Node >=20; this run uses Node 24. The lock SHA-256 before and after `npm ci` is `acc5012e7211e0deed6eb0880142bb606d9c2b879d62a8a7c24398ff3765843a`.

## Exact execution and exceptions

All commands ran sequentially in the foreground through `py -3.13 .test-tmp/b2-dependencies-evidence/router/run.py <phase> <command...>`. The wrapper records the exact child command, separate output streams and actual exit. Relevant commands:

- Focused baseline/candidate: `node apps/desktop/node_modules/tsx/dist/cli.mjs --tsconfig apps/desktop/tsconfig.json --test apps/desktop/src/lib/routerCompatibility.test.tsx` — both exit 0 (`baseline-characterization-2`, `candidate-characterization`).
- Full suite: `npm --prefix apps/desktop run test:unit` — baseline and post-ci candidate exit 0 (`baseline-unit`, `candidate-unit`).
- Builds: `npm --prefix apps/desktop run build` — both exit 0 (`baseline-build`, `candidate-build`). Existing mixed static/dynamic API import and >500 kB chunk warnings remain; candidate main JS is 554.30 kB versus 538.93 kB baseline (Vite decimal sizes).
- Migration: `npm --prefix apps/desktop install --save-exact react-router-dom@7.18.4` — exit 0 (`router-install`).
- Browser: `py -3.13 .test-tmp/b2-dependencies-evidence/router/browser.py baseline 3` and `py -3.13 .test-tmp/b2-dependencies-evidence/router/browser.py candidate` — exit 0 (`baseline-browser-3`, `candidate-browser`).
- Differential: `py -3.13 .test-tmp/b2-dependencies-evidence/router/differential.py` — exit 0 (`browser-differential`).
- Build retention: `py -3.13 .test-tmp/b2-dependencies-evidence/router/snapshot.py baseline` and `py -3.13 .test-tmp/b2-dependencies-evidence/router/snapshot.py candidate` — exit 0.
- Graph verification: `py -3.13 .test-tmp/b2-dependencies-evidence/router/graph.py before-ci` and `py -3.13 .test-tmp/b2-dependencies-evidence/router/graph.py after-ci` — exit 0.
- Before/after both build batches: `powershell -NoProfile -File tools/cleanup-scratch.ps1 -Apply -MinAgeHours 876000 -Prefix locust-owned-b2-20261021-` — all four exit 0, freed 0 GB. Empty worktree-prune preview observed before each batch; target sizes below cleanup caps. Evidence preserved.

**Test-first exception:** this dependency-only compatibility migration has no meaningful new product RED. New characterization passed on Router 6 before the upgrade and remained green on Router 7. Audit supplies the observed RED→GREEN production security signal. Do not interpret the following harness failures as product RED:

- Initial focused command lacked the frontend tsconfig and failed 15 cases with `React is not defined`; its recorder also hit Windows console encoding. Raw output and child exit 1 remain under `baseline-characterization.*`. The corrected command explicitly supplies tsconfig; recorder output is UTF-8.
- Initial browser attempt timed out at Home because the fresh profile inherited the OS locale. Second attempt passed 11 states, then timed out on a harness selector named “Defaults” rather than the actual “Translation Defaults”. Both exit-1 attempts remain in `baseline-browser/`, `baseline-browser-2/` and matching logs. Final baseline and candidate both use the corrected **same** English-locale flow.

Self-review: S1 is limited to the authorized Router migration; S2 preserves the actual route/navigation contracts with SSR plus browser evidence; S3 preserves the unrelated dependency graph and reports, rather than hides, residual audit failures. `verification-summary.json` indexes command/exit evidence, source-scope checks, script hashes and review size.

No final characterization/browser case was skipped or failed. The required full npm audit remains exit 1 for the pre-existing, explicitly deferred D2 finding; this is not a fully clean dependency-audit handoff. Parent owns independent review, full delivery gates, exact-SHA Windows/Ubuntu CI, ledger/index/memory updates and Git actions.
