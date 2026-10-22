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

---

# D2: scoped Typography/parser compatibility results

**Delegated local checkpoint (superseded by the final delivery section below):** D2 implementation and delegated local verification pass. The only dependency change is the root-manifest override `@tailwindcss/typography` → `postcss-selector-parser` **exactly 7.1.6**, replacing installed 6.0.10. Typography remains **0.5.20** and both Router packages remain **7.18.4**. Fresh full and production npm audits now exit **0**, with zero findings at every severity. The preceding D1 report is preserved as historical provenance, including its then-valid residual audit failure.

Baseline: **`6d3d3b99a466973fa992e28e231513db09eba20a`** on `deps/b2-parser-compatibility`. The parent supplied D1 delivery to main after Windows/Ubuntu CI **39664242232**; this worker did not re-query that remote proof. D2 does **not** close all of B2: at this delegated checkpoint, independent Risk5 verification, parent full gates, exact-source two-platform CI and delivery remained parent-owned; those subsequently passed as recorded below. D3 Rust scanning remains blocked on approved scanner availability. No Rust scan, commit or publication is claimed.

## Review path and exact scope

1. `apps/desktop/package.json`: one ancestor-scoped override, plus the explicit `typographyCompatibility.test.ts` unit-test allowlist entry. No global parser override or direct dependency addition.
2. `apps/desktop/package-lock.json`: **six changed lines**, only parser version/resolved URL/integrity. No additional transitive closure, package addition/removal, Typography downgrade or unrelated refresh.
3. `apps/desktop/src/lib/typographyCompatibility.test.ts`: five public-plugin/Tailwind generation tests. No Git/history lookup, fixed baseline SHA, new dependency or permanent golden fixture.
4. This appended D2 report and retained **`.test-tmp/b2-dependencies-evidence/parser/`** evidence only. The pre-existing parent-owned ledger modification was not edited. No application styles/components/router, Tailwind configuration, CI or Rust source changed.

## Public generation and CSS proof (S1–S2)

The new tests actually compile the installed public Typography plugin through Tailwind's public `compile` API, loading the installed `tailwindcss/theme.css` and public plugin/config modules. **67 candidates** cover default prose, all five explicit sizes (`sm`, `base`, `lg`, `xl`, `2xl`), all 22 color modifiers, invert/dark invert, 33 element variants, responsive size and both hover/element-variant orders. A public theme extension additionally exercises:

- Common and different pseudo-element tails, multi-pseudo chains and unequal pseudo presence.
- Pseudo-classes, nested `:is`, attribute values containing commas and escaped class names.
- Leading child selectors, low-specificity `:where`, and `not-prose` exclusions.
- Public custom `className` and legacy `target` options, generated separately in full.

The installed parser changelog documents **7.0.0: “make insertions during iteration safe (major)”**, and 7.1.6's flat-selector CPU-exhaustion fix. Inspection of the retained Typography utility shows it iterates copied/reversed child arrays and a separate pseudo matrix, removes common tails, and prepends into a separate selector; it does not rely on inserting into the same container being walked. The tests protect observable generated output, not parser 6's iteration implementation. Unequal pseudo presence deliberately characterizes existing plugin output; it is not a new selector-correctness guarantee.

**Actual complete CSS files were compared with `read_bytes()` equality**, not just hashes, normalized text, snapshots or assertions that a test exists. No output was filtered or normalized. Both phases use the exact same retained input CSS/config/candidates/theme/test source and unchanged plugin source.

| Complete generated output | Bytes per phase | Baseline/candidate SHA-256 (also directly byte-compared) |
|---|---:|---|
| Modern plugin | 75,819 | `831f342fe878fa2dae6c9a6763eab1c7c12376d74483cb883ad0d565d48fbf2d` |
| Custom-class plugin | 77,697 | `f93fb042a17470e05d4e4df08c0340f1a71a1fa22df1c585b415850b2cf875f9` |
| Legacy plugin | 48,712 | `33921f109643dc1919c1abcb0427fc925515dc4b7d73febc447df78904cf4e17` |
| Production `assets/index-C4B2ybWW.css` | 40,767 | `604b51392733b925762a5f58999db295355e97af41a51845a103886ca53ac59f` |

