# Cycle 2026-11-01 — wave 10 research (production robustness)

Two independent Codex gpt-6.1-sol high researchers on 5e43607, same brief, read-only. Found by BOTH: SQLite snapshot deletes source sidecars it did not create at sample time (parent spot-checked `crates/core/src/database/connection.rs:52-66`), `synchronous=NORMAL` for user project DBs, non-atomic OAuth credential write, non-atomic catalog export, silent queue-persistence failures, unbounded generated DB filenames. A only: DB selection not bound to game identity, device-login wait bound, draft capacity, updater state/localization. B only: queue ownership after reload, LM Studio model validation, health-check localization, degraded global memory.

Order: lane A W10-DB (sidecars + durable commits), lane B W10-ATOM (OAuth + export atomic writes); then queue persistence, DB filename/identity, environment items.

## Researcher A

**Prioritize project DB identity and preservation of existing exports.** Both have concrete paths to losing saved work.

F = verified in source. H = inferred consequence or estimate requiring reproduction. All sizes and risks are H estimates: S ≤1 day; M ≤2 days. Acceptance tests below are proposed, not executed. The final table names each proposal’s complete edit set.

I excluded the completed startup-panic fix recorded at `odd/tasks/cycle-2026-11-01.md:83`. I did **not** establish a new production panic with a concrete triggering input; I have therefore not promoted speculative `unwrap` removals. For example, the replay-buffer unwraps have preceding guards and bounded pin counts (`crates/server/src/lib.rs:242`, `crates/server/src/lib.rs:265`).

1. **W10-01 — Prevent one game from adopting another game’s DB**  
   **Category:** Durability / first use. **M; medium risk.**  
   **User value:** Opening another game must preserve the first game’s translations.

   **F — Input → wrong outcome:** Sibling folders `Game` and `_Game` both sanitize to `Game`, because leading underscores are stripped (`crates/core/src/project.rs:242`). Both consequently discover the same adjacent `Game.locust.db` (`crates/core/src/project.rs:323`, `crates/core/src/project.rs:327`). Folder opening accepts the single existing candidate and merges extraction into it without checking game ownership (`crates/core/src/project.rs:426`, `crates/core/src/project.rs:474`). A complete merge deletes saved IDs absent from the incoming game, explicitly counting lost translations (`crates/core/src/database/strings.rs:1250`, `crates/core/src/database/strings.rs:1258`). This is reachable through both native and HTTP project opening (`apps/desktop/src-tauri/src/commands.rs:116`, `crates/server/src/routes/project.rs:156`).

   **Fix:** Bind automatically selected databases to their source identity before merging. Disambiguate new adjacent names with a path hash; preserve verified legacy discovery and require explicit selection when ownership cannot be established.

   **Acceptance:** Open two synthetic HTML games named `Game` and `_Game`, with disjoint IDs. Translate the first, open the second, then reopen both. Neither project’s rows, translations or identity may change because of the other.

2. **W10-02 — Publish exports atomically**  
   **Category:** Durability. **M; medium risk.**  
   **User value:** A failed export must preserve the previous usable translation catalog.

   **F — Input → wrong outcome:** Export over an existing PO/XLIFF/TMX file, then encounter a partial-write failure: native export writes directly to the destination (`apps/desktop/src-tauri/src/commands.rs:629`), as does CLI export (`crates/cli/src/commands/io.rs:208`). There is no staged generation in either path.

   **H:** A crash or disk-full failure during those writes can leave the previous catalog truncated or replaced by incomplete output.

   **Fix:** Share a staged export writer: create a sibling temporary file, write and sync it, then atomically replace the destination. Preserve the existing destination-safety check (`apps/desktop/src-tauri/src/commands.rs:623`).

   **Acceptance:** Inject write, sync and publication failures while exporting over sentinel content. Every failure preserves the sentinel; success publishes the complete catalog. Include a Windows sharing-lock case and a Unicode destination.

