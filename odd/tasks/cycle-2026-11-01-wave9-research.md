# Cycle 2026-11-01 — wave 9 research

Two independent Codex gpt-6.1-sol high researchers on `2be37e0`, same brief, read-only. Found independently by BOTH (highest confidence): Unity refresh loses saved translations when the local-ID collision set changes (parent spot-checked `crates/formats/src/unity.rs:2894`), patch cancellation removes the job and reports `cancelled` while blocking apply can still commit (parent spot-checked `crates/server/src/routes/jobs.rs:56`), queue loses ownership of running jobs across WebSocket disconnects, editor page is not recovered/preserved after filter changes or remount, lossy legacy encoding accepted on inject (A: Wolf RPG, B: YU-RIS). A only: rollback inventory coverage, same-version content identity, read-only destination preflight, versioned patch updates, SugarCube write binding. B only: planned patch path collisions, Unity dialogue grammar validation, font generation project binding, live WebSocket peer closure, complete font-patch ZIPs.

Execution order: lane A W9-U (Unity identity), lane B W9-P (patch cancellation drain); then queue reconnect, encoding fidelity, editor page, then single-researcher items.

## Researcher A

I recommend **nine defects and one capability**, each bounded to three writer-days. These findings come from source tracing; I changed no files and ran no builds, tests, providers, or network calls.

**F** = verified source behavior, including derived input → output. **H** = proposed implementation, estimate, risk, value, acceptance criteria, or compatibility assessment.

File prefixes below expand exactly: `C=crates/core/src`, `CT=crates/core/tests`, `E=crates/formats/src`, `FT=crates/formats/tests`, `S=crates/server/src`, `Q=crates/cli/src`, `QT=crates/cli/tests`, `U=apps/desktop/src`.

1. **W9-01 — Refuse rollback with an incomplete backup inventory**

   **H — Defect · M, 2 days · high risk.** Value: prevent rollback from deleting the only recoverable originals after a backup manifest loses an entry.

   **F — Input → wrong output:** Apply a patch replacing `a.txt` and adding `b.txt`. Change the otherwise valid backup manifest to `"files":[]`, leaving its actual backup payload intact. Manifest loading accepts the parsed object; restore preflight merely iterates its listed files. Rollback consequently deletes `b.txt`, restores nothing, and recursively removes `.locust`, including the unlisted original `a.txt`. The live `a.txt` remains patched. (`crates/core/src/patch/store.rs:105`, `:324`, `:267`; `crates/core/src/patch/rollback.rs:205`, `:255`, `:263`, `:281`.)

   **H — Work/files:** Check that receipt/journal replacement paths have complete, unambiguous backup coverage before any mutation. Edit `C/patch/rollback.rs` and `CT/patch_apply.rs`.

   **H — Acceptance:** Removing one required manifest entry makes preview and real rollback refuse, including with force; all game bytes, added files, markers and backup payloads remain intact. Valid completed and interrupted rollbacks still succeed.

   **H — Compatibility/sharing:** No row-ID, database or API changes. Valid existing backups remain supported. Shares `CT/patch_apply.rs` with W9-06/W9-08.

2. **W9-02 — Preserve Unity translations when the collision set changes**

   **H — Defect · M, 3 days · high risk.** Value: preserve saved translations for unchanged physical fields when a game update introduces or removes another field with the same local ID.

   **F — Input → wrong output:** Extract only `A.assets` containing TextMesh path ID `42`: its local row ID is `textmesh/42`. Add `B.assets` containing another TextMesh `42`, then extract again. Both now receive qualified IDs, including `unity:["A.assets",0]:textmesh/42`; the unchanged A field’s old ID disappears. (`crates/formats/src/unity.rs:1288`, `:2894`, `:2900`, `:2912`.)

   **F:** Complete merge deletes absent IDs and inserts unfamiliar IDs with `NULL` translation and pending status. Thus A’s saved translation is lost despite its physical field remaining unchanged. (`crates/core/src/database/strings.rs:1071`, `:1144`, `:1173`.)

   **H — Work/files:** Reconcile Unity rows by exact physical path/local identity before deletion, retaining existing persisted keys where mapping is uniquely proven; refuse ambiguity. Edit `C/database/strings.rs`; add `FT/unity_identity_refresh.rs`.

   **H — Acceptance:** Save A’s translation, add B, refresh, then remove B and refresh again. A retains its ID, translation and status throughout; B never inherits A’s translation. Changed A source follows existing stale-source handling.

   **H — Compatibility/sharing:** No schema/API migration; preserve existing persisted IDs. No shared edit files.

