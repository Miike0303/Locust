# W3-03 correction round 2: serial default and diagnostic release evidence

Status: automatic execution now stays serial for every engine, phase, and shape. Four complete release matrices, with five runs per bucket, remained too noisy to accept: two at Normal priority and two at AboveNormal priority. The required proof that one worker is within noise of HEAD remains **UNPROVEN**. A quiet measurement window is required before performance acceptance or automatic parallel dispatch; this report does not claim the correction round is complete.

Worktree: `C:/Projects/Locust-lane-a`, branch `cycle/w3-03-parallel-files`. Existing uncommitted work is retained. No commit, push, branch switch, stash, or reset was used. The required `CARGO_TARGET_DIR=C:/Users/Mike/AppData/Local/locust-shared-target/lane-a` is unchanged; the benchmark uses that same target directory.

## Serial dispatch and ordered effects

Each engine selects its serial loop before constructing worker channels or per-file prepared outputs. One worker bypasses even the lazy shape probe. Workloads outside the file-count range, or below the phase's large-file threshold, use the serial loop. Ren'Py's serial extraction and injection bodies retain the original processing logic. RPG Maker's one-worker extraction uses the original streaming `read_dir` loop; a bounded count probe sends the many-small shape to that streaming loop without collecting the whole directory into a path vector.

Parallel candidates retain bounded, read-only file preparation. The coordinator consumes results in encounter order and owns all writes and diagnostics. Injection groups use first-seen file order; HashMaps serve as lookups rather than effect ordering. Errors and panics stop coordinator effects at the same serial prefix, and worker channels are dropped before scoped threads are joined.

Injection rechecks live bytes at the coordinator position and prepares again when preceding writes changed an aliased file. RPG Maker also revalidates a revision original at that position: a proof can alias an earlier live target and become invalid after that target is written. Source-valued RPG Maker revisions retain the exact verified original bytes, including BOM and compressed serialization.

## Conservative shape thresholds

An unset, invalid, or zero `LOCUST_FILE_WORKERS` now resolves to one worker. Automatic execution therefore uses the original serial loops for all phases and shapes, before shape probes, worker channels, or prepared-file outputs. Explicit worker counts remain bounded to eight workers. A value of one always selects serial. Explicit parallel dispatch accepts only 2 through 64 files or injection groups; its lazy shape predicate must also pass.

| Dispatch mode | Engine | Required large files/groups | Extraction bytes per file | Injection entries per group |
|---|---|---:|---:|---:|
| Stored automatic floor; currently inactive | Ren'Py | 4 | 375 KiB | 10,000 |
| Stored automatic floor; currently inactive | RPG Maker MV/MZ | 4 | 1 MiB | 20,000 |
| Explicit workers 2/4/8 | Either | 2 | 64 KiB | 1,000 |

The lower explicit thresholds exercise candidate parallel implementations and correctness tests. Stored automatic thresholds avoid enabling the much smaller 2-by-1,000-entry boundary without timing evidence. The Ren'Py extraction floor is now 375 KiB (384,000 bytes), below the benchmark's 388,906-byte, 10,000-line source fixture; its structural threshold test was updated. These automatic floors do not enable any work while the default worker count is one.

**Chosen default: every phase stays serial.** This includes Ren'Py extraction/injection and RPG Maker extraction/injection, for both shapes. The many-small fixtures also stay serial at explicit worker counts 2/4/8. Large-file candidates remain available through explicit worker settings. No automatic parallel phase is justified until accepted release measurements establish at least a 15% median improvement. Apparent large-file gains in the diagnostic table below are insufficient because the acceptance spread limit was exceeded.

## Exact HEAD baseline and release harness

The baseline is HEAD `c92d4fdf1a303abd469a3c5e40bd4a1f59c0081d`, formats source tree `70f06c9e440f41642b7665685f8e13c769ca94d2`.

`scripts/bench_w3_release.py` enumerates tracked formats Rust sources with `git ls-tree` and reads each baseline source and its manifest using `git show HEAD:<file>`. It writes a separate generated baseline crate under `.bench-w3/baseline`, changes its package name, and points it at the same core crate as the candidate. Both formats implementations run in one release binary. Generated sources and fixtures live under `.bench-w3`; no separate target directory is created.