3. **W10-03 — Make acknowledged project saves durable across power loss**  
   **Category:** Durability. **S; medium performance risk.**  
   **User value:** Successfully saved translations should survive an abrupt machine shutdown.

   **F:** Every normal database open initializes WAL and explicitly sets `synchronous = NORMAL` (`crates/core/src/database/schema.rs:10`, `crates/core/src/database/schema.rs:11`; initialization caller: `crates/core/src/database/connection.rs:19`).

   **H — Input → wrong outcome:** Save translations successfully, then lose power before the WAL generation is durably synchronized → recently acknowledged edits may disappear. This is a power-loss hypothesis, not an observed corruption or ordinary process-crash result.

   **Fix:** Use `FULL` synchronization for persistent project databases, retaining transactions and batching.

   **Acceptance:** Assert effective `PRAGMA synchronous = 2` after both open and reopen; verify committed translations and history survive reopening. Use a disposable VM power-cut check for release acceptance—killing a process alone does not validate this property.

4. **W10-04 — Save OAuth credentials as complete generations**  
   **Category:** Durability. **S; low–medium risk.**  
   **User value:** Refreshing a login should not destroy the only saved session.

   **F — Input → wrong outcome:** Login or refresh reaches `save_tokens`, which writes `xai-oauth.json` directly in place (`crates/providers/src/xai_oauth.rs:81`, `crates/providers/src/xai_oauth.rs:86`). A truncated file subsequently becomes indistinguishable from no saved session because loading discards read and parse failures (`crates/providers/src/xai_oauth.rs:76`). Refresh invokes this writer before returning (`crates/providers/src/xai_oauth.rs:417`).

   **H:** Interruption during replacement can lose the saved credential generation and force another login.

   **Fix:** Write an owner-protected staging file, sync it, atomically replace the token file, and preserve the old generation on publication failure.

   **Acceptance:** Seed valid tokens; inject failures before publication and verify the original remains readable. Successful replacement contains exactly the new tokens. A Windows lock produces an error without truncation; Unix permissions are restrictive before publication.

5. **W10-05 — Stop snapshot cleanup from deleting another connection’s sidecars**  
   **Category:** Durability / concurrent failure. **M; medium–high risk.**  
   **User value:** A preview operation must not endanger a concurrently edited project.

   **F — Input → wrong outcome:** CLI extraction preview invokes `Database::snapshot_existing` (`crates/cli/src/commands/extract.rs:62`). That method records whether sidecars exist, opens and snapshots the database, then removes every sidecar initially absent (`crates/core/src/database/connection.rs:54`, `crates/core/src/database/connection.rs:59`, `crates/core/src/database/connection.rs:62`). Cleanup does not verify ownership or whether another connection created or now uses the file.

   **H:** A writer creating a live WAL/SHM between the initial check and cleanup can have its files unlinked on platforms permitting deletion of open files, with possible loss or inconsistent subsequent access.

   **Fix:** Remove opportunistic deletion of source sidecars unless exclusive ownership can be proven. Keep cleanup confined to owned scratch storage.

   **Acceptance:** Pause after the absence check; open another connection and commit WAL-backed data; resume the snapshot. Its sidecars remain accessible, its committed rows survive reopening, and the snapshot remains consistent.

6. **W10-06 — Bound device-login network waits**  
   **Category:** First use / offline failure. **S; low risk.**  
   **User value:** A stalled authentication service should return control to the user.

   **F — Input → wrong outcome:** A service accepts a connection but never completes its response. Device-code requests and token polls construct clients and await network/body operations without an explicit operation deadline (`crates/providers/src/xai_oauth.rs:207`, `crates/providers/src/xai_oauth.rs:234`, `crates/providers/src/xai_oauth.rs:263`, `crates/providers/src/xai_oauth.rs:281`). The start-login UI clears its busy state only after that promise settles (`apps/desktop/src/pages/settings/GrokSubCard.tsx:105`, `apps/desktop/src/pages/settings/GrokSubCard.tsx:116`). Server login handlers await the provider operation directly (`crates/server/src/routes/config_auth.rs:138`, `crates/server/src/routes/config_auth.rs:193`).

   **Fix:** Apply a deadline covering connection, response headers and body decoding for each device-login operation; return a stable timeout diagnostic.

   **Acceptance:** Mock separately stalled headers and stalled bodies. Start and poll return within the deadline, preserve existing credentials, and allow another attempt.