3. **W9-03 — Drain cancelled patch workers before publishing their outcome**

   **H — Defect · M, 3 days · high risk.** Value: make cancellation status agree with the game’s actual installation state.

   **F — Input → wrong state:** Cancel while the blocking apply task is installing its final file. Patch cancellation immediately removes the registry entry, publishes terminal `cancelled`, and aborts the asynchronous supervisor. The blocking operation has no further progress callback after that final write and can still publish its receipt and delete the journal. Result: a successfully installed patch paired with a cancellation event and an unavailable reconnect lookup. (`crates/server/src/routes/jobs.rs:57`; `crates/server/src/routes/patch.rs:690`, `:703`; `crates/core/src/patch/apply.rs:658`, `:684`, `:703`.)

   **H — Work/files:** Request cancellation cooperatively, retain supervision until blocking work exits, and publish the resulting committed/interrupted outcome afterward. Edit `S/routes/jobs.rs`, `S/routes/patch.rs`, `S/job_ws_tests.rs`.

   **H — Acceptance:** Barrier-controlled cancellation before a write produces recoverable interruption; cancellation after commit reports completion. Reconnecting during drainage finds the job, and same-folder admission remains blocked until drainage finishes.

   **H — Compatibility/sharing:** Keep current HTTP and WS shapes; terminal timing becomes authoritative. No saved-data changes or shared edit files.

4. **W9-04 — Bind SugarCube injection to the extracted HTML file**

   **H — Defect · M, 2 days · medium risk.** Value: prevent a missing story file from redirecting translations into another HTML story.

   **F — Input → wrong output:** Extract `A.html` containing passage `pid="1" name="Start"` with `Hello traveler`. Remove A, leaving `B.html` with the same passage locator/text but different surrounding content. Directory injection selects B through `find_html_file`; it checks locator/source but never the entry’s stored physical file identity, so it writes `Hola viajero` into B. (`crates/formats/src/sugarcube.rs:17`, `:1364`, `:1398`, `:1495`, `:1527`.)

   **F:** Core intentionally allows missing ordinary targets to reach the format plugin. (`crates/core/src/extraction.rs:1400`, `:1456`.)

   **H — Work/files:** Resolve the exact remapped entry file, reject mixed/ambiguous targets, and retain explicit single-file selection. Edit `E/sugarcube.rs`, including inline regressions.

   **H — Acceptance:** The fixture reports a missing target and leaves B byte-identical. A two-story directory also cannot redirect injection through discovery order; normal translated-copy injection still succeeds.

   **H — Compatibility/sharing:** Existing row IDs and translations remain unchanged. Previously guessed targets refuse safely. No shared files.

5. **W9-05 — Reject lossy Wolf Shift-JIS translations**

   **H — Defect · S, 1 day · medium risk.** Value: prevent accepted translations from silently changing characters during encoding.

   **F — Input → wrong output:** Translate an ASCII slot `Hello world` to `Pay ¥100`. Wolf checks the encoder error flag and byte length, then writes the encoded bytes without checking decoded equality. The CP932 yen-to-backslash behavior is also recorded in the repository’s NScripter regression fixture. Wolf therefore accepts bytes spelling `Pay \100` while counting a successful write. (`crates/formats/src/wolf_rpg.rs:475`, `:491`, `:517`; `crates/formats/src/nscripter.rs:1749`.)

   **H — Work/files:** Require translation encode→decode equality before modifying a slot; report a classified encoding skip. Edit `E/wolf_rpg.rs`, including inline regressions.

   **H — Acceptance:** Yen and other non-round-tripping translations preserve original bytes and report refusal; ordinary representable translations still inject. Oversize translations remain refused.

   **H — Compatibility/sharing:** No row-ID/schema/API changes. Some previously accepted lossy translations become explicit skips. No shared files.

6. **W9-06 — Compare content inventories before returning AlreadyApplied**

   **H — Defect · M, 2 days · medium risk.** Value: distinguish an installed patch from a different artifact carrying the same ID and version.

   **F — Input → wrong output:** Install a text-only patch, then produce its combined text/font artifact. Font combination appends the font to the base manifest while retaining the base ID/version. Verification returns `AlreadyApplied` solely from matching ID/version, without comparing that incoming inventory to the receipt. The font can therefore be absent while verification says the combined patch is already applied. (`crates/core/src/font_patch.rs:211`, `:334`, `:338`, `:343`; `crates/core/src/patch/verify.rs:527`, `:553`.)

   **H — Work/files:** Before the equal-version shortcut, compare normalized path/hash inventories and relevant language/engine identity. Return an existing mismatch outcome when they differ. Edit `C/patch/verify.rs`, `C/font_patch.rs` and `CT/patch_apply.rs`.

   **H — Acceptance:** Installed base versus combined artifact is not `AlreadyApplied`; unchanged identical artifacts retain that outcome. Changed payload hashes under the same ID/version are also distinguished, without writing game files.

   **H — Compatibility/sharing:** Existing manifest/receipt/API shapes remain unchanged. Shares verification with W9-08 and the integration test file with W9-01/W9-08.

