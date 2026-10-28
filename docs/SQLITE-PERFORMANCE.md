# SQLite editor string-page performance (NEXT-06)

## Decision

Restore the guarded count/page optimization. Only unfiltered and status-only
pages take it: an existing covering index counts matches, and the existing
primary-key index walks the requested page in ID order. Count, page and
hydration share a deferred read transaction; caller-owned transactions stay
open. All file/tag/search combinations keep the predecessor window SQL,
parameters, empty-page count fallback and transaction behavior.

The earlier rejection was incorrect: a timing difference on a guarded-out
search path does not establish a query regression. The corrected gate is
>=25% p95 improvement for optimized cases, and byte-identical SQL plus
identical plans and interleaved timings within observed noise for untouched
paths. Round-2 evidence below replaces the earlier rejection decision.

No schema or index changes are needed. `schema.rs` already supplies
`idx_strings_status` and `sqlite_autoindex_strings_1`. A file-backed existing
DB compatibility test checks both optimized paths and unchanged schema/index
definitions across two reopens. The original benchmark, 33,600-case parity
test and historical measurements are retained.

## Reproduce the delivered benchmark

From the worktree root, with the existing `CARGO_TARGET_DIR` unchanged:

```powershell
cargo test --release -p locust-core --lib benchmark_editor_string_pages -- --ignored --nocapture --test-threads=1
```

Round-2 search measurement uses three trials of 50 samples **per arm**, for
each search and size, inside one test process and the same seeded DB:

```powershell
$env:LOCUST_SQLITE_BENCH_RUNS = '50'
$env:LOCUST_SQLITE_BENCH_TRIALS = '3'
$env:LOCUST_SQLITE_BENCH_CASE = 'search_latin,search_cjk'
$env:LOCUST_SQLITE_BENCH_OPERATIONS = 'page'
cargo test --release -p locust-core --lib benchmark_editor_string_pages -- --ignored --nocapture --test-threads=1
Remove-Item Env:LOCUST_SQLITE_BENCH_RUNS, Env:LOCUST_SQLITE_BENCH_TRIALS, Env:LOCUST_SQLITE_BENCH_CASE, Env:LOCUST_SQLITE_BENCH_OPERATIONS
```

Optional process-local settings: `LOCUST_SQLITE_BENCH_RUNS` defaults to 30
and must be at least 20; `LOCUST_SQLITE_BENCH_TRIALS` defaults to one;
`LOCUST_SQLITE_BENCH_CASE` accepts comma-separated case names;
`LOCUST_SQLITE_BENCH_SIZES` defaults to `25000,100000`; and
`LOCUST_SQLITE_BENCH_OPERATIONS` defaults to `page,count`.
`LOCUST_SQLITE_BASELINE_ONLY=1` measures only the currently
compiled public API and duplicates its samples into both output column
pairs; it is not an independent comparison. Remove these variables to
restore the default full run.

Databases are created and automatically removed in a unique
`sqlite-latency-*` directory under the existing `CARGO_TARGET_DIR`; the
working directory is the fallback if the variable is absent. The benchmark
never changes that variable or creates a different Cargo target directory.

## Method and machine

- Measured on 2026-10-28 in the shared managed Windows workspace, build
  26100, AMD64, AMD Ryzen 7 7700X 8-Core Processor. CIM inspection was
  denied; no RAM capacity is assumed. This was not a dedicated benchmark
  host. Absolute latencies and tail variation should not be generalized
  to every user's hardware.
- Rust/Cargo 1.94.0, release optimization, bundled SQLite 3.45.0 through
  rusqlite 0.31. Normal `Database::open` settings: WAL, synchronous NORMAL,
  existing indexes, default SQLite cache and temporary-storage settings.
- File-backed projects contain exactly 25,000 and 100,000 rows across
  40 files. IDs use a fixed multiplicative permutation, independent of
  insertion order; timestamps are fixed. Status mix is approximately
  40/30/10/10/10 percent pending/translated/reviewed/approved/error.
- Sources alternate CJK and Latin; non-pending rows have Spanish
  translations. Each row has context, two tags, a character limit,
  provider/timestamps where applicable, and nested locator/notes metadata.
  Even-numbered files contain CJK and odd-numbered files Latin. Setup,
  insertion, checkpointing and database deletion are outside timed regions.
