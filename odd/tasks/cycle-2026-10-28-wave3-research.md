Prioritize **W3-01–05 for speed**, then W3-06–08 for quality and workflow. The four requested follow-ups are W3-09–12.

Read-only review at `c92d4fd`; no edits, builds, or tests, and no `tmp/` searches or reads. **F = verified source fact; H = hypothesis requiring measurement.** Sizes: S ≈ 1 day, M ≈ 2–3 days, L ≈ 4–7 days.

Path prefixes below expand exactly: `C=crates/core/src`, `P=crates/providers/src`, `S=crates/server/src`, `L=crates/cli/src`, `U=apps/desktop/src`, `D=apps/desktop/src-tauri/src`. Files marked **new** are proposed.

Already present—and excluded as new features—are bulk TM lookup/write, repeated-source deduplication, canonical glossary masking, provider-specific Google defaults, and translation ETA. Google’s documented 7.3× result measures a provider fixture, not the entire game workflow. [Evidence](C:/Projects/Locust/docs/GOOGLE-FAST-MODE.md:23)

1. **W3-01 — Batch glossary preparation, persistence, and installation**  
   **Category:** Speed · **Size:** L · **Risk:** Medium.
   - **Value:** Start translation sooner on large projects with substantial dictionaries; finish glossary-only rows without thousands of commits.
   - **F:** Hint construction scans and lowercases terms per request; exact lookup scans the dictionary. Exact hits save individually through a one-item transaction. Pipeline installation launches one CLI process per term, with full invariants before each command. [Hints](C:/Projects/Locust/crates/core/src/glossary.rs:279), [exact saves](C:/Projects/Locust/crates/core/src/translation/manager.rs:974), [transactions](C:/Projects/Locust/crates/core/src/database/strings.rs:761), [installation](C:/Projects/Locust/tools/ja_en_es_pipeline.py:1440), [invariants](C:/Projects/Locust/tools/ja_en_es_pipeline.py:1469).
   - **Work / H:** Compile reusable exact/hint indexes; batch guarded exact-hit saves; install each language’s glossary in one validated bulk command. Gains depend on dictionary size and hit rate.
   - **Files:** `C/glossary.rs`, `C/translation/manager.rs`, `L/main.rs`, `L/commands/io.rs`, `tools/ja_en_es_pipeline.py`, `tools/test_ja_en_es_pipeline.py`. Reuse existing `merge_glossary_entries`.
   - **Acceptance:** Compare 100k rows/1k terms; count matching work, transactions, and subprocesses. Preserve Unicode/case/overlap/conflict behavior, source guards, provider attribution, and resume-manifest compatibility.
   - **Parallel-safety:** Independent of W3-03; serialize shared files with W3-02/04/05/06/08/09.

2. **W3-02 — Preserve successful Google subchunks and retry only failures**  
   **Category:** Speed + reliability · **Size:** L · **Risk:** Medium.
   - **Value:** A failed request or malformed singleton should not force successful translations through Google again.
   - **F:** Google accumulates results locally, but a later subchunk error returns the whole call as an error. Core transport retry then resends the original request vector; core also correctly rejects arbitrary incomplete result vectors. [Subchunks](C:/Projects/Locust/crates/providers/src/google.rs:102), [aggregation](C:/Projects/Locust/crates/providers/src/google.rs:310), [retry](C:/Projects/Locust/crates/core/src/translation/retry.rs:199), [ID validation](C:/Projects/Locust/crates/core/src/translation/manager.rs:1321).
   - **Work / H:** Introduce an explicit opt-in outcome containing validated successes and failed IDs; retain successes across retry rounds. Expected benefit is concentrated in imperfect network/provider runs.
   - **Files:** `P/google.rs`, `C/translation/provider.rs`, `C/translation/retry.rs`, `C/translation/manager.rs`, `C/translation/tests.rs`.
   - **Acceptance:** Simulate successful chunks followed by 503, malformed output, and an invalid singleton. Successful IDs are saved once and never resent; failures remain pending; duplicate/foreign IDs remain rejected; cancellation and HTTP pacing stay bounded.
   - **Parallel-safety:** Independent of W3-03/04/05/09; serialize manager/provider changes with W3-01/06/08.