7. **W10-07 — Report queue persistence failures and preserve damaged snapshots**  
   **Category:** Durability / failure UX. **M; low risk.**  
   **User value:** Users should know when their queue will not survive restarting the app.

   **F — Input → wrong outcome:** A storage quota/security exception is swallowed by `writeQueue`, which returns no failure information (`apps/desktop/src/lib/queuePersistence.ts:104`). The store invokes it without any persistence status (`apps/desktop/src/stores/queueStore.ts:421`). Invalid persisted JSON or one invalid member restores an entirely empty queue (`apps/desktop/src/lib/queuePersistence.ts:80`, `apps/desktop/src/lib/queuePersistence.ts:92`); a later queue change writes over that same key (`apps/desktop/src/lib/queuePersistence.ts:107`).

   **Fix:** Return persistence outcomes, show a persistent localized warning, preserve invalid bytes until explicit recovery/discard, and offer retry. Queue execution can continue with an explicit “not saved” state.

   **Acceptance:** Throw on `setItem`; queue operation continues with a visible warning and successful retry clears it. Seed malformed storage; adding an item must not silently destroy the recoverable original snapshot.

8. **W10-08 — Bound generated database filename components**  
   **Category:** First use / long paths. **S; medium compatibility risk.**  
   **User value:** A valid game-folder name should not prevent creating its project.

   **F:** Sanitization returns the complete stem without a length bound (`crates/core/src/project.rs:233`, `crates/core/src/project.rs:254`). Adjacent naming adds `.locust.db`; fallback naming adds a hash and suffix to that same unbounded stem (`crates/core/src/project.rs:305`, `crates/core/src/project.rs:311`).

   **H — Input → wrong outcome:** An existing 250-character ASCII folder component generates a 260-character adjacent DB component and a 269-character fallback component, exceeding common Windows filesystem component limits. Moving the DB to the profile therefore does not solve this case.

   **Fix:** Bound generated stems while reserving suffix/hash space, using Unicode-safe truncation and deterministic hashing. Keep ordinary existing names discoverable.

   **Acceptance:** Open synthetic games with long ASCII and non-ASCII names, including a read-only parent that forces fallback. Generated components fit the supported filesystem limit; reopening resolves the same DB.

9. **W10-09 — Allow existing drafts to advance at the record limit**  
   **Category:** Durability. **S; medium persistence risk.**  
   **User value:** Reaching the draft limit should not stop autosaving drafts already present.

   **F — Input → wrong outcome:** Persist 1,024 valid drafts, then edit one existing draft. `write` rejects whenever the existing record count reaches `MAX_DRAFT_RECORDS`, before identifying the predecessor that replacement would retire (`apps/desktop/src/lib/draftPersistence.ts:5`, `apps/desktop/src/lib/draftPersistence.ts:76`, `apps/desktop/src/lib/draftPersistence.ts:93`). The editor changes memory first and then attempts persistence (`apps/desktop/src/stores/draftStore.ts:88`), so restarting restores the older persisted text.

   **Fix:** Permit a verified predecessor replacement at capacity, preserving write-before-retire ordering and the protection against evicting unrelated drafts.

   **Acceptance:** At exactly 1,024 records, edit an existing draft and reload: the new text survives. A genuinely new draft remains refused. Failure while writing the replacement preserves its predecessor and all unrelated drafts.

