# Cycle 2026-11-01: wave 5 research (RESEARCH-5)

Codex gpt-6.1-sol high, read-only, single researcher, on `a650898`. Parent spot-checked SearchReplaceModal.tsx:47, memory.rs:21-23 (radius ceil(2n/5): 11 -> upper 16 excludes a length-17 candidate scoring 0.647) and strings.rs:478 (`?6 = 0 OR translation IS ?7`). Links point at the lane-a worktree that produced them; paths are repo-relative after the lane prefix.

**Recommended wave: four defects, three capabilities, three optimizations.** All are bounded to ≤3 writer-days. No files were changed, and no builds or tests were run.

**F** = verified source fact, including code-derived outcomes. **H** = proposed scope, value, estimate, risk, or acceptance criterion. Defects below are **code-proven, not runtime-reproduced**.

Paths below use `U=apps/desktop/src`, `C=crates/core/src`, `S=crates/server/src`, `D=apps/desktop/src-tauri/src`. Each proposal’s files list is its proposed edit boundary, including tests.

1. **W5-01 — Guard approvals against newer translations**  
   **H · defect · M, 2–3 days · medium risk.** Value: prevent approving unseen text or overwriting another reviewer’s correction.

   **F/evidence:** Review constructs approval patches without `expected_translation`, then submits them directly. HTTP and Tauri also ignore the expectation on status-only patches. ([Review.tsx:34](/C:/Projects/Locust-lane-a/apps/desktop/src/pages/Review.tsx:34), [Review.tsx:229](/C:/Projects/Locust-lane-a/apps/desktop/src/pages/Review.tsx:229), [strings.rs:171](/C:/Projects/Locust-lane-a/crates/server/src/routes/strings.rs:171), [commands.rs:420](/C:/Projects/Locust-lane-a/apps/desktop/src-tauri/src/commands.rs:420))

   **F/trigger → wrong output:** Load Review with translation `V1`; another client saves `V2`; edit Review to `V-review` and approve. Its unconditional patch overwrites `V2`. Without editing, approval instead approves the unseen `V2`. The translation UPDATE disables its comparison when no expectation is supplied. ([strings.rs:478](/C:/Projects/Locust-lane-a/crates/core/src/database/strings.rs:478))

   **H/work:** Guard both translation-plus-status and status-only approvals atomically; retain the local edit on conflict. Also pass the saved baseline from DetailPanel’s status action, which currently sends only status. ([DetailPanel.tsx:138](/C:/Projects/Locust-lane-a/apps/desktop/src/components/DetailPanel.tsx:138))

   **H/files:** `U/pages/Review.tsx`, `U/pages/Review.test.tsx`, `U/components/DetailPanel.tsx`, `U/components/staleSaveConflict.test.tsx`, `C/database/strings.rs`, `S/routes/strings.rs`, `D/commands.rs`.  
   **H/acceptance:** Both interleavings above return conflict through HTTP/Tauri, preserve `V2` and its status, and keep the reviewer’s draft. Unchanged approvals preserve provider attribution.  
   **H/shared files:** W5-04/W5-07; coordinate `strings.rs` with the owned W4-07b counter work.

2. **W5-02 — Restore the “Before” version from history**  
   **H · defect · S, 1 day · low risk.** Value: recover the translation immediately preceding the earliest retained change.

   **F/evidence:** History records both old and new translations, but Restore is offered only for `new_translation`. Migration creates the history table without backfilling existing translations. ([schema.rs:151](/C:/Projects/Locust-lane-a/crates/core/src/database/schema.rs:151), [schema.rs:170](/C:/Projects/Locust-lane-a/crates/core/src/database/schema.rs:170), [StringHistory.tsx:50](/C:/Projects/Locust-lane-a/apps/desktop/src/components/StringHistory.tsx:50))

   **F/trigger → wrong output:** Open an older translated project with no revisions, then change `V1` to `V2`. The sole revision displays `V1 → V2`, but with current text `V2` there is no restore action: only the already-current “After” value is considered. ([StringHistory.tsx:53](/C:/Projects/Locust-lane-a/apps/desktop/src/components/StringHistory.tsx:53), [stringHistory.ts:18](/C:/Projects/Locust-lane-a/apps/desktop/src/lib/stringHistory.ts:18))

   **H/files:** `U/components/StringHistory.tsx`, `U/components/StringHistory.test.tsx`, `U/lib/stringHistory.ts`, `U/lib/stringHistory.test.ts`, `U/lib/i18n/en.ts`, `U/lib/i18n/es.ts`.  
   **H/acceptance:** A single `V1→V2` revision offers a clearly labelled restore of `V1`; clicking it uses the existing guarded restore path. Cover empty strings and the oldest retained revision.  
   **H/shared files:** Locale catalogs with W5-04/W5-05/W5-06.

