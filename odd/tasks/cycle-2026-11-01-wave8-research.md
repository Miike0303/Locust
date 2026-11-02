# Cycle 2026-11-01: wave 8 research (RESEARCH-8, duel)

Two independent Codex gpt-6.1-sol high researchers on `7ee0c13`. Found independently by BOTH (highest confidence): glossary mutations not bound to the project (switch races), retry classification by substring instead of HTTP status, multiline PO comment export breaks re-import, stale glossary edits overwrite concurrent changes, Ollama drops per-row context/glossary in batches, DeepL zips responses without a count check, desktop backup restore has no preview. A only: initial cost-limit preflight counts only source bytes, usage lost when a paid answer is rejected, CLI translate ignores Ctrl+C cancellation. B only: native export can target another project after the Save dialog, Grok OAuth refresh turns transient 5xx into "session expired", DeepL free tier blocked under a spend limit. Parent spot-checked retry.rs:136 and deepl.rs response handling.

## Researcher A

I recommend **eight defects and two capabilities**, each bounded to ≤3 days for one writer. Findings are source-derived; no reproductions, builds, tests, network calls, or file changes were performed.

**F** = verified source behavior, including derived input → output. **H** = proposed work, value, estimate, risk, acceptance criterion, or compatibility requirement.

Path aliases below expand exactly: `C=crates/core/src`, `P=crates/providers/src`, `S=crates/server/src/routes`, `U=apps/desktop/src`, `D=apps/desktop/src-tauri/src`, `Q=crates/cli/src`. Edit boundaries include tests.

1. **W8-01 — Prevent glossary mutations from crossing a project switch**

   **H · defect · M · 2 days · medium risk.** Value: keep terminology changes in the project where the user submitted them.

   **F — Input → wrong state:** A glossary POST checks that project A is open, then calls the shared glossary without reserving the project. Interleave opening B between that check and the database write: A’s term is inserted into B. Delete has the same gap; native add follows the same pattern. ([S/glossary_memory.rs:46](C:/Projects/Locust-lane-a/crates/server/src/routes/glossary_memory.rs:46), [S/glossary_memory.rs:67](C:/Projects/Locust-lane-a/crates/server/src/routes/glossary_memory.rs:67), [D/commands.rs:995](C:/Projects/Locust-lane-a/apps/desktop/src-tauri/src/commands.rs:995))

   **F:** Glossary writes use the shared database, and project opening swaps that database’s connection in place. ([C/glossary.rs:410](C:/Projects/Locust-lane-a/crates/core/src/glossary.rs:410), [C/project.rs:507](C:/Projects/Locust-lane-a/crates/core/src/project.rs:507), [C/database/connection.rs:109](C:/Projects/Locust-lane-a/crates/core/src/database/connection.rs:109))

   **H — Work/files:** Reserve the project through add/delete completion. Edit `S/glossary_memory.rs`, `D/commands.rs`, including inline regressions.

   **H — Acceptance:** A barrier-controlled open-versus-add/delete interleaving either completes against A or refuses with conflict; B receives no mutation. Cover HTTP and native add.

   **H — Compatibility/sharing:** Preserve row IDs, schema and request shapes; conflicting operations gain HTTP 409. Shares both files with W8-04.

2. **W8-02 — Classify retries from HTTP status rather than arbitrary body substrings**

   **H · defect · M · 2 days · medium risk.** Value: avoid unnecessary retries and recover reliably from transient failures.

   **F — Input → wrong output:** `is_retryable("OpenAI returned status 503 Service Unavailable: request id 1403")` returns **false**, because `403` appears inside the request ID. Conversely, status 400 with body `maximum input 500 tokens` returns **true**. Authentication and retry checks search the entire diagnostic string. ([C/translation/retry.rs:136](C:/Projects/Locust-lane-a/crates/core/src/translation/retry.rs:136))

   **F:** The OpenAI adapter places status and arbitrary response text in that same string; retry execution consumes the classifier’s result. ([P/openai.rs:266](C:/Projects/Locust-lane-a/crates/providers/src/openai.rs:266), [C/translation/retry.rs:80](C:/Projects/Locust-lane-a/crates/core/src/translation/retry.rs:80))

   **H — Work/files:** Parse the adapters’ explicit status field first; retain a conservative fallback for status-free transport errors. Edit `C/translation/retry.rs`, including inline tests.

   **H — Acceptance:** The examples produce transient retry and immediate refusal respectively. Verify attempt counts, authentication refusals, cancellation and deadline behavior.

   **H — Compatibility/sharing:** No IDs, saved-data or API-shape changes; diagnostics remain readable. No shared edit files.

