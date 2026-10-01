# Unity 02 — expose character display-name values (M, defect, writer)

Repo C:\Projects\Locust, main. Do not commit. Re-audited commit: 09eac768a315225c764091697d9fa8a54859158a; inspect/rebase if HEAD moves. Other writers own yuris.rs and tyrano.rs: do not touch them. Do not touch core, apps/desktop/src/**, or tools/ground-truth/**. Edit only the Unity files named below and their in-file tests. No new dependencies.
NEVER modify D:\juegos. Fixtures/reports/build outputs belong in a new scratch root, under 5 GB. Archive committed HEAD and overlay only your Unity changes. Never run cargo in the live repo. Use %LOCALAPPDATA%\locust-shared-target, CARGO_INCREMENTAL=0, CARGO_PROFILE_DEV_DEBUG=0; copy only the finished executable. Stop only for an actual Windows sandbox helper failure from your own tool call.
Evidence root: C:\Users\Mike\AppData\Local\Temp\locust-c113-unity-reaudit. This run could not recreate the pinned custom-schema oracle; do not treat supplementary precision/recall as the original full audit.


Files: crates/formats/src/unity.rs; optional in-file tests in unity_serialized.rs only if needed.

## Evidence
117 script declarations omitted on HEAD (oot-misses.json, character_display_name); 6 BOXMAN CharacterNames values omitted (supplementary-misses.json). Independent current data: Characters.txt:4 "CJ", :5 "Victoria", :6 "Sarah"; BOXMAN resources.assets object 824 keys Carter/Emily/Jake. Remaining regressions display-names and managed-names reproduce both omissions. extract_text_scripts unity.rs:88/:146 skips every character declaration. is_non_player_textasset_name :1573/:1592 intentionally excludes CharacterNames as glossary-only: this writer explicitly adds player-facing value slots, keeping lookup IDs untouched.

## Change
Extract/inject only the quoted display-name value in character declarations, with a record-kind and exact slot locator. Preserve character IDs, unquoted attributes, whitespace, quote syntax, comments and line endings. Add the reciprocal quoted-slot route to inject_text_scripts (:200); reject changed declarations/IDs and unsafe quote/control translations. Avoid treating a character directive as normal dialogue.
Allow the right-hand display values of a structurally identified CharacterNames TextAsset through existing localization-line extraction/injection. Retain table keys and line boundaries. Keep TMP linebreak tables, technical docs and locale catalogs excluded; do not globally disable technical-name filtering or reinterpret arbitrary identifier maps.

## Tests (must fail on current HEAD)
remaining_regressions.py: character CJ "CJ" plus ordinary dialogue returns only the dialogue; CharacterNames TextAsset with Carter: Carter / Emily: Emily / Jake: Jake returns no rows. Require 1 declaration value and 3 table values, exactly one row per physical slot. Inject a changed visible name; assert IDs/keys and all other bytes unchanged. Test escapes, CRLF, unchanged translation, stale source, glossary fallback and names that also appear as other technical IDs.

## Real-data verification
Require 117/117 declared name fields and 6/6 CharacterNames values in the independent script/table oracle, no ID/key edits, no new technical-name FP, no duplicate IDs or collisions. Originals remain unchanged; scratch inject only. Full custom-schema recall/precision remains a separate mandatory re-audit when its pinned parser is available.

## Gates
Same stable-snapshot cargo test/clippy/fmt gates and scratch-only negative control as writer-01. Deliver verification.md with per-kind hits, 123-slot denominator, exact before/after examples, skip reasons and hashes.