3. **W3-03 — Process independent Ren’Py and RPG Maker files concurrently**  
   **Category:** Extraction/injection speed · **Size:** L · **Risk:** Medium–high.
   - **Value:** Use available CPU and SSD throughput before and after provider translation.
   - **F:** Ren’Py extraction processes loose scripts and archives serially; loose injection processes file groups serially. RPG Maker likewise extracts and injects file groups serially, although it already parses JSON once per injection file. [Ren’Py extraction](C:/Projects/Locust/crates/formats/src/renpy.rs:2715), [injection](C:/Projects/Locust/crates/formats/src/renpy.rs:2950), [RPG extraction](C:/Projects/Locust/crates/formats/src/rpgmaker_mv.rs:1941), [injection](C:/Projects/Locust/crates/formats/src/rpgmaker_mv.rs:2106).
   - **Work / H:** Add bounded file workers, initially for these two engines. Keep shared archives, collision resolution, and generated runtime files under one coordinator. Speedup depends on file distribution and storage.
   - **Files:** `crates/formats/src/renpy.rs`, `crates/formats/src/rpgmaker_mv.rs`, `crates/formats/src/lib.rs`, `crates/formats/src/file_workers.rs` **new**, `crates/formats/tests/w3_parallel_files.rs` **new**.
   - **Acceptance:** Compare workers 1/2/4 on many-small-file and few-large-file games; retain IDs, diagnostics/completeness, injected content, revision proofs, and failure behavior. Measure peak RSS as well as wall time.
   - **Parallel-safety:** Disjoint from every other listed implementation; keep each writer’s fixtures and game copies separate.

4. **W3-04 — Measure and accelerate whole-game verification and copying**  
   **Category:** End-to-end speed · **Size:** L · **Risk:** High.
   - **Value:** Reduce time spent moving and verifying gigabytes of assets unrelated to translated text.
   - **F:** The staged pipeline creates pristine, working, and installation copies and repeatedly hashes trees. Its recorded command timer starts *after* invariant scans. Backup creation separately inventories, copies, rescans the source, and verifies payload contents; Replace copying is serial. [Copies](C:/Projects/Locust/tools/ja_en_es_pipeline.py:1767), [scans](C:/Projects/Locust/tools/ja_en_es_pipeline.py:1393), [timer](C:/Projects/Locust/tools/ja_en_es_pipeline.py:1469), [backup](C:/Projects/Locust/crates/core/src/backup.rs:835), [Replace](C:/Projects/Locust/crates/core/src/extraction.rs:1715).
   - **Work / H:** First expose inclusive copy/hash/invariant timings and byte counts; then use bounded parallel hashing/copying where measured useful. Preserve the existing source-after-copy and payload proofs.
   - **Files:** `C/backup.rs`, `C/extraction.rs`, `tools/ja_en_es_pipeline.py`, `tools/test_ja_en_es_pipeline.py`, `tools/benchmark_workflow.py` **new**.
   - **Acceptance:** Report extract → both translations → validation → inject → pack wall time, including preparation. Compare SSD/HDD behavior; detect same-size/same-timestamp drift; preserve rollback and immutable-backup behavior.
   - **Parallel-safety:** Independent of W3-02/03/05/06; serialize pipeline edits with W3-01.

5. **W3-05 — Offer a fast patch-compression mode**  
   **Category:** Patch speed · **Size:** M · **Risk:** Low–medium.
   - **Value:** Produce test patches faster, especially when modified payloads include large engine containers.
   - **F:** Packing uses one serial Deflated ZIP writer. Payload streaming already hashes the exact bytes being written, so adding a separate buffering/hash pass would duplicate existing work. [Compression](C:/Projects/Locust/crates/core/src/patch/pack.rs:533), [streaming](C:/Projects/Locust/crates/core/src/patch/pack.rs:575).
   - **Work / H:** Add an opt-in fast compression policy; compare low-level Deflate and Stored modes against current behavior. Compression CPU cost and archive-size tradeoffs remain unmeasured.
   - **Files:** `C/patch/pack.rs`, `C/patch/mod.rs`, `L/main.rs`, `L/commands/patch.rs`, `S/routes/patch.rs`, `U/lib/api.ts`, `U/components/patch/PatchPackTab.tsx`.
   - **Acceptance:** Measure pack/apply time, ZIP size, and RSS on text-heavy and container-heavy recordings. Preserve payload hashes, strict apply, rollback, containment, and existing default behavior.
   - **Parallel-safety:** Backend work is disjoint from W3-02/03/04/06/10; serialize CLI/API/UI seams with their other owners.