3. **W8-03 — Make PO round-trips preserve multiline comments and tabs**

   **H · defect · M · 2 days · low–medium risk.** Value: let translators reliably exchange their project through a PO editor.

   **F — Input → wrong output:** Export an entry whose context is `Speaker: Alex\nLocation: Hall`. Export prefixes only the first physical line with `#.`; the emitted `Location: Hall` is subsequently parsed as a directive/continuation and fails as a malformed PO string. ([C/export.rs:136](C:/Projects/Locust-lane-a/crates/core/src/export.rs:136), [C/export.rs:267](C:/Projects/Locust-lane-a/crates/core/src/export.rs:267), [C/export.rs:355](C:/Projects/Locust-lane-a/crates/core/src/export.rs:355))

   **F — Second corruption case:** PO `msgstr "Go\tNow"` decodes to literal backslash-plus-`t`, not a tab. The unescaper recognizes newline, CR, quote and backslash, but preserves other escapes. Export also leaves actual tabs unescaped. ([C/export.rs:325](C:/Projects/Locust-lane-a/crates/core/src/export.rs:325))

   **H — Work/files:** Prefix every comment line and implement symmetric tab escaping. Edit `C/export.rs`; add `crates/core/tests/w8_po_roundtrip.rs`.

   **H — Acceptance:** Multiline context exports and re-imports successfully; actual tabs and literal `\\t` each round-trip distinctly. Existing CR/LF, fuzzy and stale-source behavior remains intact.

   **H — Compatibility/sharing:** IDs/schema/API unchanged. Imported `\t` gains its correct decoded meaning; literal backslashes remain representable. No shared edit files.

4. **W8-04 — Guard glossary edits against newer terminology**

   **H · defect · M · 3 days · medium risk.** Value: prevent an older glossary editor from silently replacing someone else’s correction.

   **F — Input → wrong state:** Open an edit containing translation `V1` and note `N1`. Another client saves `V2/N2`. Saving the older form sends its complete entry without a baseline, and the unconditional upsert replaces both newer fields. ([U/pages/settings/GlossarySection.tsx:136](C:/Projects/Locust-lane-a/apps/desktop/src/pages/settings/GlossarySection.tsx:136), [U/lib/api.ts:766](C:/Projects/Locust-lane-a/apps/desktop/src/lib/api.ts:766), [C/database/glossary.rs:8](C:/Projects/Locust-lane-a/crates/core/src/database/glossary.rs:8))

   **H — Work/files:** Add an optional expected-entry baseline, compare atomically, and retain the form on conflict. Keep term/pair immutable during editing.

   **H — Exact files:** `C/database/glossary.rs`, `C/glossary.rs`, `S/glossary_memory.rs`, `D/commands.rs`, `U/lib/api.ts`, `U/lib/apiError.ts`, `U/lib/apiError.test.ts`, `U/pages/settings/GlossarySection.tsx`, `U/pages/settings/GlossarySection.test.tsx`, `U/lib/i18n/en.ts`, `U/lib/i18n/es.ts`.

   **H — Acceptance:** The stale edit receives conflict through HTTP/Tauri; `V2/N2` survives and the local draft remains available. Unchanged baselines save successfully.

   **H — Compatibility/sharing:** Additive optional request field; legacy callers retain unconditional behavior. No schema/ID migration. Shares backend files with W8-01 and API/locales with W8-09.

5. **W8-05 — Carry every Ollama row’s context and glossary**

   **H · defect · S · 1 day · low risk.** Value: preserve scene-specific terminology and length guidance when translating locally.

   **F — Input → wrong outbound payload:** Supply two requests with different context/glossary hints. Ollama puts only request 0’s metadata in the system prompt and serializes the user message as source strings alone. Request 1’s metadata disappears; request 0’s guidance becomes batch-wide. This is a deterministic request-construction defect, without claiming a particular model answer. ([P/ollama.rs:96](C:/Projects/Locust-lane-a/crates/providers/src/ollama.rs:96), [P/openai.rs:86](C:/Projects/Locust-lane-a/crates/providers/src/openai.rs:86))

   **F:** The existing shared batch builder already serializes per-record metadata and rejects mixed-language batches. ([P/openai.rs:136](C:/Projects/Locust-lane-a/crates/providers/src/openai.rs:136))

   **H — Work/files:** Reuse that builder in `P/ollama.rs`, including inline request-body regressions.

   **H — Acceptance:** Two rows retain their distinct context/glossary in captured JSON; a short-slot instruction is not promoted to global guidance. Mixed-language input refuses before dispatch.

   **H — Compatibility/sharing:** Response arrays, attribution, IDs and saved data remain unchanged; the outbound prompt changes. No shared edit files.

