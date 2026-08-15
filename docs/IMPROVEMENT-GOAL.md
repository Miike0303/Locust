# Improvement goal

State for the `/goal` cycle. Read it before proposing anything; update it after every cycle.

## Goal

Make Locust's two products — the translation app and the patch tool — trustworthy for someone who is not a developer. The engine is far ahead of the surfaces that expose it: prefer work that removes a way for a user to lose work, be misled, or get stuck, over work that adds capability.

## Constraints

Standing bans. Each exists for a reason; do not re-litigate them.

- **Never write to a user's game files unattended.** Batch inject/pack in a queue is a footgun, not a feature. Read-only steps (validate) are fine.
- **No new dependency for what a few lines of stdlib or platform API can do.** The UI i18n layer is ~60 lines over `Intl.PluralRules` and needs no library.
- **Do not build the receiving half of a channel that has no sender.** Deep links, protocol handlers, and update servers wait until something exists to link from.
- **Never associate `.zip`.** Hijacking every archive on a user's machine for a game-translation tool is not acceptable scope. A `locust://` scheme is the correct granularity, when it is time.
- **Newlines in extracted text are not always width wrapping.** Unity Naninovel `@cmd` blocks and Unreal `.locres` paragraphs are semantic; re-wrapping them corrupts scripts. See `CLAUDE.md`.
- **Secrets never gain a derived `Debug`.** `TokenStore` has a hand-written redacting impl for this reason.

## Backlog

- **`t()` is not key-typed.** `apps/desktop/src/lib/i18n/index.ts:139` declares `t(key: string, ...)`, so the compile-time guarantee only covers EN↔ES parity (`es.ts` is `Record<keyof typeof en, string>`). A key the code uses but neither catalog defines compiles clean and renders its raw name on screen. Tightening to `keyof typeof en` would catch it, but the dynamic call sites — `t(JOB_STREAM_LOST_MESSAGE)`, `t(resolvedSource.error)` — would need their unions typed as key subsets first. Found by Claude, cycle 5.
- The queue cannot use translate options the Translate dialog and CLI both have: `QueuePanel.tsx:230-243` hardcodes `max_concurrent: 3`, `cost_limit_usd: null` and sends no `fallback_provider_ids`, though `TranslationStartParams` already carries them. Fallbacks and smaller batches are the documented escape from the grok count-mismatch stall on Taimanin-scale runs (`docs/VN_RPG_TRANSLATION.md:105-108`). Found by Cursor, cycle 5.
- `suggest_replacement_font` maps missing scripts to Noto families (`crates/core/src/font_validation.rs:118-184`) and is called only by its own tests. Validate reports missing glyphs and stops, so the user leaves the app to go font hunting. Found by Cursor, cycle 5.
- Packing a patch slurps whole multi-GB files that apply already streams: `pack_injection_recording` does `std::fs::read(&src)` per recorded file, then hashes and writes it (`crates/core/src/patch/pack.rs:307-332`), and `record_injection` slurps again to SHA-256 (`crates/core/src/database.rs:852-853`). Apply is explicit about never buffering (`crates/core/src/patch/stream.rs:16-17`). Peak RAM ≈ file size, or 2× with `--pristine`. Reuse the existing chunked path. Found by Grok, cycle 2.
- Unreal extract still reads the whole pak (`crates/formats/src/unreal.rs:637-644`) and scans the full buffer twice — `find_locres_offsets` byte-steps it (`unreal_locres.rs:569-584`) and `find_utf16le_strings` walks it again (`unreal.rs:455-507`). Larger and riskier than the detection fix; deliberately deferred from cycle 2.
- `find_pak_files` runs twice per open — once from `detect` (`crates/formats/src/unreal.rs:105`), again from `extract` (`:605`). Cheap now that detection only reads the tail, but still duplicated work.
- `save_entries` (`crates/core/src/database.rs:399-409`) and `merge_entries` (`:1182`, `:1205`, `:1239`) call `tx.execute` inside loops, so the same SQL is parsed and planned once per row — up to 33,767 times on a real extract. `prepare_cached` would do it once. Found by Claude, cycle 2; no automatable check identified, which is why it lost to a measurable win.
- Patch apply: closing the socket without a terminal frame leaves the UI idle while the job keeps writing, and a second apply for the same folder is accepted. Found by Grok, cycle 1.
- Server error messages are not localized. They reach the user verbatim inside translated toasts (`apps/desktop/src/lib/api.ts` reads `res.text()`, `Welcome.tsx:182` interpolates it), so a Spanish UI shows English sentences from the backend. A whole class, not one string.
- Any other database write racing `Database::reopen` is the same class as cycle 4 but was not the evidenced case — a synchronous inject, for instance. A general lock around the swap is a larger change and was deliberately not attempted.
- No web presence of any kind: no landing, no docs site, no deploy. `crates/server` cannot serve static files (`tower-http` is compiled with `cors, trace` only). Deferred by the user, not rejected.

## In flight

Nothing.

## Done

- `pending` — **cycle 7 (capability, from backlog).** A pivoted `.locust.db` could only be opened from PivotModal's success button; Recents always called extract-on-game-path open, `open_project_db` never wrote recents, and the desktop dropped `database_path`. Now `RecentProject` carries an optional `database_path` (serde-default so old configs load), open-db adds a recent that coexists with the source game, Welcome routes those entries through `completeOpenProjectDb`, and the list badges project DBs with the db path plus the game root. Pinned by config dedupe tests + `shouldOpenProjectDb` / `projectFromOpenResponse` unit checks. CLI-made dbs still need a first open-db (or pivot) to land in Recents — no blind "pick any .db" without a game path. No research duel: cited cycle-5 capability. Next rotation: `optimization`.