6. **W3-06 — Make Google repair and constrained glossary policies effective**  
   **Category:** Quality with low local overhead · **Size:** L · **Risk:** High.
   - **Value:** Preserve variables and canonical character/term names without wasting repeated requests.
   - **F:** Controls are already masked before dispatch, and canonical terms use a compiled trie—the relevant counterpart to the recorded third-party flashtext approach. However, binary/shared-slot rows bypass canonical masking; Google sends source text without glossary/context hints. Control-repair instructions alter context, leaving Google’s transmitted source unchanged. [Masking](C:/Projects/Locust/crates/core/src/glossary.rs:38), [exclusions](C:/Projects/Locust/crates/core/src/translation/manager.rs:863), [Google fields](C:/Projects/Locust/crates/providers/src/google.rs:137), [repair](C:/Projects/Locust/crates/core/src/translation/protection.rs:222).
   - **Work / H:** Extend canonical insertion where final encoded/group capacity can be validated; use an MT-specific repair strategy or targeted fallback instead of repeating instruction-only corrections. Alternate sentinel effectiveness needs measurement.
   - **Files:** `C/glossary.rs`, `C/placeholder.rs`, `C/translation/provider.rs`, `C/translation/protection.rs`, `C/translation/manager.rs`, `C/translation/tests.rs`.
   - **Acceptance:** JA→EN→ES fixtures cover Ren’Py interpolation, RPG variables/nameboxes, overlapping names, literal sentinel lookalikes, fixed slots, and shared cells. No missing/extra controls, silent truncation, or accepted glossary overflow; retry only affected IDs.
   - **Parallel-safety:** Serialize with W3-01/02/08; disjoint from engine and I/O workers.

7. **W3-07 — One-click whole-game fast workflow with stage-aware resume**  
   **Category:** Workflow · **Size:** L · **Risk:** Medium–high.
   - **Value:** Move from selecting a game to a reviewable patch with fewer manual transitions.
   - **F:** Desktop queue work currently opens, translates, and validates; injection/packing are separate. The CLI pipeline already models strict JA→EN→ES checkpoints and bounded resume. Translation ETA already exists, but estimates only the current translation pass. [Queue](C:/Projects/Locust/apps/desktop/src/stores/queueStore.ts:190), [validation](C:/Projects/Locust/apps/desktop/src/stores/queueStore.ts:302), [pipeline](C:/Projects/Locust/tools/ja_en_es_pipeline.py:1779), [ETA](C:/Projects/Locust/apps/desktop/src/components/BottomBar.tsx:7).
   - **Work / H:** Add a desktop preset over existing APIs: extract, translate, strict pivot, translate, validate, inject, pack. Persist project-bound stage receipts; show current stage and an approximate remaining-workflow ETA.
   - **Files:** `U/pages/Editor.tsx`, `U/components/BottomBar.tsx`, `U/components/FastWorkflowModal.tsx` **new**, `U/lib/fastWorkflow.ts` **new**, `U/stores/fastWorkflowStore.ts` **new**, `U/lib/fastWorkflow.test.ts` **new**.
   - **Acceptance:** Resume a completed English stage without repeating it; reject changed projects/options; distinguish provider completion from workflow completion; reconcile uncertain injection/pack outcomes before repeating writes.
   - **Parallel-safety:** Mostly disjoint frontend ownership; integrate after relevant backend contracts settle. Coordinate shared locale files.

8. **W3-08 — Scoped translation, file priorities, and retry-selected failures**  
   **Category:** Workflow + speed · **Size:** L · **Risk:** Medium.
   - **Value:** Translate menus or the opening chapter first and retry a precise failed subset.
   - **F:** Job options lack file/ID scope; fallback reloads all pending rows. Pending-only fallback already protects successful translations, so this extends selection rather than recreating resume. [Options](C:/Projects/Locust/crates/core/src/translation/provider.rs:44), [pending selection](C:/Projects/Locust/crates/core/src/translation/manager.rs:2007), [fallback](C:/Projects/Locust/crates/core/src/translation/manager.rs:2091).
   - **Work / H:** Capture an immutable selected-ID scope, order it by file priority, and carry that same scope through memory, glossary, providers, fallback, and retry.
   - **Files:** `C/translation/provider.rs`, `C/translation/manager.rs`, `C/database/strings.rs`, `L/main.rs`, `L/commands/translate.rs`, `S/routes/translate.rs`, `U/lib/api.ts`, `U/lib/translationDefaults.ts`, `U/components/TranslationModal.tsx`.
   - **Acceptance:** Unselected rows receive no writes or provider calls; priorities affect dispatch order; retries preserve successful rows; shared TextAsset constraints and cancellation still hold.
   - **Parallel-safety:** Depends on W3-02’s failure contract; serialize shared translation files with W3-01/06 and API edits with W3-05/09/11/12.