7. **W9-07 — Preserve queue ownership across translation socket loss**

   **H — Defect · M, 3 days · medium risk.** Value: let a queued paid translation survive a temporary socket disconnect without losing cancellation or advancing prematurely.

   **F — Input → wrong state:** Disconnect the queue’s socket after one batch. `waitForJob` rejects immediately on closure; its queue caller clears `activeJobId`, marks the item erroneous and continues its loop. No cancellation or reconnect accompanies that path. (`apps/desktop/src/lib/ws.ts:43`, `:54`; `apps/desktop/src/stores/queueStore.ts:260`, `:277`, `:337`, `:358`.)

   **F:** The single-job session already reconnects, while the server retains replay data and sequences translation events. (`apps/desktop/src/lib/translationJobSession.ts:282`; `crates/server/src/routes/jobs.rs:27`; `crates/server/src/lib.rs:302`.)

   **H — Work/files:** Add bounded reconnect with sequence deduplication; retain queue job ownership and Cancel/Reconnect after retry exhaustion. Edit `U/lib/ws.ts`, `U/lib/ws.test.ts`, `U/stores/queueStore.ts`, `U/stores/queueStore.test.ts`, `U/lib/queueExecution.test.ts`, `U/components/QueuePanel.tsx`, `U/components/QueuePanel.test.tsx`, `U/lib/i18n/en.ts`, `U/lib/i18n/es.ts`.

   **H — Acceptance:** Disconnect/replay causes one translation start, one terminal accounting update and no premature next-item start. Cancellation remains usable while disconnected.

   **H — Compatibility/sharing:** No backend API or database changes. Existing queue history remains readable. No shared edit files.

8. **W9-08 — Preflight read-only patch destinations before mutation**

   **H — Defect · M, 2 days · medium risk.** Value: reject predictable permission failures before leaving a partially reapplied patch.

   **F — Input → wrong state:** Install a two-file patch, make its second destination read-only, then force a same-ID/version reapply whose first payload changes. Existing committed backups are reused; installation changes the first destination before checking—and rejecting—the second destination’s read-only permission. (`crates/core/src/patch/apply.rs:165`, `:610`, `:618`, `:658`, `:673`.)

   **F:** Current destination preflight checks path types; writability verification probes only the game root. Dry-run returns before the per-file read-only check. (`crates/core/src/patch/verify.rs:318`, `:456`; `crates/core/src/patch/apply.rs:589`.)

   **H — Work/files:** Preflight read-only destinations across the complete plan before transitions/backups/journaling; retain installation-time checks. Edit `C/patch/verify.rs`, `C/patch/apply.rs`, `CT/patch_apply.rs`.

   **H — Acceptance:** The fixture’s verify, dry-run and real apply refuse before the first payload changes or a new journal appears. Removing the read-only attribute permits application.

   **H — Compatibility/sharing:** No IDs, saved-data or API-shape changes. Shares files with W9-01/W9-06 as listed above.

9. **W9-09 — Allow creators to publish versioned patch updates**

   **H — Capability · M, 3 days · medium risk.** Value: let patch authors distribute an update that follows the existing upgrade path without requiring force.

   **F — Current limitation:** Packaging always generates a new UUID and hardcodes patch version `1.0.0`. Verification distinguishes different patch IDs from same-ID newer versions. Repacking a revised translation therefore does not produce a versioned continuation of its prior release. (`crates/core/src/patch/pack.rs:789`; `crates/core/src/patch/verify.rs:529`, `:571`.)

   **H — Work/files:** Add opt-in CLI `--patch-id` and `--patch-version`, validated before output creation. Extend the existing configuration builder without adding required `PackOptions` fields. Edit `C/patch/pack.rs`, `CT/patch_identity.rs`, `Q/main.rs`, `Q/commands/patch.rs`; add `QT/patch_versions.rs`.

   **H — Acceptance:** Package releases `1.0.0` and `1.1.0` with one supplied ID; the latter verifies as an upgrade and applies without force. Invalid versions create no ZIP. Unspecified identity retains current behavior.

   **H — Compatibility/sharing:** Existing schema and default behavior remain unchanged; CLI flags are additive. No row-ID/database changes or shared edit files.