10. **W10-10 — Give updating a single, accurate operation lifecycle**  
    **Category:** Failure UX / localized errors. **M; medium risk.**  
    **User value:** Slow or failed updating should show one understandable, recoverable operation.

    **F — Input → wrong outcome:** Click Download repeatedly while the additional `check()` is unresolved. The handler establishes no busy guard before awaiting it (`apps/desktop/src/components/UpdateChecker.tsx:49`), and the available-state button remains enabled (`apps/desktop/src/components/UpdateChecker.tsx:117`). Download `Finished` sets `ready` before `downloadAndInstall` resolves (`apps/desktop/src/components/UpdateChecker.tsx:60`), while `ready` displays “installed” (`apps/desktop/src/components/UpdateChecker.tsx:150`). Rejections display the raw plugin message inside the localized panel (`apps/desktop/src/components/UpdateChecker.tsx:74`, `apps/desktop/src/components/UpdateChecker.tsx:162`).

    **Fix:** Claim single-flight ownership synchronously, distinguish downloading/installing/success, and show localized failure guidance while retaining technical details in the activity log.

    **Acceptance:** Repeated clicks cause one operation; download completion alone never displays installed; installation failure never relaunches; English and Spanish failures use localized guidance and remain retryable.

The proposed file overlaps are: **01/08** in `project.rs`; **03/05** in `connection.rs`; **04/06** in `xai_oauth.rs`; and **07/10** in the locale files. These pairs should stay with the same writer.

| Rank | ID | Title | Category | Size | Exact files to edit |
|---:|---|---|---|:---:|---|
| 1 | W10-01 | Bind DB selection to game identity | Durability | M | `crates/core/src/project.rs` |
| 2 | W10-02 | Publish exports atomically | Durability | M | `crates/core/src/export.rs`; `apps/desktop/src-tauri/src/commands.rs`; `crates/cli/src/commands/io.rs`; `crates/cli/tests/export_destination_tests.rs` |
| 3 | W10-03 | Synchronize acknowledged DB commits | Durability | S | `crates/core/src/database/schema.rs`; `crates/core/src/database/connection.rs` |
| 4 | W10-04 | Publish OAuth credentials atomically | Durability | S | `crates/providers/src/xai_oauth.rs` |
| 5 | W10-05 | Preserve live SQLite sidecars | Durability | M | `crates/core/src/database/connection.rs` |
| 6 | W10-06 | Bound device-login waits | First use | S | `crates/providers/src/xai_oauth.rs` |
| 7 | W10-07 | Expose queue persistence failures | Durability / errors | M | `apps/desktop/src/lib/queuePersistence.ts`; `apps/desktop/src/lib/queuePersistence.test.ts`; `apps/desktop/src/stores/queueStore.ts`; `apps/desktop/src/stores/queueStore.test.ts`; `apps/desktop/src/components/QueuePanel.tsx`; `apps/desktop/src/components/QueuePanel.test.tsx`; `apps/desktop/src/lib/i18n/en.ts`; `apps/desktop/src/lib/i18n/es.ts` |
| 8 | W10-08 | Bound generated DB filenames | Environment | S | `crates/core/src/project.rs` |
| 9 | W10-09 | Advance drafts at capacity | Durability | S | `apps/desktop/src/lib/draftPersistence.ts`; `apps/desktop/src/lib/draftPersistence.test.ts` |
| 10 | W10-10 | Make updater state accurate and localized | Failure UX | M | `apps/desktop/src/components/UpdateChecker.tsx`; `apps/desktop/src/components/UpdateChecker.test.tsx` **(new)**; `apps/desktop/package.json`; `apps/desktop/src/lib/i18n/en.ts`; `apps/desktop/src/lib/i18n/es.ts` |

**Suggested disjoint-file lanes:** Lane A owns **01, 02, 03, 05, 08**; Lane B owns **04, 06, 07, 09, 10**. Within A, sequence **01 → 08** and **03 → 05**; within B, sequence **04 → 06** and **07 → 10**.