9. **W3-09 — Validate catalog languages against the project**  
   **Category:** Import quality follow-up · **Size:** M · **Risk:** Medium.
   - **Value:** Prevent a valid catalog in the wrong language from populating matching IDs.
   - **F:** Parsing returns updates/counts without catalog-language metadata; preview proceeds directly to row matching. TMX derives its pair from the document. [Parser](C:/Projects/Locust/crates/core/src/export.rs:675), [preview](C:/Projects/Locust/crates/server/src/routes/io.rs:477).
   - **Work:** Retain declared language metadata; compare against the project’s semantic pair, including pivots; bind the accepted pair into the preview token. Define explicit handling for missing/ambiguous headers.
   - **Files:** `C/export.rs`, `S/routes/io.rs`, `L/commands/io.rs`, `D/commands.rs`, `U/lib/api.ts`, `U/components/ExportModal.tsx`, `crates/server/tests/import_preview.rs`.
   - **Acceptance:** Matching pairs succeed; reversed/wrong targets produce zero writes; project-language changes stale the token; HTTP/Tauri/CLI agree; distinguish `es` from `es-ES` consistently.
   - **Parallel-safety:** Serialize import/export seams with W3-01/12 and API edits with other UI contracts.

10. **W3-10 — Allow text-fit snapshots while jobs run**  
    **Category:** Workflow follow-up · **Size:** M · **Risk:** Medium.
    - **Value:** Review dialogue width while translation continues.
    - **F:** Text-fit releases admission before font I/O, but first requires the same exclusive reservation held by translation jobs. Cloning `Arc<Database>` alone is insufficient because project opening swaps its live connection. [Admission](C:/Projects/Locust/crates/server/src/routes/text_fit.rs:24), [connection swap](C:/Projects/Locust/crates/core/src/database/connection.rs:100).
    - **Work:** Capture an atomic, project-bound row snapshot through a read path that does not require job exclusivity.
    - **Files:** `S/lib.rs`, `S/routes/text_fit.rs`, `C/database/connection.rs`, `C/database/strings.rs`.
    - **Acceptance:** HTTP and Tauri estimates work during translation; concurrent project switches cannot return another project’s row; font work remains outside connection locks.
    - **Parallel-safety:** Disjoint from most speed units; coordinate `strings.rs` with W3-08 and server registration changes with W3-12.

11. **W3-11 — Give provenance QA structured reasons**  
    **Category:** Localization/diagnostics follow-up · **Size:** M · **Risk:** Medium.
    - **Value:** Explain the repair needed in the user’s language.
    - **F:** Provenance issues carry a unit kind plus an English message; producers stringify errors. Existing localization deliberately leaves these diagnostics intact. [Wire model](C:/Projects/Locust/crates/core/src/models.rs:437), [producers](C:/Projects/Locust/crates/core/src/validation.rs:485), [localization](C:/Projects/Locust/apps/desktop/src/lib/validationLabels.ts:28).
    - **Work:** Add optional typed reason/parameters from producers through persistence and API; retain the existing PascalCase kind and readable fallback message.
    - **Files:** `C/error.rs`, `C/models.rs`, `C/validation.rs`, `C/textasset_group.rs`, `C/database/schema.rs`, `C/database/validation.rs`, `U/lib/api.ts`, `U/lib/validationLabels.ts`, `U/lib/validationLabels.test.ts`.
    - **Acceptance:** Old databases/messages remain readable; ignored issues retain identity; reasons survive storage; known reasons localize without parsing English; unknown reasons fall back safely.
    - **Parallel-safety:** Start after the current QA-localization writer finishes; serialize API/locale edits and model changes with their owners.

12. **W3-12 — Expose TMX export in desktop and HTTP**  
    **Category:** Workflow follow-up · **Size:** M · **Risk:** Low.
    - **Value:** Export Locust project catalogs without switching to CLI.
    - **F:** Core/CLI already export restricted TMX; desktop export types and Tauri dispatch allow only PO/XLIFF, and the modal explicitly excludes TMX export. [Codec](C:/Projects/Locust/crates/core/src/export/tmx.rs:82), [desktop type](C:/Projects/Locust/apps/desktop/src/lib/api.ts:771), [dispatch](C:/Projects/Locust/apps/desktop/src-tauri/src/commands.rs:582), [modal](C:/Projects/Locust/apps/desktop/src/components/ExportModal.tsx:120).
    - **Work:** Connect the existing fallible codec to desktop/HTTP, with explicit source/target languages and existing file/status scope.
    - **Files:** `S/lib.rs`, `S/routes/io.rs`, `D/commands.rs`, `U/lib/api.ts`, `U/components/ExportModal.tsx`, `crates/server/tests/export_subset.rs`.
    - **Acceptance:** Scoped round-trip preserves IDs, exact sources, languages, and review markers; oversized/invalid catalogs leave destination files intact; preserve the documented restricted TMX profile.
    - **Parallel-safety:** Serialize W3-09’s import/export files and shared API changes.

For dispatch, **W3-01, W3-03, W3-05, and W3-10** have mostly separate implementation areas; assign one integration owner for shared CLI/API changes. All UI units share `U/lib/i18n/en.ts` and `U/lib/i18n/es.ts`; merge those through one owner. Performance acceptance above describes future implementation checks—none were executed during this research.