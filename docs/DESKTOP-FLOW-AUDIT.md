# Desktop flow audit

Current audit: 2026-10-16, baseline `af3bc33`. This is a coverage report, not a claim that every workflow or native window is verified. Work proceeds in separate tested units; see [task ledger](../odd/tasks/desktop-flow-audit.md).

## Existing screens

| Surface | Available capabilities | Current evidence / next check |
|---|---|---|
| Home | Open game folder/file/database, recent projects, format selection, patch and recovery entry points | Rendered with and without a synthetic project. Inspect lower format cards and file-picker behavior separately. |
| Editor | Source/translation table, filters, editing, translation, validation, injection, export/import, replacement, pivot | Two extracted RenPy rows rendered at 1280/14px, 900/18px and 640/18px. Mock translation, review navigation and validation passed. Narrow layout is cramped; manual-edit conflicts and destructive workflows are not certified by this audit. |
| Review | Translation review, editing and approval | Populated two-row Mock result rendered; both approval PATCH responses succeeded and the public API reports two approved rows. Completion and populated screenshots inspected. Editing an approved translation remains a separate case. |
| Translation Memory | Project-scoped language pairs, search and deletion | Empty and opened-project states captured. Populated memory operations pending. |
| Settings: Providers | Provider configuration and connection tests | Rendered; no external provider calls or credentials used. |
| Settings: Translation defaults | Provider, source/target languages, batch size and cost limit | Rendered; persistence acceptance pending. |
| Settings: Appearance | Theme, font size, source-column visibility, row height and interface language | Rendered; preference persistence acceptance pending. |
| Settings: Glossary | Project terms and language pair | No-project state rendered; populated operations pending. |
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
| Memory, Glossary, Backups | Empty/project-entry surfaces rendered | Populated memory/glossary and restore/delete operations not exercised |
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
| Next: usability | Associate visible translation/queue form labels with controls | Source mapping found unassociated labels; verify accessible names through public UI, then add matching labels/IDs without changing payloads |
| Next: navigation | Improve compact-window use | At 640px/18px sidebar consumes about 270px. Define supported desktop minimum; prove all actions reachable and table/detail scrolling usable before choosing collapsible navigation |
| Next: systems | Exercise failure and continuity states | Real-backend fixtures for job cancellation/reconnect, save conflict, populated memory/glossary, resume/recovery and patch refusal. Current happy paths do not certify these |
| Next: performance | Measure initial bundle and large-project rendering | Entry chunk is 507.66 kB. Profile load and Editor with a large fixture before splitting imports; do not hide the warning by raising the threshold |
| Missing proof, not missing screen | Native desktop and engine runtime QA | Tauri window/IPC/file pickers, actual game/font rendering and translation quality remain unverified here |
| Separate deferred product scope | Static web hosting/distribution and experimental-engine capability gaps | Existing ledger defers hosting. Do not build a new site or imply universal engine support from a RenPy fixture |

No additional product failure was confirmed by the completed happy-path audit. Initial History loading, an animated progress bar and broken audit WebSocket forwarding were investigated rather than patched as speculative application bugs.

Each delivery requires passing applicable checks and exact-candidate hosted CI before fast-forwarding `main`. Generated caches and owned scratch are cleaned around build/test batches; regression source tests and retained evidence are preserved.
