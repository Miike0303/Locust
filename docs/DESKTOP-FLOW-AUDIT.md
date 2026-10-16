# Desktop flow audit

Current audit: 2026-10-16, baseline `af3bc33`. This is a coverage report, not a claim that every workflow or native window is verified. Work proceeds in separate tested units; see [task ledger](../odd/tasks/desktop-flow-audit.md).

## Existing screens

| Surface | Available capabilities | Current evidence / next check |
|---|---|---|
| Home | Open game folder/file/database, recent projects, format selection, patch and recovery entry points | Rendered with and without a synthetic project. Inspect lower format cards and file-picker behavior separately. |
| Editor | Source/translation table, filters, editing, translation, validation, injection, export/import, replacement, pivot | Two extracted RenPy rows rendered at 1280/14px, 900/18px and 640/18px. Mock translation, review navigation and validation passed. Narrow layout is cramped; manual-edit conflicts and destructive workflows are not certified by this audit. |
| Review | Translation review, editing and approval | Populated two-row Mock result rendered; both approval PATCH responses succeeded and the public API reports two approved rows. Completion and populated screenshots inspected. Editing an approved translation remains a separate case. |
| Translation Memory | Project-scoped language pairs, search and deletion | Populated UI search/filter/pagination and confirmation cancellation tested with explicit synthetic API responses; real backend memory HTTP contracts tested separately. Last-page deletion recovery delivered as `619fc73` after exact Linux/Windows CI38962171469. |
| Settings: Providers | Provider configuration and connection tests | Rendered; no external provider calls or credentials used. |
| Settings: Translation defaults | Provider, source/target languages, batch size and cost limit | Rendered; persistence acceptance pending. |
| Settings: Appearance | Theme, font size, source-column visibility, row height and interface language | Rendered; preference persistence acceptance pending. |
| Settings: Glossary | Project terms and language pair | Two synthetic terms passed real-backend add/readback, language-pair isolation, live filtering, Cancel/Escape, owned deletion and navigation return; see bounded glossary evidence below. |
| Settings: History | Translation runs, costs/tokens/duration | Final no-project guidance awaited and inspected after query retry. Public run history confirms Mock en→es, two translated strings, zero cost. |
| Settings: Data | Backups, restore and delete | Empty backup state rendered; no real backups modified. |

Queue, Activity Log, Keyboard Help, Resume Project, Game Source, Translation, Inject, Patch, Export, Pivot, Search/Replace, Validation, Font Patch and Injection Recovery are dialogs/panels rather than missing top-level pages. Their presence in code does not establish runtime acceptance.

| Dialog / system | Observed acceptance | Explicit limit |
|---|---|---|
| Translation | Mock two-row job completes, final progress settles at 100%, Review action opens populated review | No real-provider quality, outage, cancellation or reconnect certification |
| Review / Validation | Two approvals persist; two strings validate with zero text issues | Mock is not a linguistic-quality test; embedded game fonts/runtime not exercised |
| Queue | Second synthetic project added through browser prompt, translated with Mock, marked Ready, then returned to Editor | Actual recorded pair was ja→en (UI defaults); this was transport/workflow acceptance, not language detection or translation quality |
| Inject, Patch Apply, Export, Search/Replace, Pivot | Five panels captured and inspected at 1280px/14px/light and 900px/18px/dark; all ten panel rectangles fit the viewport | Buttons were not submitted; scrollable content, alternate tabs, safety refusal/result states and full operations are separate checks |
| Injection Recovery / Keyboard Help | Initial dialogs opened and inspected; Escape closed Recovery | No interrupted-injection repair performed |
| Memory, Glossary, Backups | Empty/project-entry surfaces rendered; populated Memory UI checked with explicit API fixtures | Memory backend tested separately; no end-to-end Mock memory population. Populated glossary now tested separately with a real isolated backend; real backup restore/delete remain untested |
| Resume Project, Game Source, Font Patch, update prompts | Source inventory only | Not rendered in this audit; not claimed missing or broken |

## Confirmed navigation defect and recovery

An unknown URL previously rendered an entirely blank page, including loss of the sidebar. The baseline `/missing-audit-route` capture has no `main` element and zero body text.

Unknown addresses now have a localized **Page not found / Página no encontrada** screen inside the existing shell, with a **Go to Home / Ir al inicio** link. The sidebar stays available. Existing Home, Editor, Review, Memory and Settings routes retain their destinations; no database or project data is changed by recovery.

## Evidence and limits

