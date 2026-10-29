Ranked for the next session at `fc4eb02` on `feat/overnight-ui-api-polish`. **Read-only research completed; no edits, builds, tests, or `tmp/` access.**  
**F** = verified source or recorded evidence; **H** = hypothesis/proposed work. Sizes: **S** ≈ 1 day, **M** ≈ 2–3 days, **L** ≈ 4–7 days.  
File prefixes: `C=crates/core`, `P=crates/providers`, `F=crates/formats`, `S=crates/server`, `L=crates/cli`, `U=apps/desktop/src`, `D=apps/desktop/src-tauri`.

1. **W4-01 — Close the owed W3-03 release benchmark and choose defaults** · **M / medium risk**  
   **Value:** Establish whether parallel files actually shorten extraction/injection without slowing ordinary games.  
   **F:** One-worker baseline parity remains unproven; noisy RPG large-file injection measured approximately 743→227 ms at four workers. Automatic execution remains serial. [Evidence](C:/Projects/Locust/docs/performance/w3-03-correction-round-2.md:68)  
   **H/work:** First add an explicit historical baseline: the harness currently copies `HEAD`, which now contains W3-03 and would compare against the delivered implementation. Pin `7d5fe3f^` or verify the originally recorded baseline tree. [Harness](C:/Projects/Locust/scripts/bench_w3_release.py:67)  
   **Files:** `scripts/bench_w3_release.{py,rs}`, `docs/performance/w3-03-*`; policy changes confined to `F/src/file_workers.rs`.  
   **Acceptance/default:** Quiet release matrix, ≥5 rotated samples per bucket, workers 1/2/4/8, both engines/phases/shapes; retain CPU load, RSS, source hashes, content/report equivalence and alias behavior. Require ≤15% spread and one-worker parity. **Keep serial by default now**; enable only measured phase/shape combinations with ≥15% median improvement and no correctness regression.

2. **W4-02 — Finish W3-04 copy/hash work with real CLI evidence** · **M / high risk**  
   **Value:** Reduce whole-game preparation and verification time, especially for asset-heavy games.  
   **F:** Lane-b remains unintegrated. Its recorded 75.2→44.2 s result uses real filesystem I/O but mocked child commands, with wide timing ranges; real CLI and controlled storage comparisons remain owed. [Lane report](C:/Projects/Locust-lane-b/docs/performance/workflow-copy-verify.md:119)  
   **H/work:** Review the existing four-worker/8 MiB policy before adopting it; measure inclusive preparation, invariants, backup, Replace copying and publication. Preserve W3-01’s remaining content proofs.  
   **Files:** `C/src/{backup,extraction}.rs`, lane’s I/O helper, `tools/{ja_en_es_pipeline,test_ja_en_es_pipeline,benchmark_workflow}.py`, `C/tests/workflow_copy_verify.rs`.  
   **Acceptance:** Matched release binaries and pinned baseline; real scripted-provider pipeline plus storage measurements with cache state identified. Same-size/restored-timestamp source drift and payload corruption must refuse; all workers join before locks release; failure publishes no complete manifest. Report inclusive wall time without summing overlapping spans.

3. **W4-03 — Measure the complete workflow on six owned games** · **L / high risk**  
   **Value:** Determine whether today’s speed work helps actual games and still produces usable, reversible patches.  
   **F:** GFAST’s approximately 7.3× result is a synthetic provider measurement. Existing installed-engine evidence includes limited projections and historical checkpoints, not today’s complete six-engine workflow. [GFAST](C:/Projects/Locust/docs/GOOGLE-FAST-MODE.md:23), [installed-engine limits](C:/Projects/Locust/docs/INSTALLED-ENGINE-VERIFICATION.md:15)  
   **H/work:** Select one full owned game each: Ren’Py, RPG Maker **MV/MZ**, Unity, KiriKiri, YU-RIS and Godot/Dialogic. Candidate records include FindingCloud9, the MV/MZ inventory, Out of Touch, Ochiru Hitozuma, Injuu Kangoku RE and G01; revalidate availability/version/source language. [VN inventory](C:/Projects/Locust/docs/VN_RPG_TRANSLATION.md:398)  
   **Files:** New `tools/benchmark_owned_games.py` and `docs/performance/w4-owned-games-*`; reuse independent readers without modifying engine implementations.  
   **Acceptance:** Exercise **GFAST + W3-01/02/03/04/05**, recording preparation→extract→JA→EN→pivot→EN→ES→validate→inject→pack→apply→rollback wall time, bytes, requests/retries, glossary/TM hits, RSS and patch size. Use matched local replays for attribution and separately report live-provider timing. Preserve originals, controls and untouched resources; re-extract targets and verify rollback hashes. English-only releases cover EN→ES only. Log runtime/render checks separately; blocked engines remain incomplete. G01 v2 does **not** validate today’s PCK v3 addition.

