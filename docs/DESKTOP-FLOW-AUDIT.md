# Desktop flow audit

Current audit: 2026-10-16, baseline `af3bc33`. This is a coverage report, not a claim that every workflow or native window is verified. Work proceeds in separate tested units; see [task ledger](../odd/tasks/desktop-flow-audit.md).

## Existing screens

| Surface | Available capabilities | Current evidence / next check |
|---|---|---|
| Home | Open game folder/file/database, recent projects, format selection, patch and recovery entry points | Rendered with and without a synthetic project. Inspect lower format cards and file-picker behavior separately. |
| Editor | Source/translation table, filters, editing, translation, validation, injection, export/import, replacement, pivot | Two extracted RenPy rows rendered at 1280/14px, 900/18px and 640/18px. Narrow layout is cramped; editing and workflow dialogs need their own acceptance. |
| Review | Translation review, editing and approval | Empty/pending-only state captured. Populated approval flow still pending. |
| Translation Memory | Project-scoped language pairs, search and deletion | Empty and opened-project states captured. Populated memory operations pending. |
| Settings: Providers | Provider configuration and connection tests | Rendered; no external provider calls or credentials used. |
| Settings: Translation defaults | Provider, source/target languages, batch size and cost limit | Rendered; persistence acceptance pending. |
| Settings: Appearance | Theme, font size, source-column visibility, row height and interface language | Rendered; preference persistence acceptance pending. |
| Settings: Glossary | Project terms and language pair | No-project state rendered; populated operations pending. |
| Settings: History | Translation runs, costs/tokens/duration | Initial loading state captured without project; final error/empty state needs a targeted check. |
| Settings: Data | Backups, restore and delete | Empty backup state rendered; no real backups modified. |

Queue, Activity Log, Keyboard Help, Resume Project, Game Source, Translation, Inject, Patch, Export, Pivot, Search/Replace, Validation, Font Patch and Injection Recovery are dialogs/panels rather than missing top-level pages. Their presence in code does not establish runtime acceptance.

## Confirmed navigation defect and recovery

An unknown URL previously rendered an entirely blank page, including loss of the sidebar. The baseline `/missing-audit-route` capture has no `main` element and zero body text.

Unknown addresses now have a localized **Page not found / Página no encontrada** screen inside the existing shell, with a **Go to Home / Ir al inicio** link. The sidebar stays available. Existing Home, Editor, Review, Memory and Settings routes retain their destinations; no database or project data is changed by recovery.

## Evidence and limits

- Baseline browser run: `tmp/desktop-flow-audit/run-1792169214788510600/report.json`, with 23 PNGs under `screenshots/`. Real isolated backend, current production frontend, synthetic two-row RenPy fixture, Chromium 145. No page exceptions or horizontal document overflow were observed; this does not prove that every control fits or every operation works.
- Baseline build: 516 frontend tests passed; TypeScript/Vite and CLI dev build passed. Logs: `tmp/desktop-flow-audit/build/`.
- Unknown-route regression: focused tests initially failed (1 pass, 2 failures), then passed (3/3); full frontend suite passed 519/519 and production build passed. Focused browser acceptance at `tmp/desktop-flow-audit/run-1792169641263380300/` passed with five screenshots and no page exceptions: Spanish/English guidance, nested unknown URL, Home-link recovery and sidebar navigation to Editor. Recovery images were inspected.
- The first acceptance script failed because its exact Editor link selector omitted the visible shortcut from the accessible name. The corrected check targets the sidebar destination and waits for rendered content. A Python static-server connection-abort diagnostic occurred when closing the browser in the passing run; it is retained, not attributed to an app exception. Rust and hosted gates remain tracked in the task ledger.
- Native Tauri window/IPC, native file pickers, real-game runtimes, fonts and linguistic quality are **not covered** by these browser captures.
- Build warnings remain: stale Browserslist data, mixed static/dynamic API imports and a minified entry chunk above 500 kB (506.51 kB baseline, 507.66 kB with recovery). Measure the effect before choosing an optimization; no dependency update is implied.

## Priority order

1. Eliminate reproduced dead ends and retain working navigation, starting with unknown URLs.
2. Exercise populated translation → review → validation workflows, queue continuity and final error states; fix newly reproduced failures before taking the next unit.
3. Assess narrow-window usability with enlarged text. A 640px screenshot is an audit stress case, not proof of a supported mobile product.
4. Record which capabilities genuinely need development after testing existing ones. Do not confuse hidden dialogs with absent systems or propose speculative screens as defects.

Each delivery requires passing applicable checks and exact-candidate hosted CI before fast-forwarding `main`. Generated caches and owned scratch are cleaned around build/test batches; regression source tests and retained evidence are preserved.