## Researcher B

Seven proposals below are deterministic defects established from source; three are explicitly marked fault/platform hypotheses. **F** means verified source behavior; **H** means an inferred failure, proposed change, estimate, or acceptance criterion. S = up to one day; M = up to two days.

I did **not** establish an additional input-triggered production panic. I excluded guarded candidates—for example, XLIFF requires a current unit before admitting source/target elements, protecting the later unwraps (`crates/core/src/export.rs:605`, `crates/core/src/export.rs:615`, `crates/core/src/export.rs:644`). The recommendations below concern remaining failure behavior, rather than repeating the completed startup-panic work.

**W10-01 — Stop manually deleting source SQLite sidecars**  
Category: durability. **H — M; medium risk.** User value: protect an actively edited project while another process previews extraction.

- **F — Evidence:** CLI dry-run extraction snapshots an existing project (`crates/cli/src/commands/extract.rs:62`, `crates/cli/src/commands/extract.rs:195`). `snapshot_existing` samples sidecar existence, opens the source, takes its snapshot, then removes every sidecar absent during the initial sample—without checking its present ownership (`crates/core/src/database/connection.rs:52`, `crates/core/src/database/connection.rs:62`).
- **H — Trigger → failure:** Start a writer after the existence sample; keep it open through cleanup. On a platform permitting unlinking open files, cleanup can delete that writer’s live WAL/SHM. Loss of committed data after a subsequent crash needs fault validation.
- **H — Fix:** Leave source-side sidecar lifecycle to SQLite; restrict manual cleanup to exclusively owned scratch databases.
- **H — Edit:** `crates/core/src/database/connection.rs`.
- **H — Acceptance:** A barrier-controlled two-connection test starts the writer after sampling, commits through snapshot completion, and verifies its sidecars and committed rows survive cleanup and reopening.
- **H — Shared files:** None.

**W10-02 — Use durable SQLite commits for user work**  
Category: durability. **H — S; medium risk from additional disk latency.** User value: reduce the chance that acknowledged translations disappear after power loss.

- **F — Evidence:** Every initialized database enables WAL and explicitly selects `synchronous = NORMAL` (`crates/core/src/database/schema.rs:10`, `crates/core/src/database/schema.rs:11`, `crates/core/src/database/schema.rs:242`). Normal database opens use this initializer (`crates/core/src/database/connection.rs:18`).
- **H — Trigger → failure:** Power loss after a reported successful commit but before the relevant WAL synchronization can lose recent acknowledged work. This is a durability hypothesis, not a corruption result observed here.
- **H — Fix:** Select `FULL` for persistent project databases; retain suitable behavior for in-memory databases.
- **H — Edit:** `crates/core/src/database/schema.rs`.
- **H — Acceptance:** Assert the effective pragma on open/reopen; verify commit synchronization with fault instrumentation and reopening. Measure translation-save latency before accepting the change.
- **H — Shared files:** None.

**W10-03 — Atomically replace OAuth credentials**  
Category: durability. **H — S; low risk.** User value: preserve recoverable login credentials when saving a refreshed session fails.

- **F — Evidence/input → wrong outcome:** Login and token refresh both call `save_tokens` (`crates/providers/src/xai_oauth.rs:292`, `crates/providers/src/xai_oauth.rs:417`). It writes directly over `xai-oauth.json`, without staging or synchronization; permissions are applied afterward (`crates/providers/src/xai_oauth.rs:81`, `crates/providers/src/xai_oauth.rs:86`, `crates/providers/src/xai_oauth.rs:92`). An interrupted or partial write can therefore replace the old credential file with incomplete JSON; loading then silently returns `None` (`crates/providers/src/xai_oauth.rs:76`).
- **H — Fix:** Write an owner-restricted sibling staging file, synchronize it, then atomically replace the destination.
- **H — Edit:** `crates/providers/src/xai_oauth.rs`.
- **H — Acceptance:** Inject write and replacement failures. The old JSON must remain intact; successful replacement must contain the complete new token set. Check permissions before publication.
- **H — Shared files:** None.

