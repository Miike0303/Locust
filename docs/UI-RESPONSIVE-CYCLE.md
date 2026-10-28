# UI responsive cycle — NEXT-10

Checked 2026-10-28 at integrated HEAD `362286aa078a64e9951b7f042c5a92c1d1cdeae2`, branch `cycle/next-10-ui-captures`.

**PASS for the requested responsive matrix.** The final runner completed twice with 40 PNGs per run: 10 scenarios × 2 viewports × 2 UI locales. Every PNG in those runs was opened and visually inspected. A development baseline of another 40 PNGs was also inspected. No responsive defect was reproduced, so no UI component or test was changed.

## Run it

Requires Node 24+ and installed Google Chrome. From the worktree root, with the existing `CARGO_TARGET_DIR` environment preserved:

```powershell
npm --prefix apps/desktop run build
cargo build -p locust-cli
node tools/capture_ui.mjs
node tools/capture_ui.mjs .test-tmp/ui-captures/repeat
```

The runner builds nothing. It defaults to `.test-tmp/ui-captures/<full-HEAD>`; a positional argument selects another output directory. Reusing a directory overwrites the same scenario filenames and manifest. `LOCUST_CLI` can select another already-built binary; otherwise the runner uses `CARGO_TARGET_DIR/debug/locust.exe`. `CHROME_PATH` can select another installed Chrome executable.

This Windows sandbox required the following before running:

```powershell
$env:LOCUST_CHROME_NO_SANDBOX = '1'
node tools/capture_ui.mjs .test-tmp/ui-captures/repeat
```

That opt-in adds `--no-sandbox --disable-gpu`, for the trusted local synthetic fixture. Ordinary launch failed before DevTools with Crashpad `TransactNamedPipe ... (0x6D)` and `crash server failed to launch, self-terminating`, including after crash-reporting switches were added. Both final runs used the opt-in successfully; no outside-sandbox run is needed to reproduce this evidence.

## Isolation and real flows

- One owned backend binds a free `127.0.0.1` port. `LOCUST_DATA_DIR`, `LOCUST_CONFIG`, and `LOCUST_BACKUP_ROOT` all point inside one owned OS-temp directory, including configuration, project DBs, global memory and backups. Chrome has its own user-data directory there. The inherited Cargo target is never changed.
- A Node static server serves unmodified `apps/desktop/dist` bytes with SPA history fallback, matching Vite preview asset serving. Production `api.ts` uses `http://localhost:7842/api`; CDP Fetch interception changes that destination to the owned backend port. Responses are genuine backend responses, with no fabricated payloads and no connection to an existing server on 7842.
- Each locale/viewport gets a fresh synthetic project. JSON shapes derive from the MV fixtures in `crates/cli/tests/ja_en_es_pipeline.rs` and `crates/formats/tests/rpgmaker_revision.rs`. The metric font is `crates/cli/tests/fixtures/synthetic-ascii.ttf`. No real game or translation provider is used.
- The real extraction and string PATCH APIs seed 10 rows, including duplicate sources with different translations and an MV dialogue row. A small PO file contains fill, replace, unchanged and unknown-ID cases. CDP sets the browser file input to that file.
- Changing a previewed row produces the actual import HTTP 409. Full public rows are compared before/after apply and remain unchanged. QA validation produces two actual InconsistentTranslation issues. Reopening changed/deleted synthetic source text through Home produces the persistent warning; it survives 4.2 seconds before focus/hover can pause a timer.
- `localStorage['locust.ui.language']` selects EN/ES before application boot. PNGs use device scale 1 and are checked against exact 1024×700 / 1440×900 dimensions. Keyboard modality is established before focusing each captured control.

## Findings

All entries below apply to both locales and both sizes, in both final runs.