6. **W8-06 — Include complete requests in initial cost-limit preflight**

   **H · defect · M · 2 days · medium risk.** Value: stop a heavily annotated paid request before it exceeds the configured estimate limit.

   **F — Input → wrong decision:** The initial dispatch estimate counts only raw source bytes, although requests already contain game context and glossary hints. Additional repair-call estimates include all three fields. ([C/translation/manager.rs:1217](C:/Projects/Locust-lane-a/crates/core/src/translation/manager.rs:1217), [C/translation/manager.rs:1302](C:/Projects/Locust-lane-a/crates/core/src/translation/manager.rs:1302), [C/translation/manager.rs:1357](C:/Projects/Locust-lane-a/crates/core/src/translation/manager.rs:1357), [C/translation/manager.rs:403](C:/Projects/Locust-lane-a/crates/core/src/translation/manager.rs:403))

   **F — Concrete derivation:** With source length 1, game-context length 1,000, an estimator returning its input count, and limit 5, initial dispatch passes using **1**; the complete-request calculation is **1,001**.

   **H — Work/files:** Share consistent, provider-appropriate request accounting between first dispatch and repairs. Edit `C/translation/manager.rs`; add `crates/core/tests/w8_dispatch_budget.rs`.

   **H — Acceptance:** The fixture refuses before any provider call; ordinary requests within budget still dispatch. Cover sanitized source, glossary hints and split batches.

   **H — Compatibility/sharing:** No IDs/schema/API changes; some previously admitted requests refuse earlier. Estimates remain estimates. Shares `manager.rs` with W8-07.

7. **W8-07 — Retain reported usage when a paid answer is rejected**

   **H · defect · M · 3 days · high risk.** Value: show the tokens and known spend incurred by unusable provider responses.

   **F — Input → wrong ledger:** Request two rows; return HTTP 200 with content `["Hola"]` and usage `{prompt_tokens:100, completion_tokens:20, total_tokens:120}`. OpenAI returns the count-mismatch error before extracting usage. Claude has the same ordering. ([P/openai.rs:293](C:/Projects/Locust-lane-a/crates/providers/src/openai.rs:293), [P/openai.rs:315](C:/Projects/Locust-lane-a/crates/providers/src/openai.rs:315), [P/claude.rs:148](C:/Projects/Locust-lane-a/crates/providers/src/claude.rs:148))

   **F:** The engine’s error branch marks cost incomplete but receives no usage; the recorded run consequently contains zero token counters instead of the reported 120. ([C/translation/manager.rs:2018](C:/Projects/Locust-lane-a/crates/core/src/translation/manager.rs:2018), [C/translation/manager.rs:2142](C:/Projects/Locust-lane-a/crates/core/src/translation/manager.rs:2142))

   **H — Work/files:** Carry known usage independently of accepted translations, including configured pricing. Edit `C/error.rs`, `C/translation/manager.rs`, `P/openai.rs`, `P/claude.rs`, `P/model_policy.rs`; add `crates/core/tests/w8_rejected_usage.rs`. Adapter regressions stay inline.

   **H — Acceptance:** The fixture saves no translations but records 100 input/20 output/120 total tokens and the known calculated cost once. Missing usage remains explicitly unknown.

   **H — Compatibility/sharing:** Preserve error text, external response shapes and DB schema; extend internal error/accounting plumbing. Shares `manager.rs` with W8-06.

8. **W8-08 — Reject DeepL response-count mismatches before zipping**

   **H · defect · S · 1 day · low risk.** Value: prevent an ambiguous provider response from being accepted as a complete batch.

   **F — Input → wrong output:** For requests `[a,b]`, return translations `[X,Y,Z]`. DeepL returns `[a→X,b→Y]`, silently discarding `Z`, because it zips without checking cardinality. ([P/deepl.rs:154](C:/Projects/Locust-lane-a/crates/providers/src/deepl.rs:154))

   **F:** The downstream guard sees exactly the expected two IDs, so it cannot detect the discarded extra response. A *short* response is caught downstream; this proposal does not claim otherwise. ([C/translation/manager.rs:1450](C:/Projects/Locust-lane-a/crates/core/src/translation/manager.rs:1450))

   **H — Work/files:** Require exact cardinality before mapping results. Edit `P/deepl.rs`, including inline regressions.

   **H — Acceptance:** Short and extra response arrays return an explicit count mismatch; exact-count responses preserve ordering and attribution.

   **H — Compatibility/sharing:** IDs/schema/API unchanged; malformed upstream responses fail earlier. No shared edit files.