- Page size is 100, the editor route default. `first`, `offset50` and
  `offset95` use offsets 0, N/2 and 95N/100. Remaining cases use offset 0:
  translated status; chapter-07 file; both; Latin `captain`; CJK `冒険者`;
  translation-only `moonstone`; sparse `Traveller 9999,`; missing
  `not-in-seeded-project`; tag `ui`; and status+file+tag `dialogue`+search
  `captain`. The combined case's text matches all sources in that file;
  status and tag supply its selectivity. The API has no metadata predicate;
  realistic metadata is loaded by production hydration.
- Current-arm page calls use the exact editor API,
  `Database::get_entries_page`; the before arm uses the retained original
  window implementation with the same WHERE/pagination builders and row
  hydration. Timing includes returned-row destruction. Round-2 optimized
  calls include BEGIN/COMMIT overhead; untouched calls and the predecessor
  retain their original implicit transaction behavior.
- Counts call the unchanged public `Database::count_entries` in both arms
  with the same filters. Pagination does not affect totals. Count-column
  differences are host-noise controls, not an optimized count implementation.
- Five warmups per arm precede each trial. Each iteration runs old/new or
  new/old, reversing the order on every iteration and initial order on each
  trial, on the same database/connection. `Instant` samples are milliseconds;
  p50/p95 use nearest rank (p95 is sample 29 of 30 or 48 of 50). Warm means repeated
  workload and OS-cache reuse; it does not imply all rows fit SQLite's cache.
- Every case prints `EXPLAIN QUERY PLAN` with the exact bound parameters for
  the predecessor window, actual API statement and standalone count at
  both sizes. Untouched cases assert equal SQL bytes, parameters and every
  EQP column `(id, parent, unused, detail)` before timing. Output includes
  per-arm sample min/max, paired differences and trial-to-trial p50/p95 spans.

## Implementation and scope

Only filters with no file, tag or search predicate split the window query:
count first using the unchanged COUNT(*) helper, then select the requested
rows with `INDEXED BY sqlite_autoindex_strings_1` and `ORDER BY id`, without
COUNT(*) OVER(). Thus unfiltered and status-only
pages split; selective filters retained their window SQL and empty-page
fallback count. An owned deferred read transaction enclosed count, page
and hydration when the connection was in autocommit; an already-open
transaction retains ownership. Round 2 restricts the transaction wrapper to
the split branch. No schema change is required.

This guard followed an earlier exploratory blanket split that scanned
selective predicates twice and was rejected. No exploratory numbers are
used as shipping evidence here. The historical candidate below wrapped
both branches in a transaction and let SQLite choose the status page index;
round 2 measures the current guard and forced ID-index page walk directly.

## Round-2 optimized-path measurements

Thirty interleaved samples per arm, five warmups per arm, one trial, both
sizes in the same process. **Before / after cells show p50 / p95 ms.**
The current API includes the deferred transaction, covering count, forced
ID-index page walk, hydration and destruction. All eight cases clear the
>=25% p95 improvement gate (actual reductions 91.5%–99.6%). The benchmark
passed in 464.23 seconds.

| Rows | Case | Before p50 / p95 | Restored API p50 / p95 | p95 improvement |
| ---: | --- | ---: | ---: | ---: |
| 25000 | first | 78.200 / 91.726 | 2.083 / 3.541 | 96.1% |
| 25000 | offset50 | 462.499 / 835.930 | 3.991 / 10.778 | 98.7% |
| 25000 | offset95 | 626.848 / 686.744 | 5.319 / 10.886 | 98.4% |
| 25000 | status | 36.775 / 57.978 | 3.003 / 4.914 | 91.5% |
| 100000 | first | 752.243 / 1329.096 | 9.612 / 37.464 | 97.2% |
| 100000 | offset50 | 4666.279 / 6561.257 | 15.510 / 28.382 | 99.6% |
| 100000 | offset95 | 5073.158 / 7170.089 | 18.047 / 29.918 | 99.6% |
| 100000 | status | 209.083 / 294.838 | 5.539 / 10.836 | 96.3% |

Reproduce with the same command as the search run, changing these settings:

```powershell
$env:LOCUST_SQLITE_BENCH_RUNS = '30'
$env:LOCUST_SQLITE_BENCH_TRIALS = '1'
$env:LOCUST_SQLITE_BENCH_CASE = 'first,offset50,offset95,status'
$env:LOCUST_SQLITE_BENCH_OPERATIONS = 'page'
cargo test --release -p locust-core --lib benchmark_editor_string_pages -- --ignored --nocapture --test-threads=1
Remove-Item Env:LOCUST_SQLITE_BENCH_RUNS, Env:LOCUST_SQLITE_BENCH_TRIALS, Env:LOCUST_SQLITE_BENCH_CASE, Env:LOCUST_SQLITE_BENCH_OPERATIONS
```