| Scenario | Observed issue / responsive finding | Fixed? |
|---|---|---|
| DetailPanel / Width estimate, within + exceeds | No problem found. Inputs, line estimates and explanatory copy fit. At 1024 the panel is a bounded overlay; at 1440 its longer Spanish copy wraps and the panel scrolls vertically. Native input focus outline is visible. | No fix needed |
| Export scope | No problem found. One file and exactly two statuses remain legible, with live count 5 and a visible select focus ring. Status labels wrap onto another row as needed. | No fix needed |
| Export zero matches | No problem found. Count 0, empty-scope explanation and disabled Download remain visible; the selected checkbox has a visible ring. | No fix needed |
| Import keep / replace / table | No problem found. Summary, both radio choices, table columns and footer fit. At 700 px height the modal body and replacement region scroll internally. Top and final rows were inspected; the focused region outline remains visible. | No fix needed |
| Import stale plan | No problem found. Localized error and repeat-preview link wrap fully, with a visible focus ring and the disabled Apply control inside the modal. | No fix needed |
| QA advisory chips | No problem found. The selected advisory chip, count and action buttons fit. Narrow row badges wrap over two readable lines; no horizontal table scrolling was needed. | No fix needed |
| Long persistent warning toast | No problem found. All three lines and dismiss control stay within the viewport. The dismiss focus ring is visible. Persistence is asserted without focusing the toast during the wait. | No fix needed |
| Isolated backend profile, behind Editor | Saving the recent-project entry returned `IO error: Acceso denegado. (os error 5)` in this sandbox. The existing recovery banner is visible in captures. Real extraction, edits, preview, QA and reopening still succeed. | Not fixed; backend persistence is outside the allowed layout files |

Across all 80 final PNGs, document scroll width equals viewport width, dialog rectangles are bounded, and focused controls are visible with an outline/ring. These DOM measurements accompany the visual review in each manifest. Scrolled content remains reachable through the visible internal scrollbars. Long font paths use the input's normal horizontal text scrolling; estimate labels and warning/error meaning remain readable. Existing Editor cell previews use ellipsis and expose the full selected text in DetailPanel.

## Execution and cleanup evidence

| Run | UTC interval | Captures | Browser / backend | Temp and descendants |
|---|---|---:|---|---|
| [A — default HEAD output](../.test-tmp/ui-captures/362286aa078a64e9951b7f042c5a92c1d1cdeae2/manifest.json) | 20:15:04.196–20:16:04.216 | 40 | Chrome exit 0; owned backend terminated and awaited | Temp removed; all 8 browser process IDs gone |
| [B — repeat](../.test-tmp/ui-captures/next-10-repeat/manifest.json) | 20:16:11.578–20:16:54.971 | 40 | Chrome exit 0; owned backend terminated and awaited | Different temp removed; all 8 browser process IDs gone |
| [Deliberate failure cleanup](../.test-tmp/ui-captures/next-10-failure-cleanup/manifest.json) | After A and B | 0 | Existing Node executable substituted for Chrome; rejects Chrome flags after backend startup | Exit 1 as expected; owned processes awaited and temp removed |

Both complete runs returned exit 0, `success: true`, `cleanup.ok: true`, zero browser/DevTools errors and zero failed geometry/focus checks. Each observed four expected import-apply 409 responses; every other recorded API response was HTTP 200. Capture timing can change the number of background refetches.

Browser: Chrome 156.0.8078.48. Node: 24.17.0. Both final manifests bind the identical runner SHA-256 `ad5f5a23f0b6b42ecc869425eefcdb4a65d0e0574151842c7ad57598b56b69b9`, CLI SHA-256 `3566cbe49782065a0cad24be499e381c39d6eeeeeb96b0d639b6c0e137cf82d0`, all 28 built asset hashes, source HEAD and UI-diff hash. Each capture records its own timestamp, locale, viewport, SHA-256, scenario evidence and focus/overflow measurements.

Cleanup uses Browser.close, then exact owned-process termination as needed, awaits process exits, closes the static listener and connections, verifies browser-scoped process IDs from CDP, and removes only the validated mkdtemp result directly below the OS temp root. SIGINT/SIGTERM settle the active operation before the same finally cleanup. Windows CIM inspection was denied during development; the final runner uses browser-scoped CDP process IDs instead.

Validation: `node --check tools/capture_ui.mjs`, help output, two complete final runs, deliberate post-backend-start failure cleanup, `git diff --check`, frontend production build and `cargo build -p locust-cli` passed. UI source was unchanged; the conditional UI unit-test rerun was not required.

This evidence covers the requested browser layouts with these synthetic strings and default appearance. Native Tauri dialogs and game rendering were not part of this matrix.

## Capture paths — run A