The harness builds with `cargo build --release`, refuses debug assertions, and uses identical fixture paths for HEAD and the candidate. It warms every configuration once, then rotates configuration order across at least five measured runs. Fixture restoration, canonical-entry comparison, canonical report comparison, and BLAKE3 file-tree comparison occur outside the timed phases. Only report file ordering and creation timestamps are normalized for comparison with HEAD.

The matrix contains both engines, many-small and few-large shapes, extraction and injection, and HEAD serial plus candidate workers 1/2/4/8. Ren'Py many-small has 2,000 scripts with four dialogue lines each; RPG Maker many-small has 1,000 maps with four message blocks each. Both few-large shapes have four files with 10,000 dialogue lines/message blocks each. RPG Maker fixtures also contain a small `System.json`.

Every sample records aggregate CPU busy time using Windows `GetSystemTimes`, process CPU time using `GetProcessTimes`, and estimated external CPU activity. Metadata records the CPU model, logical processors, OS, compiler, UTC start, target directory, HEAD/tree identity, and source digests. A compiled source manifest guard verifies that the executable matches the candidate and harness sources before execution; the normal finalizer also rejects evidence if candidate source digests changed during measurement. Sources were frozen during each measured matrix. The AboveNormal run captured and verified its process priority for the baseline and all candidate samples; no other job's priority was changed.

Spread is `100 * (maximum - minimum) / median`. A shape is rerun in full if any phase/configuration exceeds 15%; the latest whole-shape attempt replaces all earlier buckets for that shape. Quiet buckets are not cherry-picked across attempts. Raw attempts are retained. Remaining noisy rows produce an explicit unresolved-noise result and cannot justify a default change.

## Release timing matrix: diagnostic, not accepted

Machine: AMD Ryzen 7 7700X, 16 logical processors, Windows 11 build 26100. Compiler: `rustc 1.94.0 (4a4ef493e 2026-03-02)`. The Normal-priority run started at `2026-10-28T21:38:25.339777+00:00`; the AboveNormal-priority run started at `2026-10-28T21:52:00.607225+00:00`.

Two complete standard-launch matrices (nominal Normal priority, not runtime-verified) were followed by two complete AboveNormal-priority matrices. Every matrix covered 40 buckets, each with five measured samples after warmup. External CPU activity remained high. In the latest complete AboveNormal attempt, CPU busy time had a 100% median (range 51.5% to 100%); estimated external CPU had a 93.75% median (range 42.4% to 99.0%). Raising only the benchmark's priority did not produce acceptable stability: **29 of 40 buckets still exceeded 15% spread**.

The session stopped its own benchmark after those four full matrices. Raw files retain partial third attempts, which are excluded from the diagnostic summaries. This was a cancellation of the session's benchmark process; other jobs were left running. The runner's completed-run acceptance/finalization path was not reached, so these are explicitly diagnostic artifacts.

The table shows the latest complete AboveNormal attempt (attempt 2): **median milliseconds (spread percent)**, with five runs in every cell. Spread is `(max - min) / median`, not a confidence interval. Minimum/maximum times and per-bucket CPU/load details are retained in the linked JSON/Markdown diagnostics.

