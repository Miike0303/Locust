# Cycle 2026-11-01: wave 7 research (RESEARCH-7, duel)

Two independent Codex gpt-6.1-sol high researchers (A on lane-a, B on lane-b, same brief, same commit `3e6f598`). Agreement (found independently by both, highest confidence): archive partial extraction deletes saved rows (A: YU-RIS YPF; B: KiriKiri XP3 + YU-RIS), Ren'Py Replace double-escapes `
`, MV SHOW_TEXT/T_TEXT/GN_TEXT dispatcher deleted, patch destination directory preflight, Ren'Py define bracket tracking counts quoted brackets. A only: RPA pickle overflow panic (parent spot-checked renpy.rs:2411-2432), Unreal heuristic offset ignored on inject, Tyrano fragment context, Unity CSV multi-cell reconstruction, NScripter numbered scripts. B only: Tyrano basename target guessing, Unreal reset not applied to overlay, UnityFS LZ4 decoded size, Dialogic display names, HTML textarea. B explicitly claimed no reachable panic; A proved one. Researcher A ranked higher this round (one concrete panic, more defects).

## Researcher A

I recommend **nine defects and one capability**, all bounded to three writer-days. The defects are proven by source tracing; I did not run reproductions, builds or tests, or change files.

**F** = verified source behavior, including derived output. **H** = proposed work, estimate, risk, value or acceptance criterion. Below, `E` means `crates/formats/src` and `C` means `crates/core/src`.

1. **W7-01 — Preserve saved translations when YPF members become unreadable**

   **H — Defect · M · 2 days · medium risk.** Value: prevent refreshing an archive from deleting translations for a temporarily damaged script.

   **F — Trigger → wrong output:** Start with a YPF containing valid `a.ybn` and `b.ybn`, and save translations. Repack it with `a.ybn`’s payload truncated to `YSTB`, retaining valid `b.ybn`. The archived parser failure is logged and skipped; extraction still returns the remaining rows. No extraction diagnostic is attached. (`E/yuris.rs:1693`, `E/yuris.rs:1708`, `E/yuris.rs:1744`.)

   **F:** The default diagnostic wrapper consequently reports complete extraction: it recognizes warnings from row metadata, not tracing logs. Project refresh selects the deleting merge, removing `a.ybn`’s saved rows and translations. (`C/extraction.rs:122`, `C/project.rs:197`, `C/project.rs:475`, `C/database/strings.rs:1144`.)

   **H — Files:** `crates/formats/src/yuris.rs`; new `crates/formats/tests/yuris_refresh.rs`. Return explicit incomplete diagnostics for failed archives, failed members and unsupported text-bearing layouts.

   **H — Acceptance:** Refresh preserves the damaged member’s saved translations, updates the healthy member, and names the failure. Repairing the archive restores complete-refresh deletion semantics.

   **H — Compatibility/sharing:** Keep existing IDs and sources unchanged. No shared edit files with other proposals.

2. **W7-02 — Honor Unreal heuristic row offsets during injection**

   **H — Defect · M · 2 days · high risk.** Value: prevent translating one binary string from overwriting an earlier, unrelated string.

   **F — Trigger → wrong output:** A small heuristic `.pak` fixture contains these consecutive NUL-terminated UTF-16LE strings:

   ```text
   Prefix Hello world
   Hello world
   ```

   Extraction produces separate rows and records their offsets in their IDs. The second string starts at byte **38**. (`E/unreal.rs:1361`, `E/unreal.rs:335`; small malformed-footer fixtures reach this fallback through `E/unreal.rs:529`.)

   **F:** Translate only the second row to `Hola mundo`. Injection searches source bytes globally and takes the first matching substring, at byte **14** inside the first string. It changes that first string and leaves the intended second string untranslated. The offset-bearing ID is never consulted by this writer. (`E/unreal.rs:1861`, `E/unreal.rs:1899`, `E/unreal.rs:1903`, `E/binary_search.rs:139`.)

   **H — Files:** `crates/formats/src/unreal.rs`, including inline regressions. Validate the recorded offset, complete source span and boundaries; permit legacy location-free rows only when unambiguous.

   **H — Acceptance:** Only bytes belonging to the second slot change. Stale offsets and ambiguous legacy targets refuse without writes.

   **H — Compatibility/sharing:** Preserve existing offset IDs and translations. No shared edit files.

3. **W7-03 — Bound RPA index pickle lengths before arithmetic**

   **H — Defect · M · 2 days · medium risk.** Value: turn malformed archive input into a recoverable extraction error.

   **F — Concrete panic input:** Use this RPA header, followed at offset `0x22` by a zlib stream whose decoded bytes are `8E FF FF FF FF FF FF FF FF`:

   ```text
   RPA-3.0 0000000000000022 00000000
   ```

   RPA extraction decompresses the index and passes it directly to `parse_rpa_pickle`. (`E/renpy.rs:630`, `E/renpy.rs:655`, `E/renpy.rs:670`.)

   **F:** Opcode `0x8E` reads `u64::MAX` as the byte-string length. At `pos = 9`, unchecked `pos + slen` either panics on overflow or wraps to **8**, after which slicing `data[9..8]` panics. (`E/renpy.rs:2411`, `E/renpy.rs:2426`, `E/renpy.rs:2430`.)

   **H — Files:** `crates/formats/src/renpy.rs`, including inline regressions. Audit this RPA-index reader’s length arithmetic and bound memo growth in the same unit; `LONG_BINPUT` currently grows a vector to the supplied index. (`E/renpy.rs:2343`.)

   **H — Acceptance:** The archive returns a filename-bearing error under `catch_unwind`; oversized memo indices also refuse promptly. Valid RPA fixtures remain unchanged.

   **H — Compatibility/sharing:** No ID/source migration. Shares `renpy.rs` with W7-05 and W7-09.

4. **W7-04 — Reject non-file patch destinations before the first asset write**

   **H — Defect · M · 2 days · medium risk.** Value: avoid a predictably half-applied patch when a destination is already a directory.

   **F — Trigger → wrong state:** Prepare a manifest with one correctly hashed replacement `c.txt` and added files `a.txt`, `b.txt`. In the game, `a.txt` is absent and `b.txt` is a directory. Strict verification treats the directory as an absent added path; link checks reject links but accept ordinary directories. (`C/patch/verify.rs:542`, `C/patch/verify.rs:569`, `C/patch/zipsec.rs:151`.)

   **F:** Apply writes replacements first, then added files. It changes `c.txt`, creates `a.txt`, and only then rejects `b.txt` as a non-regular replacement target. The result is an error with a partially patched game and an interrupted journal. (`C/patch/apply.rs:634`, `C/patch/apply.rs:645`, `C/patch/apply.rs:677`, `C/patch/store.rs:203`.)

   **H — Files:** `crates/core/src/patch/verify.rs`, `crates/core/src/patch/apply.rs`, `crates/core/tests/patch_apply.rs`. Preflight destination and parent types before any game mutation, retaining the checks at installation time.

   **H — Acceptance:** The fixture refuses with all game bytes unchanged and no apply journal. Cover terminal directories and an intermediate parent that is a file; retain successful apply/rollback coverage.

   **H — Compatibility/sharing:** No project-row or saved-translation changes. No shared edit files.

5. **W7-05 — Preserve Ren’Py escape spelling during Replace injection**

   **H — Defect · M · 2 days · medium risk.** Value: prevent preserved newline and quote escapes from becoming literal backslashes.

   **F — Trigger → wrong output:** Extract:

   ```renpy
   label start:
       e "Hello\nworld"
   ```

   Here `\n` is two physical characters inside the script literal. Extraction retains that spelling in the source. (`E/renpy.rs:1120`, `E/renpy.rs:1051`.)

   **F:** Save translation `Hola\nmundo` with the same escape spelling. Replace injection escapes every backslash, producing:

   ```renpy
       e "Hola\\nmundo"
   ```

   The write path passes the translation directly through that encoder. The file’s existing canonical-say helper explicitly distinguishes these two escape forms. (`E/renpy.rs:1899`, `E/renpy.rs:1914`, `E/renpy.rs:3205`, `E/renpy.rs:1439`.)

   **H — Files:** `crates/formats/src/renpy.rs`, including inline regressions. Define one compatible literal-encoding contract for extracted escapes and newly typed characters.

   **H — Acceptance:** Loose and RPA Replace preserve `\n`, escaped quotes and literal backslashes correctly; two injections do not add another escaping layer. Include actual newline/quote characters entered manually.

   **H — Compatibility/sharing:** Keep existing IDs and extracted sources unchanged. Shares `renpy.rs` with W7-03/W7-09.

6. **W7-06 — Validate Tyrano fragments in their rebuilt line context**

   **H — Defect · M · 2 days · medium risk.** Value: prevent an otherwise accepted translation from turning dialogue and controls into a comment or command.

   **F — Trigger → wrong output:** Loose `data/scenario/a.ks` contains:

   ```text
   Hello[glink text=Go]
   ```

   Because the line has a display attribute, extraction creates a separate `Text` slot for `Hello` and an attribute slot for `Go`. (`E/tyrano.rs:620`, `E/tyrano.rs:640`.)

   **F:** Translate `Hello` to `;Hola`. `SlotKind::Text` only prohibits `[` beyond the general empty/control-character checks, so injection accepts and writes:

   ```text
   ;Hola[glink text=Go]
   ```

   That entire line is subsequently classified as a comment, yielding neither original slot. (`E/tyrano.rs:823`, `E/tyrano.rs:836`, `E/tyrano.rs:914`, `E/tyrano.rs:395`.)

   **H — Files:** `crates/formats/src/tyrano.rs`, including inline regressions. Validate rebuilt line/scenario structure, including prefix dispatch and preserved controls.

   **H — Acceptance:** Comment, label, speaker and command prefixes introduced through a leading fragment are rejected; ordinary fragment and attribute translations still round-trip.

   **H — Compatibility/sharing:** Keep current locators and sources. No shared edit files.

7. **W7-07 — Preserve dispatchers for the remaining MV display commands**

   **H — Defect · M · 2 days · medium risk.** Value: prevent translating a display command from deleting the command itself.

   **F — Trigger → wrong output:** A code-356 command has parameters `["SHOW_TEXT Hello"]`. Extraction sends the entire string `SHOW_TEXT Hello` as the row source; only `D_TEXT` receives body-only extraction. (`E/rpgmaker_mv.rs:146`, `E/rpgmaker_mv.rs:619`.)

   **F:** Save translation `Hola`. Injection’s non-`D_TEXT` branch replaces the whole first parameter with `Hola`, deleting `SHOW_TEXT`. The same full-command behavior applies to the allow-listed `T_TEXT` and `GN_TEXT`. (`E/rpgmaker_mv.rs:37`, `E/rpgmaker_mv.rs:1123`, `E/rpgmaker_mv.rs:1142`.)

   **H — Files:** `crates/formats/src/rpgmaker_mv.rs`, including inline regressions. Extract only supported display bodies and rebuild with the original dispatcher.

   **H — Acceptance:** Maps, common events and troops retain dispatcher spelling and unrelated parameters. Legacy full-command sources cannot be silently treated as body-only translations.

   **H — Compatibility/sharing:** Keep command IDs, but body-only extraction changes these sources. Reopen should retain old translations as stale pending work requiring review. No shared edit files.

8. **W7-08 — Reconstruct every translated Unity CSV cell in the same row**

   **H — Defect · M · 1–2 days · medium risk.** Value: make validation and translation budgeting agree with injection for tables containing multiple text columns.

   **F — Trigger → wrong output:** A TextAsset contains:

   ```csv
   NAME,DESC
   Sword,Sharp blade
   Shield,Strong guard
   ```

   Unity’s CSV extractor emits both nonnumeric columns. (`E/unity.rs:1939`, `E/unity.rs:1957`.)

   **F:** Supply patches translating both cells in the first data row. Shared reconstruction applies the first patch, marks its physical line used, then rejects the second with `error` because that same line is already used. It returns a partially reconstructed row. Validation converts that error into `UnsupportedTextAssetReconstruction`. (`C/textasset_group.rs:575`, `C/textasset_group.rs:643`, `C/textasset_group.rs:787`.) The Unity injector itself permits separate cell edits on that row. (`E/unity.rs:2135`, `E/unity.rs:2157`, `E/unity.rs:2195`.)

   **H — Files:** `crates/core/src/textasset_group.rs`, including inline regressions. Track consumed CSV cells by row-and-column while retaining line ownership for locale-line groups.

   **H — Acceptance:** Both cell patches appear in reconstruction; validation/budgeting match the injected blob. Duplicate patches to one cell still refuse.

   **H — Compatibility/sharing:** No ID, source or translation migration. No shared edit files.

9. **W7-09 — Ignore quoted delimiters when tracking Ren’Py define blocks**

   **H — Defect · S · 1 day · low–medium risk.** Value: recover dialogue silently omitted after a definition containing ordinary unmatched punctuation.

   **F — Trigger → wrong output:**

   ```renpy
   define gui.about = _("Welcome (")
   label start:
       e "Hello"
   ```

   Define-block tracking counts delimiters inside quoted strings. The first line has two opening parentheses and one closing parenthesis, so it leaves depth **1**. Every following line is skipped while that depth remains positive; `Hello` produces no row. (`E/renpy.rs:883`, `E/renpy.rs:894`, `E/renpy.rs:901`.)

   **H — Files:** `crates/formats/src/renpy.rs`, including inline regressions. Count structural delimiters outside quoted literals and comments, respecting escapes.

   **H — Acceptance:** The fixture extracts `Welcome (` and `Hello`; quoted brackets and comment punctuation do not affect state, while genuine multiline definitions remain excluded.

   **H — Compatibility/sharing:** Preserve existing IDs and sources; previously omitted dialogue gains rows. Shares `renpy.rs` with W7-03/W7-05.

10. **W7-10 — Support numbered loose NScripter scripts**

    **H — Capability · M · 3 days · medium risk.** Value: translate later chapters stored outside the initial script.

    **F — Current gap:** Given `0.txt` containing `` `Opening\ `` and `1.txt` containing `` `Later chapter\ ``, extraction selects only `0.txt`. Selection returns the first supported container, and that list contains no numbered continuation files. Injection also recognizes only the initial plain filenames and encrypted containers. (`E/nscripter.rs:49`, `E/nscripter.rs:65`, `E/nscripter.rs:179`, `E/nscripter.rs:536`, `E/nscripter.rs:665`.) Numbered-file concatenation is explicitly outside the current implementation. (`E/nscripter.rs:26`.)

    **H — Files:** `crates/formats/src/nscripter.rs`, including inline regressions. Bound this unit to the plain `0.txt` plus `1.txt`–`99.txt` layout; preserve individual file bytes and locators.

    **H — Acceptance:** Both fixture lines extract, translate and re-extract in order. Injection changes only selected files and preserves XOR-container selection behavior.

    **H — Compatibility/sharing:** Preserve existing `0.txt#N` IDs and saved translations; continuation files receive new filename-qualified IDs. No shared edit files.

The ranked edit boundaries follow. Inline regression tests reside in the listed production file unless a separate test file is named.

| Rank | ID | Title | Category | Size | Exact files |
|---:|---|---|---|---|---|
| 1 | W7-01 | Preserve rows after unreadable YPF members | Defect | M | `E/yuris.rs`; new `crates/formats/tests/yuris_refresh.rs` |
| 2 | W7-02 | Honor Unreal heuristic offsets | Defect | M | `E/unreal.rs` |
| 3 | W7-03 | Bound RPA index pickle lengths | Defect | M | `E/renpy.rs` |
| 4 | W7-04 | Preflight non-file patch destinations | Defect | M | `C/patch/verify.rs`, `C/patch/apply.rs`, `crates/core/tests/patch_apply.rs` |
| 5 | W7-05 | Preserve Ren’Py escape spelling | Defect | M | `E/renpy.rs` |
| 6 | W7-06 | Validate rebuilt Tyrano fragment context | Defect | M | `E/tyrano.rs` |
| 7 | W7-07 | Preserve MV display-command dispatchers | Defect | M | `E/rpgmaker_mv.rs` |
| 8 | W7-08 | Reconstruct multiple Unity CSV cells | Defect | M | `C/textasset_group.rs` |
| 9 | W7-09 | Track Ren’Py define delimiters lexically | Defect | S | `E/renpy.rs` |
| 10 | W7-10 | Numbered loose NScripter scripts | Capability | M | `E/nscripter.rs` |

**H — Suggested disjoint-file lanes:** Run each lane sequentially. All three `renpy.rs` units stay with one writer; no file crosses lanes.

| Pair | Lane A | Lane B |
|---:|---|---|
| 1 | W7-03 — RPA panic | W7-01 — YPF refresh |
| 2 | W7-05 — Ren’Py escapes | W7-02 — Unreal offsets |
| 3 | W7-09 — Ren’Py delimiters | W7-04 — Patch destinations |
| 4 | W7-06 — Tyrano fragments | W7-08 — Unity CSV reconstruction |
| 5 | W7-07 — MV dispatchers | W7-10 — NScripter continuations |


## Researcher B

I recommend **eight defects and two capabilities**, all bounded to ≤3 writer-days. Findings are source-derived; no files were changed, builds/tests run, or game folders inspected.

**F** = verified source behavior, including derived outputs. **H** = proposed work, estimate, risk, acceptance criteria, or compatibility assessment. No reachable panic is claimed: the unchecked operations traced did not yield a complete, defensible triggering input.

Paths: **E** = `crates/formats/src`; **T** = `crates/formats/tests`; **C** = `crates/core/src`; **CT** = `crates/core/tests`. Rust edit boundaries include inline tests.

1. **W7-01 — Preserve saved rows after partial XP3/YPF extraction**

   **H · defect · M, 3 days · medium risk.** Value: prevent refreshing a partly damaged archive from deleting saved translations.

   **F — Trigger → wrong output:** Start with two readable scripts and saved translations. Make one XP3 member undecodable while retaining the other—for example, replace its script payload with an unsupported `FE FE 03` payload. KiriKiri catches member read/decode failures, logs them, and returns the remaining rows. YU-RIS similarly skips a malformed YSTB member inside an otherwise readable YPF. Neither path returns extraction diagnostics. ([kirikiri.rs:1714](C:/Projects/Locust-lane-b/crates/formats/src/kirikiri.rs:1714), [kirikiri.rs:1740](C:/Projects/Locust-lane-b/crates/formats/src/kirikiri.rs:1740), [kirikiri.rs:1783](C:/Projects/Locust-lane-b/crates/formats/src/kirikiri.rs:1783), [yuris.rs:1708](C:/Projects/Locust-lane-b/crates/formats/src/yuris.rs:1708), [yuris.rs:1744](C:/Projects/Locust-lane-b/crates/formats/src/yuris.rs:1744))

   **F:** The default diagnostic wrapper derives warnings from row metadata; that collector recognizes `iostore_unread_containers`. Consequently these extractions appear complete, refresh selects the deleting merge, and absent saved rows are deleted. ([extraction.rs:122](C:/Projects/Locust-lane-b/crates/core/src/extraction.rs:122), [project.rs:197](C:/Projects/Locust-lane-b/crates/core/src/project.rs:197), [project.rs:475](C:/Projects/Locust-lane-b/crates/core/src/project.rs:475), [strings.rs:1144](C:/Projects/Locust-lane-b/crates/core/src/database/strings.rs:1144))

   **H — Work/files:** Override diagnostics for both archive paths. Edit `E/kirikiri.rs`, `E/yuris.rs`; add `T/archive_partial_refresh.rs`.

   **H — Acceptance:** Corrupt one member, refresh, and retain its saved rows/translations while updating the readable member; report archive/member names. Repairing the member restores complete-refresh deletion semantics.

   **H — Compatibility/sharing:** Existing IDs unchanged; saved translations gain protection. No shared files with other proposals.

2. **W7-02 — Preserve Ren’Py literal escapes during replacement**

   **H · defect · M, 2 days · medium risk.** Value: keep line breaks and escaped quotation marks working after translation.

   **F — Trigger → wrong output:** Source script `e "Hello\nWorld"` extracts the literal backslash-plus-`n`, because quoted extraction returns a raw slice. Translate it to `Hola\nMundo`, preserving that escape. Replacement escapes every backslash again, producing script `e "Hola\\nMundo"`. Re-extraction therefore contains two backslashes instead of the original escape spelling. ([renpy.rs:1125](C:/Projects/Locust-lane-b/crates/formats/src/renpy.rs:1125), [renpy.rs:1137](C:/Projects/Locust-lane-b/crates/formats/src/renpy.rs:1137), [renpy.rs:1899](C:/Projects/Locust-lane-b/crates/formats/src/renpy.rs:1899), [renpy.rs:1920](C:/Projects/Locust-lane-b/crates/formats/src/renpy.rs:1920))

   **F:** Both loose preparation and RPA replacement call this replacement helper. ([renpy.rs:821](C:/Projects/Locust-lane-b/crates/formats/src/renpy.rs:821), [renpy.rs:506](C:/Projects/Locust-lane-b/crates/formats/src/renpy.rs:506))

   **H — Work/files:** Define and enforce one escape representation at the extraction/replacement boundary. Edit `E/renpy.rs`, `T/renpy_ui_injection.rs`.

   **H — Acceptance:** Loose and RPA fixtures preserve `\n`, `\"`, and literal `\\` through changed translations and re-extraction; new unescaped quotes remain safely encoded.

   **H — Compatibility/sharing:** Keep row IDs and stored-source representation unchanged; avoid a bulk database migration. Shares `E/renpy.rs` with W7-07.

3. **W7-03 — Preserve MV display-command dispatchers beyond D_TEXT**

   **H · defect · M, 2 days · medium risk.** Value: prevent translated plugin text from disabling its display command.

   **F — Trigger → wrong output:** Command `356` with `parameters:["SHOW_TEXT Hello traveler"]` extracts **`SHOW_TEXT Hello traveler`**, including the dispatcher. Saving translation `Hola viajero` writes **`parameters:["Hola viajero"]`**. Only D_TEXT receives dispatcher-preserving reconstruction; SHOW_TEXT, T_TEXT and GN_TEXT use whole-command replacement. ([rpgmaker_mv.rs:37](C:/Projects/Locust-lane-b/crates/formats/src/rpgmaker_mv.rs:37), [rpgmaker_mv.rs:146](C:/Projects/Locust-lane-b/crates/formats/src/rpgmaker_mv.rs:146), [rpgmaker_mv.rs:1106](C:/Projects/Locust-lane-b/crates/formats/src/rpgmaker_mv.rs:1106), [rpgmaker_mv.rs:1131](C:/Projects/Locust-lane-b/crates/formats/src/rpgmaker_mv.rs:1131))

   **H — Work/files:** Add explicit body locators and reconstruction for the three remaining allowlisted dispatchers. Edit `E/rpgmaker_mv.rs`.

   **H — Acceptance:** Map, common-event and troop fixtures translate only the display body; dispatcher spelling and separators survive. Unknown commands remain excluded.

   **H — Compatibility/sharing:** Preserve IDs, but source values change from whole commands to bodies. Require explicit refresh/review of affected legacy translations; do not silently reuse their old whole-command values. No within-wave file overlap.

4. **W7-04 — Stop Tyrano loose injection from guessing another filename**

   **H · defect · S, 1 day · medium risk.** Value: prevent a missing scenario from redirecting a translation into a different script.

   **F — Trigger → wrong output:** Extract `data/scenario/chapter/a.ks` containing `Hello traveler`. Remove that file, leaving root-level `a.ks` with the same first line. Injection falls back from the missing stored path to `search_root.join(file_name)`. The matching line locator/source/guard then permits writing `Hola viajero` into the root-level file. ([tyrano.rs:1328](C:/Projects/Locust-lane-b/crates/formats/src/tyrano.rs:1328), [tyrano.rs:1337](C:/Projects/Locust-lane-b/crates/formats/src/tyrano.rs:1337), [tyrano.rs:697](C:/Projects/Locust-lane-b/crates/formats/src/tyrano.rs:697), [tyrano.rs:897](C:/Projects/Locust-lane-b/crates/formats/src/tyrano.rs:897), [tyrano.rs:1361](C:/Projects/Locust-lane-b/crates/formats/src/tyrano.rs:1361))

   **F:** Core deliberately permits missing ordinary targets to reach the plugin, so missing-path preflight does not eliminate this case. ([extraction.rs:1400](C:/Projects/Locust-lane-b/crates/core/src/extraction.rs:1400))

   **H — Work/files:** Resolve exact game-relative paths; remove basename guessing for loose scripts. Edit `E/tyrano.rs`.

   **H — Acceptance:** The fixture reports `target_missing`, with root `a.ks` byte-identical. Exact-path injection and translated-copy path remapping still work.

   **H — Compatibility/sharing:** IDs/translations unchanged; ambiguous legacy paths require re-extraction. No shared files.

5. **W7-05 — Make explicit Unreal source resets replace existing overlays**

   **H · defect · M, 2 days · medium risk.** Value: let a translator undo a translation without restoring the entire game.

   **F — Trigger → wrong output:** Extract an English LocRes inside a PAK, inject `Hello traveler → Hola viajero`, then set that saved translation back to **`Hello traveler`** and inject again. `checked_locres_translations` discards translation-equals-source rows. An empty map skips the resource; an empty output list returns without rewriting the previously generated overlay. The overlay therefore retains **`Hola viajero`**. ([unreal.rs:1257](C:/Projects/Locust-lane-b/crates/formats/src/unreal.rs:1257), [unreal.rs:792](C:/Projects/Locust-lane-b/crates/formats/src/unreal.rs:792), [unreal.rs:851](C:/Projects/Locust-lane-b/crates/formats/src/unreal.rs:851))

   **H — Work/files:** Treat an explicit physical-source translation as a reset when reconciling an existing owned overlay, including the all-reset case. Edit `E/unreal.rs`, `T/unreal_overlay_ownership.rs`.

   **H — Acceptance:** Translate→reset→re-extract returns the source value; resetting one resource preserves unrelated translated resources. Cover both a single reset and an entirely reset overlay.

   **H — Compatibility/sharing:** IDs and saved translation representation unchanged. No shared files.

6. **W7-06 — Reject directory/file destination conflicts before patch mutation**

   **H · defect · S, 1 day · low–medium risk.** Value: avoid leaving a game partly patched when an incompatible destination already exists.

   **F — Trigger → wrong output:** Game contains original `a.txt` and a **directory** named `b.txt`. A strict patch replaces `a.txt` and adds a file `b.txt`. Strict verification treats every non-file destination as absent, accepting the added path. Apply writes replacements before additions, so it replaces `a.txt`, then `replace_file` rejects directory `b.txt`. Result: apply errors with `a.txt` already patched and an interrupted journal. ([verify.rs:542](C:/Projects/Locust-lane-b/crates/core/src/patch/verify.rs:542), [verify.rs:569](C:/Projects/Locust-lane-b/crates/core/src/patch/verify.rs:569), [apply.rs:634](C:/Projects/Locust-lane-b/crates/core/src/patch/apply.rs:634), [apply.rs:645](C:/Projects/Locust-lane-b/crates/core/src/patch/apply.rs:645), [store.rs:203](C:/Projects/Locust-lane-b/crates/core/src/patch/store.rs:203))

   **H — Work/files:** Validate destination types before mutation and distinguish “absent” from “exists but incompatible.” Edit `C/patch/verify.rs`, `CT/patch_apply.rs`.

   **H — Acceptance:** Verify, apply dry-run and apply reject the fixture before changing `a.txt` or publishing recovery state. Include a regular-file parent blocking a nested added destination.

   **H — Compatibility/sharing:** No row-ID or translation changes. No shared files.

7. **W7-07 — Ignore quoted brackets in Ren’Py define-block tracking**

   **H · defect · S, 1–2 days · low–medium risk.** Value: recover dialogue hidden by punctuation in an ordinary definition.

   **F — Trigger → wrong output:**
   ```renpy
   define marker = "["
   label start:
       "Hello traveler."
   ```
   The tracker counts brackets across the entire definition, including quoted strings. It sets depth to one and skips subsequent lines until a closing bracket appears; this fixture loses the dialogue row entirely. Comments containing unmatched brackets can affect the same counter. ([renpy.rs:883](C:/Projects/Locust-lane-b/crates/formats/src/renpy.rs:883), [renpy.rs:894](C:/Projects/Locust-lane-b/crates/formats/src/renpy.rs:894), [renpy.rs:901](C:/Projects/Locust-lane-b/crates/formats/src/renpy.rs:901))

   **H — Work/files:** Replace raw bracket counting with bounded lexical tracking of quoted literals, escapes and comments. Edit `E/renpy.rs`.

   **H — Acceptance:** The fixture extracts `Hello traveler.`; actual multiline dictionaries/lists remain excluded. Cover escaped quotes and unmatched brackets inside comments.

   **H — Compatibility/sharing:** Existing emitted IDs remain unchanged; previously omitted dialogue gains rows. Shares `E/renpy.rs` with W7-02.

8. **W7-08 — Enforce exact UnityFS LZ4 decoded sizes**

   **H · defect · S, 1 day · low risk.** Value: reject damaged bundle geometry before it hides or misaddresses embedded assets.

   **F — Trigger → wrong output:** A storage block declares eight decoded bytes but contains a valid LZ4 encoding of four bytes. The decoder resizes to eight, accepts the decoder’s returned count, truncates to that count, and returns success without checking equality. Parsing subsequently returns the archive without checking final decoded length against the declared total. ([unity_fs.rs:1003](C:/Projects/Locust-lane-b/crates/formats/src/unity_fs.rs:1003), [unity_fs.rs:598](C:/Projects/Locust-lane-b/crates/formats/src/unity_fs.rs:598), [unity_fs.rs:628](C:/Projects/Locust-lane-b/crates/formats/src/unity_fs.rs:628), [unity_fs.rs:652](C:/Projects/Locust-lane-b/crates/formats/src/unity_fs.rs:652))

   **F:** With another four-byte block and a node at declared offset eight, that node is now past the actual eight-byte decoded buffer and fails when read. ([unity_fs.rs:174](C:/Projects/Locust-lane-b/crates/formats/src/unity_fs.rs:174))

   **H — Work/files:** Require `written == uncompressed` for LZ4/LZ4HC and verify the aggregate decoded length. Edit `E/unity_fs.rs`.

   **H — Acceptance:** Short-decoding fixtures fail during parsing with block-specific diagnostics; exact-size multiblock bundles retain identical node bytes. This is not a claimed panic.

   **H — Compatibility/sharing:** Valid-file IDs/translations unchanged; malformed bundles fail earlier. No shared files.

9. **W7-09 — Translate Dialogic character display names**

   **H · capability · M, 3 days · high risk.** Value: translate speaker nameplates alongside timeline dialogue.

   **F — Current gap:** `.dch` parsing reads `display_name`, but explicitly reports character names as metadata only. The adapter authenticates character resources, discards the parsed values, and emits rows only from `.dtl` spans. A character dictionary `{"display_name":"The Wanderer"}` therefore produces no name row. ([dialogic.rs:1027](C:/Projects/Locust-lane-b/crates/formats/src/dialogic.rs:1027), [dialogic.rs:1055](C:/Projects/Locust-lane-b/crates/formats/src/dialogic.rs:1055), [godot_dialogic.rs:214](C:/Projects/Locust-lane-b/crates/formats/src/godot_dialogic.rs:214), [godot_dialogic.rs:404](C:/Projects/Locust-lane-b/crates/formats/src/godot_dialogic.rs:404))

   **H — Work/files:** Support only literal `display_name` initially, preserving filename-based speaker identifiers. Integrate verified name changes with the existing `.dch` revision-integrity check. ([godot_dialogic.rs:552](C:/Projects/Locust-lane-b/crates/formats/src/godot_dialogic.rs:552)) Edit `E/dialogic.rs`, `E/godot_dialogic.rs`, `T/dialogic_codec.rs`, `T/godot_dialogic_lifecycle.rs`.

   **H — Acceptance:** Extract/inject/re-extract the name, then revise it a second time; preserve timeline speaker identifiers, opaque dictionary fields and unrelated PCK members.

   **H — Compatibility/sharing:** Add name rows without changing existing timeline IDs. No shared files.

10. **W7-10 — Support literal HTML textarea content**

    **H · capability · M, 2 days · medium risk.** Value: translate existing text shown inside game text-entry controls.

    **F — Current gap:** `<textarea>Hello traveler</textarea>` produces no content row. The scanner handles `textarea` with script/style-like bodies and jumps directly beyond its closing tag. That branch also bypasses its editable-attribute scan. ([html_slots.rs:93](C:/Projects/Locust-lane-b/crates/formats/src/html_slots.rs:93), [html_slots.rs:112](C:/Projects/Locust-lane-b/crates/formats/src/html_slots.rs:112), [html_slots.rs:117](C:/Projects/Locust-lane-b/crates/formats/src/html_slots.rs:117))

    **H — Work/files:** Add a dedicated textarea-content slot with entity decoding and safe text encoding. Edit `E/html_slots.rs`, `E/html_game.rs`.

    **H — Acceptance:** Extract and translate textarea text containing `&amp;` and literal `<b>` spelling; preserve attributes and surrounding HTML. Translation containing `</textarea>` remains encoded content.

    **H — Compatibility/sharing:** Existing slots keep their byte-offset IDs; new textarea slots add rows. No shared files.

**H — Ranked dispatch table**

| Rank | ID | Title | Category | Size | Exact files |
|---:|---|---|---|---|---|
| 1 | W7-01 | Preserve rows after partial XP3/YPF extraction | Defect | M | `E/kirikiri.rs`, `E/yuris.rs`, new `T/archive_partial_refresh.rs` |
| 2 | W7-02 | Preserve Ren’Py literal escapes | Defect | M | `E/renpy.rs`, `T/renpy_ui_injection.rs` |
| 3 | W7-03 | Preserve MV display dispatchers beyond D_TEXT | Defect | M | `E/rpgmaker_mv.rs` |
| 4 | W7-04 | Remove Tyrano basename target guessing | Defect | S | `E/tyrano.rs` |
| 5 | W7-05 | Apply Unreal source resets to overlays | Defect | M | `E/unreal.rs`, `T/unreal_overlay_ownership.rs` |
| 6 | W7-06 | Preflight patch destination types | Defect | S | `C/patch/verify.rs`, `CT/patch_apply.rs` |
| 7 | W7-07 | Ignore quoted brackets in Ren’Py definitions | Defect | S | `E/renpy.rs` |
| 8 | W7-09 | Translate Dialogic display names | Capability | M | `E/dialogic.rs`, `E/godot_dialogic.rs`, `T/dialogic_codec.rs`, `T/godot_dialogic_lifecycle.rs` |
| 9 | W7-10 | Translate HTML textarea content | Capability | M | `E/html_slots.rs`, `E/html_game.rs` |
| 10 | W7-08 | Enforce UnityFS LZ4 decoded sizes | Defect | S | `E/unity_fs.rs` |

**H — Suggested pairing into two disjoint-file lanes**

| Pair | Lane A — sequential | Lane B — sequential |
|---:|---|---|
| 1 | W7-01 | W7-02 |
| 2 | W7-03 | W7-04 |
| 3 | W7-05 | W7-06 |
| 4 | W7-08 | W7-07 |
| 5 | W7-10 | W7-09 |

W7-02 and W7-07 stay in Lane B because they share `renpy.rs`; the complete file sets of Lane A and Lane B are disjoint.