## Round-2 interleaved search results

Three trials, 50 samples per arm per trial, same process and DB/connection
for each size; AB/BA order reverses each iteration. Each cell is **p50 / p95
milliseconds**, measured through full row hydration and destruction. These
are the current compiled API versus the retained predecessor, not separate
old/new databases or builds. All four SQL/parameter/full-EQP identity
assertions passed before timing. The benchmark passed in 291.34 seconds.

| Rows | Search | Trial | Before p50 / p95 | Restored API p50 / p95 |
| ---: | --- | ---: | ---: | ---: |
| 25000 | Latin | 1 | 74.005 / 120.396 | 74.890 / 112.121 |
| 25000 | Latin | 2 | 62.294 / 146.778 | 60.252 / 105.886 |
| 25000 | Latin | 3 | 61.264 / 90.446 | 59.516 / 82.035 |
| 25000 | CJK | 1 | 58.170 / 76.677 | 56.411 / 81.809 |
| 25000 | CJK | 2 | 73.712 / 143.835 | 74.720 / 124.945 |
| 25000 | CJK | 3 | 71.800 / 105.647 | 71.389 / 92.200 |
| 100000 | Latin | 1 | 496.346 / 1032.298 | 489.494 / 1290.926 |
| 100000 | Latin | 2 | 307.019 / 532.087 | 293.516 / 480.772 |
| 100000 | Latin | 3 | 328.980 / 391.794 | 316.101 / 429.491 |
| 100000 | CJK | 1 | 288.295 / 379.536 | 289.964 / 426.585 |
| 100000 | CJK | 2 | 289.290 / 377.510 | 303.704 / 391.993 |
| 100000 | CJK | 3 | 280.568 / 360.903 | 273.280 / 358.112 |

Run-to-run spread is the minimum through maximum of the three trial
percentiles, not a pooled percentile or a confidence interval:

| Rows | Search | Before p50 span | API p50 span | Before p95 span | API p95 span | Paired median delta span |
| ---: | --- | ---: | ---: | ---: | ---: | ---: |
| 25000 | Latin | 61.264–74.005 | 59.516–74.890 | 90.446–146.778 | 82.035–112.121 | -3.413% to +0.358% |
| 25000 | CJK | 58.170–73.712 | 56.411–74.720 | 76.677–143.835 | 81.809–124.945 | -2.477% to +0.177% |
| 100000 | Latin | 307.019–496.346 | 293.516–489.494 | 391.794–1032.298 | 429.491–1290.926 | -5.385% to -0.557% |
| 100000 | CJK | 280.568–289.290 | 273.280–303.704 | 360.903–379.536 | 358.112–426.585 | -1.386% to +3.837% |

Paired deltas compare the API and predecessor from the same iteration;
their median is calculated separately in each trial. No repeatable search
regression is observed. For example, CJK p95 changes by +5.132/-18.890/
-13.447 ms at 25k and +47.049/+14.483/-2.791 ms at 100k. At 100k CJK,
two API tails were higher, but the third reversed; paired medians ranged
from -1.386% to +3.837%, while API p95 varied 68.473 ms across trials.
These differences are compatible with the measured variability. At 100k Latin,
trial 1 alone would suggest a 25% p95 regression, despite a -4.571% paired
median; the next trial's p95 is lower in the API arm. With identical SQL,
parameters and plans, these measurements support host/cache variation,
not a causal query regression or a claimed search speedup. They do not
establish formal statistical equivalence.

## Historical round-1 measurements (retained evidence)

All values below are milliseconds, written as **p50 / p95**. Negative p95
change means faster. "Candidate" means the earlier measured implementation,
including both statements and transaction overhead where applicable.
Counts are repeated measurements of the same API in both columns.

### Full run: 30 paired samples

**25000 rows, page**