10. **W9-10 — Keep Previous available after the last editor page empties**

    **H — Defect · S, 1 day · low risk.** Value: let a translator continue working after an edit removes the last row from the current filtered page.

    **F — Input → wrong UI:** With 101 pending rows and page size 100, open offset 100 and translate its sole row. Once the filtered total becomes 100, pagination disappears because rendering requires `total > limit`, although the offset remains 100. The user cannot return through Previous while retaining that filter. (`apps/desktop/src/components/FilterBar.tsx:220`, `:224`; `apps/desktop/src/stores/editorStore.ts:41`, `:48`; `apps/desktop/src/pages/Editor.tsx:153`, `:612`.)

    **H — Work/files:** Render backward navigation whenever offset is positive, including an empty page. Edit `U/components/FilterBar.tsx`; add `U/components/FilterBar.test.tsx`; register it in `apps/desktop/package.json`.

    **H — Acceptance:** With `total=100`, `showing=0`, `offset=100`, Previous remains enabled and returns to offset zero without clearing status/search/file filters.

    **H — Compatibility/sharing:** No row-ID, saved-data or API changes. No shared edit files.

**H — Ranked dispatch table.** Rust source boundaries include inline regressions; new files are explicitly marked.

| Rank | ID | Title | Category | Size | Exact edit files |
|---:|---|---|---|---|---|
| 1 | W9-01 | Validate rollback inventory coverage | Defect | M | `C/patch/rollback.rs`; `CT/patch_apply.rs` |
| 2 | W9-02 | Preserve Unity IDs across collision changes | Defect | M | `C/database/strings.rs`; new `FT/unity_identity_refresh.rs` |
| 3 | W9-03 | Drain patch cancellation truthfully | Defect | M | `S/routes/jobs.rs`; `S/routes/patch.rs`; `S/job_ws_tests.rs` |
| 4 | W9-04 | Bind SugarCube writes to their file | Defect | M | `E/sugarcube.rs` |
| 5 | W9-05 | Reject lossy Wolf encoding | Defect | S | `E/wolf_rpg.rs` |
| 6 | W9-06 | Verify same-version content identity | Defect | M | `C/patch/verify.rs`; `C/font_patch.rs`; `CT/patch_apply.rs` |
| 7 | W9-07 | Reconnect queued translation jobs | Defect | M | `U/lib/ws.ts`, `ws.test.ts`, `queueExecution.test.ts`; `U/stores/queueStore.ts`, `queueStore.test.ts`; `U/components/QueuePanel.tsx`, `QueuePanel.test.tsx`; `U/lib/i18n/en.ts`, `es.ts` |
| 8 | W9-08 | Preflight read-only patch destinations | Defect | M | `C/patch/verify.rs`; `C/patch/apply.rs`; `CT/patch_apply.rs` |
| 9 | W9-09 | Publish versioned patch updates | Capability | M | `C/patch/pack.rs`; `CT/patch_identity.rs`; `Q/main.rs`; `Q/commands/patch.rs`; new `QT/patch_versions.rs` |
| 10 | W9-10 | Recover an emptied editor page | Defect | S | `U/components/FilterBar.tsx`; new `U/components/FilterBar.test.tsx`; `apps/desktop/package.json` |

**H — Suggested pairing:** Execute each lane sequentially. Their complete edit-file sets are disjoint; the shared patch verification/test files stay in Lane A.

| Pair | Lane A — patch lifecycle and distribution | Lane B — extraction identities and editor |
|---:|---|---|
| 1 | W9-01 | W9-02 |
| 2 | W9-03 | W9-04 |
| 3 | W9-06 | W9-05 |
| 4 | W9-08 | W9-07 |
| 5 | W9-09 | W9-10 |

## Researcher B

I recommend **nine defects and one capability**, each bounded to three writer-days. Findings are source-derived; I changed no files and ran no builds, tests, providers, or network calls.

**F** = verified source behavior or a code-derived counterexample, not runtime reproduction. **H** = proposed implementation, value, estimate, risk, acceptance criteria, or compatibility assessment.

Exact path prefixes: **E** = `crates/formats/src`; **FT** = `crates/formats/tests`; **C** = `crates/core/src`; **CT** = `crates/core/tests`; **S** = `crates/server/src`; **U** = `apps/desktop/src`. Rust edit boundaries include inline tests.

