# MT control repair and canonical names (W3-06)

## Live sentinel measurement

Measured on 2026-10-28 using the real Google web endpoint and the production
Google adapter. This is **live evidence**, not replayed responses. Five synthetic
rows per encoding/language pair cover three adjacent Ren'Py controls, an RPG
namebox plus two variables, a numeric placeholder, overlapping-name positions,
and controls touching translated words. No game text was uploaded.

| Encoding (index 0) | JA→EN exact tokens / rows | EN→ES exact tokens / rows | JA→ES exact tokens / rows | Total exact tokens |
| --- | --- | --- | --- | --- |
| `{PL_0}` (current) | 11/11 · 5/5 | 11/11 · 5/5 | 11/11 · 5/5 | 33/33 |
| `__LOCUST_0_0__` | 11/11 · 5/5 | 11/11 · 5/5 | 11/11 · 5/5 | 33/33 |
| `ZXQ0X0QXZ` | 11/11 · 5/5 | 11/11 · 5/5 | 11/11 · 5/5 | 33/33 |
| `⟦L0X0⟧` (selected alternate) | 11/11 · 5/5 | 11/11 · 5/5 | 11/11 · 5/5 | 33/33 |
| `U+E000` + `0` + `U+E001` | 11/11 · 5/5 | 11/11 · 5/5 | 11/11 · 5/5 | 33/33 |

The invocation completed in **14.698 seconds**, with **15 POSTs, 75 translated
rows, 165/165 preserved expected tokens, zero HTTP 429s**. All candidates tied on
this small corpus; there is no measured preservation winner or service guarantee.
The alternate uses Unicode brackets because they delimit ordinary words better
than the ASCII candidate in these responses. For example:

| Sent source (EN→ES) | Recorded live response |
| --- | --- |
| `Hello {PL_0} {PL_1}traveler{PL_2}!` | `¡Hola {PL_0} {PL_1}viajero{PL_2}!` |
| `Hello ZXQ0X0QXZ ZXQ0X1QXZtravelerZXQ0X2QXZ!` | `¡Hola ZXQ0X0QXZ ZXQ0X1QXZtravelerZXQ0X2QXZ!` |
| `Hello ⟦L0X0⟧ ⟦L0X1⟧traveler⟦L0X2⟧!` | `¡Hola ⟦L0X0⟧ ⟦L0X1⟧viajero⟦L0X2⟧!` |
| `⟦L0X0⟧<錬金術師>こんにちは ⟦L0X1⟧、⟦L0X2⟧！` (JA→EN) | `⟦L0X0⟧<Alchemist>Hello ⟦L0X1⟧, ⟦L0X2⟧!` |
| `⟦L0X0⟧<Alchemist>Hello ⟦L0X1⟧, ⟦L0X2⟧!` (EN→ES) | `⟦L0X0⟧<Alquimista>¡Hola ⟦L0X1⟧, ⟦L0X2⟧!` |

The earlier NMT spike (`C:/Projects/Locust-nmt-spike/samples.md`, rows
ja11–ja17/en11–en17) supplies the unprotected failure examples below. It measures
OPUS/NLLB, **not Google**. Current Google brace masking succeeded in this probe;
repair fixtures deliberately damage answers to exercise the failure paths.

Reproduce the opt-in probe:

```powershell
cargo test -p locust-providers --test google_sentinel_probe -- --ignored --nocapture
```

The test has a 300-second wall deadline, one shared 60-RPM limiter, one in-flight
POST, a circuit latched after three consecutive 429/blocked responses, and stops
on the first transport/service error. Dropping the supervised future cancels its
subrequests. Every source and raw translation is printed as JSON, followed by CSV
counts. Ordinary crate tests do not make these live requests.

## Repair and acceptance

1. Controls and explicit canonical glossary spans retain the original compiled
   trie and `{PL_…}` / `{G_…}` first-pass masking.
2. `TranslationProvider::supports_instructions()` identifies source-only MT.
   Google explicitly returns false; the default recognizes Google, DeepL and
   Argos IDs, including wrappers retaining those IDs. Other source-only adapters
   must override this capability. Instruction-following adapters retain their
   two bounded correction attempts.
3. If raw sentinel restoration or restored controls fail, MT gets **one** logical
   repair for that ID. Every control and glossary identity is replaced in the
   actual request source by `⟦L{nonce}X{index}⟧`. A nonce absent from both sanitized
   source and protected originals prevents literal collisions. With no protected
   identities, no identical request is sent.
4. Alternate output must contain each identity exactly once and no unknown or
   malformed alternate identities. A single span pass decodes to the original
   opaque identities. Existing canonical insertion, speaker ownership, nesting,
   and missing/extra-control checks then run unchanged. No controls are invented
   or appended to repair an answer. Literal sentinel look-alikes in source are
   masked and restored as opaque source data, never recursively decoded.