| Engine | Shape | Phase | HEAD serial | Worker 1 | Worker 2 | Worker 4 | Worker 8 |
|---|---|---|---|---|---|---|---|
| Ren'Py | Many small | Extract | 455.5 (46.0%) | 324.4 (33.2%) | 357.8 (82.9%) | 329.2 (200.1%) | 437.6 (333.9%) |
| Ren'Py | Many small | Inject | 1649.2 (40.7%) | 1622.9 (11.1%) | 1663.1 (7.4%) | 1601.8 (37.2%) | 1611.7 (8.5%) |
| Ren'Py | Few large | Extract | 80.5 (45.8%) | 68.8 (47.1%) | 44.6 (31.3%) | 33.5 (19.6%) | 41.2 (145.6%) |
| Ren'Py | Few large | Inject | 261.4 (20.4%) | 235.2 (21.3%) | 191.0 (4.3%) | 162.7 (19.1%) | 151.7 (24.2%) |
| RPG Maker | Many small | Extract | 182.6 (28.4%) | 174.8 (114.3%) | 180.4 (166.4%) | 174.6 (148.6%) | 177.9 (14.4%) |
| RPG Maker | Many small | Inject | 975.5 (5.9%) | 948.8 (16.7%) | 988.0 (6.4%) | 958.0 (9.6%) | 1029.1 (12.7%) |
| RPG Maker | Few large | Extract | 312.8 (38.2%) | 286.1 (60.7%) | 152.1 (151.6%) | 99.6 (25.2%) | 95.2 (32.7%) |
| RPG Maker | Few large | Inject | 742.6 (5.4%) | 769.7 (10.7%) | 394.3 (52.6%) | 227.1 (17.3%) | 254.4 (49.5%) |

Diagnostics: [AboveNormal Markdown](w3-03-release-above-normal.md), [AboveNormal JSON](w3-03-release-above-normal.json), [AboveNormal raw attempts](w3-03-release-above-normal.jsonl), [Normal Markdown](w3-03-release-normal-diagnostic.md), [Normal JSON](w3-03-release-normal-diagnostic.json), and [Normal raw attempts](w3-03-release.jsonl).

The measured AboveNormal source snapshot is preserved under `.bench-w3/above-normal-source` and verified against the run's source metadata. After the benchmark stopped, the final source changed its automatic fallback to one worker and removed an unnecessary RPG Maker grouping-key allocation per entry by cloning a key only when its group is first seen. Every candidate matrix cell explicitly sets `LOCUST_FILE_WORKERS` to 1/2/4/8, so the fallback change does not alter those explicit dispatch choices; the allocation cleanup does change the final injection implementation. The diagnostic table therefore describes the preserved measured candidate. **The final source has no accepted performance evidence**, and neither its default policy nor its grouping cleanup was timed in this matrix.

Outstanding requirement: establish one-worker baseline parity in a quiet release run with spread at most 15%, then consider automatic parallel phases only where median gains are at least 15%. **One-worker parity remains UNPROVEN; all current timings are diagnostic.**

When external CPU contention subsides, rerun with a new output prefix so the retained diagnostics are preserved. The default process priority is Normal; this command rebuilds and measures the final source in release mode:

```powershell
python scripts/bench_w3_release.py --runs 5 --output docs/performance/w3-03-release-quiet
```

## Determinism and structural tests

All eight preexisting functional worker/determinism tests and the ignored legacy synthetic benchmark are retained. The functional cases cover equivalent extraction and injection, corrupt-file diagnostics and serial write prefixes, revision output and immutable originals, worker errors and panics, hard-link aliases, compressed/BOM pristine restoration, loose/compiled twins and duplicate basenames, and preparation overlap/bounds.

Original small fixtures remain, with additional 2,000-line variants that cross the explicit worker thresholds. Plain Ren'Py and RPG Maker large fixtures exercise parallel extraction and injection. Compressed large variants exercise the injection entry threshold even when compression reduces their extraction byte size.

Six new tests cover structural policy and proof ordering:

- `default_policy_keeps_small_files_on_the_original_serial_loops`: all phases reject small shapes; count limits use panic probes to prove that many-small shapes avoid the lazy shape scan.
- `default_policy_does_not_enable_unproven_large_file_phases`: the automatic worker count is one and all phases reject even large shapes without invoking a preparation probe.
- `default_thresholds_require_the_measured_large_file_shape`: pins conservative automatic thresholds and the lower explicit worker thresholds.
- `one_worker_and_single_files_skip_even_the_shape_probe`: panic probes prove one worker and fewer than two files skip shape inspection.
- `explicit_workers_exercise_the_parallel_large_file_path`: confirms explicit 2/4/8-worker large shapes select candidate parallel preparation.
- `revision_proofs_observe_preceding_coordinator_writes`: two large maps; a later revision proof captures the earlier live target directly. Serial and 2/4/8-worker runs must return the same invalidated-proof error and identical BLAKE3 write prefix. Only the preceding target may change.