All paths below are relative to this document. Captures remain local evidence under `.test-tmp/ui-captures/`.

| Scenario / PNG basename | EN 1024×700 | EN 1440×900 | ES 1024×700 | ES 1440×900 |
|---|---|---|---|---|
| DetailPanel / Width estimate, 900 px (`editor-width-within.png`) | [PNG](../.test-tmp/ui-captures/362286aa078a64e9951b7f042c5a92c1d1cdeae2/en/1024x700/editor-width-within.png) | [PNG](../.test-tmp/ui-captures/362286aa078a64e9951b7f042c5a92c1d1cdeae2/en/1440x900/editor-width-within.png) | [PNG](../.test-tmp/ui-captures/362286aa078a64e9951b7f042c5a92c1d1cdeae2/es/1024x700/editor-width-within.png) | [PNG](../.test-tmp/ui-captures/362286aa078a64e9951b7f042c5a92c1d1cdeae2/es/1440x900/editor-width-within.png) |
| DetailPanel / Width estimate, 100 px (`editor-width-exceeds.png`) | [PNG](../.test-tmp/ui-captures/362286aa078a64e9951b7f042c5a92c1d1cdeae2/en/1024x700/editor-width-exceeds.png) | [PNG](../.test-tmp/ui-captures/362286aa078a64e9951b7f042c5a92c1d1cdeae2/en/1440x900/editor-width-exceeds.png) | [PNG](../.test-tmp/ui-captures/362286aa078a64e9951b7f042c5a92c1d1cdeae2/es/1024x700/editor-width-exceeds.png) | [PNG](../.test-tmp/ui-captures/362286aa078a64e9951b7f042c5a92c1d1cdeae2/es/1440x900/editor-width-exceeds.png) |
| One file, translated + reviewed, live count 5 (`export-scope.png`) | [PNG](../.test-tmp/ui-captures/362286aa078a64e9951b7f042c5a92c1d1cdeae2/en/1024x700/export-scope.png) | [PNG](../.test-tmp/ui-captures/362286aa078a64e9951b7f042c5a92c1d1cdeae2/en/1440x900/export-scope.png) | [PNG](../.test-tmp/ui-captures/362286aa078a64e9951b7f042c5a92c1d1cdeae2/es/1024x700/export-scope.png) | [PNG](../.test-tmp/ui-captures/362286aa078a64e9951b7f042c5a92c1d1cdeae2/es/1440x900/export-scope.png) |
| One file, approved, live count 0 (`export-zero.png`) | [PNG](../.test-tmp/ui-captures/362286aa078a64e9951b7f042c5a92c1d1cdeae2/en/1024x700/export-zero.png) | [PNG](../.test-tmp/ui-captures/362286aa078a64e9951b7f042c5a92c1d1cdeae2/en/1440x900/export-zero.png) | [PNG](../.test-tmp/ui-captures/362286aa078a64e9951b7f042c5a92c1d1cdeae2/es/1024x700/export-zero.png) | [PNG](../.test-tmp/ui-captures/362286aa078a64e9951b7f042c5a92c1d1cdeae2/es/1440x900/export-zero.png) |
| Import summary, keep existing (`import-keep.png`) | [PNG](../.test-tmp/ui-captures/362286aa078a64e9951b7f042c5a92c1d1cdeae2/en/1024x700/import-keep.png) | [PNG](../.test-tmp/ui-captures/362286aa078a64e9951b7f042c5a92c1d1cdeae2/en/1440x900/import-keep.png) | [PNG](../.test-tmp/ui-captures/362286aa078a64e9951b7f042c5a92c1d1cdeae2/es/1024x700/import-keep.png) | [PNG](../.test-tmp/ui-captures/362286aa078a64e9951b7f042c5a92c1d1cdeae2/es/1440x900/import-keep.png) |
| Import summary and first replacement rows (`import-replace.png`) | [PNG](../.test-tmp/ui-captures/362286aa078a64e9951b7f042c5a92c1d1cdeae2/en/1024x700/import-replace.png) | [PNG](../.test-tmp/ui-captures/362286aa078a64e9951b7f042c5a92c1d1cdeae2/en/1440x900/import-replace.png) | [PNG](../.test-tmp/ui-captures/362286aa078a64e9951b7f042c5a92c1d1cdeae2/es/1024x700/import-replace.png) | [PNG](../.test-tmp/ui-captures/362286aa078a64e9951b7f042c5a92c1d1cdeae2/es/1440x900/import-replace.png) |
| Replacement table scrolled to final rows (`import-replace-table.png`) | [PNG](../.test-tmp/ui-captures/362286aa078a64e9951b7f042c5a92c1d1cdeae2/en/1024x700/import-replace-table.png) | [PNG](../.test-tmp/ui-captures/362286aa078a64e9951b7f042c5a92c1d1cdeae2/en/1440x900/import-replace-table.png) | [PNG](../.test-tmp/ui-captures/362286aa078a64e9951b7f042c5a92c1d1cdeae2/es/1024x700/import-replace-table.png) | [PNG](../.test-tmp/ui-captures/362286aa078a64e9951b7f042c5a92c1d1cdeae2/es/1440x900/import-replace-table.png) |
| Stale-plan error and repeat-preview control (`import-stale.png`) | [PNG](../.test-tmp/ui-captures/362286aa078a64e9951b7f042c5a92c1d1cdeae2/en/1024x700/import-stale.png) | [PNG](../.test-tmp/ui-captures/362286aa078a64e9951b7f042c5a92c1d1cdeae2/en/1440x900/import-stale.png) | [PNG](../.test-tmp/ui-captures/362286aa078a64e9951b7f042c5a92c1d1cdeae2/es/1024x700/import-stale.png) | [PNG](../.test-tmp/ui-captures/362286aa078a64e9951b7f042c5a92c1d1cdeae2/es/1440x900/import-stale.png) |
| InconsistentTranslation advisory chip and two issues (`qa-inconsistent.png`) | [PNG](../.test-tmp/ui-captures/362286aa078a64e9951b7f042c5a92c1d1cdeae2/en/1024x700/qa-inconsistent.png) | [PNG](../.test-tmp/ui-captures/362286aa078a64e9951b7f042c5a92c1d1cdeae2/en/1440x900/qa-inconsistent.png) | [PNG](../.test-tmp/ui-captures/362286aa078a64e9951b7f042c5a92c1d1cdeae2/es/1024x700/qa-inconsistent.png) | [PNG](../.test-tmp/ui-captures/362286aa078a64e9951b7f042c5a92c1d1cdeae2/es/1440x900/qa-inconsistent.png) |
| Persistent merge warning after 4.2 seconds (`warning-toast.png`) | [PNG](../.test-tmp/ui-captures/362286aa078a64e9951b7f042c5a92c1d1cdeae2/en/1024x700/warning-toast.png) | [PNG](../.test-tmp/ui-captures/362286aa078a64e9951b7f042c5a92c1d1cdeae2/en/1440x900/warning-toast.png) | [PNG](../.test-tmp/ui-captures/362286aa078a64e9951b7f042c5a92c1d1cdeae2/es/1024x700/warning-toast.png) | [PNG](../.test-tmp/ui-captures/362286aa078a64e9951b7f042c5a92c1d1cdeae2/es/1440x900/warning-toast.png) |

