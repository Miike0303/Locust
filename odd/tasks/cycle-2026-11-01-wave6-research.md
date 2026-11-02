# Cycle 2026-11-01: wave 6 research (RESEARCH-6)

Codex gpt-6.1-sol high, read-only, single researcher, on `d8294d5`. Parent spot-checked rpgmaker_vxa.rs:1078-1087 (any non-empty first 101 parameter is taken as text, so a VX Ace face name becomes dialogue) and sugarcube.rs:809-848 (substring checks such as `border`). Links point at the lane-b worktree; paths are repo-relative after the lane prefix.

**Recommended wave: seven defects and three capabilities, each bounded to ≤3 writer-days.** The defects below are proven by source tracing, not runtime reproduction. No files were changed; no builds or tests were run.

**F** = verified source behavior. **H** = proposed implementation, estimate, risk, value, or future acceptance test. Path abbreviations: `E = crates/formats/src`, `U = apps/desktop/src`, `C = crates/core/src`. Proposed tests are included in each edit boundary.

1. **W6-01 — Preserve VX/Ace face headers during dialogue extraction and injection**

   **H · defect · M, 2–3 days · high risk.** Value: prevent a normal dialogue translation from replacing a portrait filename and deleting its dialogue.

   **F — Input → wrong output:** In `Map001.rvdata2`, use command `101` with parameters `["Actor1",0,0,2]`, followed by `401 ["Hello"]`. Extraction treats any nonempty first string in `101` as dialogue, producing **`Actor1\nHello`**. Common events use the same condition. ([E/rpgmaker_vxa.rs:1078](C:/Projects/Locust-lane-b/crates/formats/src/rpgmaker_vxa.rs:1078), [1172](C:/Projects/Locust-lane-b/crates/formats/src/rpgmaker_vxa.rs:1172))

   **F:** Injecting the short translation `Hola` writes it into the `101` first parameter and splices away the following `401`: the resulting header names face **`Hola`**, with no dialogue command left. ([E/rpgmaker_vxa.rs:1423](C:/Projects/Locust-lane-b/crates/formats/src/rpgmaker_vxa.rs:1423), [1469](C:/Projects/Locust-lane-b/crates/formats/src/rpgmaker_vxa.rs:1469))

   **H — Work/files:** Distinguish XP text-bearing `101` from VX/Ace face headers using the file/profile and parameter shape. Edit `E/rpgmaker_vxa.rs`, including inline regressions.

   **H — Acceptance:** The fixture extracts only `Hello`; injection retains all four header parameters and writes `Hola` into a `401`. Cover maps, common events, empty/nonempty faces, and XP’s text-bearing `101`.

   **H — Shares files:** W6-09; serialize those two units.

2. **W6-02 — Preserve VNTextPatch translations when a JSON file becomes unreadable**

   **H · defect · M, 2 days · medium risk.** Value: prevent project refresh from deleting saved translations because one script JSON is temporarily malformed.

   **F — Input → wrong output:** Start with valid `a.json` and `b.json`, each containing a message, and save translations. Then truncate `a.json` while leaving `b.json` valid. `looks_like_vntp(a)` returns false on parse failure, and extraction silently skips it. ([E/vntextpatch.rs:122](C:/Projects/Locust-lane-b/crates/formats/src/vntextpatch.rs:122), [187](C:/Projects/Locust-lane-b/crates/formats/src/vntextpatch.rs:187))

   **F:** With no extraction warning, the default diagnostic wrapper reports completeness. Project refresh consequently chooses the deleting merge; absent IDs, including their translations, are removed. ([C/extraction.rs:122](C:/Projects/Locust-lane-b/crates/core/src/extraction.rs:122), [C/project.rs:471](C:/Projects/Locust-lane-b/crates/core/src/project.rs:471), [C/database/strings.rs:1144](C:/Projects/Locust-lane-b/crates/core/src/database/strings.rs:1144))

   **H — Work/files:** Add explicit incomplete-extraction diagnostics for unreadable candidate files. Edit `E/vntextpatch.rs`; add `crates/formats/tests/vntextpatch_refresh.rs`.

   **H — Acceptance:** The refresh reports the failing filename, preserves `a.json`’s saved rows/translations, and still updates `b.json`. After repair, a complete refresh resumes normal removal semantics.

   **H — Shares files:** None.