**W10-04 — Publish exported catalogs atomically**  
Category: durability/environment failure. **H — M; medium risk around destination protection.** User value: a failed export should preserve the user’s previous catalog.

- **F — Evidence/input → wrong outcome:** CLI export with overwrite permission ends in an in-place `fs::write` (`crates/cli/src/commands/io.rs:157`, `crates/cli/src/commands/io.rs:208`). Native export does the same after destination validation (`apps/desktop/src-tauri/src/commands.rs:623`, `apps/desktop/src-tauri/src/commands.rs:629`). A write failure after truncation can leave the existing catalog empty or partial.
- **H — Fix:** Share a staged, synchronized catalog writer while preserving destination checks and CLI overwrite policy.
- **H — Edit:** `crates/core/src/export.rs`; `crates/cli/src/commands/io.rs`; `apps/desktop/src-tauri/src/commands.rs`.
- **H — Acceptance:** Cover PO/XLIFF/TMX, Unicode destinations, write failure, and a Windows destination held without delete sharing. Failure must preserve the previous bytes; success must publish the entire catalog.
- **H — Shared files:** None.

**W10-05 — Recover queue ownership after a UI reload**  
Category: failure recovery/persistence. **H — M; medium risk around reconciliation.** User value: recover progress and cancellation controls for work still running in the backend.

- **F — Evidence/input → wrong outcome:** After receiving a translation job, the queue stores its ID only in runtime state (`apps/desktop/src/stores/queueStore.ts:260`). Persistence excludes that ID (`apps/desktop/src/lib/queuePersistence.ts:69`); initialization resets it to `null`, and restoration converts a translating item into an interrupted error (`apps/desktop/src/stores/queueStore.ts:88`, `apps/desktop/src/lib/queuePersistence.ts:86`). Cancellation returns immediately without an ID (`apps/desktop/src/stores/queueStore.ts:146`). Thus reloading the UI loses cancellation ownership even when the backend job remains alive; the embedded backend runs on its own thread (`apps/desktop/src-tauri/src/main.rs:82`).
- **H — Fix:** Persist the received job ID and project identity; reconcile with replay before allowing retry.
- **H — Edit:** `apps/desktop/src/lib/queuePersistence.ts`; `apps/desktop/src/stores/queueStore.ts`; `apps/desktop/src/lib/queuePersistence.test.ts`; `apps/desktop/src/stores/queueStore.test.ts`.
- **H — Acceptance:** Reload after receiving a job ID: reconnect to that job and cancel it without starting another. After a backend restart, a missing job must produce an explicit unknown/interrupted outcome and never auto-retry.
- **H — Shared files:** W10-06.

**W10-06 — Make queue-storage failures visible and recoverable**  
Category: durability/error quality. **H — M; low-to-medium risk.** User value: users can tell when their queue will not survive closing the app.

- **F — Evidence/input → wrong outcome:** `setItem` failures are swallowed and return no result (`apps/desktop/src/lib/queuePersistence.ts:104`). Queue changes continue through a subscription that cannot report persistence failure (`apps/desktop/src/stores/queueStore.ts:421`). Separately, one invalid item/history record or malformed JSON causes restoration of an entirely empty queue (`apps/desktop/src/lib/queuePersistence.ts:81`, `apps/desktop/src/lib/queuePersistence.ts:92`).
- **H — Fix:** Return persistence status, show a localized persistent warning, and preserve unreadable serialized data until explicit recovery/reset.
- **H — Edit:** `apps/desktop/src/lib/queuePersistence.ts`; `apps/desktop/src/stores/queueStore.ts`; `apps/desktop/src/components/QueuePanel.tsx`; `apps/desktop/src/lib/queuePersistence.test.ts`; `apps/desktop/src/stores/queueStore.test.ts`; `apps/desktop/src/components/QueuePanel.test.tsx`; `apps/desktop/src/lib/i18n/en.ts`; `apps/desktop/src/lib/i18n/es.ts`.
- **H — Acceptance:** Throw quota/security errors from storage and supply a corrupt snapshot. Show a warning, preserve recoverable data, keep runtime cancellation functional, and clear the warning after a successful save.
- **H — Shared files:** W10-05; locale files with W10-09/W10-10.

