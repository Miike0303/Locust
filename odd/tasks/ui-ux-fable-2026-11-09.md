# UI/UX check with Fable 5.2 (2026-11-09): findings queued for the next session

Objective: fix the UI/UX findings below. Status: **nothing fixed yet**; owner 2026-11-09: "todo eso guardalo para la siguiente sesion".
Source: three read-only Fable 5.2 reviewers on the 0.3.0 release build (`main` code `777998c`). Two looked at 44 real production captures (`tools/capture_ui.mjs`, 11 scenarios x EN/ES x 1440x900 and 1024x700); the third read the code of the views without captures. Nobody ran the app. Not reviewed: Pivot modal, Patch Pack tab internals; Translate, Inject, Patch and Queue modals at 900x600; expanded sidebar.
Paths are under `apps/desktop/src/` unless stated. Line numbers are as of `777998c`; re-check before editing.

## Specs

### High

- S1. **Tabbing through the API key field deletes the saved key.** `pages/settings/ProvidersSection.tsx:86-92`: a stored key comes back masked (`***`), the field renders empty and `onBlur` saves the empty value; `crates/core/src/config.rs:166-167` treats an empty `api_key` as an explicit clear. Parent confirmed both code sites. Fix: save only when non-empty and changed, add an explicit "Remove key" action, show "saved" as placeholder.
- S2. **Import "Apply" is disabled with no visible reason.** `components/ExportModal.tsx:543` gates on the unstyled checkbox at `:482-487`; the caption reads "language not declared or ambiguous (not declared)" (`:476`, `lib/i18n/en.ts:1333`, `es.ts:1311`). Fix: drop the parenthesis when nothing is declared, put the checkbox in a warning callout, add a hint next to the disabled button.
- S3. **Replace mode gives no destructive cue.** `ExportModal.tsx:469-471`, `:544`: "Replace 8" is neutral grey and the button is the same green as the safe mode. Fix: warning colour for the count and a button label such as "Replace 8 translations" when `!keepExisting && preview.replace > 0`.

### Medium

- S4. Toast covers the editor header actions (both sizes; reaches Patch at 900 px). `components/ToastContainer.tsx:27` (`fixed top-4 right-4`). Fix: anchor bottom-right above the BottomBar or below the header.
- S5. Detail panel header scrolls away and Status starts below the fold (worse in ES). `components/DetailPanel.tsx:178-193`. Fix: sticky header or scroll only the body; consider Status above the width estimate or collapsing the estimate in compact.
- S6. Import preview has two nested scroll areas (about 2.5 rows visible at 700 px). `ExportModal.tsx:489` (`max-h-64 overflow-auto`) inside the modal body (`:351`). Fix: one scroller with a sticky `thead`.
- S7. Editor table squeezes below about 1000 px (code only). `components/StringTable.tsx:182,191,204,212,229` (940 px of hints, no min width), translation `truncate` at `:111`; a saved expanded sidebar overrides compact (`components/Layout.tsx:53`, `:89`). Fix: hide File and Tags in compact, `min-w` on Translation, optionally ignore the expanded preference below about 1000 px.
- S8. Filtered editor shows no active filter ("1-2 of 2" with only "Clear"). Cause not confirmed; probably a hidden "changed rows" filter in `pages/Editor.tsx`. Fix: visible chip naming the filter.
- S9. QA rows for an inconsistent translation are identical. `pages/Qa.tsx:141-144` has no translation column. Fix: show each row's translation.
- S10. Stale import preview shows two actions for one thing in danger colours, with different ES labels. `ExportModal.tsx:506-510`, `:538-541`. Fix: warning colours, one action, same label.
- S11. Workflow step label and status run together ("3 Check Not checked yet"). `components/WorkflowGuideBanner.tsx:85-91`. Fix: muted status with a separator or a pill.
- S12. Width-estimate panel is verbose; ES lines wrap to two rows; font path shows the start of the path; over-width only differs by tint. `components/TextFitPreview.tsx:164-167`, `:184-185`, `es.ts:77-78`. Fix: "192 / 900 px - fits/exceeds", an icon for exceeds, file name with full path in `title`.
- S13. `?` never opens Shortcuts on common layouts. `lib/hotkeys.ts:65` compares Shift before the `?` special case at `:68-69`. Fix: skip the Shift comparison when `binding.key === "?"`.
- S14. Review: Ctrl+A approves, Ctrl+S skips, Ctrl+P goes back. `pages/Review.tsx:407-410` has no modifier check. Fix: return early on ctrl/meta/alt except Ctrl+Enter.
- S15. Patch dialog "Rollback" runs on one click and is enabled with an empty game path. `components/patch/PatchApplyTab.tsx:429-439`, `components/PatchModal.tsx:608-620`. Fix: disable without a path; require the preview or a destructive confirm (as `pages/settings/DataSection.tsx:87` does for restore).
- S16. Speakers: "Clear saved note" saves an empty note with no confirmation (`pages/Speakers.tsx:141,159`); unsaved note text is lost on leaving (`:45`); the selected speaker has no visual state (`:129`).
- S17. Inject dialog defaults the language to Spanish regardless of the project target. `components/InjectModal.tsx:111-113`. Fix: default from the last-used translation target, as `components/FontPatchDialog.tsx:27-31` does.
- S18. Settings > History says "no runs" when no project is open. `pages/settings/HistorySection.tsx:98,112-115`. Fix: derive "no project" from the store (as `pages/settings/GlossarySection.tsx:32`).
- S19. Queue: Start stays enabled when the provider has no API key. `components/QueuePanel.tsx:599-616,645-646`.
- S20. Update checker: "Later" is final for the session and a failed startup check is only logged. `components/UpdateChecker.tsx:50-53,162-167`. Fix: a "Check for updates" action in Settings or on the version label.
- S21. Apply-patch dialog opened from Home ignores Escape. `components/PatchModal.tsx:160` (`ownEscape: false`), `pages/Welcome.tsx:689-697`, `pages/Editor.tsx:343`.
- S22. Activity Log is not keyboard-usable: clickable `div` rows, no Escape/focus/dialog role, clear without confirmation. `components/ActivityLog.tsx:50,98-108`.
- S23. Spanish users still read English: hard-coded Activity Log details (`components/InjectModal.tsx:292,317-331,348`, `components/PatchModal.tsx:639`), raw mode "Modo: replace" (`InjectModal.tsx:683`), unmapped backend text (`lib/apiError.ts:1-8`).