1. **W9-01 — Reject collisions between planned patch files and their parent paths**

   **H — Defect · M, 2 days · high risk.** Value: prevent an internally inconsistent archive from leaving a game partly patched.

   **F — Input → wrong state:** Supply a correctly hashed strict patch replacing existing `c.txt` and adding both `a` and `a/b.txt`, with `a` initially absent. Archive scanning rejects equal normalized paths, while destination inspection examines the current filesystem independently for each target; these two additions therefore pass those checks. Apply writes replacements first, changing `c.txt`, before the incompatible additions eventually fail. ([Evidentiary scan](/C:/Projects/Locust-lane-b/crates/core/src/patch/verify.rs:370), [destination classification](/C:/Projects/Locust-lane-b/crates/core/src/patch/verify.rs:275), [write order](/C:/Projects/Locust-lane-b/crates/core/src/patch/apply.rs:650))

   **H — Work/files:** Validate the complete planned namespace, rejecting any regular payload that is another payload’s ancestor, using the existing portable case-folding policy. Edit `C/patch/verify.rs`, `CT/patch_apply.rs`.

   **H — Acceptance:** Verify, dry-run and actual apply reject that fixture before changing game bytes or publishing a journal. Cover reversed archive order and `A` versus `a/b.txt`.

   **H — Compatibility/sharing:** Valid archives, row IDs, saved data and API shapes remain unchanged. No shared edit files within this wave. This extends the delivered existing-destination checks to conflicts entirely inside the incoming plan.

2. **W9-02 — Preserve Unity translations when ID collisions appear or disappear**

   **H — Defect · M, 3 days · high risk.** Value: keep an unchanged scene’s saved translations when another file introduces a colliding local ID.

   **F — Input → wrong output:** Initially extract `SCRIPTS~/chapter/a.txt` containing `L Hello`: its local ID is `a.txt#1`. Add `SCRIPTS~/extra/a.txt` containing another dialogue line. Extraction now qualifies **both** colliding IDs by physical path, so the original row’s ID changes despite its unchanged file and source. ([local ID creation](/C:/Projects/Locust-lane-b/crates/formats/src/unity.rs:188), [conditional qualification](/C:/Projects/Locust-lane-b/crates/formats/src/unity.rs:2891))

   **F:** Complete refresh merges by ID and deletes saved IDs absent from the incoming extraction, counting their translations as lost. Thus the original translation is removed instead of retained under its new alias. ([refresh](/C:/Projects/Locust-lane-b/crates/core/src/project.rs:471), [incoming keys](/C:/Projects/Locust-lane-b/crates/core/src/database/strings.rs:1071), [deletion](/C:/Projects/Locust-lane-b/crates/core/src/database/strings.rs:1144))

   **H — Work/files:** Reconcile incoming Unity aliases to existing IDs using an unambiguous physical-path/local-ID identity before merge deletion. Preserve existing IDs; refuse ambiguous reconciliation. Edit `E/unity.rs`, `C/database/strings.rs`; add `FT/unity_refresh_identity.rs`.

   **H — Acceptance:** Translate the initial row, introduce a collision, refresh, remove the other file, and refresh again. The surviving row retains its ID, translation, status and history throughout; distinct physical rows never share translations.

   **H — Compatibility/sharing:** Add identity metadata without bulk rekeying saved rows or changing API shapes. Shares `E/unity.rs` with W9-04. This addresses identity churn across refreshes, beyond collision detection within one extraction.

3. **W9-03 — Drain patch cancellation and retain its actual final outcome**

   **H — Defect · M, 3 days · high risk.** Value: prevent “cancelled” from concealing a completed installation and preserve reconnectable recovery information.

   **F — Input → wrong state:** Cancel after the final progress callback has returned but before its file replacement. Cancellation immediately removes the registry entry, publishes `cancelled`, and aborts the asynchronous wrapper. The blocking apply path can still finish its last replacement and write a receipt: there is no further progress callback before commit. ([cancel path](/C:/Projects/Locust-lane-b/crates/server/src/routes/jobs.rs:56), [blocking worker](/C:/Projects/Locust-lane-b/crates/server/src/routes/patch.rs:690), [last write and receipt](/C:/Projects/Locust-lane-b/crates/core/src/patch/apply.rs:658))

   **F:** Removing the registry entry also prevents a new WebSocket connection from obtaining its retained cancellation frame. ([subscription lookup](/C:/Projects/Locust-lane-b/crates/server/src/routes/jobs.rs:93))

   **H — Work/files:** Request cooperative cancellation, retain the wrapper until blocking work drains, derive the terminal result from the actual outcome, and retain replay for the normal completion window. Edit `S/routes/jobs.rs`, `S/routes/patch.rs`, `S/job_ws_tests.rs`.

   **H — Acceptance:** Barrier-controlled cancellation before a write leaves recoverable interrupted state; cancellation after commit reports the installed outcome. Reconnection receives exactly one authoritative terminal result.

   **H — Compatibility/sharing:** Preserve request shapes and existing terminal-frame schemas; correct timing and outcome selection. No row/data migration. Shares `jobs.rs` and `job_ws_tests.rs` with W9-08.