**W10-07 — Bound generated project-database filenames**  
Category: first-run/path failure. **H — M; medium compatibility risk.** User value: opening a game should work when its original folder name is valid but nearly fills the filesystem’s component limit.

- **F — Evidence:** Sanitization replaces characters but never bounds filename length (`crates/core/src/project.rs:233`). The adjacent filename appends `.locust.db`; fallback appends both a hash and that suffix (`crates/core/src/project.rs:302`, `crates/core/src/project.rs:308`). Saved-project discovery independently constructs those same names (`crates/core/src/project.rs:318`).
- **H — Trigger → failure:** On an NTFS volume with a 255-unit component limit, a valid 250-character ASCII game-folder name produces an invalid 260-character adjacent DB filename; the fallback is longer still. Platform reproduction remains required.
- **H — Fix:** Use a bounded Unicode-safe stem plus stable hash, with compatible discovery of existing valid legacy names.
- **H — Edit:** `crates/core/src/project.rs`.
- **H — Acceptance:** Open/reopen games with near-limit ASCII and Japanese names, under writable and read-only parents. Verify stable identity, preserved translations, and discovery of legacy databases.
- **H — Shared files:** None.

**W10-08 — Make LM Studio health validate the selected model**  
Category: first-use/environment failure. **H — S; low risk if scoped to LM Studio.** User value: “Test connection” should detect that the configured local model cannot be used.

- **F — Evidence/input → wrong outcome:** LM Studio defaults to `qwen/qwen3-14b` (`crates/providers/src/openai.rs:49`). Its shared health check requests `/models`, checks only HTTP status, and returns success without reading the body (`crates/providers/src/openai.rs:366`, `crates/providers/src/openai.rs:384`, `crates/providers/src/openai.rs:393`). A `200 {"data":[]}` therefore passes despite containing no selected model. The server reports that success as `"healthy"` (`crates/server/src/routes/config_auth.rs:62`).
- **H — Fix:** For LM Studio, validate the models response and selected model; report missing-model guidance.
- **H — Edit:** `crates/providers/src/openai.rs`.
- **H — Acceptance:** Empty models, malformed JSON and a different model must fail; the configured model must pass. Preserve cloud-provider behavior.
- **H — Shared files:** None.

**W10-09 — Localize successful-HTTP health-check failures**  
Category: user-facing error quality. **H — S; low risk.** User value: Spanish users receive understandable local-provider and timeout guidance.

- **F — Evidence/input → wrong outcome:** Provider-health failures are returned as normal JSON, including English exception text; timeout already supplies a `message_key` (`crates/server/src/routes/config_auth.rs:63`, `crates/server/src/routes/config_auth.rs:65`). The frontend API declares only `ok` and `message`, and the modal stores/renders that message directly (`apps/desktop/src/lib/api.ts:537`, `apps/desktop/src/components/TranslationModal.tsx:302`, `apps/desktop/src/components/TranslationModal.tsx:441`). A Spanish session receiving the timeout payload therefore displays its English message. Missing Ollama also produces explicit English health text (`crates/providers/src/ollama.rs:203`).
- **H — Fix:** Normalize health responses centrally, honor recognized message keys, and retain raw diagnostics in activity details.
- **H — Edit:** `apps/desktop/src/lib/api.ts`; `apps/desktop/src/lib/apiError.ts`; `apps/desktop/src/lib/apiError.test.ts`; `apps/desktop/src/lib/i18n/en.ts`; `apps/desktop/src/lib/i18n/es.ts`.
- **H — Acceptance:** In Spanish, test healthy, missing provider, stopped Ollama, missing model, and timeout payloads returned with HTTP 200. Unknown diagnostics remain inspectable.
- **H — Shared files:** Locale files with W10-06/W10-10; `api.ts` with W10-10.