| Case | Before p50 / p95 | Candidate p50 / p95 | p95 change | Total |
| --- | ---: | ---: | ---: | ---: |
| first | 125.341 / 153.644 | 1.185 / 11.790 | -92.3% | 25000 |
| offset50 | 486.293 / 549.145 | 3.092 / 6.365 | -98.8% | 25000 |
| offset95 | 584.956 / 858.159 | 4.506 / 7.714 | -99.1% | 25000 |
| status | 30.532 / 44.736 | 13.377 / 20.803 | -53.5% | 7500 |
| file | 6.373 / 11.103 | 6.029 / 10.636 | -4.2% | 625 |
| status_file | 4.665 / 7.755 | 4.571 / 10.294 | +32.7% | 186 |
| search_latin | 60.232 / 86.860 | 61.268 / 93.464 | +7.6% | 12500 |
| search_cjk | 73.921 / 120.844 | 90.657 / 150.252 | +24.3% | 12500 |
| search_translation | 27.440 / 46.529 | 25.861 / 98.945 | +112.7% | 1174 |
| search_sparse | 23.855 / 39.040 | 24.750 / 38.520 | -1.3% | 1 |
| search_missing | 55.720 / 151.195 | 52.787 / 110.195 | -27.1% | 0 |
| tag | 39.942 / 52.373 | 39.430 / 53.723 | +2.6% | 8334 |
| combined | 5.611 / 8.771 | 6.320 / 14.738 | +68.0% | 124 |

**25000 rows, count**

| Case | Before p50 / p95 | Candidate p50 / p95 | p95 change | Total |
| --- | ---: | ---: | ---: | ---: |
| first | 0.269 / 0.393 | 0.274 / 0.304 | -22.6% | 25000 |
| offset50 | 0.267 / 0.746 | 0.269 / 0.388 | -48.0% | 25000 |
| offset95 | 0.255 / 0.291 | 0.254 / 0.287 | -1.4% | 25000 |
| status | 0.240 / 0.695 | 0.251 / 0.596 | -14.2% | 7500 |
| file | 0.026 / 0.041 | 0.026 / 0.047 | +14.6% | 625 |
| status_file | 3.031 / 4.148 | 2.938 / 4.746 | +14.4% | 186 |
| search_latin | 18.299 / 54.008 | 20.040 / 73.935 | +36.9% | 12500 |
| search_cjk | 19.606 / 50.839 | 18.619 / 27.394 | -46.1% | 12500 |
| search_translation | 18.564 / 40.697 | 21.105 / 49.844 | +22.5% | 1174 |
| search_sparse | 18.908 / 35.844 | 22.041 / 33.313 | -7.1% | 1 |
| search_missing | 21.051 / 31.523 | 20.295 / 32.483 | +3.0% | 0 |
| tag | 14.722 / 21.701 | 14.589 / 19.250 | -11.3% | 8334 |
| combined | 3.930 / 12.656 | 4.979 / 12.734 | +0.6% | 124 |

**100000 rows, page**

| Case | Before p50 / p95 | Candidate p50 / p95 | p95 change | Total |
| --- | ---: | ---: | ---: | ---: |
| first | 580.753 / 731.114 | 8.851 / 21.496 | -97.1% | 100000 |
| offset50 | 4041.187 / 6013.438 | 14.357 / 30.101 | -99.5% | 100000 |
| offset95 | 4308.496 / 6148.583 | 14.929 / 41.928 | -99.3% | 100000 |
| status | 257.915 / 435.080 | 92.202 / 186.559 | -57.1% | 30000 |
| file | 23.712 / 77.274 | 24.556 / 56.275 | -27.2% | 2500 |
| status_file | 21.059 / 44.290 | 22.573 / 54.242 | +22.5% | 750 |
| search_latin | 321.190 / 418.116 | 349.290 / 512.596 | +22.6% | 50000 |
| search_cjk | 358.459 / 413.808 | 368.118 / 469.453 | +13.4% | 50000 |
| search_translation | 117.563 / 157.044 | 120.438 / 163.494 | +4.1% | 4621 |
| search_sparse | 105.187 / 151.225 | 101.672 / 150.579 | -0.4% | 1 |
| search_missing | 256.251 / 711.262 | 260.798 / 521.332 | -26.7% | 0 |
| tag | 248.095 / 377.514 | 259.636 / 364.308 | -3.5% | 33334 |
| combined | 18.993 / 50.331 | 21.351 / 66.562 | +32.2% | 500 |

**100000 rows, count**