All **26** production files, including JavaScript and HTML, are byte-identical as well. The retained D1 production CSS was additionally compared directly to the D2 baseline: identical, so adding the characterization test did not alter shipped CSS. The application stylesheet currently does not activate Typography with `@plugin`; therefore production CSS equality alone would be insufficient. The separate real plugin compilation is essential evidence, not an inferred production exercise of the plugin.

## Sequential checks and audit/graph evidence (S3)

| Check | Baseline parser 6.0.10 | Candidate parser 7.1.6, after clean `npm ci` |
|---|---|---|
| Focused public generation tests | **5 passed / 0 failed / 0 skipped**, exit 0 | **5 / 0 / 0**, exit 0 |
| Full frontend unit suite | **779 / 0 / 0**, exit 0 | **779 / 0 / 0**, exit 0 |
| TypeScript + production Vite build | Exit 0; 1,930 modules | Exit 0; 1,930 modules |
| Full npm audit JSON | Exit **1**; **2 moderate** package nodes, only Typography/parser | Exit **0**; **0 info/low/moderate/high/critical** |
| Production npm audit JSON | Exit **0**; all severities zero | Exit **0**; all severities zero |
| Installed dependency tree | Typography 0.5.20 → parser 6.0.10 | Typography 0.5.20 → parser **7.1.6**, `overridden: true` |
| Full lock graph | 221 records including root | **220 records unchanged**, parser record only changed; no additions/removals |

The baseline full audit retains GHSA-rj75-hqrm-r3gf and npm's unacceptable suggested Typography downgrade to 0.5.4; neither the finding nor exit 1 was hidden. Candidate full and production audits were freshly requested separately after `npm ci`, not inferred from install output. Registry integrity matches the new lock record. `cssesc` **3.0.0** and `util-deprecate` **1.0.2** remain unchanged; no extra parser transitive migration was necessary.

`graph-before-ci.json` compares the saved post-install/pre-ci snapshot with baseline; only `graph-after-ci.json` and the actual byte differential establish clean-install lock preservation. Every field in the other 220 records is identical, not merely package versions. Of D1's original 216 unrelated records, **215 remain exact** and the one now-authorized exception is parser; the D1 Router closure is unchanged too. npm does not duplicate the manifest's scoped override into the lock's root record. Candidate lock SHA-256 before/after `npm ci`: **`a7a034ab8e4d71c1f68b64db5a50ed48cd769961a9bc79d6b4051422b1b66fec`** (baseline `acc5012e7211e0deed6eb0880142bb606d9c2b879d62a8a7c24398ff3765843a`).

### Exact commands, retained streams and exits

Every command below ran sequentially in the foreground through `py -3.13 .test-tmp/b2-dependencies-evidence/parser/run.py <phase> <command...>`. Each phase has its own `*.command.json` (exact argv, cwd, actual exit, duration, timeout status), `*.stdout.log` and `*.stderr.log`. Evidence is retained in the parent checkout, never solely child-runtime TEMP. Tool versions: Node **24.17.0**, npm **11.13.0**, Python **3.13.12**; Tailwind and its PostCSS integration **4.2.1**, PostCSS **8.5.31**, Vite **6.4.4**.