9. **W8-09 — Preview backup restore impact in desktop**

   **H · capability · M · 2–3 days · medium risk.** Value: let a translator see which current files will be replaced or removed before restoring.

   **F — Current friction:** Desktop confirmation shows backup/game/path and then performs the restore. The HTTP handler always calls the writing method. Core already exposes a locked preview, and CLI uses it for dry-run output. ([U/pages/settings/DataSection.tsx:158](C:/Projects/Locust-lane-a/apps/desktop/src/pages/settings/DataSection.tsx:158), [U/lib/api.ts:1011](C:/Projects/Locust-lane-a/apps/desktop/src/lib/api.ts:1011), [S/backups.rs:51](C:/Projects/Locust-lane-a/crates/server/src/routes/backups.rs:51), [C/backup.rs:1174](C:/Projects/Locust-lane-a/crates/core/src/backup.rs:1174), [Q/commands/project.rs:49](C:/Projects/Locust-lane-a/crates/cli/src/commands/project.rs:49))

   **H — Work/files:** Add an optional dry-run query and show bounded lists/counts for replacements, recreations, removals and kept additions. Edit `S/backups.rs`, `U/lib/api.ts`, `U/pages/settings/DataSection.tsx`, `U/lib/i18n/en.ts`, `U/lib/i18n/es.ts`, `apps/desktop/package.json`; add `U/pages/settings/DataSection.test.tsx`.

   **H — Acceptance:** Preview changes no game bytes; Cancel writes nothing. Confirmation performs the normal revalidated restore. Preview failures retain useful recovery guidance.

   **H — Compatibility/sharing:** Additive query; existing restore behavior and report shape remain. Preview is advisory, not a frozen restore plan. Shares API/locales with W8-04.

10. **W8-10 — Connect CLI interruption to translation cancellation**

    **H · capability · M · 2 days · medium risk.** Value: stop a CLI translation through the engine’s cancellation path and retain its final accounting.

    **F — Current gap:** CLI creates a cancellation token and moves it into the worker, then only receives events; that command retains no cancellation handle or signal branch. The final summary waits until event processing ends. ([Q/commands/translate.rs:109](C:/Projects/Locust-lane-a/crates/cli/src/commands/translate.rs:109), [Q/commands/translate.rs:126](C:/Projects/Locust-lane-a/crates/cli/src/commands/translate.rs:126), [Q/commands/translate.rs:149](C:/Projects/Locust-lane-a/crates/cli/src/commands/translate.rs:149), [Q/commands/translate.rs:201](C:/Projects/Locust-lane-a/crates/cli/src/commands/translate.rs:201))

    **F:** Core already cancels in-flight waits and records observed usage on cancelled runs. ([C/translation/retry.rs:61](C:/Projects/Locust-lane-a/crates/core/src/translation/retry.rs:61), [C/translation/manager.rs:2126](C:/Projects/Locust-lane-a/crates/core/src/translation/manager.rs:2126))

    **H — Work/files:** Retain a token clone, select interruption alongside events, cancel once, then drain completion/accounting. Edit `Q/commands/translate.rs`; add `crates/cli/tests/translate_cancel.rs`.

    **H — Acceptance:** An injected interrupt during a delayed fixture cancels promptly, starts no fallback call, preserves committed rows, prints an interrupted summary and exits nonzero.

    **H — Compatibility/sharing:** No IDs/schema/API changes; CLI interruption behavior changes deliberately. No shared edit files.

**H — Ranked dispatch table.** The detailed boundaries above include every proposed test and supporting file.

| Rank | ID | Title | Category | Size | Main files |
|---:|---|---|---|---|---|
| 1 | W8-01 | Bind glossary mutations to their project | Defect | M | `S/glossary_memory.rs`, `D/commands.rs` |
| 2 | W8-02 | Classify retries from HTTP status | Defect | M | `C/translation/retry.rs` |
| 3 | W8-03 | Preserve PO comments and tabs | Defect | M | `C/export.rs` |
| 4 | W8-04 | Guard stale glossary edits | Defect | M | `C/database/glossary.rs`, `C/glossary.rs`, `S/glossary_memory.rs`, `D/commands.rs`, `U/pages/settings/GlossarySection.tsx`, API/error/locales |
| 5 | W8-05 | Preserve every Ollama row’s guidance | Defect | S | `P/ollama.rs` |
| 6 | W8-06 | Budget complete initial requests | Defect | M | `C/translation/manager.rs` |
| 7 | W8-07 | Retain usage from rejected answers | Defect | M | `C/error.rs`, `C/translation/manager.rs`, `P/openai.rs`, `P/claude.rs`, `P/model_policy.rs` |
| 8 | W8-08 | Validate DeepL response counts | Defect | S | `P/deepl.rs` |
| 9 | W8-09 | Preview desktop backup restores | Capability | M | `S/backups.rs`, `U/pages/settings/DataSection.tsx`, API/locales/package |
| 10 | W8-10 | Cancel CLI translation gracefully | Capability | M | `Q/commands/translate.rs` |

**H — Suggested pairing into two lanes with disjoint complete file sets:**