| Case | Before p50 / p95 | Candidate p50 / p95 | p95 change | Total |
| --- | ---: | ---: | ---: | ---: |
| first | 6.720 / 11.066 | 6.746 / 13.264 | +19.9% | 100000 |
| offset50 | 6.737 / 9.655 | 6.833 / 11.433 | +18.4% | 100000 |
| offset95 | 6.803 / 50.238 | 7.595 / 64.753 | +28.9% | 100000 |
| status | 0.985 / 11.917 | 1.030 / 16.990 | +42.6% | 30000 |
| file | 0.093 / 0.275 | 0.093 / 0.173 | -37.1% | 2500 |
| status_file | 20.839 / 70.212 | 18.383 / 75.612 | +7.7% | 750 |
| search_latin | 83.230 / 156.032 | 89.429 / 114.066 | -26.9% | 50000 |
| search_cjk | 87.278 / 112.891 | 83.952 / 130.995 | +16.0% | 50000 |
| search_translation | 89.712 / 117.863 | 87.381 / 125.583 | +6.5% | 4621 |
| search_sparse | 96.346 / 124.300 | 94.382 / 132.844 | +6.9% | 1 |
| search_missing | 112.562 / 222.914 | 117.167 / 216.363 | -2.9% | 0 |
| tag | 79.954 / 156.989 | 87.931 / 137.533 | -12.4% | 33334 |
| combined | 16.035 / 35.889 | 15.685 / 29.957 | -16.5% | 500 |

### Focused repeat: 50 paired samples

**25000 rows, page**

| Case | Before p50 / p95 | Candidate p50 / p95 | p95 change | Total |
| --- | ---: | ---: | ---: | ---: |
| first | 139.099 / 219.361 | 3.000 / 8.521 | -96.1% | 25000 |
| status_file | 4.855 / 15.858 | 5.403 / 17.126 | +8.0% | 186 |
| search_latin | 157.313 / 520.500 | 114.570 / 667.752 | +28.3% | 12500 |
| search_cjk | 91.356 / 137.445 | 93.455 / 152.618 | +11.0% | 12500 |
| search_translation | 27.223 / 36.789 | 26.126 / 40.072 | +8.9% | 1174 |
| combined | 4.055 / 13.558 | 4.174 / 9.677 | -28.6% | 124 |

**25000 rows, count**

| Case | Before p50 / p95 | Candidate p50 / p95 | p95 change | Total |
| --- | ---: | ---: | ---: | ---: |
| first | 0.266 / 0.620 | 0.273 / 0.805 | +29.8% | 25000 |
| status_file | 3.392 / 14.511 | 3.935 / 13.840 | -4.6% | 186 |
| search_latin | 22.908 / 39.625 | 21.723 / 30.563 | -22.9% | 12500 |
| search_cjk | 22.422 / 36.927 | 21.628 / 52.075 | +41.0% | 12500 |
| search_translation | 22.685 / 43.772 | 20.988 / 34.845 | -20.4% | 1174 |
| combined | 2.832 / 12.508 | 3.017 / 25.373 | +102.9% | 124 |

**100000 rows, page**

| Case | Before p50 / p95 | Candidate p50 / p95 | p95 change | Total |
| --- | ---: | ---: | ---: | ---: |
| first | 552.806 / 1069.685 | 9.902 / 32.781 | -96.9% | 100000 |
| status_file | 19.830 / 43.949 | 19.382 / 38.295 | -12.9% | 750 |
| search_latin | 353.392 / 469.663 | 357.069 / 482.230 | +2.7% | 50000 |
| search_cjk | 371.157 / 508.093 | 360.682 / 527.830 | +3.9% | 50000 |
| search_translation | 108.910 / 158.706 | 102.987 / 143.365 | -9.7% | 4621 |
| combined | 17.184 / 29.085 | 15.791 / 28.375 | -2.4% | 500 |

**100000 rows, count**

| Case | Before p50 / p95 | Candidate p50 / p95 | p95 change | Total |
| --- | ---: | ---: | ---: | ---: |
| first | 6.862 / 70.696 | 7.451 / 87.415 | +23.6% | 100000 |
| status_file | 17.215 / 31.511 | 16.105 / 35.358 | +12.2% | 750 |
| search_latin | 88.558 / 134.962 | 81.695 / 121.857 | -9.7% | 50000 |
| search_cjk | 86.329 / 137.037 | 86.836 / 109.447 | -20.1% | 50000 |
| search_translation | 91.046 / 157.548 | 90.717 / 128.468 | -18.5% | 4621 |
| combined | 13.387 / 26.517 | 14.018 / 20.579 | -22.4% | 500 |