5. Failure leaves the ID pending. The existing configured fallback chain reloads
   only pending IDs within the immutable job scope and rebuilds fresh masks for
   its provider. With no configured fallback, the ID remains pending and emits
   a failure event. Successful IDs are never sent to fallback for control repair.
6. Source-only MT also skips instruction-only fixed/shared length retries. All
   semantic and transport attempts still use existing cancellation, deadlines,
   model-input allowance, cost and usage accounting. W3-02 transport receipts
   remain intact; they are independent of these logical repair calls.

## Fixture matrix

These are deterministic acceptance fixtures, separate from the live measurement.
The Ren'Py, RPG, overlapping-name and literal-look-alike cases run consecutively
JA→EN→ES. Binary slots and shared TextAsset cells retain their existing glossary
hints and capacity policy; canonical masking applies only to eligible rows.

| Case | Before / rejected behavior | After |
| --- | --- | --- |
| Ren'Py | Spike translates/drops `[player]` and `{b}…{/b}` | `こんにちは [player] {b}旅人{/b}!` → `Hello [player] {b}traveler{/b}!` → `Hola [player] {b}viajero{/b}!` |
| RPG variables/namebox | Spike mangles `\V[1]` and namebox syntax | `\n<錬金術師>こんにちは \V[1]、\N[2]!` → `\n<Alchemist>Hello \V[1]、\N[2]!` → `\n<Alquimista>Hola \V[1]、\N[2]!` |
| Overlapping names | Hints alone can rename, drop or partially match names | `リディア様、リディア アンナ アン` → `Lady Lydia、Lydia Anna Ann` → `Dama Lidia、Lidia Anna Ana`; longest complete name wins |
| Literal look-alikes | Literal `{PL_0}`, `{G_0_0}` or alternate text could collide with tokens | Literal brace and `⟦L0X0⟧`/malformed `⟦L…` spellings survive both hops exactly; alternate nonce advances |
| Adversarial repair | Missing/duplicated/unknown/malformed identities, foreign IDs, extra `\V[99]`, moved namebox glossary identity | Every such answer stays pending, is not cached, and receives at most one MT alternate attempt |
| W3-02 + scoped fallback | Successful subchunks could be resent with failed rows | Receipt, transport, alternate and fallback calls contain exactly the affected IDs listed below |

## Request-count evidence

`partial_outcomes_then_control_failure_fallback_only_resend_affected_ids` records
the following dispatches (unselected rows never enter the job):

| Call | Provider | IDs | Sent encoding |
| --- | --- | --- | --- |
| 1 | Source-only MT, partial result | `good`, `bad`, `transport` | Original braces |
| 2 | Transport retry for the 503 ID | `transport` | Original braces |
| 3 | Semantic repair | `bad` | Unicode alternate, different source |
| 4 | Configured fallback | `bad` | Fresh original braces |

`good` is submitted once, `transport` twice, `bad` three times across both
providers. Each ID is saved and emits success exactly once. Five result receipts
at three synthetic tokens each account for **15 tokens**, including rejected
control answers; the failed 503 reports no receipt.

The Google HTTP mock separately asserts exactly **two POSTs**: one two-ID initial
form, then one changed single-ID alternate form. A damaged alternate sends only
`bad` to fallback. The successful `good` row keeps Google attribution. The core
fixtures also verify malformed controls remain pending and are never cached.

## Correction round 2

Canonical masking is again restricted to eligible rows without binary-slot or
shared TextAsset capacity policy. Binary/fixed slots and shared cells use their
existing glossary hints; oversized glossary terms cannot force a pending row.
The original memory/dedup eligibility, group restoration, explicit lossy fitting,
and review/injection capacity behavior are restored. Removed four canonical
capacity/pivot integration fixtures, the shared canonical shortening unit fixture,
and the corresponding capacity-policy documentation. The MT alternate resend,
scoped fallback, sentinel probe, nonce handling and Ren'Py/RPG fixtures remain.

`crates/core/src/translation/tests.rs` matches HEAD again, including
`test_glossary_exact_oversize_binary_slot_falls_through` and its original
`Some("Opcns")` expectation. That unit test passed in the full core run.
`crates/core/tests/translation_integrity.rs` is unchanged; its complete suite
passed **21/21**, including these three required tests:

| Existing test, unchanged | Result |
| --- | --- |
| `only_matched_eligible_rows_bypass_memory_and_dedup_preserves_capacity_policy` | PASS |
| `shared_textasset_rows_remain_unmasked_even_without_cell_budget` | PASS |
| `exact_and_binary_glossary_keep_the_existing_budgeted_behavior` | PASS |