- `pending` — **cycle 6 (defect, from backlog).** Inject celebrated every non-throwing response: the toast was always success, the panel always green "Injection complete", and Pack was offered even when recording was `NothingRecorded`. The engine already returned `strings_written`, `languages_failed`, `warnings`, `files_written`, and `outcomes`; the desktop dropped them at the door. Now `classifyInjectReport` maps zero-write → error toast + red panel, partial failures → warning, and real writes → success; warnings and files written are listed; multi-lang HTTP/Tauri attach `outcomes` the way direct already did; Pack CTA only when packing can succeed. Pinned by `injectOutcome.test.ts` (negative-tested: forcing zero-write to success fails the suite). Folded the cycle-5 "show warnings/files_written" backlog item into the same change. No research duel: two cited defect items were waiting and both lived in InjectModal. Next rotation: `capability`.

- `pending` — **cycle 5 (capability).** Reopening a game after it updated said nothing about what the merge had done. `stale_source_reset` means a translation was kept but sent back to pending because the game's text moved under it — and the desktop's `ProjectOpenResponse` dropped all five merge counters at the type level, so a grep for them across `apps/desktop/src/` returned nothing. We shipped those counters in the session's first commit and never wired them to a screen. Now the Activity Log records every open with the numbers, and a toast fires only when something needs acting on; a first extract and an unchanged re-open stay quiet. **First real duel:** both researchers independently ranked this gap first, and Cursor's other four findings went to the backlog.

- `pending` — **cycle 4 (defect, from backlog).** Opening a project while a translation was running swapped the SQLite connection under the live job, so game A's remaining translations landed in game B's database and the run ledger billed the wrong project. Project switching now refuses with a 409 while a translation is in flight, on both the HTTP and Tauri paths, and says what to do. It refuses rather than cancelling — silently discarding minutes of paid translation to satisfy a click is worse than refusing the click. A finished job inside its 30-second retention window does not block, and patch jobs never do; both are pinned by tests, and disabling the guard makes exactly the two 409 tests fail. No research duel: taken from the backlog. Grok stopped at the TDD red phase for the fourth consecutive change.

- `pending` — **cycle 3 (defect, from backlog).** Restore poured a backup into whichever project happened to be open instead of the game it came from, so restoring game A's backup while game B was open overwrote B. `restore` no longer takes a destination at all: it reads the source path from the backup's own manifest, refuses when that directory is gone, and no longer requires a project to be open. Cycle 1 had raised the stakes by making every format back up, filling the list with several games. **No research duel this cycle** — four cited defects were already sitting in the backlog, and spending two researchers to invent capabilities while those waited would have been process for its own sake. Grok stopped at the TDD red phase for the third consecutive change.
- `pending` — **cycle 2 (optimization).** Unreal pak detection read the entire file to inspect its last megabyte, on a path that runs merely from opening a folder — twice per open, 8.4 GB each on Last Hope. Now seeks the tail. The win is pinned by a byte-counting reader test, and that test was negative-tested: breaking the helper makes it fail. Researcher: Grok won; Cursor produced nothing for the second consecutive cycle (114 bytes, exit 0).
- `pending` — **cycle 1 (defect).** Direct inject backed up only 4 of 14 formats while writing into the original tree for all of them, and the UI promised a backup that was never made. `mutates_original_tree` was not a design decision but a list that never grew with the plugins; it turned out to be universally true, so it collapsed and the fix removed code from three files. Researcher: Grok won; Cursor produced nothing (connection lost, exit 0).

- `pending` — Review pages instead of loading the project: it fetched up to 50k translated + 50k reviewed rows and concatenated them for a screen that shows one entry at a time. The queue now validates each item after translating, so a batch user finds breakage before injecting rather than one game at a time. Copy stopped naming internals at the user (`register-lang failed`, `locust server`, `plugins=` field dumps).

- `602cd92` — project-wide filter facets, pivot workflow in the app, `open-db` without extraction, Astro stub pointed at `locust apply`, dead chrome removed. Filters had been built from one page of ≤100 rows.
- `b6a8dc2` — patch apply streams progress over a job websocket, apply-only entry without a project, in-app grok-sub login, translation jobs survive closing their modal.
- `4354ce1` — **stopped destroying translations on project open.** `open_project` ran `DELETE FROM strings` every time, against one global database; opening game B destroyed game A.
- `a2f7952` — first workspace-wide `cargo fmt`, isolated so it could not bury the change beside it.

## Rejected

- **`.zip` file association / `locust://` protocol handler** — 2026-08-14. The handler exists so a user can click a link on a patch distribution site. No such site exists; the Astro stub points at third-party rule95. Building the receiver now means OS-level registration, per-platform, untestable against anything real. Revisit when the web decision is made.
- **Aligning the translation default languages** — 2026-08-14. The chain is `lastUsed ?? config ?? "auto"/"es"` and config always exists, so the hardcoded fallback is unreachable. Changing it changes nothing for anyone.
- **Moving ~700 lines of `#[cfg(test)]` fixtures to satisfy `items_after_test_module`** — 2026-08-14. Pure churn and real risk for a layout lint. Allowed with a written reason at `crates/formats/src/unity_serialized.rs`.
- **Batch inject/pack in the queue** — 2026-08-14. See Constraints.

## Failures

None yet.