| Pair | Lane A — sequential | Lane B — sequential |
|---:|---|---|
| 1 | W8-02 — Retry classification | W8-01 — Glossary project ownership |
| 2 | W8-05 — Ollama metadata | W8-03 — PO round-trips |
| 3 | W8-06 — Dispatch budgeting | W8-04 — Guarded glossary edits |
| 4 | W8-07 — Rejected-answer usage | W8-08 — DeepL cardinality |
| 5 | W8-10 — CLI cancellation | W8-09 — Restore preview |

**H:** Keep both `manager.rs` changes in Lane A, both glossary transport changes in Lane B, and all frontend API/localization changes in Lane B.


## Researcher B

I recommend **nine defects and one capability**, each bounded to three writer-days. Findings are source-derived; I changed no files and ran no builds, tests, or network calls.

**F** means verified source behavior, including derived outputs. **H** means proposed work, value, estimate, risk, acceptance criteria, or compatibility assessment. The defects below have not been runtime-reproduced.

Paths use **C** = `crates/core/src`, **P** = `crates/providers/src`, **S** = `crates/server/src`, **U** = `apps/desktop/src`, **D** = `apps/desktop/src-tauri/src`. Rust file boundaries include inline regression tests.

1. **W8-01 — Bind glossary mutations to their originating project**

   **H — Defect · M, 3 days · medium risk.** Value: prevent a terminology edit or deletion intended for one game from changing another game’s glossary.

   **F — Input → wrong state:** Open A, begin opening B, and submit A’s glossary mutation while B’s opening operation holds the project reservation. Glossary POST and DELETE never acquire that reservation; they merely check that some project is open. Once B’s candidate replaces the live connection, those mutations operate on B. Tauri addition has the same omission. ([S/routes/glossary_memory.rs:42](/C:/Projects/Locust-lane-b/crates/server/src/routes/glossary_memory.rs:42), [S/routes/glossary_memory.rs:62](/C:/Projects/Locust-lane-b/crates/server/src/routes/glossary_memory.rs:62), [D/commands.rs:995](/C:/Projects/Locust-lane-b/apps/desktop/src-tauri/src/commands.rs:995), [C/project.rs:507](/C:/Projects/Locust-lane-b/crates/core/src/project.rs:507))

   **F:** The frontend sends the entry or term/pair without project identity, so a delayed request cannot be distinguished after switching. ([U/lib/api.ts:766](/C:/Projects/Locust-lane-b/apps/desktop/src/lib/api.ts:766))

   **H — Work/files:** Reserve the project through each mutation and accept an optional expected database identity, supplied by the desktop. Edit `S/routes/glossary_memory.rs`, `D/commands.rs`, `U/lib/api.ts`, `U/pages/settings/GlossarySection.tsx`, `U/pages/settings/GlossarySection.test.tsx`.

   **H — Acceptance:** A held opening reservation makes HTTP/Tauri additions and HTTP deletion return busy. An A-bound request delivered after switching to B returns conflict; neither glossary changes.

   **H — Compatibility/sharing:** No row IDs, schema, or saved-data migration. Additive optional request field; legacy requests gain reservation protection. Shares files with W8-02, W8-03 and W8-09.

2. **W8-02 — Guard glossary edits against concurrent changes**

   **H — Defect · M, 2–3 days · medium risk.** Value: preserve another reviewer’s terminology correction instead of silently replacing it.

   **F — Input → wrong output:** Open glossary entry `HP → PV` for editing. Another client saves `HP → Salud`. Saving the original editor as `HP → Vida` overwrites `Salud`: the UI sends only the edited entry, and storage unconditionally updates translation, context and case sensitivity on conflict. ([U/pages/settings/GlossarySection.tsx:136](/C:/Projects/Locust-lane-b/apps/desktop/src/pages/settings/GlossarySection.tsx:136), [C/database/glossary.rs:8](/C:/Projects/Locust-lane-b/crates/core/src/database/glossary.rs:8))

   **H — Work/files:** Preserve the original entry separately and compare its complete baseline atomically before updating. Retain the draft on conflict and offer reload or explicit overwrite. Edit `C/database/glossary.rs`, `C/glossary.rs`, `S/routes/glossary_memory.rs`, `D/commands.rs`, `U/lib/api.ts`, `U/pages/settings/GlossarySection.tsx`, `U/pages/settings/GlossarySection.test.tsx`, `U/lib/i18n/en.ts`, `U/lib/i18n/es.ts`.

   **H — Acceptance:** The interleaving above returns conflict and preserves `Salud` plus the local `Vida` draft. Repeat with concurrent context changes, case changes and deletion. An unchanged baseline saves normally through both transports.

   **H — Compatibility/sharing:** Existing glossary keys and schema remain unchanged; baseline is optional for legacy callers. Shares files with W8-01, W8-03 and W8-09. This concerns glossary records, beyond the delivered string-approval guards.