3. **W6-03 — Stop translating QSP location references as prose**

   **H · defect · M, 3 days · high risk.** Value: prevent translation from changing executable references while leaving their referenced identifiers unchanged.

   **F — Input → wrong output:** Put `gt 'Bedroom'` in a location’s code and include a location named `Bedroom`. The extractor accepts every alphabetic quoted literal meeting its generic filter, without inspecting the surrounding statement, so it emits `Bedroom` as `code_string`. ([E/qsp.rs:424](C:/Projects/Locust-lane-b/crates/formats/src/qsp.rs:424), [443](C:/Projects/Locust-lane-b/crates/formats/src/qsp.rs:443), [469](C:/Projects/Locust-lane-b/crates/formats/src/qsp.rs:469))

   **F:** Translating that row to `Dormitorio` rebuilds the code as **`gt 'Dormitorio'`**. The translation application changes descriptions, quoted code literals and action names, leaving the location name `Bedroom` unchanged. ([E/qsp.rs:496](C:/Projects/Locust-lane-b/crates/formats/src/qsp.rs:496), [619](C:/Projects/Locust-lane-b/crates/formats/src/qsp.rs:619))

   **H — Work/files:** Add bounded statement-aware literal roles: protect destination arguments for `GT/GOTO/GS/GOSUB`, and retain explicitly supported display literals. Quarantine ambiguous executable strings. Edit `E/qsp.rs`, including inline tests.

   **H — Acceptance:** Mixed fixtures preserve destination names and executable arguments byte-for-byte while translating supported print text and action captions. Cover case variants and doubled quotes.

   **H — Shares files:** None.

4. **W6-04 — Stop rejecting ordinary SugarCube dialogue as CSS**

   **H · defect · S, 1 day · low–medium risk.** Value: recover dialogue that disappears from extraction because it contains ordinary English words.

   **F — Input → wrong output:** A normal passage containing **`Remember: stay together.`** yields no row: the CSS heuristic rejects text containing `:` plus the substring `em`. **`You cross the border.`** is independently rejected because it contains `border`. ([E/sugarcube.rs:809](C:/Projects/Locust-lane-b/crates/formats/src/sugarcube.rs:809), [843](C:/Projects/Locust-lane-b/crates/formats/src/sugarcube.rs:843))

   **F:** These filters run on already identified visible fragments; the scanner separately excludes HTML script/style bodies and script/widget macro bodies. ([E/sugarcube.rs:505](C:/Projects/Locust-lane-b/crates/formats/src/sugarcube.rs:505), [543](C:/Projects/Locust-lane-b/crates/formats/src/sugarcube.rs:543), [583](C:/Projects/Locust-lane-b/crates/formats/src/sugarcube.rs:583))

   **H — Work/files:** Replace substring-based prose rejection with narrowly justified structural checks. Edit `E/sugarcube.rs`, including inline regressions.

   **H — Acceptance:** Both sentences extract and inject successfully; script, stylesheet and widget-code fixtures remain excluded, and markup remains unchanged.

   **H — Shares files:** None.

5. **W6-05 — Extract MV/MZ nicknames and event-driven profile changes**

   **H · capability · M, 2 days · medium risk.** Value: let translators finish actor identity and biography text that changes during play.

   **F — Input → current gap:** `Actors.json` containing `[null,{"name":"Hero","nickname":"The Wanderer"}]` emits the name but no nickname: extraction iterates an explicit field list that omits `nickname`. ([E/rpgmaker_mv.rs:16](C:/Projects/Locust-lane-b/crates/formats/src/rpgmaker_mv.rs:16), [474](C:/Projects/Locust-lane-b/crates/formats/src/rpgmaker_mv.rs:474))

   **F:** Event commands `324` and `325` also fall through the extractor’s default branch; its actor-text branch handles only `320`. ([E/rpgmaker_mv.rs:602](C:/Projects/Locust-lane-b/crates/formats/src/rpgmaker_mv.rs:602), [639](C:/Projects/Locust-lane-b/crates/formats/src/rpgmaker_mv.rs:639))

   **H — Work/files:** Add nickname rows and paired extraction/injection for literal nickname/profile changes in maps, common events and troops. Edit `E/rpgmaker_mv.rs`, including inline tests.

   **H — Acceptance:** The actor fixture and `324 [1,"The Wanderer"]` / `325 [1,"A former knight."]` produce stable rows; injection changes only their text fields and preserves actor IDs and other command parameters.

   **H — Shares files:** None.