## Capture paths — run B

| Scenario / PNG basename | EN 1024×700 | EN 1440×900 | ES 1024×700 | ES 1440×900 |
|---|---|---|---|---|
| DetailPanel / Width estimate, 900 px (`editor-width-within.png`) | [PNG](../.test-tmp/ui-captures/next-10-repeat/en/1024x700/editor-width-within.png) | [PNG](../.test-tmp/ui-captures/next-10-repeat/en/1440x900/editor-width-within.png) | [PNG](../.test-tmp/ui-captures/next-10-repeat/es/1024x700/editor-width-within.png) | [PNG](../.test-tmp/ui-captures/next-10-repeat/es/1440x900/editor-width-within.png) |
| DetailPanel / Width estimate, 100 px (`editor-width-exceeds.png`) | [PNG](../.test-tmp/ui-captures/next-10-repeat/en/1024x700/editor-width-exceeds.png) | [PNG](../.test-tmp/ui-captures/next-10-repeat/en/1440x900/editor-width-exceeds.png) | [PNG](../.test-tmp/ui-captures/next-10-repeat/es/1024x700/editor-width-exceeds.png) | [PNG](../.test-tmp/ui-captures/next-10-repeat/es/1440x900/editor-width-exceeds.png) |
| One file, translated + reviewed, live count 5 (`export-scope.png`) | [PNG](../.test-tmp/ui-captures/next-10-repeat/en/1024x700/export-scope.png) | [PNG](../.test-tmp/ui-captures/next-10-repeat/en/1440x900/export-scope.png) | [PNG](../.test-tmp/ui-captures/next-10-repeat/es/1024x700/export-scope.png) | [PNG](../.test-tmp/ui-captures/next-10-repeat/es/1440x900/export-scope.png) |
| One file, approved, live count 0 (`export-zero.png`) | [PNG](../.test-tmp/ui-captures/next-10-repeat/en/1024x700/export-zero.png) | [PNG](../.test-tmp/ui-captures/next-10-repeat/en/1440x900/export-zero.png) | [PNG](../.test-tmp/ui-captures/next-10-repeat/es/1024x700/export-zero.png) | [PNG](../.test-tmp/ui-captures/next-10-repeat/es/1440x900/export-zero.png) |
| Import summary, keep existing (`import-keep.png`) | [PNG](../.test-tmp/ui-captures/next-10-repeat/en/1024x700/import-keep.png) | [PNG](../.test-tmp/ui-captures/next-10-repeat/en/1440x900/import-keep.png) | [PNG](../.test-tmp/ui-captures/next-10-repeat/es/1024x700/import-keep.png) | [PNG](../.test-tmp/ui-captures/next-10-repeat/es/1440x900/import-keep.png) |
| Import summary and first replacement rows (`import-replace.png`) | [PNG](../.test-tmp/ui-captures/next-10-repeat/en/1024x700/import-replace.png) | [PNG](../.test-tmp/ui-captures/next-10-repeat/en/1440x900/import-replace.png) | [PNG](../.test-tmp/ui-captures/next-10-repeat/es/1024x700/import-replace.png) | [PNG](../.test-tmp/ui-captures/next-10-repeat/es/1440x900/import-replace.png) |
| Replacement table scrolled to final rows (`import-replace-table.png`) | [PNG](../.test-tmp/ui-captures/next-10-repeat/en/1024x700/import-replace-table.png) | [PNG](../.test-tmp/ui-captures/next-10-repeat/en/1440x900/import-replace-table.png) | [PNG](../.test-tmp/ui-captures/next-10-repeat/es/1024x700/import-replace-table.png) | [PNG](../.test-tmp/ui-captures/next-10-repeat/es/1440x900/import-replace-table.png) |
| Stale-plan error and repeat-preview control (`import-stale.png`) | [PNG](../.test-tmp/ui-captures/next-10-repeat/en/1024x700/import-stale.png) | [PNG](../.test-tmp/ui-captures/next-10-repeat/en/1440x900/import-stale.png) | [PNG](../.test-tmp/ui-captures/next-10-repeat/es/1024x700/import-stale.png) | [PNG](../.test-tmp/ui-captures/next-10-repeat/es/1440x900/import-stale.png) |
| InconsistentTranslation advisory chip and two issues (`qa-inconsistent.png`) | [PNG](../.test-tmp/ui-captures/next-10-repeat/en/1024x700/qa-inconsistent.png) | [PNG](../.test-tmp/ui-captures/next-10-repeat/en/1440x900/qa-inconsistent.png) | [PNG](../.test-tmp/ui-captures/next-10-repeat/es/1024x700/qa-inconsistent.png) | [PNG](../.test-tmp/ui-captures/next-10-repeat/es/1440x900/qa-inconsistent.png) |
| Persistent merge warning after 4.2 seconds (`warning-toast.png`) | [PNG](../.test-tmp/ui-captures/next-10-repeat/en/1024x700/warning-toast.png) | [PNG](../.test-tmp/ui-captures/next-10-repeat/en/1440x900/warning-toast.png) | [PNG](../.test-tmp/ui-captures/next-10-repeat/es/1024x700/warning-toast.png) | [PNG](../.test-tmp/ui-captures/next-10-repeat/es/1440x900/warning-toast.png) |

The inspected development baseline is under `.test-tmp/ui-captures/next-10-baseline/{en,es}/{1024x700,1440x900}/` with the same ten filenames, and its own manifest.