3. **W5-03 — Correct the asymmetric fuzzy-memory length cutoff**  
   **H · defect · M, 2 days · medium risk.** Value: recover useful translation-memory suggestions for modestly expanded dialogue.

   **F/evidence:** Candidate selection permits approximately ±40% raw length, while scoring accepts similarity ≥0.6 using distance divided by the longer normalized string. Both project and global suggestions use this candidate filter. ([memory.rs:21](/C:/Projects/Locust-lane-a/crates/core/src/database/memory.rs:21), [fuzzy.rs:34](/C:/Projects/Locust-lane-a/crates/core/src/fuzzy.rs:34), [glossary_memory.rs:171](/C:/Projects/Locust-lane-a/crates/server/src/routes/glossary_memory.rs:171), [glossary_memory.rs:259](/C:/Projects/Locust-lane-a/crates/server/src/routes/glossary_memory.rs:259))

   **F/trigger → wrong output:** Entry source `Hello world` has length 11. Memory source `Hello world again` has length 17 and score `1−6/17≈0.647`. The current upper candidate bound is `11+ceil(22/5)=16`, so the otherwise-eligible suggestion never reaches scoring.

   **H/work:** Derive conservative asymmetric bounds from the scorer’s threshold; preserve the existing candidate cap and exact-hit bypass.  
   **H/files:** `C/database/memory.rs`, `C/database/global_memory.rs`, `S/routes/glossary_memory.rs`, `crates/server/tests/memory_exact_match.rs`.  
   **H/acceptance:** The example appears through both stores; below-threshold candidates remain excluded; existing exact-hit/cap behavior remains intact. Document that the cap still prevents exhaustive fuzzy recall.  
   **H/shared files:** W5-10.

4. **W5-04 — Review one file at a time**  
   **H · capability · M, 2–3 days · medium risk.** Value: let a translator finish reviewing a newly translated scene without traversing the entire project.

   **F/evidence:** Review fetches translated/reviewed buckets using only status, limit and offset. Exact file filtering already exists in the database query. ([Review.tsx:108](/C:/Projects/Locust-lane-a/apps/desktop/src/pages/Review.tsx:108), [Review.tsx:128](/C:/Projects/Locust-lane-a/apps/desktop/src/pages/Review.tsx:128), [strings.rs:1412](/C:/Projects/Locust-lane-a/crates/core/src/database/strings.rs:1412))

   **H/work:** Add an All files/single-file selector; bind pagination, totals and approval bookkeeping to that selection. Scope changes begin a fresh review session.
   
   **H/files:** `U/pages/Review.tsx`, `U/pages/Review.test.tsx`, `U/lib/reviewQueue.ts`, `U/lib/reviewQueue.test.ts`, `U/lib/i18n/en.ts`, `U/lib/i18n/es.ts`.  
   **H/acceptance:** With 60 reviewable rows in each of two files, selecting A visits exactly A’s 60 rows across approval-driven pagination; B receives zero writes. Switching scope during a delayed fetch cannot mix queues.  
   **H/shared files:** W5-01/W5-07 and locale catalogs with W5-02/W5-05/W5-06.

5. **W5-05 — Edit glossary case sensitivity and notes in desktop**  
   **H · capability · M, 1–2 days · low–medium risk.** Value: distinguish terms such as `US` from ordinary `us` and preserve terminology notes.

   **F/evidence:** The desktop add form always sends `context: null` and `case_sensitive: false`; its table has no edit action. Storage already preserves both fields, and matching honors case sensitivity. ([GlossarySection.tsx:93](/C:/Projects/Locust-lane-a/apps/desktop/src/pages/settings/GlossarySection.tsx:93), [GlossarySection.tsx:223](/C:/Projects/Locust-lane-a/apps/desktop/src/pages/settings/GlossarySection.tsx:223), [glossary.rs:11](/C:/Projects/Locust-lane-a/crates/core/src/database/glossary.rs:11), [glossary.rs:125](/C:/Projects/Locust-lane-a/crates/core/src/glossary.rs:125))

   **H/work:** Add case sensitivity, notes and editing of existing entries; keep term/pair immutable during editing to avoid expanding into rename semantics.
   
   **H/files:** `U/pages/settings/GlossarySection.tsx`, new `U/pages/settings/GlossarySection.test.tsx`, `U/lib/i18n/en.ts`, `U/lib/i18n/es.ts`, `apps/desktop/package.json`.  
   **H/acceptance:** Create/edit `US→EE.UU.` with case sensitivity enabled and a note; reload preserves both, and lowercase `us` does not match. Editing translation alone preserves existing metadata.  
   **H/shared files:** Locale catalogs with W5-02/W5-04/W5-06.

