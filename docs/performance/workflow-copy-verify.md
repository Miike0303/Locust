# W3-04: whole-game copy and verification measurements

## Reproduce

Run from the checkout, using the existing `CARGO_TARGET_DIR`. No command below
changes it or builds a second target directory.

```powershell
python tools/benchmark_workflow.py --json .test-tmp/workflow.json --storage-label local
cargo test -p locust-core --test workflow_copy_verify -- --ignored --nocapture
```

The Python benchmark defaults to a **192 MiB source**, 2,048 small assets and
three large assets. It physically generates bytes in a fresh temporary directory
under `.test-tmp`, rotates worker order across three repetitions, checks copied
content, and deletes only its generated tree. Eight source-sized trees plus
64 MiB of overhead must fit below **2 GiB**; oversized requests are refused
before generation. Repeated verification reads multiple GiB without retaining a
multi-GiB game. `--small-files` and `--large-files` change the distribution.

The default `--mode mocked` replaces child commands with no-ops. Production
Python preflight, copies, hashes, invariant checks and manifest publication run
against real files. Backup-shaped copying, modified-resource ZIP creation,
installation, byte-exact rollback and reapplication also use real files. It is
an I/O workload model, not an integration test of Rust backup transactions,
engine extraction, provider latency, or patch receipts.
Successful generated trees are deleted. On any failure the generated tree,
locks and recovery evidence remain; stderr identifies the retained directory.

For the complete pipeline, including actual CLI extraction, both translations,
validation, immutable backup creation, injection, pack, strict apply and rollback:

```powershell
python tools/benchmark_workflow.py --mode real --cli "$env:CARGO_TARGET_DIR/debug/locust.exe" --json .test-tmp/workflow-real.json --storage-label SSD
```

Real mode reuses the production suite's Ren'Py fixture and scripted loopback
endpoint. It needs no provider account and does not launch a game. The caller
builds the CLI. The parent must run real mode and the real Python pipeline suite:
this task's sandbox cannot run real Locust commands. Repeat with a different
`--scratch` on an HDD and `--storage-label HDD` for a storage comparison. Device
type and cold-cache behavior cannot be inferred from these sandbox recordings.

## Report contract

`pipeline-manifest.json` now has an optional `performance` object with version 1,
`scope: current-invocation`, worker cap, elapsed wall time, totals and stage
records. It contains no source text, provider output, credential or input paths.
It is diagnostic only. The closed resume validator accepts an absent object,
validates present numeric/structural fields, and never uses it as authority.
On resume, measurements start again for that invocation; old counters are not
accumulated or used to skip verification. Existing stage receipts and their
child-only `seconds` retain their previous meaning.

Each new stage record has inclusive `seconds`, `completed`, and these counters:

| Counter | Meaning |
| --- | --- |
| `hash_bytes`, `hash_files` | Actual streamed bytes and successfully hashed files; repeated reads count again, including ZIP payload hashes |
| `copy_bytes`, `copy_files` | Actual bytes written and successfully copied files |
| `hash_seconds`, `copy_seconds` | Operation time, summed across workers |
| `scan_entries`, `scan_seconds` | Structural entries visited and full scan time, including hashing when requested |
| `invariant_calls`, `invariant_seconds` | Full invariant checks, including source/pristine and pre-injection working/install hashes plus owned-tree structural checks |
| `child_seconds` | Child wall time, including spawn/wait; child-internal byte counts are opaque to Python |

Work seconds overlap by category: hashing happens inside a scan, which can
happen inside an invariant. Nested stages also overlap. Do not sum table rows
or category times as independent work. Use the `workflow` span for execution
and `preflight` for construction; overall elapsed time also includes setup.
Command spans start before invariants and end after result publication.
Translation/validation and injection/pack proof spans expose their additional
checks. Seed snapshot copying also records bytes and duration. The benchmark's
`overall (includes preflight)` row uses elapsed invocation time and all counters.
Final telemetry publication is outside the completed workflow span.
Failures retain partial byte counts and mark unfinished stage spans false;
publication ambiguity retains the existing terminal recovery behavior.

The existing opt-in `LOCUST_PIPELINE_TIMINGS=1` stderr timing events keep their
W3-01 schema. File-content counters exclude small in-memory semantic JSON
digests. They describe Python I/O; native kernels are recorded separately.

## Safety and validation

All source and payload proofs remain content-based. No mtime/size cache,
discarded digest, hard link to a live source, or skipped payload verification
is introduced. New tests overwrite a source after copying, restore its exact
timestamp and length, and require refusal. A second test corrupts the copy
with the same metadata. Proof-copy creation also checks the source inventory
after copying. Native tests cover same-size/restored-timestamp source drift
before manifest publication and corruption of a published backup payload.