| Exact child command | Phases / observed exits |
|---|---|
| `node apps/desktop/node_modules/tsx/dist/cli.mjs --tsconfig apps/desktop/tsconfig.json --test apps/desktop/src/lib/typographyCompatibility.test.ts` | `baseline-characterization-corrected`, `candidate-characterization`: **0**, 5 tests each |
| `node apps/desktop/node_modules/tsx/dist/cli.mjs --tsconfig apps/desktop/tsconfig.json .test-tmp/b2-dependencies-evidence/parser/generate.mts baseline` | `baseline-plugin-generation`: **0**, complete files and 5 tests |
| `node apps/desktop/node_modules/tsx/dist/cli.mjs --tsconfig apps/desktop/tsconfig.json .test-tmp/b2-dependencies-evidence/parser/generate.mts candidate` | `candidate-plugin-generation`: **0**, complete files and 5 tests |
| `npm --prefix apps/desktop run test:unit` | `baseline-unit`, `candidate-unit`: **0**, 779 tests each |
| `npm --prefix apps/desktop run build` | `baseline-build`, `candidate-build`: **0** |
| `npm --prefix apps/desktop audit --json` | `baseline-audit-full`: **1**; `candidate-audit-full`: **0** |
| `npm --prefix apps/desktop audit --omit=dev --json` | `baseline-audit-prod`, `candidate-audit-prod`: **0** |
| `npm --prefix apps/desktop view postcss-selector-parser@7.1.6 version engines dependencies dist.integrity --json` | `parser-registry`: **0** |
| `npm --prefix apps/desktop install --ignore-scripts` | `parser-install`: **0**, one package changed after manifest-scoped override |
| `npm --prefix apps/desktop ci` | `candidate-ci`: **0**, lock unchanged |
| `npm --prefix apps/desktop ls @tailwindcss/typography postcss-selector-parser react-router-dom react-router cssesc util-deprecate --json` | `baseline-tree`, `candidate-tree`: **0** |
| `py -3.13 .test-tmp/b2-dependencies-evidence/parser/snapshot.py baseline` | `baseline-snapshot`: **0** |
| `py -3.13 .test-tmp/b2-dependencies-evidence/parser/snapshot.py candidate-before-ci` | `candidate-before-ci-snapshot`: **0** |
| `py -3.13 .test-tmp/b2-dependencies-evidence/parser/snapshot.py candidate` | `candidate-snapshot`: **0** |
| `py -3.13 .test-tmp/b2-dependencies-evidence/parser/graph.py before-ci` | `graph-before-ci`: **0** |
| `py -3.13 .test-tmp/b2-dependencies-evidence/parser/graph.py after-ci` | `graph-after-ci`: **0** |
| `py -3.13 .test-tmp/b2-dependencies-evidence/parser/differential.py` | `css-and-audit-differential`: **0**, direct bytes and audit assertions |
| `node .test-tmp/b2-dependencies-evidence/parser/installed-v2.mjs` | `installed-resolution-corrected`: **0**, parser resolved from actual Typography entry point |
| `npm --version` / `py -3.13 --version` | `npm-version`, `python-version`: **0** |
| `git worktree prune --dry-run` | `{baseline,candidate}-{pre,post}-prune`: **0**, all four previews empty |
| `powershell -NoProfile -File tools/cleanup-scratch.ps1 -Apply -MinAgeHours 876000 -Prefix locust-owned-b2-20261021-` | `{baseline,candidate}-{pre,post}-cleanup`: **0**, all four freed **0 GB**; retained proof/backups preserved; repo/shared targets 5.97/0.91 GB, below caps |

`git diff --check` (`final-diff-check-corrected`) and `py -3.13 .test-tmp/b2-dependencies-evidence/parser/selfcheck.py` (`selfcheck`) provide final whitespace/scope/provenance/evidence-index checks; their observed exits are retained beside all other commands. `verification-summary.json` indexes complete file hashes, command records, direct byte comparisons and graph evidence. The selfcheck archives the original D1 report from local HEAD and verifies it remains a prefix of this report (allowing documentary checkout line endings only, never normalizing CSS).

Existing mixed static/dynamic API import and >500 kB chunk warnings remain in both build stderr streams. They were not repaired or suppressed.

### Test-first exception, failures and limits

This dependency-only compatibility repair has no meaningful new product RED: baseline passing preservation tests are required, not fabricated failures. The honest security signal is **full audit exit 1 → exit 0**, with baseline two moderate nodes → zero. Public generation tests pass before and after the override; negative/alternate selectors, exclusion, custom options and full byte equality triangulate compatibility.

All failed harness checks remain visible:

- `baseline-characterization` exited **1** (3 pass/2 fail): initial expectations incorrectly assumed every palette defines body color and that parser serialization removes comma spaces. The actual public output established links as the common palette property and preserved spaces. `baseline-diagnostic` ran the same generator with `baseline-diagnostic`, retained complete original test/output files, and likewise exited **1**. Corrected expectations then passed **before any dependency edit**; these are not product RED.
- `node .test-tmp/b2-dependencies-evidence/parser/installed.mjs` (`installed-resolution`) exited **1** because `@tailwindcss/postcss` does not export its `package.json` subpath. The retained `installed-v2.mjs` reads known installed manifest files and independently resolves the parser from Typography; corrected exit **0**. No dependency/source accommodation was made for this evidence-harness mistake.