4. **W4-04 — Harden Google partial receipts together with frozen scopes** · **M / high risk**  
   **Value:** Retain the speed benefit of successful subchunks through failures, cancellation and fallback.  
   **F:** W3-02 validates success/failure partitions; W3-08 freezes selection through memory, glossary and fallback. Existing tests already cover several cancellation and malformed-response cases. [Receipt contract](C:/Projects/Locust/crates/core/src/translation/provider.rs:43), [existing Google cases](C:/Projects/Locust/crates/providers/src/google.rs:744)  
   **H/work:** Audit interactions at the combined delivery boundary: duplicate-source rows spanning selected/unselected files, shared TextAssets, concurrent edits, interrupted receipts and fallback after partial semantic rejection.  
   **Files:** `C/src/translation/{retry,provider,manager}.rs`, `P/src/google.rs`, focused integration fixtures; coordinate database selection changes through the integration owner.  
   **Acceptance:** Exact per-ID call/save/accounting traces; successful IDs never resent; unselected rows receive zero calls/writes; stale edits remain guarded; cancellation/deadlines bound all subrequests. Exercise shared limiter cooldown and `Retry-After`; **retain current GFAST defaults**, since 140 RPM is not a proven sustained quota.

5. **W4-05 — Finish and verify the corrected MT repair lane** · **M / high risk**  
   **Value:** Recover damaged controls using a changed MT source instead of ineffective instruction-only retries.  
   **F:** Lane-d’s report records one alternate-sentinel repair, affected-ID fallback and a small live probe where all encodings tied. Correction round 2 **reverted canonical masking for binary/shared-capacity rows**. It is not yet delivered on the inspected branch. [Lane evidence](C:/Projects/Locust-lane-d/docs/MT-PROTECTION.md:117)  
   **H/work:** Review/integrate the corrected scope; retain existing capacity behavior. Expanded binary/shared canonical names remain a separate unproven experiment.  
   **Files:** `C/src/{placeholder,glossary}.rs`, `C/src/translation/{provider,protection,manager}.rs`, `P/src/google.rs`, lane’s `mt_protection` and sentinel fixtures.  
   **Acceptance:** Unchanged integrity/capacity suites; independently verify reported environment-restricted failures. JA→EN→ES controls, nameboxes, literal sentinel collisions and overlapping names survive; malformed repairs stay pending and uncached; only affected IDs reach fallback. No claimed superiority from the tied synthetic probe.

6. **W4-06 — Decision-gated OPUS EN→ES local NMT pilot** · **L / high risk; owner decision pending**  
   **Value:** Test whether local translation improves real EN→ES turnaround and offline availability.  
   **F:** Python CT2 measured approximately 560 warm strings/s at B256, but OPUS EN→ES broke 7/10 raw-control and 4/10 masked probes; its first GPU batch took approximately 16 s. Rust/MSVC/CUDA integration, noisy pivot quality and Google comparison were not established. [Report](C:/Projects/Locust-nmt-spike/REPORT.md:5)  
   **H/scope:** Prepare the decision first. If chosen, add optional `ct2rs` features (`nmt`, separately `nmt-cuda`), EN→ES only, pinned local model/tokenizer artifacts, one persistent prewarmed bounded worker, strict validation and pending-only Google fallback. Keep Google for JA→EN. Add explicit per-phase provider configuration/provenance; defer a full download-management UI.  
   **Files:** `P/Cargo.toml`, new `P/src/opus_nmt.rs`, registration/feature forwarding, model-manifest fixture; pipeline phase configuration after W4-02.  
   **Risks:** Native Blackwell build/DLL packaging, tokenizer parity, cold start, OOM, CPU-backend differences, semantic errors that control validation cannot detect, and fallback erasing speed gains.  
   **Proposed go/no-go:** Owner elects the pilot; native and feature-off checks pass; zero accepted control corruption/truncation; blind review of actual JA→EN pivot text finds no material quality loss against Google; ≥20% median warm EN→ES wall-time gain **including validation/fallback**. Report cold/small-job timing separately. Otherwise **no-go for broader adoption/default changes**.