4. **W9-04 — Validate rebuilt Unity dialogue grammar**

   **H — Defect · M, 2 days · medium risk.** Value: prevent translated prose from becoming an engine modifier or disappearing from extraction.

   **F — Input → wrong output:** Translate `L Hello` to `+Smile Hola`. The writer accepts it when the source and control-token inventory match, then writes `L +Smile Hola`. The same parser now classifies `+Smile` as an engine prefix and returns only `Hola` as dialogue. A translation beginning `{` instead makes this parser reject the dialogue record. ([writer guards](/C:/Projects/Locust-lane-b/crates/formats/src/unity.rs:338), [replacement](/C:/Projects/Locust-lane-b/crates/formats/src/unity.rs:361), [prefix parsing](/C:/Projects/Locust-lane-b/crates/formats/src/unity.rs:2357))

   **H — Work/files:** Reparse each rebuilt dialogue record and require unchanged speaker/prefix structure plus the intended complete text body. Reject unsafe translations with a specific skip reason. Edit `E/unity.rs`.

   **H — Acceptance:** Both examples refuse without modifying the script; ordinary dialogue translations round-trip. Existing authored modifiers, indentation, controls and line endings remain intact.

   **H — Compatibility/sharing:** No IDs, saved-data or API changes; newly unsafe translations are skipped. Shares `E/unity.rs` with W9-02.

5. **W9-05 — Keep queue ownership through translation-stream disconnection**

   **H — Defect · M, 3 days · medium risk.** Value: prevent one temporary socket failure from abandoning the active job and failing subsequent queued projects.

   **F — Input → wrong state:** Disconnect the queue’s socket while translation continues. Queue uses `waitForJob`, whose close handler rejects immediately and unsubscribes. Its `finally` clears `activeJobId`; the outer catch marks the item erroneous and allows the loop to advance. ([one-shot wait](/C:/Projects/Locust-lane-b/apps/desktop/src/lib/ws.ts:43), [job-ID cleanup](/C:/Projects/Locust-lane-b/apps/desktop/src/stores/queueStore.ts:257), [error/advance path](/C:/Projects/Locust-lane-b/apps/desktop/src/stores/queueStore.ts:337))

   **F:** Project admission refuses switching while that backend translation remains active. The queue consequently loses its cancellation handle while later opens can be refused. ([admission](/C:/Projects/Locust-lane-b/crates/server/src/lib.rs:197), [queue cancellation handle](/C:/Projects/Locust-lane-b/apps/desktop/src/stores/queueStore.ts:133))

   **H — Work/files:** Add bounded reconnect with replay deduplication to queue waiting. On exhausted retries, retain job ownership and expose reconnect/cancel before advancing. Edit `U/lib/ws.ts`, `U/lib/ws.test.ts`, `U/stores/queueStore.ts`, `U/lib/queueExecution.test.ts`, `U/components/QueuePanel.tsx`, `U/components/QueuePanel.test.tsx`.

   **H — Acceptance:** Disconnect after progress, reconnect to replayed progress/completion, and finish once without duplicate accounting. Exhausted retries preserve Cancel and prevent opening the next item.

   **H — Compatibility/sharing:** No backend API, row-ID or database changes. Queue reconnection state remains session-local. No shared edit files within this wave. This targets the queue’s separate one-shot waiter.