- Baseline browser run: `tmp/desktop-flow-audit/run-1792169214788510600/report.json`, with 23 PNGs under `screenshots/`. Real isolated backend, current production frontend, synthetic two-row RenPy fixture, Chromium 145. No page exceptions or horizontal document overflow were observed; this does not prove that every control fits or every operation works.
- Baseline build: 516 frontend tests passed; TypeScript/Vite and CLI dev build passed. Logs: `tmp/desktop-flow-audit/build/`.
- Unknown-route regression: focused tests initially failed (1 pass, 2 failures), then passed (3/3); full frontend suite passed 519/519 and production build passed. Focused browser acceptance at `tmp/desktop-flow-audit/run-1792169641263380300/` passed with five screenshots and no page exceptions: Spanish/English guidance, nested unknown URL, Home-link recovery and sidebar navigation to Editor. Recovery images were inspected.
- The first acceptance script failed because its exact Editor link selector omitted the visible shortcut from the accessible name. The corrected check targets the sidebar destination and waits for rendered content. A Python static-server connection-abort diagnostic occurred when closing the browser in the passing run; it is retained, not attributed to an app exception. Rust and hosted gates remain tracked in the task ledger.
- Follow-up `run-1792171512836548500`: final no-project History guidance and Translation configuration captured. Provider selection stopped before submission because the harness selector matched both primary and fallback selects; this is not a demonstrated application defect.
- Translation probe `run-1792171578468315500` and Queue probe `run-1792171834989849900` timed out. Root cause in both: the audit adapter incorrectly called Playwright `WebSocketRoute.connect_to_server` with a URL argument, producing a TypeError. The completion-text selector was also too strict because cost shares the element. These are retained harness failures, not evidence of product failure. Public CLI `stats` and HTTP history confirmed the original two Mock en→es translations. After repairing the adapter and selector, distinct fresh-fixture acceptance runs below completed successfully.
- Saved-result acceptance `run-1792171762880211200`: reopened through `/api/project/open-db`, rendered Review, approved both rows through UI, returned to Editor and validated two strings with zero text issues. Public API confirms two approved rows; zero page exceptions, three inspected workflow screenshots. An earlier probe incorrectly used the extracting `/project/open` endpoint for a DB and received HTTP 500; that harness error is retained.
- Native Tauri window/IPC, native file pickers, real-game runtimes, fonts and linguistic quality are **not covered** by these browser captures.
- Build warnings remain: stale Browserslist data, mixed static/dynamic API imports and a minified entry chunk above 500 kB (506.51 kB baseline, 507.66 kB with recovery). Measure the effect before choosing an optimization; no dependency update is implied.

## Follow-up acceptance and delivery