### Low

- S24. Import summary is cryptic ("Fill 1 empty - Replace 0 - ..."). `en.ts:1344-1345`, `es.ts:1322-1323`.
- S25. "Workflow options" disclosure looks like an empty field; malformed focus ring. `components/FastWorkflowModal.tsx:170`.
- S26. Inconsistent buttons and focus rings: QA primary action is a pale tint (`pages/Qa.tsx:23,117`); Export "Download" uses a folder icon (`ExportModal.tsx:556`); black versus green focus rings.
- S27. Detail panel close is a bare "x" of about 10 px, rendered red. `components/DetailPanel.tsx:183-190`.
- S28. QA row actions stack vertically. `pages/Qa.tsx:145`.
- S29. Filter bar wraps in Spanish (and in English at 900 px). `components/FilterBar.tsx:98,119,221`.
- S30. Workflow banner description is the only shrinkable item and truncates at 900 px. `components/WorkflowGuideBanner.tsx:100`.
- S31. Naming: "Inject" versus "insertion" in recovery (`en.ts:313-338`); QA / Quality assurance / Run validation; Speakers / Speaker workspace; `es.ts` mixes usted (about 88 strings) and tu (about 22); "Pivotar" is a calque.
- S32. Shortcuts help omits Review keys; QA and Speakers have no Alt shortcut; Alt numbers do not follow sidebar order. `lib/hotkeys.ts:35-56`.
- S33. Icon-only buttons without accessible names: `components/HotkeyHelp.tsx:74-79`, `components/UpdateChecker.tsx:218-223`.
- S34. Providers section: labels not associated with inputs (`ProvidersSection.tsx:85,98,109,129`), test errors cut to 60 characters (`:157`), Test not disabled while testing.
- S35. Dates ignore the UI language: `pages/TranslationMemory.tsx:267`, `pages/settings/DataSection.tsx:231`, `pages/settings/HistorySection.tsx:17`.
- S36. Missing error states: backups load failure has no Retry (`DataSection.tsx:200-203`); Home ignores `formats` and `config` query errors (`pages/Welcome.tsx:144-155`).

### Reported as solid (no change needed)

Export modal in both languages and sizes; no unreachable content or action at 1024x700; shared modal shell keeps footers pinned (`lib/modalA11y.ts:60-73`); Dashboard, QA page, Memory, Glossary, Appearance, Defaults and model policy have loading, empty, error and retry states; backup restore/delete, recovery and resume dialogs have focus trap, Escape and confirmations; EN and ES catalogs have identical key sets (1557).

## Tasks

- T1 | S1 | inline | pending | API key blur fix with a RED test first.
- T2 | S2, S3, S6, S10, S24 | inline | pending | Import dialog.
- T3 | S4, S5, S7, S8, S11, S12, S27, S29, S30 | inline | pending | Editor chrome and compact layout.
- T4 | S13, S14, S32, S33 | inline | pending | Keyboard and shortcuts.
- T5 | S15, S16, S19, S21, S22 | inline | pending | Destructive actions and dialogs.
- T6 | S9, S17, S18, S20, S23, S25, S26, S28, S31, S34, S35, S36 | inline | pending | Remaining views, i18n and polish.

After each task: `npm --prefix apps/desktop run build`, `npm run test:unit` in `apps/desktop`, then `node tools/capture_ui.mjs` with the release or debug CLI to re-capture and compare.

## Log

- L1 (owner, 2026-11-09): "haz una checada nuevva con fable, del ui ux, vistas, resonsibe, etc"
- L2 (owner, 2026-11-09, after the findings were reported): "todo eso guardalo para la siguiente sesion"
- L3: Captures were generated with `target/release/locust.exe` (0.3.0): 44/44 captures, runner success. They lived in the session scratchpad and were not kept.