6. **W9-06 — Reject lossy Shift-JIS aliases before YU-RIS serialization**

   **H — Defect · S, 1 day · medium risk.** Value: prevent apparently encodable characters from turning into different text or executable escapes.

   **F — Input → wrong output:** Serialize expression translation `Cost ¥n coin`. Light escaping leaves `¥` unchanged, and the encoder checks only `had_errors`. CP932 encodes the yen sign as ASCII backslash—the same mapping explicitly guarded elsewhere in this workspace. YU-RIS decoding then interprets the resulting `\n` as a line break, yielding `Cost <CRLF> coin`. ([escaping](/C:/Projects/Locust-lane-b/crates/formats/src/yuris.rs:447), [encoding](/C:/Projects/Locust-lane-b/crates/formats/src/yuris.rs:406), [serialization](/C:/Projects/Locust-lane-b/crates/formats/src/yuris.rs:860), [documented mapping](/C:/Projects/Locust-lane-b/crates/formats/src/nscripter.rs:809), [expression decoding](/C:/Projects/Locust-lane-b/crates/formats/src/yuris.rs:628))

   **H — Work/files:** Require semantic round-trip fidelity for newly encoded text before accepting the row, preserving the opaque-byte token contract. Edit `E/yuris.rs`.

   **H — Acceptance:** Yen/backslash-alias fixtures refuse with original attribute bytes intact; ordinary representable translations and protected raw-byte tokens still round-trip exactly.

   **H — Compatibility/sharing:** No IDs/schema/API changes. Previously accepted lossy translations gain an explicit refusal. No shared edit files within this wave.

7. **W9-07 — Bind font-patch generation to its originating project**

   **H — Defect · M, 2 days · medium risk.** Value: ensure font coverage is checked against the game’s intended translated text.

   **F — Input → wrong output:** Prepare A’s font-patch form, switch the backend project to B through another client, then submit. The request carries game/font paths but no expected database. The handler reserves whichever project is currently open, loads that database’s rows, and combines **A’s requested game paths with B’s text and format**. ([form submission](/C:/Projects/Locust-lane-b/apps/desktop/src/components/FontPatchDialog.tsx:41), [request shape](/C:/Projects/Locust-lane-b/apps/desktop/src/lib/api.ts:523), [project capture](/C:/Projects/Locust-lane-b/crates/server/src/font_patch.rs:62), [coverage input](/C:/Projects/Locust-lane-b/crates/server/src/font_patch.rs:93))

   **H — Work/files:** Capture project identity when the form opens; send an optional expected database and verify it under project ownership before generation. Edit `S/font_patch.rs`, `U/lib/api.ts`, `U/components/FontPatchDialog.tsx`, `U/lib/i18n/en.ts`, `U/lib/i18n/es.ts`, `apps/desktop/package.json`; add `U/components/FontPatchDialog.test.tsx`.

   **H — Acceptance:** A-bound submission delivered after switching to B returns conflict and creates no ZIP. Unchanged-project generation works, including selecting a separate clean game copy.

   **H — Compatibility/sharing:** Optional additive request field; no row/data migration. Shares `apps/desktop/package.json` with W9-10.

8. **W9-08 — Handle peer disconnects while a job WebSocket is idle**

   **H — Capability · M, 2 days · low–medium risk.** Value: release abandoned connections promptly during slow jobs and keep live socket control traffic responsive.

   **F — Current gap:** After sending pending events, the handler waits exclusively on `rx.recv()`. It reads the socket only in terminal cleanup. With a silent, unfinished worker, a peer Close or Ping has no receive branch in the live loop. ([live loop](/C:/Projects/Locust-lane-b/crates/server/src/routes/jobs.rs:129), [terminal-only receive](/C:/Projects/Locust-lane-b/crates/server/src/routes/jobs.rs:171))

   **H — Work/files:** Select incoming socket traffic alongside job events, exit promptly on peer closure, and bound blocked outgoing sends while retaining job replay. Edit `S/routes/jobs.rs`, `S/job_ws_tests.rs`.

   **H — Acceptance:** Park a worker after its initial event. Ping receives a timely response; peer Close terminates the handler without waiting for another job event. A stalled reader cannot retain the handler indefinitely, and reconnect still receives final replay.

   **H — Compatibility/sharing:** JSON event schemas, job execution, row IDs and saved data remain unchanged. Shares files with W9-03. This extends live handling rather than revisiting the delivered terminal-close handshake.

9. **W9-09 — Publish font-patch ZIPs only after a complete durable write**

   **H — Defect · S, 1 day · low–medium risk.** Value: avoid leaving a broken patch at the chosen output path after disk or write failure.

   **F — Input → wrong output:** `write_font_patch` creates the final destination, then calls `write_all` and `sync_all` with immediate error propagation and no cleanup guard. A failure after a partial write leaves a truncated final ZIP; retrying the same name then fails its exclusive-create check. ([entire publication path](/C:/Projects/Locust-lane-b/crates/core/src/font_patch.rs:163))

   **H — Work/files:** Write through operation-owned staging, publish without overwriting an existing destination, and clean owned remnants on ordinary failure. Edit `C/font_patch.rs`.

   **H — Acceptance:** Inject write and sync failures: no partial final artifact remains, and retry succeeds. Existing files remain byte-identical; concurrent attempts at one destination cannot overwrite the winner.

   **H — Compatibility/sharing:** Preserve the function signature, successful ZIP bytes and no-overwrite behavior. No row/data/API changes. No shared edit files within this wave.