## Query plans and SQL identity (round 2)

The current benchmark prints actual SQL and all EQP columns with the bound
parameters at both sizes. Unfiltered and status-only pages change; all
other filter combinations retain the predecessor query.

Unfiltered first/50%/95% pages previously used:

```text
CO-ROUTINE (subquery-2)
SCAN strings
SCAN (subquery-2)
USE TEMP B-TREE FOR ORDER BY
```

The restored unfiltered path uses two statements:

```text
count: SCAN strings USING COVERING INDEX idx_strings_file
page:  SCAN strings USING INDEX sqlite_autoindex_strings_1
```

Status-only previously used:

```text
CO-ROUTINE (subquery-2)
SEARCH strings USING INDEX idx_strings_status (status=?)
SCAN (subquery-2)
USE TEMP B-TREE FOR ORDER BY
```

The restored status-only path uses:

```text
count: SEARCH strings USING COVERING INDEX idx_strings_status (status=?)
page:  SCAN strings USING INDEX sqlite_autoindex_strings_1
```

Both optimized page types avoid window materialization and the temporary
sort. The historical candidate used `idx_strings_status` and still sorted
status pages; round 2 explicitly walks the existing ID index. Rare statuses
or late offsets can still require examining many IDs; this is not keyset
pagination. The measured status case is the 30% translated cohort.

`selective_page_sql_is_byte_identical_to_pre_next06` compares SQL bytes and
bindings with an independent frozen predecessor builder in 630 cases:
all 14 guarded-out filter-presence combinations, CJK/Latin/empty/wildcard/
quoted values, limits absent/zero/100 and offsets absent/zero/500.
`search_sql_and_full_query_plans_are_identical_with_metadata_rows` compares
every EQP column for CJK, Latin and metadata-only search values and checks
full hydrated-row parity. `EntryFilter` has no metadata-filter field;
metadata remains returned payload and does not become a search predicate.

Both CJK and Latin searches produced these exact tuples in both arms at
25k and 100k (including the numeric columns, not only the plan description):

```text
(3,   0, 0, "CO-ROUTINE (subquery-2)")
(6,   3, 0, "SCAN strings")
(42,  0, 0, "SCAN (subquery-2)")
(114, 0, 0, "USE TEMP B-TREE FOR ORDER BY")
```

All other page plans retained `CO-ROUTINE (subquery-2)`, their inner scan,
`SCAN (subquery-2)` and `USE TEMP B-TREE FOR ORDER BY`. Inner scans and the
standalone count plans (unchanged for every case) were:

| Cases, both sizes | Window inner scan | Standalone count |
| --- | --- | --- |
| first, offset50, offset95 | `SCAN strings` | `SCAN strings USING COVERING INDEX idx_strings_file` |
| status | `SEARCH strings USING INDEX idx_strings_status (status=?)` | `SEARCH strings USING COVERING INDEX idx_strings_status (status=?)` |
| file | `SEARCH strings USING INDEX idx_strings_file (file_path=?)` | `SEARCH strings USING COVERING INDEX idx_strings_file (file_path=?)` |
| status_file, combined | `SEARCH strings USING INDEX idx_strings_file (file_path=?)` | `SEARCH strings USING INDEX idx_strings_file (file_path=?)` |
| search_latin, search_cjk, search_translation, search_sparse, search_missing, tag | `SCAN strings` | `SCAN strings` |

`search_missing` returns no window rows and therefore also executes the
predecessor's fallback count. All selective window/count plans are retained.

## Parity and schema scope

The retained parity test compares complete serialized row vectors,
including row order, all hydrated fields and metadata, plus exact totals
against the predecessor oracle and public count API on a seeded project.
It exercises 33,600 combinations: six status choices, four files, five tags,
eight searches, seven offsets and five limits. Coverage includes every
filter-presence combination, all statuses, CJK/Latin/translation matches,
empty and wildcard searches, apostrophes, missing matches, zero limits,
offset-only pagination and offsets beyond the result set.

The parity test remains unchanged and passed against the restored optimized
API. New tests cover split query counts, covering-count and ID-walk plans,
transaction cleanup after hydration errors, caller transaction ownership,
and a pinned WAL snapshot across an independent writer's commit.