**W10-10 — Expose degraded global-memory startup**  
Category: environment failure/error quality. **H — M; low risk.** User value: distinguish unavailable saved memory from an honestly empty memory database.

- **F — Evidence/input → wrong outcome:** Any failure opening `global_memory.db` silently substitutes an in-memory database and discards the original error (`crates/server/src/lib.rs:454`). Memory statistics then return an ordinary count without a degraded-state field (`crates/server/src/routes/glossary_memory.rs:127`). A malformed or inaccessible existing database thus appears as zero available global entries. This concerns existing legacy memory: the source explicitly says nothing currently writes new global entries (`crates/server/src/routes/glossary_memory.rs:362`).
- **H — Fix:** Keep startup operational but expose degraded status, affected path and actionable localized guidance.
- **H — Edit:** `crates/server/src/lib.rs`; `crates/server/src/routes/glossary_memory.rs`; `apps/desktop/src/lib/api.ts`; `apps/desktop/src/pages/TranslationMemory.tsx`; `apps/desktop/src/pages/TranslationMemory.test.tsx`; `apps/desktop/src/lib/i18n/en.ts`; `apps/desktop/src/lib/i18n/es.ts`.
- **H — Acceptance:** Start with malformed and inaccessible global-memory files. Show degraded status while project memory remains usable; preserve the existing file and return to normal after repair/restart.
- **H — Shared files:** `api.ts` and locale files with W10-09; locale files with W10-06.

**H — Suggested disjoint-file lanes:** Lane A owns W10-01, W10-02, W10-03, W10-07 and W10-08. Lane B owns W10-04, W10-05, W10-06, W10-09 and W10-10. Within B, serialize the two queue units and coordinate the shared API/catalog edits. Suggested concurrent pairs are **01/05, 02/06, 03/04, 07/09, 08/10**. Full file ownership, including tests, is listed above.

**H — Ranked table** — filenames below refer to the exact paths listed in each proposal.

| Rank | ID | Title | Category | Size | Files |
|---:|---|---|---|:---:|---|
| 1 | W10-01 | Stop deleting source SQLite sidecars | Durability | M | core `database/connection.rs` |
| 2 | W10-02 | Durable SQLite commits | Durability | S | core `database/schema.rs` |
| 3 | W10-03 | Atomic OAuth credential replacement | Durability | S | providers `xai_oauth.rs` |
| 4 | W10-04 | Atomic catalog export | Durability | M | core `export.rs`; CLI `commands/io.rs`; Tauri `commands.rs` |
| 5 | W10-05 | Recover queue ownership after reload | Recovery | M | `queuePersistence.ts`, `queueStore.ts`, their tests |
| 6 | W10-06 | Visible queue-storage failures | Durability/errors | M | Queue persistence/store/panel, their tests, `en.ts`, `es.ts` |
| 7 | W10-07 | Bound generated DB filenames | Paths/first use | M | core `project.rs` |
| 8 | W10-08 | Validate LM Studio’s selected model | Local environment | S | providers `openai.rs` |
| 9 | W10-09 | Localize health-check failures | Error quality | S | `api.ts`, `apiError.ts`, `apiError.test.ts`, `en.ts`, `es.ts` |
| 10 | W10-10 | Expose degraded global memory | Environment/errors | M | server `lib.rs`, `glossary_memory.rs`; `api.ts`; TranslationMemory page/test; `en.ts`, `es.ts` |