3. **W8-03 — Keep native exports bound to the project shown in the dialog**

   **H — Defect · M, 2 days · medium risk.** Value: prevent exporting another game’s translations under the filename selected for the original game.

   **F — Input → wrong output:** Start exporting A and leave the native Save dialog open. Switch the backend to B through another client, then complete Save. After awaiting the dialog, `handleExport` submits without checking project identity. Tauri exports whichever database is current when the command executes, producing B’s catalog at A’s selected destination. ([U/components/ExportModal.tsx:137](/C:/Projects/Locust-lane-b/apps/desktop/src/components/ExportModal.tsx:137), [U/lib/api.ts:903](/C:/Projects/Locust-lane-b/apps/desktop/src/lib/api.ts:903), [D/commands.rs:574](/C:/Projects/Locust-lane-b/apps/desktop/src-tauri/src/commands.rs:574))

   **H — Work/files:** Capture the expected project at export initiation, recheck after the dialog, and verify that identity under backend ownership for PO/XLIFF/TMX. Edit `U/components/ExportModal.tsx`, `U/components/ExportModal.test.tsx`, `U/lib/api.ts`, `D/commands.rs`, `S/routes/io.rs`.

   **H — Acceptance:** Suspend Save on A, switch to B, then resolve it: export refuses and leaves the destination untouched. A stale expected identity also refuses through HTTP. Unchanged-project exports retain their content and scope.

   **H — Compatibility/sharing:** No row-ID or saved-data changes. Optional export identity is additive. Shares files with W8-01/W8-02 through Tauri/API and W8-09 through API.

4. **W8-04 — Reject excess DeepL translations before positional mapping**

   **H — Defect · S, 1 day · low risk.** Value: prevent malformed responses from assigning translations to the wrong rows.

   **F — Input → wrong output:** Send two requests, `Hello` and `Goodbye`, and receive:
   ```json
   {"translations":[{"text":"EXTRA"},{"text":"Hola"},{"text":"Adiós"}]}
   ```
   DeepL zips requests with responses without checking length, returning `Hello → EXTRA`, `Goodbye → Hola` and silently discarding `Adiós`. The resulting IDs satisfy the engine’s cardinality/identity check because the adapter reconstructed them from the requests. ([P/deepl.rs:154](/C:/Projects/Locust-lane-b/crates/providers/src/deepl.rs:154), [C/translation/manager.rs:1450](/C:/Projects/Locust-lane-b/crates/core/src/translation/manager.rs:1450))

   **H — Work/files:** Require exact response cardinality before constructing results. Edit `P/deepl.rs`.

   **H — Acceptance:** Short and excess responses both fail without saving any row; an exact two-element response preserves ordered mapping. Include the excess-prefix fixture above.

   **H — Compatibility/sharing:** Valid responses, IDs, schema and API shape remain unchanged. Shares `P/deepl.rs` with W8-10.

5. **W8-05 — Give actual HTTP status precedence in retry classification**

   **H — Defect · M, 2 days · medium risk.** Value: stop repeating invalid requests and recover transient failures despite incidental numbers in diagnostics.

   **F — Input → wrong decision:** OpenAI incorporates upstream status and body into one string. The classifier then searches that entire string for numeric substrings. Consequently:
   - `OpenAI returned status 400 Bad Request: max_tokens must be <= 500` becomes **retryable** because of `500`.
   - `OpenAI returned status 503 Service Unavailable: request_id=401abc` becomes **non-retryable** because the `401` check runs first.

   ([P/openai.rs:266](/C:/Projects/Locust-lane-b/crates/providers/src/openai.rs:266), [C/translation/retry.rs:136](/C:/Projects/Locust-lane-b/crates/core/src/translation/retry.rs:136))

   **H — Work/files:** Recognize the adapters’ explicit HTTP-status envelope before consulting legacy free-text signals; isolate incidental body numbers. Edit `C/translation/retry.rs`, `P/retry.rs`, `P/openai.rs`.

   **H — Acceptance:** Both examples yield the correct decision and call count. Cover real 401/403, 429, transient server errors, connection errors and cancellation; retain redaction.

   **H — Compatibility/sharing:** No IDs, saved-data or public API changes; retry behavior changes for misclassified errors. No shared edit files within this wave.