The native microbenchmark hashes 98 MiB in 515 files and exercises the actual
`sha256_file` kernel and `File::create_new`/`io::copy` kernels, with and without
`sync_all`. Each copied file is independently content-verified. It runs only
when explicitly requested, uses rotated 1/2/4-worker order, and is a warm-cache
debug-profile measurement, not a release throughput guarantee.

## Scheduling policy

Python tree hashing and independent file copies use at most four workers by
default. Set `LOCUST_PIPELINE_IO_WORKERS=1` for the serial reference, or `2`/`3`/`4`
to lower the cap. This is independent of provider concurrency. Trees below
8 MiB and single-file jobs stay serial; metadata sizes choose scheduling only.
Every content hash and source-after-copy check still runs. The benchmark
explicitly sets the cap for each sample. Tiny-workload thread overhead motivated
the threshold; the larger workload determines whether concurrency is useful.
Worker settings in the Python benchmark affect Python only, including in real
mode. Native worker comparisons use the separate storage-kernel benchmark.

Native backup inventory hashing uses the same 8 MiB/four-worker bounds, including
the source-after-copy rescan and payload verification. Backup copying, immutable
payload reuse, candidate checks, flushes and manifest publication retain their
coordinator. Replace reserves and locks its destination, validates the namespace
and creates directories before copying independent files with bounded workers.
Each worker rechecks link containment and uses `create_new`; both game locks
remain held until every worker has joined. Results retain deterministic inventory
order and no complete manifest is published on failure.

## Recorded workflow results (2026-10-28)

This recording used 128 small assets and three large assets in a **192 MiB**
source, with three rotated repetitions per worker cap. The benchmark defaults
to 2,048 small assets for a larger namespace stress test; this recording's exact
command is:

```powershell
python tools/benchmark_workflow.py --mib 192 --small-files 128 --large-files 3 --workers 1,2,4 --repeats 3 --storage-label sandbox-C --json .test-tmp/w3-04-resume-workflow.json
```

All preparation, copies, content hashing and invariant checks below ran against
real generated files. Child commands were mocked, so translation and engine
execution time is excluded. The 1-worker reference uses the same instrumentation
and proofs as the worker candidates, including the new work-proof source rescan;
it is not a timing of an uninstrumented historical checkout. Medians follow;
nested rows overlap and their medians must not be added.

| Stage | Before: 1 worker (s) | 2 workers (s) | After: 4 workers (s) | Hash MiB | Copy MiB |
| --- | ---: | ---: | ---: | ---: | ---: |
| Preflight | 1.869 | 0.653 | 1.091 | 384.0 | 0.0 |
| Prepare pristine / working / install copies | 10.385 | 5.863 | 8.409 | 2304.0 | 576.0 |
| Extract | 2.062 | 2.061 | 1.674 | 768.0 | 0.0 |
| JA to EN | 3.951 | 1.756 | 1.261 | 768.0 | 0.0 |
| Validate English | 3.176 | 1.876 | 1.373 | 768.0 | 0.0 |
| Strict pivot | 2.926 | 2.923 | 2.231 | 768.0 | 0.0 |
| EN to ES | 3.546 | 1.980 | 1.434 | 768.0 | 0.0 |
| Validate Spanish | 2.651 | 1.689 | 1.145 | 768.0 | 0.0 |
| Mock inject and backup | 5.638 | 3.001 | 2.052 | 1344.0 | 192.0 |
| Work proof copy | 2.589 | 1.461 | 0.795 | 576.0 | 192.0 |
| Mock pack and ZIP proof | 3.048 | 1.010 | 1.524 | 384.1 | 0.0 |
| Mock apply / rollback / reapply | 16.002 | 15.546 | 10.302 | 3840.5 | <0.1 |
| Workflow, including preparation | 72.715 | 44.304 | 43.018 | 14208.8 | 960.0 |
| Overall, including preflight | **75.216** | **45.034** | **44.176** | **14592.8** | **960.0** |

Four workers improved the overall median **1.70x** (41.3% less elapsed time).
The invariant-time median fell from 42.027 to 18.379 s. Four workers beat serial
in each paired repetition. Storage timing was variable: overall ranges were
41.198–85.488 s (1), 28.847–96.580 s (2), and 31.493–82.882 s (4). Two workers
were faster for some individual stages; no guarantee is made for every stage,
device, cache state or namespace size. The table's bytes are unchanged across
worker caps, apart from small diagnostic-publication size differences.

Each invocation hashes about 14.25 GiB and copies 960 MiB while retaining no
more than the enforced 1,600 MiB fixture budget (eight 192 MiB trees plus
64 MiB overhead), below 2 GiB. The source and byte-exact rollback/install proofs
passed in all nine samples. Real CLI and controlled SSD/HDD recordings remain
parent-side checks; this sandbox recording does not establish either result.