## Validation

Results from the corrected working tree on 2026-10-28. The crate commands use
`--no-fail-fast` so every target runs despite sandbox link failures. Counts
include unit, integration and doc-test summaries; ignored tests were not run.
The existing live sentinel measurement above was retained, not rerun.

| Check | Passed | Failed | Ignored | Exit |
| --- | ---: | ---: | ---: | ---: |
| `cargo test -p locust-core --no-fail-fast` | 1243 | 17 sandbox-only | 8 | 101 |
| `cargo test -p locust-providers --no-fail-fast` | 151 | 0 | 4 | 0 |
| `cargo test -p locust-cli --no-fail-fast` | 257 | 3 sandbox-only | 0 | 101 |
| `cargo fmt --all -- --check` | PASS | 0 | — | 0 |
| `cargo clippy --workspace --all-targets -- -D warnings` | PASS | 0 | — | 0 |

The three retained `mt_protection` integration tests passed, as did the Google
POST-body alternate-repair/fallback fixture. Overall: **1,651 passed, 20
sandbox-only failures, 12 ignored**. No translation-policy failures remain.

Sandbox-only failures are listed by name below. Direct hard-link and junction
creation report Windows access denied (OS code 5); deduplication uses copies,
so tests expecting shared file identity or shared on-disk byte accounting fail.
These unrelated source tests were not altered or skipped.

```text
locust-core:
backup::tests::backup_dedupe_both_backups_restore_and_verify
backup::tests::backup_dedupe_delete_and_retention_preserve_shared_payload
backup::tests::backup_dedupe_matches_content_across_names_and_single_file_backups
backup::tests::backup_dedupe_restore_then_modify_does_not_mutate_shared_payloads
backup::tests::backup_dedupe_revision_copies_only_changed_file
backup::tests::backup_dedupe_skips_damaged_manifests_and_missing_candidates
backup::tests::backup_dedupe_unchanged_tree_shares_every_payload_file
backup::tests::replacement_does_not_mutate_external_hardlink
export::tests::export_destination_refuses_hard_links
extraction::tests::entry_preflight_accepts_selected_root_link_but_rejects_interior_links
file_identity::tests::same_file_matches_hard_link
patch_apply::apply_and_rollback_reject_linked_game_directories
patch_apply::selected_game_root_can_be_a_directory_link
patch_scratch_ownership::link_inserted_at_write_boundary_is_rejected_before_asset_mutation
patch_scratch_ownership::unrelated_scratch_links_are_not_followed_or_deleted
project::tests::direct_project_resume_deduplicates_aliases_but_rejects_distinct_candidates
project::tests::open_recent_same_file_aliases_are_one_candidate

locust-cli:
backup_recovery_tests::cli_backups_total_counts_shared_payload_once
export_destination_tests::cli_export_refuses_sidecars_and_hard_links_even_with_overwrite
tmx_cli_tests::tmx_export_filters_overwrite_and_database_sidecar_guards_are_shared
```

Raw command output is retained directly in the existing lane-d target as
`w3-06-correction2-core.log`, `w3-06-correction2-providers.log`,
`w3-06-correction2-cli.log` and `w3-06-correction2-clippy.log`.

No commits, pushes, branch changes, stash or reset operations were performed.
No `tmp/` directory was searched or read. `CARGO_TARGET_DIR` remains
`C:/Users/Mike/AppData/Local/locust-shared-target/lane-d`; no other target directory
was created. The pre-existing W3-01 evidence file is untouched. Broad cleanup
was not run because its recursive traversal/deletion includes other targets and
unclassified scratch outside this correction's authorized scope.

## Changed files and line counts

Additions/deletions are the remaining uncommitted W3-06 diff relative to HEAD;
new files count wholly as additions. Final lines count the working copy.

| File | Added / deleted | Final lines |
| --- | --- | ---: |
| `crates/core/src/glossary.rs` | +1 / −0 | 1142 |
| `crates/core/src/placeholder.rs` | +26 / −1 | 1676 |
| `crates/core/src/translation/provider.rs` | +6 / −0 | 444 |
| `crates/core/src/translation/protection.rs` | +82 / −0 | 525 |
| `crates/core/src/translation/manager.rs` | +35 / −6 | 2501 |
| `crates/providers/src/google.rs` | +96 / −1 | 1759 |
| `crates/core/tests/mt_protection.rs` | +436 / −0 | 436 |
| `crates/providers/tests/google_sentinel_probe.rs` | +96 / −0 | 96 |
| `docs/MT-PROTECTION.md` | +218 / −0 | 218 |

`crates/core/src/translation/tests.rs` is restored to HEAD: +0 / −0, 5,750 lines.