`schema.rs` was read: schema work runs in one IMMEDIATE transaction with
idempotent CREATE statements, column introspection and rollback on error.
No schema/index changes ship, so no migration or version bump is needed.
`guarded_entry_pages_reopen_existing_database_without_schema_changes`
opens a populated file-backed DB twice, asserts unchanged `schema_version`
and index definitions, and exercises unfiltered/status pages on each reopen.

## Round-2 validation

- Release optimized-path benchmark: passed, 30 interleaved samples per arm
  for all four cases at both sizes (464.23 s).
- Release CJK/Latin search benchmark: passed, three trials of 50 interleaved
  samples per arm at both sizes, 1,200 timed calls total (291.34 s).
- `cargo test -p locust-core`: 822 passed, 13 failed, 3 ignored (838 total).
  All 70 string tests, including the unchanged 33,600-comparison parity test
  and six new regression tests, passed; the benchmark remains ignored in
  normal test runs. All nine schema tests passed, including existing-DB
  migration preservation, second-open idempotence and rollback on failure.
- `cargo test -p locust-server`: 155 passed, 1 failed, 2 ignored; editor
  string-list/search/out-of-range-total tests passed.
- Both required test commands stop at the failed library suites. Supplemental
  `cargo test -p locust-core --test '*' --no-fail-fast` completed all 29 core
  integration targets: 332 passed, 4 failed, 1 ignored. All four failures
  explicitly report denied junction creation, listed below.
- `cargo test -p locust-server --test '*' --no-fail-fast` completed all 19
  server integration targets: 86 passed, no failures or ignored tests.
- `cargo test -p locust-core -p locust-server --doc`: passed; both crates
  contain zero doc tests. Across library/integration suites: 1,395 passed,
  18 sandbox-linked failures and 6 ignored tests.
- `cargo fmt --all -- --check`: passed.
- `cargo clippy -p locust-core -p locust-server --all-targets -- -D warnings`:
  passed.
- `git diff --check`: passed.

The library failure names match round 1. Five hard-link fixtures explicitly report
Windows Access Denied (code 5), and the extraction fixture reports denied
junction creation. The seven backup-deduplication failures are failed sharing
assertions, consistent with copy fallback when hard links are unavailable;
those seven panic messages do not themselves contain a permission error.
The dedicated link-failure/copy-fallback test passes. None exercises the
SQLite page query.
Core failure names:

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

Server failure name:

- `routes::backups::backup_operation_tests::backup_http_delete_preserves_other_shared_backup`

Supplemental core integration failure names (target followed by test name):

- `patch_apply::apply_and_rollback_reject_linked_game_directories`
- `patch_apply::selected_game_root_can_be_a_directory_link`
- `patch_scratch_ownership::link_inserted_at_write_boundary_is_rejected_before_asset_mutation`
- `patch_scratch_ownership::unrelated_scratch_links_are_not_followed_or_deleted`

All four panic during the fixture's `New-Item -ItemType Junction` with
`Acceso denegado` / `NewItemCreateIOError`, before product assertions.

The server assertion expected 8 deduplicated physical bytes and observed 16,
consistent with the core hard-link/deduplication failures. All 48 integration
targets were executed directly after the required commands stopped at their
library failures. The sandbox-linked failures were recorded without changing
unrelated code.

Round-2 stdout is retained under the unchanged `CARGO_TARGET_DIR`:
`sqlite-next06-r2-optimized.log`, `sqlite-next06-r2-search.log`,
`sqlite-next06-r2-core.log`, `sqlite-next06-r2-server.log`,
`sqlite-next06-r2-fmt.log`, `sqlite-next06-r2-clippy.log` and
`sqlite-next06-r2-diff-check.log`. Required command exit codes were
core=101, server=101, fmt=0, clippy=0 and diff-check=0.
Supplemental logs are `sqlite-next06-r2-core-integration.log`,
`sqlite-next06-r2-server-integration.log` and `sqlite-next06-r2-doc-tests.log`,
with exit codes 101, 0 and 0 respectively.

For historical comparison, round 1 had 816/13/3 core and 155/1/2 server
passed/failed/ignored, with the same sandbox failure names. Its full release
benchmark passed in 587.29 s, followed by six focused 50-sample repeats.
Historical full/repeat stdout is retained on this machine under the unchanged
`CARGO_TARGET_DIR` as `sqlite-next06-release.log` and
`sqlite-next06-repeat.log`; round-1 checks used `sqlite-next06-core-final.log`,
`sqlite-next06-server-final.log` and `sqlite-next06-clippy-final.log`.