6. **W5-06 — Add advisory QA for changed visible integers**  
   **H · capability · M, 2–3 days · medium risk.** Value: catch changed quantities, prices and quest requirements before injection.

   **F/evidence:** Current validation covers stale text, emptiness, identical text, character limits, controls, encoding, capacity and consistency; there is no numeric-mismatch kind or check. ([validation.rs:493](/C:/Projects/Locust-lane-a/crates/core/src/validation.rs:493), [validation.rs:617](/C:/Projects/Locust-lane-a/crates/core/src/validation.rs:617), [models.rs:539](/C:/Projects/Locust-lane-a/crates/core/src/models.rs:539))

   **H/work:** Start conservatively: advisory comparison of standalone visible integer multisets, normalizing fullwidth digits and excluding protected controls. Skip ambiguous decimals, grouping and embedded identifiers; never rewrite text.
   
   **H/files:** `C/models.rs`, `C/validation.rs`, new `crates/core/tests/numeric_qa.rs`, `U/lib/api.ts`, `U/lib/validationLabels.ts`, `U/lib/validationLabels.test.ts`, `U/lib/qaDashboard.ts`, `U/lib/qaDashboard.test.ts`, `U/lib/i18n/en.ts`, `U/lib/i18n/es.ts`.  
   **H/acceptance:** `Collect 12 gems → Recoge 2 gemas` produces a persisted, localized advisory; `１２→12` and unchanged control arguments do not. Ignore/revalidate works without altering translations.  
   **H/shared files:** Locale catalogs above; also overlaps owned W4-10’s models/validation/labels files—schedule after that owner.

7. **W5-07 — Build Review’s context index once per file**  
   **H · optimization · M, 2 days · medium risk.** Value: keep advancing through large scripts responsive.

   **F/evidence:** Each selected entry reconstructs the entire file from cached page promises; `reviewContext` then filters and sorts those candidates again. Cached network pages therefore do not eliminate repeated collection and sorting. ([Review.tsx:183](/C:/Projects/Locust-lane-a/apps/desktop/src/pages/Review.tsx:183), [Review.tsx:190](/C:/Projects/Locust-lane-a/apps/desktop/src/pages/Review.tsx:190), [reviewContext.ts:18](/C:/Projects/Locust-lane-a/apps/desktop/src/lib/reviewContext.ts:18))

   **H/work:** Cache one indexed file result, group by logical script, and find neighboring rows by indexed position. Bound retained files; retain current ordering.
   
   **H/files:** `U/pages/Review.tsx`, `U/pages/Review.test.tsx`, `U/lib/reviewContext.ts`, `U/lib/reviewContext.test.ts`.  
   **H/acceptance:** Across 100 selections in one 100k-row fixture, collection/index construction occurs once rather than per selection; all seven-row context windows remain identical. Measure navigation latency and retained heap. Initial whole-file loading remains outside this unit.  
   **H/shared files:** W5-01/W5-04.

8. **W5-08 — Bound rendering in the validation-result modal**  
   **H · optimization · S, 1 day · low risk.** Value: keep a large QA result usable immediately after validation.

   **F/evidence:** The modal renders every issue with `issues.map`, although the separate QA dashboard already uses a 50-row page size. The modal also derives its complete editor worklist from the full issue collection. ([ValidationResultsModal.tsx:280](/C:/Projects/Locust-lane-a/apps/desktop/src/components/ValidationResultsModal.tsx:280), [qaDashboard.ts:3](/C:/Projects/Locust-lane-a/apps/desktop/src/lib/qaDashboard.ts:3), [ValidationResultsModal.tsx:176](/C:/Projects/Locust-lane-a/apps/desktop/src/components/ValidationResultsModal.tsx:176))

   **H/work:** Paginate modal rendering using existing Previous/Next labels; retain full summary and worklist semantics.
   
   **H/files:** `U/components/ValidationResultsModal.tsx`, `U/components/ValidationResultsModal.test.tsx`.  
   **H/acceptance:** A 50k-issue response mounts ≤50 issue rows, every issue remains reachable, and Review in Editor still receives all unique IDs. Record render time and DOM reduction.  
   **H/shared files:** None with the other proposed edit boundaries.