10. **W9-10 — Preserve editor pagination when the filter bar remounts**

   **H — Defect · S, 1 day · low risk.** Value: let translators return from another screen to the page they were editing.

   **F — Input → wrong state:** Set editor offset to `200`, navigate away, then return without changing search. FilterBar initializes its local search from the stored filter but unconditionally schedules `setFilter({search, offset:0})` after 300 ms. The remembered page therefore resets simply because the component mounted. ([effect](/C:/Projects/Locust-lane-b/apps/desktop/src/components/FilterBar.tsx:38), [stored filter](/C:/Projects/Locust-lane-b/apps/desktop/src/stores/editorStore.ts:40), [mount location](/C:/Projects/Locust-lane-b/apps/desktop/src/pages/Editor.tsx:610))

   **H — Work/files:** Debounce actual search changes only; synchronize external search changes without replaying stale input. Edit `U/components/FilterBar.tsx`, `apps/desktop/package.json`; add `U/components/FilterBar.test.tsx`.

   **H — Acceptance:** Mount/remount with offset `200`, advance timers, and retain that offset. Editing search resets pagination once after debounce; rapid typing and unmount cancel obsolete updates.

   **H — Compatibility/sharing:** No IDs, saved-data or API changes. Shares `apps/desktop/package.json` with W9-07.

**H — Ranked dispatch table.** All edit files, including proposed tests, are listed.

| Rank | ID | Title | Category | Size | Files |
|---:|---|---|---|---|---|
| 1 | W9-01 | Reject planned patch path collisions | Defect | M | `C/patch/verify.rs`; `CT/patch_apply.rs` |
| 2 | W9-02 | Preserve Unity rows through collision churn | Defect | M | `E/unity.rs`; `C/database/strings.rs`; new `FT/unity_refresh_identity.rs` |
| 3 | W9-03 | Drain cancellation; retain actual patch outcome | Defect | M | `S/routes/jobs.rs`; `S/routes/patch.rs`; `S/job_ws_tests.rs` |
| 4 | W9-04 | Validate rebuilt Unity dialogue grammar | Defect | M | `E/unity.rs` |
| 5 | W9-05 | Retain queue ownership through disconnects | Defect | M | `U/lib/ws.ts`; `U/lib/ws.test.ts`; `U/stores/queueStore.ts`; `U/lib/queueExecution.test.ts`; `U/components/QueuePanel.tsx`; `U/components/QueuePanel.test.tsx` |
| 6 | W9-06 | Reject lossy YU-RIS encoding aliases | Defect | S | `E/yuris.rs` |
| 7 | W9-07 | Bind font generation to its project | Defect | M | `S/font_patch.rs`; `U/lib/api.ts`; `U/components/FontPatchDialog.tsx`; new matching `.test.tsx`; `U/lib/i18n/en.ts`; `U/lib/i18n/es.ts`; `apps/desktop/package.json` |
| 8 | W9-08 | Handle live WebSocket peer closure | Capability | M | `S/routes/jobs.rs`; `S/job_ws_tests.rs` |
| 9 | W9-09 | Publish complete font-patch ZIPs | Defect | S | `C/font_patch.rs` |
| 10 | W9-10 | Preserve editor page on remount | Defect | S | `U/components/FilterBar.tsx`; new matching `.test.tsx`; `apps/desktop/package.json` |

**H — Suggested pairing:** Run each lane sequentially. Their complete file sets are disjoint; both Unity units stay in A, and shared socket/UI files stay in B.

| Pair | Lane A | Lane B |
|---:|---|---|
| 1 | W9-01 — Patch namespace validation | W9-03 — Patch cancellation lifecycle |
| 2 | W9-02 — Unity refresh identity | W9-05 — Queue reconnection |
| 3 | W9-04 — Unity dialogue grammar | W9-08 — Live socket handling |
| 4 | W9-06 — YU-RIS encoding fidelity | W9-07 — Font project binding |
| 5 | W9-09 — Font ZIP publication | W9-10 — Editor pagination |