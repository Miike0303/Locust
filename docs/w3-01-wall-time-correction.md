W3-01 correction round 2 — pipeline wall time

The matched mocked comparison is faster on the branch. The reported 922 s versus
233 s real-suite regression cannot be attributed conclusively inside this
sandbox; the parent must rerun with matched binaries. No real CLI timing is
claimed by this report. Existing uncommitted W3-01 changes remain in place.

Confirmed findings and fixes:

- `tools/test_ja_en_es_pipeline.py:19` changed executable selection from the
  checkout's `target/debug` to `CARGO_TARGET_DIR/debug`. The named production
  test body is unchanged relative to HEAD (`aa684be`). An older base executable
  is therefore not sufficient for a controlled comparison. Every command hashes
  the complete CLI at `tools/ja_en_es_pipeline.py:1423`, so executable size also
  affects time outside the recorded child duration. This is a confirmed
  comparison confound, not proof that it explains the entire real regression.
- `tools/ja_en_es_pipeline.py:1447`: the final owned-tree safety walk previously
  hashed every regular file and discarded every digest. It reread copies already
  checked above it, plus growing databases, reports, catalogs and logs, before
  every child. It now checks the entire tree structure without reading unused
  content. Link/reparse, hardlink, special-file, sensitive-name, ancestor and
  directory checks remain. Source, pristine and pre-injection copy hashes, CLI
  hashes, bound-input proofs, database checks and publication checks remain.
- `crates/core/src/glossary.rs:468` and `:520`: standalone hint/exact queries
  rebuilt every prepared map and both search tries per call. They now scan once;
  the translation manager still prepares and shares indexes once per job.
- `tools/ja_en_es_pipeline.py:1484`: empty phases no longer materialize a
  resource snapshot that cannot be used. Nonempty import phases still compare
  complete resource rows/metadata before and after, verify the exact glossary,
  and verify the bound import payload immediately before launch.

The Rust import at `crates/cli/src/commands/io.rs:24` parses before opening the
destination, then uses `database/glossary.rs:30`'s existing atomic indexed merge.
It does not validate every project string. Database opening initializes schema
and indexes backup provenance; it does not introduce a full string validation
loop. The named fixture imports only two terms per language. No new
seconds-scale Rust wait or project-row validation loop was found. Existing
provider retries have second-scale delays (`translation/retry.rs:26`); matching
the Rust revisions and comparing child durations distinguishes these from
Python invariant overhead.

Timing and fair reproduction:

`tools/benchmark_pipeline_wall_time.py` loads HEAD and working Python source in
memory, uses the same current test module for both, records source/test/binary
SHA-256 and sizes, and alternates AB/BA order. No checkout or target-directory
change is needed. First pass the same freshly built branch executable for both
CLI arguments to isolate Python. Then pass preserved base/branch executables
built with identical toolchain, features and profile to measure the whole change.

```powershell
python tools/benchmark_pipeline_wall_time.py --mock --repeats 3 --binary-mib 64
python tools/benchmark_pipeline_wall_time.py --base-cli C:/binaries/branch/locust.exe --branch-cli C:/binaries/branch/locust.exe --repeats 3
python tools/benchmark_pipeline_wall_time.py --base-cli C:/binaries/base/locust.exe --branch-cli C:/binaries/branch/locust.exe --repeats 3 --test ProductionTransportTests
```

The helper captures inclusive `command`, `invariants`, and `install_glossary`
times for both revisions; branch component records identify executable hashing,
tree checks, publication, payload proof, child execution and post-import checks.
For a direct pipeline invocation, `LOCUST_PIPELINE_TIMINGS=1` enables these JSON
records on stderr. Normal invocations retain empty stderr and unchanged manifest
receipt schema. The comparison helper intercepts only timing records so tests
that assert empty stderr remain valid.

Final mocked evidence (64 MiB identical fake executables; real Python filesystem,
hashing, invariant and manifest-publication work; CLI and database reads mocked):

| Metric | HEAD | Branch |
| --- | ---: | ---: |
| Wall samples, seconds | 6.006650 / 5.558468 / 7.226500 | 6.089159 / 3.959192 / 3.773511 |
| Median wall, seconds | 6.006650 | 3.959192 |
| Median inclusive glossary installation, seconds, both phases | 1.566534 | 0.543814 |
| Child commands | 14 | 12 |

Raw identities and per-operation/component samples are retained in
`docs/w3-01-wall-time-evidence.jsonl`. Samples were taken under concurrent checks
and include fixture creation/cleanup; they are descriptive, not a guarantee of
real CLI wall time. Executable hashing alone consumed 1.383–2.373 s per branch
mock run. A separate synthetic 128 MiB owned-tree probe measured median 186 ms
with discarded hashing versus 4 ms with structural checks.

Expected effect: two fewer child starts and invariant passes for the named
fixture; no redundant owned-tree byte reads before any command; no unnecessary
empty-phase snapshot; no index construction for standalone glossary queries.
The W3-01 1,000-term count benchmark still reduces subprocesses/invariant passes
from 1,000 to 1 and database reads from 1,001 to 2. Job-local matching and guarded
exact-hit batch transactions are retained. Real-suite acceptance remains the
parent's matched CLI rerun.

Validation performed:

- Mocked pipeline suite: 59 tests run, 2 platform skips, no failures (70.738 s).
- Opt-in 100k-row/1k-term orchestration benchmark passed; its database and safety
  scans are mocked and its scope is now explicit.
- Python compilation, `cargo fmt --all -- --check`, and `git diff --check` passed.
- `cargo test -p locust-core`, `cargo test -p locust-cli`, and
  `cargo clippy --workspace --all-targets -- -D warnings` were attempted. All stop
  before compilation because the configured target's parent
  `C:\Users\Mike\AppData\Local\locust-shared-target` is absent and creation is
  denied (OS error 5). `CARGO_TARGET_DIR` was never changed and no alternate
  target directory was created.

Files touched in this correction, with cumulative worktree line changes versus
HEAD (these counts include preserved earlier W3-01 work):

| File | Added | Removed |
| --- | ---: | ---: |
| `crates/core/src/glossary.rs` | 510 | 33 |
| `tools/ja_en_es_pipeline.py` | 123 | 65 |
| `tools/test_ja_en_es_pipeline.py` | 184 | 8 |
| `tools/ja_en_es_pipeline.md` | 60 | 4 |
| `tools/benchmark_pipeline_wall_time.py` (new) | 192 | 0 |
| `tools/test_glossary_batch_benchmark.py` (existing untracked) | 95 | 0 |

This report and the raw evidence are additional new documentation files. Earlier
CLI, database, translation-manager and batch-test edits were preserved. No
commit, push, branch switch, stash or reset was performed.