- Initial `git diff --check` (`final-diff-check`) exited **2** for one newly added blank line at the report's EOF. That whitespace-only issue was removed; the corrected check is separately retained. Git also emitted the existing checkout LF→CRLF warnings for this report and the parent-owned ledger; no ledger edit was made.

No command timed out or was automatically retried. No final focused/full-suite case failed, skipped or was cancelled. No browser/Playwright run was needed for the parser-only change with unchanged complete production artifacts; no new D2 browser/native/runtime certification is implied. No Cargo command, scanner install/scan, real game, provider, native UI, external profile or secret was accessed. Parent independent Risk5 verification, full delivery gates and exact-source Windows/Ubuntu CI remain **pending**, not skipped successes. Rust scanner availability remains the **D3 blocker**; overall B2 is **not closed**.

## Independent D2 verification

Independent Risk5 verifier returned **PASS for S1–S3**. It reran the five public-plugin tests (exit 0), independently compared all three complete plugin CSS outputs and all 26 production files directly as bytes, recomputed both hash manifests, checked identical generation inputs/plugin sources and the baseline lock against Git `6d3d3b9`, and confirmed 220 unrelated whole lock records unchanged. Read-only Node resolution from Typography's actual entry point confirmed parser 7.1.6 and unchanged Typography/Router versions; raw audit counts/exits were checked independently. Exact stdin assertion payloads/output are retained in verifier task `mvik93g6-f-25nu`'s conversation, not claimed as separate scratch files. The verifier did not reinstall/build, rerun Rust, query CI or certify native/game/provider behavior. Parent integrated gates and exact-source CI/delivery remain separate.

## Parent integrated gates

Parent job-17 completed with exit 0: frontend **779 passed / 0 failed**, production build/typecheck, Rust format, strict locked workspace Clippy, and workspace Rust tests **2747 passed / 0 failed / 25 ignored** across 114 summaries. The raw log is retained as `parser/parent-integrated.log`. Conservative pre/post cleanup both exited 0 and removed 0 GB, preserving evidence. Exact-source hosted CI and delivery follow this local result; ignored tests were not run. No Rust advisory scanner ran.

## D2 self-review by specification

- **S1:** The authorized non-downgrading parser repair is isolated from the already-delivered D1 Router migration. Existing imports, routes, styles and configuration remain untouched; no broad/force repair or unrelated upgrade was used.
- **S2:** Actual complete plugin generation and production files are byte-identical, backed by public output assertions and 779 passing frontend tests per phase. This preserves the tested compatibility surface without asserting native/game/browser behavior that was not rerun.
- **S3:** Baseline and candidate full/production audit JSON, exits, graph fields, scoped installed resolution and lock-preserving `npm ci` are retained. npm is clean for this candidate; no Rust audit or complete-B2/delivery claim is substituted for missing evidence.

## Key Learnings

- Production CSS alone cannot exercise an installed but inactive Typography plugin. Compile the real public plugin with explicit candidates as well as checking the actual application build.
- A major parser iteration change warrants public selector-output characterization; Typography's copied-array/separate-container mutation pattern remained compatible here. This evidence is bounded to the admitted plugin/config/compiler surface, not every possible third-party selector.
- The scoped manifest override is effective even though npm leaves the lock root record unchanged. Verify the actual plugin resolution after `npm ci` and compare whole graph records; no transitive closure expansion was required.
- Preserve audit failures and evidence-harness errors separately from product compatibility. A clean npm candidate and a completed local D2 unit do not close scanner provisioning, independent review, CI or delivery.


## Final npm delivery and remaining B2 blocker