6. **W8-06 — Preserve transient failures during Grok subscription refresh**

   **H — Defect · S, 1 day · low–medium risk.** Value: avoid telling a logged-in user to authenticate again during a temporary refresh outage.

   **F — Input → wrong output:** With a token near expiry, return HTTP 503 from token refresh. Every unsuccessful refresh response becomes `xAI session expired — run locust auth grok to log in again`; its status disappears. That message has none of the retry classifier’s transient markers, so the failure is treated as permanent. ([P/xai_oauth.rs:369](/C:/Projects/Locust-lane-b/crates/providers/src/xai_oauth.rs:369), [P/xai_oauth.rs:417](/C:/Projects/Locust-lane-b/crates/providers/src/xai_oauth.rs:417), [C/translation/retry.rs:136](/C:/Projects/Locust-lane-b/crates/core/src/translation/retry.rs:136))

   **H — Work/files:** Preserve sanitized status information and distinguish credential rejection from 429/5xx refresh failures. Introduce an internal injectable refresh endpoint for offline tests. Edit `P/xai_oauth.rs`.

   **H — Acceptance:** A 503-then-success refresh recovers through bounded retry; 401/403 retain login guidance and do not retry. Transient failures leave the stored tokens unchanged, and diagnostics expose no secrets.

   **H — Compatibility/sharing:** Token format, paths, provider ID and translation results remain unchanged. No shared edit files.

7. **W8-07 — Export multiline PO context as actual comment lines**

   **H — Defect · S, 1 day · low risk.** Value: make Locust’s own exported catalogs re-importable when source context spans lines.

   **F — Input → wrong output:** Set an entry’s context to `Speaker\nScene`. Export emits:
   ```po
   #. Speaker
   Scene
   ```
   Only the first line is a comment. Import treats `Scene` as a required quoted continuation and returns a malformed-PO-string error, blocking the catalog. ([C/export.rs:135](/C:/Projects/Locust-lane-b/crates/core/src/export.rs:135), [C/export.rs:226](/C:/Projects/Locust-lane-b/crates/core/src/export.rs:226), [C/export.rs:267](/C:/Projects/Locust-lane-b/crates/core/src/export.rs:267), [C/export.rs:355](/C:/Projects/Locust-lane-b/crates/core/src/export.rs:355))

   **H — Work/files:** Prefix every physical context-comment line safely, including CRLF and lone-CR inputs. Edit `C/export.rs`.

   **H — Acceptance:** Export→import succeeds with multiline context and preserves every entry ID, source and translation. Context containing PO-looking directives remains comment content rather than creating entries.

   **H — Compatibility/sharing:** No IDs, schema or API changes; generated comment formatting changes. No shared edit files.

8. **W8-08 — Carry every Ollama record’s context and glossary**

   **H — Defect · M, 2 days · low–medium risk.** Value: let local-model users apply each row’s terminology and length guidance.

   **F — Input → wrong wire output:** Batch request A with context `menu caption` and request B with context `maximum 4 characters` plus glossary `Save → Guar`. Ollama builds its system prompt solely from A and serializes only the sources into the user message. B’s context and glossary are absent from the request. ([P/ollama.rs:96](/C:/Projects/Locust-lane-b/crates/providers/src/ollama.rs:96))

   **F:** The shared batch builder already serializes each record’s context and glossary. The engine’s Ollama singleton workaround applies only when character notes exist, leaving ordinary multi-record context exposed. ([P/openai.rs:148](/C:/Projects/Locust-lane-b/crates/providers/src/openai.rs:148), [C/translation/manager.rs:935](/C:/Projects/Locust-lane-b/crates/core/src/translation/manager.rs:935))

   **H — Work/files:** Use the existing record-aware batch prompt while retaining ordered string-array responses. Edit `P/ollama.rs`.

   **H — Acceptance:** A captured two-row request contains both contexts and B’s glossary under B’s record, with neither promoted to shared instructions. Response-count checks and batch token attribution remain intact.

   **H — Compatibility/sharing:** No row IDs, saved-data or Locust API changes; Ollama prompt content changes. No shared edit files.

9. **W8-09 — Show restore consequences before desktop confirmation**

   **H — Capability · M, 2–3 days · medium risk.** Value: show translators which files restoration will replace, recreate, remove or retain before they commit.

   **F — Current friction:** Desktop confirmation contains backup/game/path identification; confirmation immediately invokes restoration. The parsed file report is available afterward. ([U/pages/settings/DataSection.tsx:36](/C:/Projects/Locust-lane-b/apps/desktop/src/pages/settings/DataSection.tsx:36), [U/pages/settings/DataSection.tsx:158](/C:/Projects/Locust-lane-b/apps/desktop/src/pages/settings/DataSection.tsx:158))

   **F:** Core already supplies `preview_restore`, and CLI `restore-backup --dry-run` uses it. The HTTP handler always calls `restore_with_report`. This proposal exposes that existing preview in desktop. ([C/backup.rs:1174](/C:/Projects/Locust-lane-b/crates/core/src/backup.rs:1174), [crates/cli/src/commands/project.rs:47](/C:/Projects/Locust-lane-b/crates/cli/src/commands/project.rs:47), [S/routes/backups.rs:44](/C:/Projects/Locust-lane-b/crates/server/src/routes/backups.rs:44))

   **H — Work/files:** Add an optional dry-run query to the existing restore handler, then display an ID-bound preview before enabling confirmation. Edit `S/routes/backups.rs`, `U/lib/api.ts`, `U/pages/settings/DataSection.tsx`, `U/lib/backupRestore.test.ts`, `U/lib/i18n/en.ts`, `U/lib/i18n/es.ts`.

   **H — Acceptance:** Opening confirmation fetches the preview without restoring game bytes or changing injection history. Display all categories with bounded rendering; preview errors block confirmation. Actual restoration reruns preflight, including changes made after preview.

   **H — Compatibility/sharing:** Existing restore requests and report shape remain valid; optional query is additive. No saved-data migration. Shares API and locale files with W8-01/W8-02/W8-03.