## Recorded native results (2026-10-28)

Three repetitions, rotating 1/2/4-worker order, on the sandbox's C: storage with
warm OS cache. The device type is not established; this is not an SSD/HDD
comparison. The debug-profile kernel medians for the same 98 MiB / 515 files:

| Kernel | Before: 1 worker (s) | 2 workers (s) | After: 4 workers (s) | Speedup at 4 |
| --- | ---: | ---: | ---: | ---: |
| Content hashing | 4.475 | 2.538 | 1.614 | 2.77x |
| Replace copy | 1.928 | 1.633 | 1.176 | 1.64x |
| Copy with per-file sync (probe only) | 6.876 | 5.622 | 3.483 | 1.97x |

Hashing and Replace copying therefore retain bounded workers. Copy timing is
variable: serial Replace ranged from 1.131 to 2.891 s, four workers from 0.526
to 1.294 s; four workers won two of three paired samples. The sync-copy probe
shows potential but is **not shipped as parallel backup copying**: immutable
candidate linking, fallback copying and publication remain on the coordinator.
These are kernel timings, not measurements of an entire backup transaction or
release-profile throughput. Full source-after-copy and payload proofs still run.

## Validation record

- `cargo fmt --all -- --check`: passed.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed.
- Final mocked Python suite: 70 tests, passed with the two skips named below,
  including failed-run evidence retention and the 2 GiB budget guard. A separate
  1 MiB benchmark smoke run passed and cleaned up its read-only generated trees.
- `cargo test -p locust-core`: 853 passed, 13 capability failures, 5 ignored.
- `cargo test -p locust-cli`: 77 passed before its first capability failure
  stopped the run.
- A combined `--no-fail-fast` run excluded those 14 known cases so every other
  suite could execute: core 1,229 passed / 4 further capability failures /
  9 ignored / 13 excluded; CLI 248 passed / 2 further capability failures /
  1 excluded. All six additional failures are link-creation denials and are named
  below. All new worker, content-drift and independent-copy tests passed.
- The native storage benchmark passed all copied-content
  assertions across all nine worker/repetition combinations.

## Sandbox capability failures

The unfiltered core/CLI commands were attempted. Hard-link creation returns
Windows error 5 in this sandbox; the existing fallback correctly makes independent
copies, so tests that require shared inode identity or physical-byte deduplication
fail. Junction creation is also denied. These are the named failures, not waived
content-proof failures:

- `backup::tests::backup_dedupe_both_backups_restore_and_verify`
- `backup::tests::backup_dedupe_delete_and_retention_preserve_shared_payload`
- `backup::tests::backup_dedupe_matches_content_across_names_and_single_file_backups`
- `backup::tests::backup_dedupe_restore_then_modify_does_not_mutate_shared_payloads`
- `backup::tests::backup_dedupe_revision_copies_only_changed_file`
- `backup::tests::backup_dedupe_skips_damaged_manifests_and_missing_candidates`
- `backup::tests::backup_dedupe_unchanged_tree_shares_every_payload_file`
- `backup::tests::replacement_does_not_mutate_external_hardlink`
- `export::tests::export_destination_refuses_hard_links`
- `file_identity::tests::same_file_matches_hard_link`
- `project::tests::direct_project_resume_deduplicates_aliases_but_rejects_distinct_candidates`
- `project::tests::open_recent_same_file_aliases_are_one_candidate`
- `extraction::tests::entry_preflight_accepts_selected_root_link_but_rejects_interior_links` (junction)
- `cli_backups_total_counts_shared_payload_once` (hard-link physical-byte accounting)
- `cli_export_refuses_sidecars_and_hard_links_even_with_overwrite` (CLI `export_destination_tests`, hard link)
- `tmx_export_filters_overwrite_and_database_sidecar_guards_are_shared` (CLI `tmx_cli_tests`, hard link)
- `apply_and_rollback_reject_linked_game_directories` (core `patch_apply`, junction)
- `selected_game_root_can_be_a_directory_link` (core `patch_apply`, junction)
- `unrelated_scratch_links_are_not_followed_or_deleted` (core `patch_scratch_ownership`, junction)
- `link_inserted_at_write_boundary_is_rejected_before_asset_mutation` (core `patch_scratch_ownership`, junction)

The mocked Python suite skips
`AdditionalGates.test_sensitive_names_hardlinks_and_special_files` for the same
hard-link denial and `AdditionalGates.test_fifo_refused_without_open` because
Windows has no POSIX FIFO. Production transport tests and benchmark real mode
are left to the parent session, as required by this task's sandbox contract.