| Unit | Tested source | Exact two-platform CI | Delivery proof |
|---|---|---|---|
| Router compatibility | `6d3d3b99a466973fa992e28e231513db09eba20a` | [39664242232](https://github.com/Miike0303/Locust/actions/runs/39664242232) | Parent job-16 non-force main promotion and exact remote/local readback |
| Scoped Typography/parser | `d91446900fed104d2023b07ec7b47ed6859443c3` | [39671059728](https://github.com/Miike0303/Locust/actions/runs/39671059728) | Parent job-19 non-force main promotion and exact remote/local readback |

Parent validated each CI head SHA, both Windows/Ubuntu jobs and all ten required gates before promotion. CI metadata and delivery receipts are retained under the corresponding `router/` and `parser/` evidence roots. Subsequent ledger/index/report closure is documentation-only after tested source `d914469`, not a new tested binary.

**npm repair outcome: complete and delivered.** Fresh D2 full and production npm audits exit 0, all severities zero; there were no unresolved final compatibility or integrated-test failures. Historical baseline audit and harness failures remain documented above; 25 Rust tests were ignored, not executed.

**Overall B2: partial / needs an owner decision.** Neither `cargo-audit` nor `cargo-deny` is installed. Approved scanner provisioning is needed before a Rust advisory scan or the remaining report-only Rust audit infrastructure can be completed. No tool was installed implicitly, no Rust advisory scan ran, and passing Rust tests do not substitute for that missing proof. Native Tauri, real games/providers and broader human/runtime certification remain outside this dependency outcome.


## D3: explicitly approved Rust audit continuation

**Final outcome: audit/report scope complete and delivered as `ffdeec4`.** Exact [CI39688158085](https://github.com/Miike0303/Locust/actions/runs/39688158085) passed Windows/Ubuntu (all ten required gates each) and the informational audit job. Parent inspected downloaded artifacts rather than trusting overall green: scanner setup exit0, Rust childexit1/47 records/44 IDs/17 warnings, npm production childexit0/clean; hosted database commit and counts match the local scan. Parent job26 non-force main promotion and exact remote/local readback passed. Subsequent state closure is documentation-only after this tested source; findings are **recorded, not repaired**.

User **"hazlo"** authorized scanner provisioning after the prior blocker. Pinned/locked `cargo-audit` **0.22.2**, with default binary-scanning features disabled, was installed in `.test-tmp/b2-dependencies-evidence/rust/scanner` using the existing checkout target. No global installation, product dependency change or lockfile rewrite occurred. Version/help and conservative pre/post cleanup passed; log: `rust/install.log`.

The actual lockfile scan completed: auditor **exit 1**, **47 vulnerability records / 44 distinct advisory IDs**, plus **17 warnings** (11 unmaintained, 5 unsound, 1 yanked). Wrapper exit 0 means a successfully collected findings report, **not a clean audit**. Database commit `4b587f00996ebe5c05a73eaef105ef241e876414`: 1,348 advisories, updated `2026-10-21T08:20:38+02:00`. Raw streams/argv/exits and index: `rust/scan-2/`, `rust/findings-index.json`. Production npm returned child exit 0 / zero findings (`rust/npm-production-1/`).

The full lockfile includes optional/platform-specific dependencies; this is **not** proof every record ships or is exploitable. Feature/reachability triage and Rust upgrades remain separate follow-up work. No advisory was ignored or automatically fixed.

| Locked package/version | Records | Advisory IDs |
|---|---:|---|
| `crossbeam-epoch 0.9.18` | 1 | RUSTSEC-2026-0204 |
| `h2 0.4.13` | 1 | RUSTSEC-2026-0258 |
| `quick-xml 0.36.2` | 2 | RUSTSEC-2026-0194, RUSTSEC-2026-0195 |
| `quinn-proto 0.11.16` | 5 | RUSTSEC-2026-0338, RUSTSEC-2026-0339, RUSTSEC-2026-0340, RUSTSEC-2026-0341, RUSTSEC-2026-0342 |
| `rustls 0.23.37` | 4 | RUSTSEC-2026-0285, RUSTSEC-2026-0346, RUSTSEC-2026-0348, RUSTSEC-2026-0349 |
| `rustls-webpki 0.103.9` | 5 | RUSTSEC-2026-0049, RUSTSEC-2026-0098, RUSTSEC-2026-0099, RUSTSEC-2026-0104, RUSTSEC-2026-0382 |
| `wasmtime 25.0.3` | 21 | RUSTSEC-2025-0046, RUSTSEC-2025-0118, RUSTSEC-2026-0020, RUSTSEC-2026-0021, RUSTSEC-2026-0085, RUSTSEC-2026-0086, RUSTSEC-2026-0087, RUSTSEC-2026-0088, RUSTSEC-2026-0089, RUSTSEC-2026-0091, RUSTSEC-2026-0092, RUSTSEC-2026-0093, RUSTSEC-2026-0094, RUSTSEC-2026-0095, RUSTSEC-2026-0096, RUSTSEC-2026-0222, RUSTSEC-2026-0269, RUSTSEC-2026-0316, RUSTSEC-2026-0363, RUSTSEC-2026-0379, RUSTSEC-2026-0380 |
| `webpki-root-certs 1.0.7` | 1 | RUSTSEC-2026-0383 |
| `webpki-roots 1.0.9` | 1 | RUSTSEC-2026-0381 |
| `zip 2.4.2` | 3 | RUSTSEC-2026-0367, RUSTSEC-2026-0368, RUSTSEC-2026-0369 |
| `zip 4.6.1` | 3 | RUSTSEC-2026-0367, RUSTSEC-2026-0368, RUSTSEC-2026-0369 |

### Collector and informational CI

`python tools/dependency_audit_report.py --help` describes `--kind rust|npm`, `--output <new-directory>` and the auditor argv after `--`. Linux examples after installing the pinned scanner:

```bash
python tools/dependency_audit_report.py --kind rust --output .test-tmp/dependency-audit/rust -- .test-tmp/b2-audit-scanner/bin/cargo-audit audit --json --file Cargo.lock --db .test-tmp/b2-audit-db
python tools/dependency_audit_report.py --kind npm --output .test-tmp/dependency-audit/npm -- npm --prefix apps/desktop audit --omit=dev --json
python -m unittest discover -s tools -p test_dependency_audit_report.py
```

Windows: use the scanner's resolved native absolute path (PowerShell `Resolve-Path`); invoke npm through `cmd.exe /d /c npm ...`. The collector adds no implicit shell or fix flags; use read-only audit commands as shown. Unmodified child streams go to `stdout.json` and `stderr.txt`; UTF-8 `result.json` and stdout carry command, actual `exit_code`, status, counts and operational error. Status: `clean`, `findings`, `warnings`, `error`. Wrapper exit 0 means collection succeeded (including findings/warnings); exit 2 means invalid/unavailable/failed collection. Malformed JSON, operational failure and the fixed 300-second timeout are never called clean. Existing output directories are rejected, never overwritten.

The separate `dependency-audit` **job in the existing CI workflow** is informational (`continue-on-error`, no `needs` relationship), with read-only repository permission. The existing Windows/Ubuntu gates and tag/manual triggers are unchanged. The exact scanner is installed; Rust/npm production outcomes and raw setup exits are summarized and uploaded even after operational failures (14-day artifact retention). This makes the report job manually verifiable on the exact candidate before main delivery; no new default-branch workflow registration is needed.

Public subprocess tests: RED exit 1 while collector absent, then GREEN **9 tests** covering clean/findings/warnings, exact child streams/exits, malformed/tool/unavailable failures, overwrite rejection, unchanged fixture lockfiles and existing gate/trigger policy. Tests use no network/scanner. First real scan launch failed before child creation (`WinError 2`, relative executable path); `rust/scan-1/` retains error/null exit. Resolving the installed executable to a native absolute path produced the completed scan. A report-append helper initially failed on Windows default text decoding before writing; explicit UTF-8 corrected it. No timeout/automatic timeout retry occurred.

Initial independent Risk5 review confirmed scope, CI prefix, unchanged locks and actual scan, but found a collector defect: explicit falsey malformed Rust `warnings` values were coerced to an empty object and called clean (4/17 adversarial cases failed). Parent reproduced this through a new public regression test (five values including null): RED exit 1, then defaulted only the absent field instead of all falsey values. GREEN now has **10 passing tests**; raw `rust/warnings-{red,green}.*` records are retained. The initial nine-test pass was insufficient and is not hidden. The bounded independent correction check passed: **10 public tests and the same 17 adversarial cases**, all exit 0; all four formerly failing cases now report error/2. Evidence is retained in `rust/independent/verify-20261021-r3-correction/`; earlier failed runs were preserved. Exact-source hosted CI (both required platform jobs and informational report job) and delivery subsequently passed as recorded above; these were pending at the correction checkpoint. The actual 300-second timeout path has not been exercised; its handler was inspected only.

## D6: compatible transport/TLS remediation continuation

User **"subelo a main, continua con los pendientes para cerrar la sesion"** authorized this bounded continuation after main was reconfirmed at `48bd628`. The selected unit changes only six transport/TLS lock records, not manifests, features, application code or the Rust toolchain. Larger XML/ZIP/Wasmtime migrations and independent Crossbeam remediation are not mixed into this unit.

| Package | Baseline | Candidate | Declared target MSRV | Target vulnerability records removed |
|---|---|---|---|---:|
| h2 | 0.4.13 | 0.4.16 | 1.63 | 1 |
| quinn-proto | 0.11.16 | 0.11.18 | 1.85 | 5 |
| rustls | 0.23.37 | 0.23.46 | 1.71 | 4 |
| rustls-webpki | 0.103.9 | 0.103.16 | 1.71 | 5 |
| webpki-root-certs | 1.0.7 | 1.0.10 | 1.71 | 1 |
| webpki-roots | 1.0.9 | 1.0.10 | 1.71 | 1 |

Precise Cargo updates initially also rebound 14 unrelated existing records' `windows-sys`/`socket2` edges. That draft was **rejected**, not silently accepted. The minimal candidate retains all **776 unrelated whole package records** from baseline and copies only the six complete target records produced by Cargo. All six registry checksums match and none of the targets is yanked. `cargo metadata --locked --offline --no-deps --format-version 1` exits 0; actual full locked-build compatibility and lock immutability are checked separately, not inferred from this metadata probe.

Retained public-interface security differential: **47 → 30 vulnerability records**, **17 targeted → 0**, **27 distinct IDs remain**, with all other advisory/package/version tuples and **17 warnings unchanged**. Both audits use the same database with `--no-fetch`. Its Git HEAD is independently `4b587f00996ebe5c05a73eaef105ef241e876414`; cargo-audit's no-fetch JSON omits commit/update metadata (null), so that provenance is supplied by the Git probe, not invented JSON fields. The Rust auditor still exits **1** for the remaining findings; wrapper exit0 means successful collection. This is observed targeted security RED→GREEN, not a fabricated product-test RED.

Remaining package families: `crossbeam-epoch`, `quick-xml`, `wasmtime`, `zip`. Full-lock optional/platform reachability caveats still apply: reqwest's HTTP/2 and Rustls paths are enabled in project manifests, while default HTTP/3/Quinn reachability and the wasm32-specific root-certificate parent are not generalized into shipped exploit claims.

Evidence root: `.test-tmp/b2-dependencies-evidence/transport/`. `candidate-Cargo.lock` and `graph-diff.json` retain the rejected raw solver draft; **`minimal-Cargo.lock`** is the candidate actually under verification. `minimal-graph-validation.json` records checksums/MSRVs/776 unchanged records; `baseline-audit/`, `candidate-audit/` and `audit-differential.json` retain raw streams, exact child exits and comparisons. Each precise update has its own command/stdout/stderr record. `run.py candidate` only collects the candidate audit; do not rerun the baseline phase over retained evidence.

Parent job31 passed with exit0: production build/typecheck, frontend779/0, fmt, strict locked workspace Clippy, Rust2747/0/25ignored across114 summaries, reporter10tests/OK and exact lock byte-immutability through all gates. Pre/post conservative cleanup exited0 and removed0GB; raw `parent-integrated.log` is retained. The25ignored tests were not run.

Independent S5 verification **passed**: six whole Cargo-generated target records,776 unchanged records (parsed fields and complete normalized record text), candidate byte identity/checksums/MSRVs, full raw audit/settings/warnings equality outside targeted findings, clean same-DB Git HEAD, and an independent locked offline metadata exit0 with no lock mutation. Evidence: `transport/independent/verify-s5-r3/verification.json` plus metadata streams. Independent probe-only Windows encoding/cache-shard errors were corrected and disclosed in its conversation; no product defect or source fix was indicated. It did not independently rebuild or rescan.

Exact-source hosted CI and main delivery of D6 remain **pending**. The previous audit/report unit remains delivered; no new hosted result is inferred from earlier CI.