10. **W8-10 — Allow DeepL free-tier translation under a spend limit**

   **H — Defect · S, 1 day · low risk.** Value: stop an existing budget setting from blocking a configured free-tier provider before any translation begins.

   **F — Input → wrong output:** Construct DeepL with `free_tier=true` and translate with `cost_limit_usd=0`, memory/glossary disabled. Free-tier estimation returns `None`, but `is_free()` always returns false. The engine therefore aborts with `cannot enforce a cost limit: this provider has no cost estimate`. Free-tier results also report no cost. ([P/deepl.rs:70](/C:/Projects/Locust-lane-b/crates/providers/src/deepl.rs:70), [P/deepl.rs:172](/C:/Projects/Locust-lane-b/crates/providers/src/deepl.rs:172), [P/deepl.rs:181](/C:/Projects/Locust-lane-b/crates/providers/src/deepl.rs:181), [C/translation/manager.rs:1357](/C:/Projects/Locust-lane-b/crates/core/src/translation/manager.rs:1357))

   **H — Work/files:** Make configured free-tier monetary classification, estimation and receipts consistent, preserving paid-tier arithmetic. Edit `P/deepl.rs`; add `crates/providers/tests/deepl_free_budget.rs`.

   **H — Acceptance:** The free-tier fixture dispatches under a zero-dollar limit and records complete zero monetary cost. Paid-tier jobs still respect their cost gate; quota/auth errors retain normal handling.

   **H — Compatibility/sharing:** No IDs, config migration or API-shape changes. Configured free-tier provider metadata and future cost receipts become accurate; historical runs remain unchanged. Shares `P/deepl.rs` with W8-04.

**H — Ranked dispatch table.** Prefixes expand as defined above; test files are included.

| Rank | ID | Title | Category | Size | Exact edit files |
|---:|---|---|---|---|---|
| 1 | W8-01 | Bind glossary writes to project | Defect | M | `S/routes/glossary_memory.rs`; `D/commands.rs`; `U/lib/api.ts`; `U/pages/settings/GlossarySection.tsx`; its `.test.tsx` |
| 2 | W8-02 | Guard concurrent glossary edits | Defect | M | W8-01 files; `C/database/glossary.rs`; `C/glossary.rs`; `U/lib/i18n/en.ts`, `es.ts` |
| 3 | W8-03 | Bind exports to displayed project | Defect | M | `U/components/ExportModal.tsx`; its `.test.tsx`; `U/lib/api.ts`; `D/commands.rs`; `S/routes/io.rs` |
| 4 | W8-04 | Reject excess DeepL responses | Defect | S | `P/deepl.rs` |
| 5 | W8-05 | Classify actual HTTP status first | Defect | M | `C/translation/retry.rs`; `P/retry.rs`; `P/openai.rs` |
| 6 | W8-06 | Preserve transient OAuth refresh errors | Defect | S | `P/xai_oauth.rs` |
| 7 | W8-07 | Preserve multiline PO comments | Defect | S | `C/export.rs` |
| 8 | W8-08 | Preserve Ollama record metadata | Defect | M | `P/ollama.rs` |
| 9 | W8-09 | Preview desktop backup restoration | Capability | M | `S/routes/backups.rs`; `U/lib/api.ts`; `U/pages/settings/DataSection.tsx`; `U/lib/backupRestore.test.ts`; `U/lib/i18n/en.ts`, `es.ts` |
| 10 | W8-10 | Honor free-tier monetary cost | Defect | S | `P/deepl.rs`; new `crates/providers/tests/deepl_free_budget.rs` |

**H — Suggested two-lane pairing:** Run each lane sequentially. Complete file sets are disjoint across lanes.

| Pair | Lane A — project/data surfaces | Lane B — providers/retry |
|---:|---|---|
| 1 | W8-01 | W8-04 |
| 2 | W8-02 | W8-05 |
| 3 | W8-03 | W8-06 |
| 4 | W8-07 | W8-08 |
| 5 | W8-09 | W8-10 |
