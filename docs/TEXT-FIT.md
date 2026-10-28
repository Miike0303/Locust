# Dialogue width estimate

The optional **Width estimate** panel implements the [research MVP](research/rpgmaker-text-fit-preview.md). It starts collapsed and measures only extractor-owned MV/MZ message rows: `format_id = rpgmaker-mv`, the `dialogue` tag, and the `#cmd_<digits>#msg` message locator. Scrolling text, choices, speakers, database messages/descriptions, packs, plugins, RGSS and unknown provenance are excluded. A matching filename is insufficient.

Choose a single static TTF/OTF file, a positive finite font size in px, and a positive finite usable content width in px. All three fields start empty; the profile is stored per project in browser localStorage. Locust does not discover a system font or infer window geometry. Portraits, padding and plugins must already be accounted for in the chosen usable width.

For each original line, unkerned advance is `sum(glyph horizontal advances) × font size / units per em`. Source and the unsaved draft are assessed independently. LF/CRLF, empty lines, trailing lines and spaces are preserved. Supported text is printable ASCII plus the explicitly supported Latin letters/punctuation present in the selected face. Any backslash, tab, combining sequence, emoji, complex script, bidi/format control, inline markup or missing glyph makes that line unavailable with a reason. No partial or missing-glyph zero-width substitution is reported. Empty valid lines have zero advance and a separate no-text label.

The result says **estimated advance within/exceeds chosen width**. Advance does not certify ink clipping, vertical capacity, page breaks, rendered font coverage, engine compatibility or linguistic quality. Injection may rewrap these lines. The estimator never wraps, saves, injects or changes validation/status; character and encoded-byte checks remain independent.

Entering estimate controls from the draft textarea also bypasses its ordinary blur autosave, so an unsaved draft can be assessed. Other editor save gestures keep their existing behavior.

`POST /api/text-fit` and the direct Tauri `text_fit` command accept the same request: `entry_id`, `font_path`, `font_size_px`, `usable_width_px`, and optional `draft_text` (omission uses the saved translation). Both return `source` and `draft` line arrays containing `status`, nullable `advance_px`, `usable_width_px`, and nullable `reason`. Invalid row admission or more than 16,384 combined source/draft Unicode scalars is refused. Invalid profile/font inputs produce unavailable lines. Font reads reuse the existing 64 MiB ceiling and `ttf-parser`; no dependency was added. Font I/O and metrics run in a blocking worker; the cache is bounded and keyed by font content and settings.

Draft/profile changes are debounced; request ownership includes project, row, source, draft and profile. Responses from older requests are discarded, pending HTTP requests are aborted, and unmount/collapse releases UI ownership. Tauri blocking work can finish after cancellation, but cannot publish an obsolete result.

Each cache holds at most four paths, parsed faces, profiles and text results. Parsed metrics are shared across sizes/widths. Unchanged file length, modification time and creation time avoid another content read; changed metadata triggers a bounded reread and SHA-256 check. An external same-length rewrite preserving both timestamps requires clearing the cache or restarting the app.

The tiny synthetic fixtures test arithmetic and conservative refusal, not rendering. The existing fixed fixture gives `Hola = 56 px` at 28 px. The new proportional fixture has 1000 units/em, `i = 200`, `W = 900`: at 20 px, `iii = 12 px` and `WWW = 54 px`; width 30 distinguishes them, 54 is exact equality, and 53.9 is exceeded. See [fixture generation](../crates/core/tests/fixtures/README.md) for reproducible generation and limitations. Core tests retain cold/warm and repeated/distinct-text timing samples without asserting a latency guarantee. HTTP/native parity, no-write snapshots, input limits, unsupported text/font cases and UI request ownership are tested separately.

One debug Windows run (`cargo test -p locust-core --test text_fit -- --nocapture`) recorded these milliseconds. Each pair used a fresh cache for the cold call and exactly one font read/parse across both calls; every cache contained one path/font/profile/result. These are fixture observations, not performance guarantees.

| Input scalars | Repeated cold / warm (ms) | Distinct Latin cold / warm (ms) |
| --- | --- | --- |
| 1,024 | 7.3243 / 0.1725 | 2.4891 / 0.1521 |
| 4,096 | 3.0826 / 0.2309 | 3.2504 / 0.2368 |
| 16,384 | 6.2731 / 0.5513 | 7.0623 / 0.5968 |

The supported non-ASCII repertoire is Latin alphabetic U+00C0–024F / U+1E00–1EFF plus an explicit punctuation whitelist (see `supported_character`); every character still needs a glyph in the selected face. Other Unicode is unavailable even if the face has a glyph. No fallback, shaping, kerning, synthetic style or engine control evaluation is attempted.

Sandbox link failures observed during workspace validation (all outside this feature):

- `backup::tests::backup_dedupe_both_backups_restore_and_verify`
- `backup::tests::backup_dedupe_delete_and_retention_preserve_shared_payload`
- `backup::tests::backup_dedupe_matches_content_across_names_and_single_file_backups`
- `backup::tests::backup_dedupe_restore_then_modify_does_not_mutate_shared_payloads`
- `backup::tests::backup_dedupe_revision_copies_only_changed_file`
- `backup::tests::backup_dedupe_skips_damaged_manifests_and_missing_candidates`
- `backup::tests::backup_dedupe_unchanged_tree_shares_every_payload_file`
- `backup::tests::replacement_does_not_mutate_external_hardlink`
- `export::tests::export_destination_refuses_hard_links`
- `extraction::tests::entry_preflight_accepts_selected_root_link_but_rejects_interior_links`
- `file_identity::tests::same_file_matches_hard_link`
- `project::tests::direct_project_resume_deduplicates_aliases_but_rejects_distinct_candidates`
- `project::tests::open_recent_same_file_aliases_are_one_candidate`
- `routes::backups::backup_operation_tests::backup_http_delete_preserves_other_shared_backup`
- `patch_apply::apply_and_rollback_reject_linked_game_directories`
- `patch_apply::selected_game_root_can_be_a_directory_link`
- `patch_scratch_ownership::unrelated_scratch_links_are_not_followed_or_deleted`
- `patch_scratch_ownership::link_inserted_at_write_boundary_is_rejected_before_asset_mutation`

Explicit hard-link/junction creation returned Windows access denied. Backup sharing/accounting assertions failed because the sandbox forced independent copies. The focused text-fit tests do not require those privileges.

Validation: core TDD started with an unresolved-module RED (18 tests written first), then 16 passed / 2 failed, then 18 passed / 0 failed. The two implementation failures were corrected before the final checks. Formatting and two frontend test TypeScript errors were also corrected.

| Check | Final observed result |
| --- | --- |
| `cargo test -p locust-core --test text_fit` | 18 passed; 0 failed |
| `cargo test -p locust-core` | Unit stage: 823 passed; 13 sandbox failures; 3 ignored |
| Core integration follow-up, `--test '*' --no-fail-fast` | 363 passed; 4 sandbox failures; 1 ignored |
| `cargo test -p locust-server` | Unit stage: 167 passed; 1 sandbox failure; 2 ignored |
| Server integration follow-up, `--test '*'` | 104 passed; 0 failed |
| `cargo test -p locust-desktop` | 43 passed; 0 failed, including native/HTTP parity |
| `cargo fmt --all -- --check` / workspace Clippy with `-D warnings` | Both passed |
| Desktop `npm ci` | Passed; 115 packages installed; 0 audit vulnerabilities |
| Desktop `test:unit` | 1,265 passed; 0 failed; 29 focused text-fit cases |
| Desktop `build` | Passed; existing nonfatal chunk-size/mixed-import warnings |