9. **W5-09 — Treat replacement text literally in both case modes**  
   **H · defect · S, ≤1 day · low risk.** Value: prevent bulk replacement from silently changing dollar sequences.

   **F/evidence:** Case-sensitive replacement uses `split().join()`, while case-insensitive replacement passes the user’s string directly to JavaScript regex replacement. Preview and apply use that same helper. ([SearchReplaceModal.tsx:47](/C:/Projects/Locust-lane-a/apps/desktop/src/components/SearchReplaceModal.tsx:47), [SearchReplaceModal.tsx:123](/C:/Projects/Locust-lane-a/apps/desktop/src/components/SearchReplaceModal.tsx:123), [SearchReplaceModal.tsx:158](/C:/Projects/Locust-lane-a/apps/desktop/src/components/SearchReplaceModal.tsx:158))

   **F/trigger → wrong output:** Translation `Reward: gold`, find `gold`, replacement `$$`, case sensitivity off: output becomes `Reward: $`. Turning case sensitivity on produces literal `Reward: $$`. `$&` similarly expands to the matched text instead of remaining literal.

   **H/files:** `U/components/SearchReplaceModal.tsx`, `U/components/SearchReplaceModal.test.tsx`.  
   **H/acceptance:** Preview and persisted results preserve `$$`, `$&`, ``$` `` and `$'` literally in both modes; guarded batch-conflict behavior remains unchanged.  
   **H/shared files:** None.

10. **W5-10 — Prepare fuzzy comparison input once per request**  
    **H · optimization · M, 2 days · low–medium risk.** Value: reduce CPU and allocations while browsing memory suggestions.

    **F/evidence:** Every non-exact candidate calls `similarity_at_threshold`, which normalizes and allocates character vectors for both strings. Accepted candidates are normalized again for deduplication; each store supplies up to 3,000 candidates. ([glossary_memory.rs:175](/C:/Projects/Locust-lane-a/crates/server/src/routes/glossary_memory.rs:175), [glossary_memory.rs:190](/C:/Projects/Locust-lane-a/crates/server/src/routes/glossary_memory.rs:190), [glossary_memory.rs:197](/C:/Projects/Locust-lane-a/crates/server/src/routes/glossary_memory.rs:197), [fuzzy.rs:13](/C:/Projects/Locust-lane-a/crates/core/src/fuzzy.rs:13), [fuzzy.rs:32](/C:/Projects/Locust-lane-a/crates/core/src/fuzzy.rs:32))

    **H/work:** Prepare query characters once; reuse candidate preparation where compatible with the existing prefix-cap and deduplication semantics.
    
    **H/files:** `C/fuzzy.rs`, `S/routes/glossary_memory.rs`.  
    **H/acceptance:** Existing scores, ties and suggestion order remain identical across Unicode/threshold fixtures; source preparation falls from per-candidate to once per request. Retain only with measured allocation reduction and useful latency improvement.  
    **H/shared files:** W5-03; land after its candidate-bound correction.

The ranked dispatch table follows; test files are included in the exact boundaries above.

| Rank | ID | Title | Category | Size | Main files |
|---:|---|---|---|---|---|
| 1 | W5-01 | Guard stale approvals | Defect | M | `Review.tsx`, `DetailPanel.tsx`, `C/database/strings.rs`, `S/routes/strings.rs`, `D/commands.rs` |
| 2 | W5-02 | Restore history’s Before version | Defect | S | `StringHistory.tsx`, `stringHistory.ts`, locale catalogs |
| 3 | W5-03 | Correct fuzzy length cutoff | Defect | M | `C/database/memory.rs`, `global_memory.rs`, `S/routes/glossary_memory.rs` |
| 4 | W5-04 | Review one file | Capability | M | `Review.tsx`, `reviewQueue.ts`, locale catalogs |
| 5 | W5-05 | Edit glossary metadata | Capability | M | `GlossarySection.tsx`, locale catalogs, desktop `package.json` |
| 6 | W5-06 | Visible-integer QA | Capability | M | `C/models.rs`, `validation.rs`, `U/lib/api.ts`, `validationLabels.ts`, `qaDashboard.ts`, locale catalogs |
| 7 | W5-07 | Index review context once | Optimization | M | `Review.tsx`, `reviewContext.ts` |
| 8 | W5-08 | Paginate validation modal | Optimization | S | `ValidationResultsModal.tsx` |
| 9 | W5-09 | Literal replacement strings | Defect | S | `SearchReplaceModal.tsx` |
| 10 | W5-10 | Prepare fuzzy input once | Optimization | M | `C/fuzzy.rs`, `S/routes/glossary_memory.rs` |

**H/suggested two-lane allocation, disjoint across the complete proposed file sets:**

| Lane A — sequential | Lane B — sequential |
|---|---|
| W5-01 → W5-02 → W5-04 → W5-05 → W5-06 → W5-07 → W5-09 | W5-03 → W5-10 → W5-08 |

**H/initial pairing:** W5-01 + W5-03. Keep all shared Review and locale files in Lane A; Lane B owns memory scoring and the standalone validation-modal rendering change. Coordinate the separately owned W4-07b/W4-10 seams before dispatch.