- Unknown-route fix `d0f57d87bab3ee57684bb88a82d80795e156987e` is on local and remote `main`. [CI 38936699767](https://github.com/Miike0303/Locust/actions/runs/38936699767) passed `check` and `check-windows` for that exact SHA. Local Rust: 2,584 passed, zero failed, 25 ignored (not executed); fmt/strict Clippy passed.
- Corrected Queue run `tmp/desktop-flow-audit/run-1792171921022670400/`: Ready state, populated pending row and return-to-Editor captured; public API reports one Mock job, two translated strings, zero cost, zero page exceptions. Pending/Ready captures inspected.
- Full dialog run `tmp/desktop-flow-audit/run-1792172034833390000/`: 18 screenshots, zero page exceptions. Translation → Review → two approvals → Validation passed. Five dialogs at two sizes/themes fit the viewport and were inspected; Recovery and Keyboard Help initial states inspected. The progress bar had inline width 100% and computed width 0px immediately after state change, then 404px out of its 404px parent after the CSS animation. The earlier near-empty-bar capture was transient, not a product defect.
- All browser workers terminate their owned backend, static server and browser. A failed earlier probe used an unsupported adapter; later acceptance redirects the browser WebSocket constructor only to the owned loopback backend and preserves real protocol frames. No external provider calls, real-game writes or dependency installs were used.

## Prioritized optimization and development backlog

This is an evidence-ranked backlog, not a claim that all optimization work was implemented. No new top-level screen is proven necessary merely because a feature lives in a dialog.

| Priority | Work unit | Why / acceptance before delivery |
|---|---|---|
| Delivered | Recover unknown URLs without losing navigation | Reproduced blank app; localized recovery and sidebar navigation now tested and delivered |
| Delivered: usability | Associate visible translation/queue form labels with controls | Sixteen label/control associations passed accessible-name, label-click focus and Mock request checks; 488cbea delivered after Linux/Windows CI 38950561797 |
| Audited: navigation | Preserve the existing compact-window minimum | Tauri already defines 900×600. Five routes, sidebar hit targets, Queue/Help dismissal and Home scrolling passed at that size in light/14px and dark/18px. No reachability defect was reproduced; no collapsible sidebar or minimum-size change was made |
| Delivered: memory | Recover after deleting the only entry on a later page | Reproduced 51→50 rows leaving an empty offset with no pagination; UI fixture regression and separate real HTTP tests passed; `619fc73` delivered after exact Linux/Windows CI38962171469 |
| Next: systems | Exercise failure and continuity states | Job cancellation/reconnect, save conflict, resume/recovery and patch refusal remain separate checks. Populated glossary passed its bounded two-term audit; settled Settings highlight remains unmeasured. Memory UI fixtures and backend HTTP tests do not certify a full provider-to-memory workflow |
| Next: performance | Measure initial bundle and large-project rendering | Entry chunk is 507.66 kB. Profile load and Editor with a large fixture before splitting imports; do not hide the warning by raising the threshold |
| Missing proof, not missing screen | Native desktop and engine runtime QA | Tauri window/IPC/file pickers, actual game/font rendering and translation quality remain unverified here |
| Separate deferred product scope | Static web hosting/distribution and experimental-engine capability gaps | Existing ledger defers hosting. Do not build a new site or imply universal engine support from a RenPy fixture |

No additional product failure was confirmed by the completed happy-path audit. Initial History loading, an animated progress bar and broken audit WebSocket forwarding were investigated rather than patched as speculative application bugs.

## Delivery gate follow-up

The report-only candidate `35ddef6` did **not** pass its hosted gate: [CI 38941104332](https://github.com/Miike0303/Locust/actions/runs/38941104332) passed Linux but failed Windows in `argos::tests::test_argos_health_check_down`. An earlier watcher network failure obscured the final outcome; it was not a test result.

That test assumed `127.0.0.1:59999` had no listener. A controlled HTTP 503 responder on that port reproduced the exact failed installation-guidance assertion (zero passed, one failed). The test now owns an ephemeral TCP endpoint that deliberately closes without an HTTP response; a separate 503 test preserves status-error coverage. Production Argos behavior is unchanged. Corrected candidate `911d895281882431b3053ab4cec94d00803bb850` passed [CI 38944911794](https://github.com/Miike0303/Locust/actions/runs/38944911794) on Linux and Windows and reached remote/local `main` by non-force fast-forward.

## Delivered Translation and Queue form accessibility

Eight visible labels in each form now identify their select, input or textarea through matching `htmlFor`/`id` attributes. Control values, handlers and translation options were not changed. Clicking a label focuses its control, and assistive technology can use the localized visible name rather than an unnamed field.

- Focused structural regression: 16 failures before the fix, then 16 passes. Full frontend suite: 535 passed, zero failed or skipped; TypeScript and production build passed.
- Public browser acceptance: `tmp/desktop-flow-audit/run-1792176355147241000/` (Translation → Review → two approvals → clean Validation) and `run-1792176370670672000/` (Queue → Ready → Editor). All 16 controls were found by their exact visible label and checked for focus on label click; both forms' screenshots were inspected. No page exceptions occurred.
- Both requests used only Mock, en→es and the entered synthetic game context. Queue history reports two translations at zero cost. No external providers or real games were used. These runs supplement, rather than replace, the earlier ja→en Queue evidence.
- An auxiliary source-printer comparison failed because JSX whitespace formatting differed; it is not recorded as a passing check. Diff readback showed attribute additions and whitespace only; browser checks establish submitted options and completed workflows.
- Existing build warnings remain, including a 508.04 kB minified entry chunk. Cleanup removed 0 GB; checkout target 23.23 GB and shared target 3.89 GB remained below their caps, with 362 GB free after the build batch. Retained captures and regression tests were not deleted.

Candidate `488cbea65a4e06661cb5abd78f1f98c637ccb7ad` passed [CI 38950561797](https://github.com/Miike0303/Locust/actions/runs/38950561797) (`check` and `check-windows`, exact SHA) and reached remote/local `main` by non-force fast-forward. Native Tauri and real-game limitations still apply.

## Existing minimum-window navigation

Tauri already sets `minWidth: 900` and `minHeight: 600` in `apps/desktop/src-tauri/tauri.conf.json`. The earlier cramped 640px browser sample is below the native minimum, not evidence that the desktop minimum needs changing.

- `tmp/desktop-flow-audit/run-1792178223015306500/`: all five routes at 900×600 in light/14px and dark/18px, ten inspected screenshots. Measured sidebar links/buttons and visible main buttons stayed inside the viewport; no page exceptions or horizontal document overflow.
- `run-1792178314360692800/`: sidebar clicks reached each route; sidebar target centers were unobscured. Empty Queue opened and closed, Keyboard Help opened and closed with Escape, and Home scrolled to the bottom in both appearances. Six additional overlay/scroll screenshots inspected. Help content is scrollable; this check does not certify every shortcut or populated dialog.
- A Home capture retained the Settings visual highlight. Targeted probe `run-1792178388415516700/` confirmed exactly one `aria-current="page"` link for Settings and then Home, with matching URL. The remaining visual highlight is hover/focus, not an incorrect active route; no speculative routing patch was made.

This is browser evidence using simulated DOM font/theme state and an isolated synthetic project, not native Tauri acceptance or Settings persistence proof. No application code or configured window minimum changed. Documentation candidate `3558a1cab1e80b442b28e266c17ce056ce1f3ea5` passed [CI 38954269317](https://github.com/Miike0303/Locust/actions/runs/38954269317) on Linux/Windows and reached remote/local `main` by non-force fast-forward.

## Populated memory and last-page recovery candidate

Mock translations intentionally do not write translation memory, even when memory lookup is enabled. The first attempt to seed 51 rows through Mock therefore had an invalid test precondition and timed out before pagination; this was not a product failure. No external provider was introduced and the cache policy was not changed.

Using explicitly synthetic memory API responses, the browser then reproduced a UI defect: deleting the single entry on page 2 of a 51-row list left offset 50 with total 50, displaying “No hay coincidencias” and no pagination controls. Successful query results now clamp only an out-of-range offset to the last valid page; existing valid pages, filters, confirmation handling and backend deletion requests are unchanged.

- RED: `tmp/desktop-flow-audit/run-1792180229310973800/`, browser exit 1, failed to recover the remaining 50 rows after the one-entry deletion. Failure image inspected.
- GREEN: `run-1792180370125935700/`, browser exit 0, no page exceptions. Search match/no-match/reset, language filter, page 2, both confirmation cancellations leaving all entries/counters unchanged, deleting exactly one synthetic entry, and returning to the remaining 50 rows passed. Corrected image inspected.
- These populated-memory UI responses are fixtures, **not real backend memory writes**. Other endpoints use the isolated real backend; its public API confirmed zero Mock memory rows, preserving policy. Separately, real-server `memory_page_scope` HTTP integration tests passed 5/5, covering project scope, search/filter/stats, deletion and guards using their own isolated setup.
- Offset helper regression: 3/3; full frontend: 538 passed, zero failed/skipped; TypeScript/Vite build passed. Cleanup removed 0 GB; checkout target 25.04 GB/shared target 3.89 GB remained below caps, 352.2 GB free. Existing 508.04 kB entry-chunk warning remains.

The correction `619fc73e8e5e14f462374c3812b54d20cc8bad92` passed [CI38962171469](https://github.com/Miike0303/Locust/actions/runs/38962171469) on Linux/Windows and reached remote/local `main` by non-force fast-forward. Earlier CI38958357549 was cancelled during Rust tests, not a passing gate. No real memory or games were modified.

## Populated glossary: bounded public acceptance

The existing production frontend and a real isolated backend passed 13 check groups with two fresh synthetic terms. No application change was needed. The run used English/light at 1280×900; it does not certify native Tauri, large glossaries, other appearances, duplicate/update semantics, invalid input, non-null context filtering or concurrent project changes.

| Operation | Observed effect |
|---|---|
| Add and readback | Two UI additions returned HTTP201; independent GET responses matched exact terms/translations in en-es and en-fr. Form fields cleared. |
| Pair isolation | Each language pair showed only its term. Deleting the Spanish term left the French API object unchanged. |
| Live filter | Mixed-case term and uppercase accented translation matched; no-match showed zero/filter guidance; clearing restored the row. |
| Cancel and Escape | Neither sent DELETE; both preserved exact snapshots of both pairs and the visible count. |
| Navigation and deletion | Editor → Settings → Glossary retained the term. Two confirmed owned DELETEs returned204; final independent reads showed both pairs empty. |

Evidence: `tmp/desktop-flow-audit/glossary-run-1792186340309980700/report.json` and 13 individually inspected PNGs in its `screenshots/` directory. Audit and independent cleanup exited0; stderr was empty, page exceptions were absent and document width stayed1280. Owned backend, Playwright driver and harness PIDs were independently absent after cleanup; static thread/ports closed. Existing builds were reused, retaining roughly5.5MB evidence without a global cleanup sweep.

Two earlier attempts failed in the harness, not the product: urllib method logging, then a synthetic config-path/default-pair mismatch. They remain preserved in `glossary-report.md` and `glossary-attempt2-report.md`. Owner-authorized corrections aligned config with `LOCUST_DATA_DIR/config.json` and kept capture/closure inside the live Playwright manager; the third attempt was not retried.

**Visual qualification:** screenshot11 immediately after navigation shows Providers highlighted while Glossary content and URL are present; later screenshots highlight Glossary correctly. The parent inspected that image. Settled-state styling was not measured, so neither a persistent defect nor a fully verified active-highlight state is claimed. Other inspected controls/rows/confirmation were legible and unclipped. No speculative product patch was made.

Each delivery requires passing applicable checks and exact-candidate hosted CI before fast-forwarding `main`. Generated caches and owned scratch are cleaned around build/test batches; regression source tests and retained evidence are preserved.
