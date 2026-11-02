# W4-04: partial receipts and frozen scopes

Audited base `9b25f2f` on `cycle/w4-04-partial-scope`. Tests use scripted providers, temporary SQLite catalogs, and localhost httpmock (`body_contains`); no live Google requests or game files. SQLite triggers on an independent connection count actual catalog writes, including writes that leave values unchanged.

| Scenario | Regression test | RED before fix? | Result / fix |
| --- | --- | --- | --- |
| Selected/unselected source twins, memory, exact glossary, dedupe | `w4_04_partial_scope_twins_shortcuts_and_dedupe_have_zero_excluded_writes` | No; initially GREEN | Only selected IDs are written, once each. Provider receives one selected dedupe representative. Excluded twins have zero requests and zero SQL writes. Empty ID/file selections do no work, including with memory/glossary enabled. No production fix. |
| Repeated partial retries, then fallback | `w4_04_two_partial_retries_then_fallback_keep_frozen_ids_and_exact_accounting` | No; initially GREEN | Primary request IDs shrink from `[a,b,c]` to `[b,c]` to `[c]`; fallback receives only `[c]`. A selected duplicate reuses `a` without additional usage. An ID inserted while the first call is gated stays outside the frozen job even though it was requested before admission. SQL writes, events, scoped remaining count, project stats, token/cost totals, and both run records agree. No production fix. |
| Cancellation during partial retry backoff | `w4_04_cancel_during_partial_retry_backoff_saves_receipts_once` | No; initially GREEN | Receipts from both attempts commit exactly once; remaining IDs stay pending; terminal event is Paused. No production fix. |
| Overall deadline during partial retry backoff | `w4_04_deadline_during_partial_retry_backoff_saves_receipts_once` | No; initially GREEN | Same retained receipts and exact ledger usage; deadline prevents another attempt. No production fix. |
| Cancellation during an in-flight partial retry | `w4_04_cancel_during_partial_retry_drops_inflight_request_and_saves_receipts_once` | No; initially GREEN | Previously collected and newly appended subchunk receipts survive interruption. A request lease proves the in-flight future has dropped before the job returns; no active request remains. No production fix. |
| Overall deadline during an in-flight partial retry | `w4_04_deadline_during_partial_retry_drops_inflight_request_and_saves_receipts_once` | No; initially GREEN | Same receipt preservation and request-drop checks under the overall deadline. No production fix. |
| Manual edit with a stale success receipt | `w4_04_user_edit_rejects_stale_partial_batch_without_success_accounting` | No; initially GREEN | Existing guarded transaction rejects the entire stale batch with `translation_conflict`, preserves the manual edit, and rolls back its peer. Zero success events / zero run successes; reported provider tokens and cost remain recorded. No production fix. |
| Manual edit of an ID that fails partially | `w4_04_manual_edit_of_partial_failure_is_skipped_in_fallback_total` | **Yes** | Before the fix, Completed reported **2** successes although only `a` had a success event and the run ledger recorded **1**; `b` was manually translated while its request was in flight. Fallback totals now accumulate committed successes from each pass instead of deriving successes from the drop in pending count. The manual edit is excluded from job success accounting. |
| Scoped Google partial 429 and shared cooldown | `google::tests::w4_04_scoped_partial_429_cools_shared_limiter_and_preserves_receipts` | No; initially GREEN | A scoped success is saved once, its excluded twin stays pending, and a sibling's 429 applies Retry-After=90 to a shared limiter. A second adapter makes zero POSTs during an 80-second virtual timeout, and allowance remains blocked until at least 89 seconds after feedback. Normal constructors share the registry limiter; an isolated shared limiter avoids contaminating parallel tests. No Google production fix. |

The stale-save API intentionally rejects an entire conflicting batch; it has no separate skipped counter. Coverage checks that rejected receipts publish no success and do not increment run successes. This audit preserves that existing transaction contract. The demonstrated defect was the fallback terminal count, not an overwrite by the stale guard.

The production change is confined to `manager.rs`: propagate committed pass counts, including counts already committed before a later pass error, into the chain's terminal total. The original internal helper remains available to existing tests. GFAST defaults remain 60 initial / 8 minimum / 140 maximum RPM; provider batching defaults are unchanged.

## Verification

- Initial core regression run, before the production fix: **7 passed, 1 failed, 0 ignored**. The sole failing assertion was the manual-edit terminal total described above.
- Core regression rerun immediately after the fix: **8 passed, 0 failed, 0 ignored**.
- Google focused regression: **1 passed, 0 failed, 0 ignored** (155 filtered across test binaries).
- `cargo fmt --all`: PASS. Existing edited Rust files retain CRLF line endings.
- `cargo clippy -p locust-core -p locust-providers --all-targets -- -D warnings`: PASS.
- Required `cargo test -p locust-core -p locust-providers`: FAIL, **869 passed, 13 failed, 5 ignored**; Cargo stopped after the core unit-test binary.
- Remaining targets were run separately, without repeating that binary:
  - `cargo test -p locust-core -p locust-providers --test '*' --no-fail-fast`: **405 passed, 4 failed, 6 ignored**.
  - `cargo test -p locust-providers --lib`: **134 passed, 0 failed, 2 ignored**.
  - `cargo test -p locust-core -p locust-providers --doc`: **0 passed, 0 failed, 0 ignored**.
- Aggregate of all final targets, counted once: **1,408 passed, 17 failed, 13 ignored**. All **9 new regressions passed**. Live probes remained ignored. `CARGO_TARGET_DIR` stayed at the supplied lane target and `CARGO_INCREMENTAL=0` throughout.

### Sandbox failures for the parent to rerun

The following unchanged tests failed outside this lane's translation work. Direct hard-link and junction creation report Windows error 5 / `Acceso denegado`. Seven backup dedupe assertions expected shared file identity; their failures are consistent with the hard-link restriction demonstrated by the direct probes in the same run. No fix or defect claim is made for these out-of-lane failures.

Core unit tests (13):

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

Core integration tests (4):

- `patch_apply`: `apply_and_rollback_reject_linked_game_directories`, `selected_game_root_can_be_a_directory_link`
- `patch_scratch_ownership`: `link_inserted_at_write_boundary_is_rejected_before_asset_mutation`, `unrelated_scratch_links_are_not_followed_or_deleted`

## Changed files

- `crates/core/src/translation/manager.rs`
- `crates/core/tests/w4_04_partial_scope.rs` (new)
- `crates/providers/src/google.rs` (test only)
- `docs/performance/w4-04-partial-scope-audit.md` (new)

No commits created. No out-of-lane fixes required.