7. **W4-07 — Invalidate stale fast-workflow receipts before resumed writes** · **M / high risk**  
   **Value:** Prevent a resumed workflow from trusting validation or translation completed before intervening edits.  
   **F:** Resume checks project/options identity, while completed stages are skipped. This establishes a content-drift audit target, **not a reproduced defect**. Existing uncertain inject/pack reconciliation already has extensive fixtures. [Resume identity](C:/Projects/Locust/apps/desktop/src/lib/fastWorkflow.ts:102), [stage skipping](C:/Projects/Locust/apps/desktop/src/lib/fastWorkflow.ts:246)  
   **H/work:** Test DB edits, resource drift and project replacement between validation and injection; bind relevant revisions or rerun invalidated checks. Include reconnect after bounded replay and job retention.  
   **Files:** `U/lib/fastWorkflow{,.test}.ts`, `U/stores/fastWorkflowStore.ts`, modal/store tests.  
   **Acceptance:** An edit after validation cannot silently reuse its receipt; unchanged resumptions preserve valid work. Real backend interruption/reopen checks reconcile uncertain writes once; expired/replayed events never prove completion alone; existing Add/Replace policy remains enforced.

8. **W4-08 — Close import-language edge cases and pivot ergonomics** · **M / medium risk**  
   **Value:** Avoid importing into the wrong semantic language while keeping legitimate external catalogs usable.  
   **F:** W3-09 already implements primary-tag matching, regional warnings, mismatch refusal and token binding. Before a pivot’s first run, configured fallback languages remain necessary; physical Japanese provenance is not semantic-language evidence. [Policy](C:/Projects/Locust/docs/TMX-CATALOGS.md:32)  
   **H/work:** Audit fresh no-run pivots, mixed/partial XLIFF declarations, empty/undeclared TMX orientation, duplicate PO headers, unknown project pairs and script-subtag differences. Distinguish intentional policy from bugs.  
   **Files:** `C/src/export.rs`, `C/src/export/tmx.rs`, `S/tests/import_preview.rs`, `L/tests/import_languages.rs`, import modal tests; shared transport changes through integration owner.  
   **Acceptance:** HTTP/Tauri/CLI agree; known mismatches cannot be overridden; ambiguous cases require explicit confirmation; pair/catalog changes stale preview tokens; zero writes on refusal. Preserve existing regional policy and strict standalone TMX behavior unless a separately documented change is justified.

9. **W4-09 — Expose reliable font-cache refresh for text-fit** · **S / low–medium risk**  
   **Value:** Make edited fonts usable without restarting while retaining cheap draft updates.  
   **F:** W3-10 solved job exclusivity. The remaining cache uses length and timestamps to avoid rereads; identical-metadata rewrites need cache clear/restart. [Documented limitation](C:/Projects/Locust/docs/TEXT-FIT.md:17)  
   **H/work:** Add explicit font reread/refresh with bounded cache invalidation; avoid hashing the font on every keystroke.  
   **Files:** `C/src/text_fit.rs`, `C/tests/text_fit.rs`, `S/src/routes/text_fit.rs`, `U/components/TextFitPreview.tsx`; API/native forwarding by integration owner.  
   **Acceptance:** Same-size/restored-timestamp font edits change estimates after refresh; ordinary warm requests retain caching. Concurrent refresh/project switches cannot publish obsolete results; translation jobs continue; estimates remain read-only and bounded.

10. **W4-10 — Audit provenance reason coverage through actual producers** · **S / medium risk**  
    **Value:** Ensure Spanish QA explains repairable provenance failures consistently.  
    **F:** Fourteen reasons already have storage round-trip coverage and frontend mappings. The validator still permits untyped errors to produce `reason: None`; that is a coverage seam, not proof that a known producer is missing. [Producer seam](C:/Projects/Locust/crates/core/src/validation.rs:478), [existing coverage](C:/Projects/Locust/crates/core/tests/provenance_reasons.rs:430)  
    **H/work:** Map each producer/error path to its typed reason or an explicitly justified fallback; close only demonstrated gaps.  
    **Files:** `C/src/{validation,textasset_group,models,error}.rs`, `C/tests/provenance_reasons.rs`, `U/lib/validationLabels{,.test}.ts`.  
    **Acceptance:** Producer-generated failures survive persistence and HTTP/native delivery into EN/ES messages; ignored identities remain stable; legacy/unknown reasons remain readable; no English-message parsing or cosmetic duplicate tests.

**Dispatch:** W4-01 gets the quiet machine first. W4-02, W4-07 and W4-10 have largely separate writer areas. Serialize W4-04→W4-05→conditional W4-06 across shared translation files; centralize API/Tauri/locale seams. Start W4-03 inventory early, then measure the frozen integrated candidate.

**Release evidence:** Re-read the ledger and both unfinished lanes next session. Last recorded integrated CI covers `0ebed00`, not inspected HEAD `fc4eb02`; the final merged SHA needs its own required gates. The ledger’s generic Backlog row is stale—do not redispatch completed features; startup remains lower priority pending a fresh reproduction.