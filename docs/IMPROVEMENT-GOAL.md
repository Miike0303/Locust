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

- Packing a patch slurps whole multi-GB files that apply already streams: `pack_injection_recording` does `std::fs::read(&src)` per recorded file, then hashes and writes it (`crates/core/src/patch/pack.rs:307-332`), and `record_injection` slurps again to SHA-256 (`crates/core/src/database.rs:852-853`). Apply is explicit about never buffering (`crates/core/src/patch/stream.rs:16-17`). Peak RAM ≈ file size, or 2× with `--pristine`. Reuse the existing chunked path. Found by Grok, cycle 2.
- Unreal extract still reads the whole pak (`crates/formats/src/unreal.rs:637-644`) and scans the full buffer twice — `find_locres_offsets` byte-steps it (`unreal_locres.rs:569-584`) and `find_utf16le_strings` walks it again (`unreal.rs:455-507`). Larger and riskier than the detection fix; deliberately deferred from cycle 2.
- `find_pak_files` runs twice per open — once from `detect` (`crates/formats/src/unreal.rs:105`), again from `extract` (`:605`). Cheap now that detection only reads the tail, but still duplicated work.
- `save_entries` (`crates/core/src/database.rs:399-409`) and `merge_entries` (`:1182`, `:1205`, `:1239`) call `tx.execute` inside loops, so the same SQL is parsed and planned once per row — up to 33,767 times on a real extract. `prepare_cached` would do it once. Found by Claude, cycle 2; no automatable check identified, which is why it lost to a measurable win.
- **Opening a project while a translation runs writes into the wrong database.** `Database::reopen` swaps the connection inside the shared object (`crates/core/src/database.rs:380-388`); the job holds that same `Arc` (`crates/server/src/lib.rs:796`); `project_open` never consults `state.active_jobs` (`crates/server/src/lib.rs:399-431`). Translations for game A land in game B. Reachable since jobs began surviving modal close. Found by Claude in cycle 1; lost the duel only because the winner destroyed files on disk rather than rows in a database.
- Inject reports success when nothing usable was written: the toast fires on any HTTP 200, ignoring `languages_failed`, `strings_written == 0`, and `NothingRecorded` (`apps/desktop/src/components/InjectModal.tsx:275-280`, `crates/core/src/extraction.rs:143-144`). Found by Grok, cycle 1.
- Patch apply: closing the socket without a terminal frame leaves the UI idle while the job keeps writing, and a second apply for the same folder is accepted. Found by Grok, cycle 1.
- No web presence of any kind: no landing, no docs site, no deploy. `crates/server` cannot serve static files (`tower-http` is compiled with `cors, trace` only). Deferred by the user, not rejected.

## In flight

Nothing.

## Done

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