The proof-order regression uses a direct path alias so it remains testable in this sandbox. The original hard-link test remains active and unchanged in intent.

## Checks and sandbox fixture failures

The unfiltered formats suite encountered four sandbox fixture-creation failures. An earlier verification invocation excluded exactly those four names; the final full `cargo test -p locust-formats --no-fail-fast` run included them all and reproduced only these failures. All four tests remain active in the suite:

| Test | Fixture operation denied |
|---|---|
| `html_game::tests::injection_rejects_external_html_and_directory_links` | PowerShell `New-Item -ItemType Junction` |
| `vntextpatch::tests::vntp_preflight_allows_explicit_root_alias` | PowerShell `New-Item -ItemType Junction` |
| `vntextpatch::tests::vntp_preflight_rejects_interior_reparse_targets_and_preserves_sentinels` | PowerShell `New-Item -ItemType Junction` |
| `aliased_files_observe_preceding_coordinator_writes` | Rust `fs::hard_link` |

All four failures are Windows access-denied/code-5 fixture restrictions. The hard-link failure was independently reproduced with the existing test binary and occurs before extraction or injection runs. These four cases still require execution in an environment permitting junction and hard-link creation.

| Required check | Observed result |
|---|---|
| `cargo test -p locust-formats --no-fail-fast` | 1,231 passed, 4 sandbox fixture failures named above, 19 ignored |
| `cargo test -p locust-cli --test inject_json` | 7 passed |
| `cargo fmt --all -- --check` | Passed |
| `cargo clippy --workspace --all-targets -- -D warnings` | Passed |

The full formats run covers the final automatic-one-worker fallback and its new structural assertion. Its aggregate was verified from every test-result row in `.bench-w3/formats-final-policy-check.log`. After the subsequent grouping allocation cleanup, `extraction_and_injection_are_identical` passed against the final source, including extraction/report equivalence and BLAKE3 output trees, in `.bench-w3/grouping-final-check-rerun.log`. The final source also passed all seven CLI tests in `.bench-w3/cli-final-grouping-check.log`, strict workspace Clippy in `.bench-w3/clippy-final-grouping-check.log`, formatting, and `git diff --check`. This session did not run builds or checks concurrently with its measured matrices; external activity is recorded in the timing samples.

## Files and line-count snapshot

Whole-file line counts below describe the final workspace snapshot. Added/deleted tracked-file counts are relative to HEAD and include the retained round-1 work.

| File | Total lines | Added/deleted versus HEAD |
|---|---:|---:|
| `crates/formats/src/lib.rs` | 66 | +1 / -0 |
| `crates/formats/src/file_workers.rs` | 120 | New file |
| `crates/formats/src/renpy.rs` | 6,950 | +287 / -76 |
| `crates/formats/src/rpgmaker_mv.rs` | 4,476 | +490 / -120 |
| `crates/formats/tests/w3_parallel_files.rs` | 798 | New file |
| `scripts/bench_w3_release.py` | 266 | New file |
| `scripts/bench_w3_release.rs` | 396 | New file |
| `docs/performance/w3-03-correction-round-2.md` | 138 | New file |
| `docs/performance/w3-03-release-above-normal.md` | 70 | New file |
| `docs/performance/w3-03-release-above-normal.json` | 688 | New file |
| `docs/performance/w3-03-release-above-normal.jsonl` | 419 | New file |
| `docs/performance/w3-03-release-normal-diagnostic.md` | 54 | New file |
| `docs/performance/w3-03-release-normal-diagnostic.json` | 677 | New file |
| `docs/performance/w3-03-release.jsonl` | 441 | New file |
| `docs/performance/w3-03-host-load.jsonl` | 1 | New file |
| `.bench-w3/.gitignore` | 2 | New generated-input guard |

Generated `.bench-w3` build inputs are excluded from these counts. Diagnostic artifacts are retained above; the performance acceptance work remains open.