6. **W6-06 — Preserve complete Wolf text prefixes and halfwidth-katakana strings**

   **H · defect · M, 2 days · medium risk.** Value: give the translator complete text instead of silently omitting prefixes or entire short Japanese messages.

   **F — Input → wrong output:** For a NUL-delimited byte sequence **`123 apples\0`**, scanning starts at `a`, yielding source **`apples` at offset 4**. A string consisting entirely of Shift-JIS halfwidth katakana yields no row: those bytes are accepted inside a region but never accepted as its start. ([E/wolf_rpg.rs:75](C:/Projects/Locust-lane-b/crates/formats/src/wolf_rpg.rs:75), [92](C:/Projects/Locust-lane-b/crates/formats/src/wolf_rpg.rs:92), [126](C:/Projects/Locust-lane-b/crates/formats/src/wolf_rpg.rs:126))

   **F:** The shortened offset/source becomes the stored injection locator and byte budget. ([E/wolf_rpg.rs:44](C:/Projects/Locust-lane-b/crates/formats/src/wolf_rpg.rs:44), [53](C:/Projects/Locust-lane-b/crates/formats/src/wolf_rpg.rs:53))

   **H — Work/files:** Recognize complete printable runs at demonstrated boundaries, including halfwidth starts, while retaining meaningful-content filtering. Edit `E/wolf_rpg.rs`, including inline tests.

   **H — Acceptance:** Extract `123 apples` at offset 0 and `ｺﾝﾆﾁﾊ` in full; inject within their correct slots. Numeric-only runs and adjacent binary sentinels remain excluded/intact.

   **H — Shares files:** None.

7. **W6-07 — Confine unpacked ASAR scenario reads**

   **H · defect / robustness · M, 2 days · medium risk.** Value: keep opening an archive from importing text through paths outside its unpacked resource directory.

   **F — Input → wrong output:** An ASAR header can contain an unpacked member named **`data/scenario/../../../outside.ks`**. Directory traversal constructs names without validating components; scenario selection accepts any path containing `data/scenario/`. ([E/tyrano_asar.rs:123](C:/Projects/Locust-lane-b/crates/formats/src/tyrano_asar.rs:123), [284](C:/Projects/Locust-lane-b/crates/formats/src/tyrano_asar.rs:284))

   **F:** `read_entry` joins that path onto `app.asar.unpacked` and calls `fs::read`, resolving the example to its parent’s `outside.ks` when intermediate directories exist. Tyrano extraction calls this reader directly and turns the returned text into rows. ([E/tyrano_asar.rs:264](C:/Projects/Locust-lane-b/crates/formats/src/tyrano_asar.rs:264), [E/tyrano.rs:1073](C:/Projects/Locust-lane-b/crates/formats/src/tyrano.rs:1073))

   **H — Work/files:** Validate member components and confine unpacked reads, including symlink/reparse traversal. Edit `E/tyrano_asar.rs` and `E/tyrano.rs`, with inline regressions.

   **H — Acceptance:** Traversal, rooted-path and interior-link fixtures are refused without importing outside text; a normal unpacked scenario still extracts and injects. Check both separator spellings.

   **H — Shares files:** None.

8. **W6-08 — Retain unapproved Review edits across navigation**

   **H · defect · M, 2 days · medium risk.** Value: prevent losing a reviewer’s correction when moving between entries, files or screens.

   **F — Input → wrong output:** Edit translation `V1` to `V-review`, choose Skip, then Previous. Review reloads the selected entry’s saved translation, restoring **`V1`**. Its editable text is component state; changing scope explicitly clears it, and Exit navigates away without preserving it. ([U/pages/Review.tsx:78](C:/Projects/Locust-lane-b/apps/desktop/src/pages/Review.tsx:78), [130](C:/Projects/Locust-lane-b/apps/desktop/src/pages/Review.tsx:130), [295](C:/Projects/Locust-lane-b/apps/desktop/src/pages/Review.tsx:295), [335](C:/Projects/Locust-lane-b/apps/desktop/src/pages/Review.tsx:335))

   **F:** Existing durable drafts already provide project/entry keys, saved baselines and synchronous persistence. ([U/stores/draftStore.ts:6](C:/Projects/Locust-lane-b/apps/desktop/src/stores/draftStore.ts:6), [88](C:/Projects/Locust-lane-b/apps/desktop/src/stores/draftStore.ts:88))

   **H — Work/files:** Integrate Review text with those existing draft APIs. Edit `U/pages/Review.tsx` and `U/pages/Review.test.tsx`.

   **H — Acceptance:** Skip→Previous, scope changes, route navigation and remount recover `V-review`; successful approval retires only the acknowledged draft. A changed server baseline retains the draft and produces a conflict.

   **H — Shares files:** None within this wave.

9. **W6-09 — Support VX Ace System UI terms**

   **H · capability · M, 3 days · medium risk.** Value: translate the remaining system menus and currency label through the same project workflow.

   **F — Input → current gap:** A `System.rvdata2` root object containing `@currency_unit="Gold"` and `@terms.@commands=["Fight","Escape"]` produces no rows. Root dispatch handles maps/common events specially and sends everything else to array extraction; System has no field mapping, and a non-array root is ignored. ([E/rpgmaker_vxa.rs:979](C:/Projects/Locust-lane-b/crates/formats/src/rpgmaker_vxa.rs:979), [991](C:/Projects/Locust-lane-b/crates/formats/src/rpgmaker_vxa.rs:991), [1011](C:/Projects/Locust-lane-b/crates/formats/src/rpgmaker_vxa.rs:1011))

   **H — Work/files:** Implement an explicit VX Ace System field allowlist and paired writer, initially covering currency and term arrays. Edit `E/rpgmaker_vxa.rs`, including inline fixtures.

   **H — Acceptance:** The fixture exposes and reinjects those three values; nontext System fields, object links and unrelated strings survive unchanged. Unsupported System layouts produce a clear diagnostic.

   **H — Shares files:** W6-01; implement after its header correction.

10. **W6-10 — Connect injection capacity failures directly to filtered QA**

   **H · capability · M, 2 days · low–medium risk.** Value: let a translator reach the strings blocking injection without reconstructing the recovery steps manually.

   **F — Input → current friction:** When validation reports `ExceedsBinarySlot > 0`, Inject shows a warning and continues; Replace requests the strict capacity gate. A refusal is handled through the generic log/toast path. ([U/components/InjectModal.tsx:277](C:/Projects/Locust-lane-b/apps/desktop/src/components/InjectModal.tsx:277), [297](C:/Projects/Locust-lane-b/apps/desktop/src/components/InjectModal.tsx:297), [372](C:/Projects/Locust-lane-b/apps/desktop/src/components/InjectModal.tsx:372))

   **F:** The localized refusal tells the user to run Validate, while QA already supports kind-filtered issue queries and opening an issue in Editor. ([U/lib/i18n/en.ts:1568](C:/Projects/Locust-lane-b/apps/desktop/src/lib/i18n/en.ts:1568), [U/pages/Qa.tsx:32](C:/Projects/Locust-lane-b/apps/desktop/src/pages/Qa.tsx:32), [131](C:/Projects/Locust-lane-b/apps/desktop/src/pages/Qa.tsx:131))

   **H — Work/files:** Retain the capacity result in an inline recovery panel with “Review oversized strings”; open QA with `ExceedsBinarySlot` selected. Edit `U/components/InjectModal.tsx`, `U/components/InjectModal.test.tsx`, `U/pages/Qa.tsx`, `U/pages/Qa.test.tsx`, `U/lib/i18n/en.ts`, `U/lib/i18n/es.ts`.

   **H — Acceptance:** An oversized translation exposes a persistent recovery action; clicking it opens the correct project’s filtered issues and then its editor row. After correction, retry succeeds; unrelated errors retain their existing behavior.

   **H — Shares files:** None within this wave.

**H — Ranked dispatch table.** File abbreviations expand as defined above; “inline tests” reside in the named Rust file.

| Rank | ID | Title | Category | Size | Files |
|---:|---|---|---|---|---|
| 1 | W6-01 | Preserve VX/Ace face headers | Defect | M | `E/rpgmaker_vxa.rs` |
| 2 | W6-02 | Preserve rows after unreadable VNTextPatch JSON | Defect | M | `E/vntextpatch.rs`; new `crates/formats/tests/vntextpatch_refresh.rs` |
| 3 | W6-03 | Protect QSP location references | Defect | M | `E/qsp.rs` |
| 4 | W6-04 | Recover SugarCube prose rejected as CSS | Defect | S | `E/sugarcube.rs` |
| 5 | W6-05 | MV/MZ nickname and profile changes | Capability | M | `E/rpgmaker_mv.rs` |
| 6 | W6-06 | Complete Wolf text boundaries | Defect | M | `E/wolf_rpg.rs` |
| 7 | W6-07 | Confine unpacked ASAR reads | Defect | M | `E/tyrano_asar.rs`, `E/tyrano.rs` |
| 8 | W6-08 | Retain Review drafts | Defect | M | `U/pages/Review.tsx`, `U/pages/Review.test.tsx` |
| 9 | W6-09 | VX Ace System UI terms | Capability | M | `E/rpgmaker_vxa.rs` |
| 10 | W6-10 | Injection capacity → filtered QA | Capability | M | `U/components/InjectModal.tsx`, its test; `U/pages/Qa.tsx`, its test; `U/lib/i18n/en.ts`, `es.ts` |

**H — Suggested pairing into two lanes with disjoint complete file sets:**

| Pair | Lane A — sequential | Lane B — sequential |
|---:|---|---|
| 1 | W6-01 | W6-02 |
| 2 | W6-03 | W6-04 |
| 3 | W6-05 | W6-07 |
| 4 | W6-06 | W6-08 |
| 5 | W6-09 | W6-10 |

W6-01 and W6-09 share `rpgmaker_vxa.rs` and stay in Lane A. All frontend files and locale changes stay in Lane B.
