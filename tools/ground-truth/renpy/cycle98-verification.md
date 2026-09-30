# Cycle 98: typed compiled RenPy extraction verification

Final CLI rebuilt with `cargo build -p locust-cli`; `python run_all.py` rerun on all 16 games. All 16 extraction commands exited 0. The runner exits 1 only at its historical menu-regression guard, which expects a regression that the current code already fixed. All audit stages and report generation completed.

| Metric | Before (cycle 97) | After |
|---|---:|---:|
| Recall | 96.4202% (559,761/580,543) | 97.9245% (568,494/580,543) |
| Precision-ish | 91.5589% (539,131/588,835) | 98.4063% (545,171/554,000) |
| Confirmed FP flags in existing audit | 35,320 | 270 |
| compiled code/expression or locator | 35,055 | 5 |
| compiled other AST fields (unresolved) | 6,318 | 1 |
| compiled string not mapped (unresolved) | 636 | 1,127 |
| Compiled Say.what misses | 8,740 | 0 |

Compiled code/locator flags fell 99.9857%, exceeding the 80% gate. Recall increased and no game using compiled dialogue regressed.

The three compiled-dialogue games still have 4/1/1 loose setup files respectively. Their dialogue scripts are compiled:

| Game | TL total | Matched before -> after | Recall before -> after | Precision-ish before -> after | Extracted rows before -> after |
|---|---:|---:|---:|---:|---:|
| Area69-0.83-pc | 13,317 | 12,807 -> 13,317 | 96.1703% -> 100.0000% | 29.5922% -> 93.1247% | 37,324 -> 12,145 |
| Lust-Academy-0.7.1f-pc | 24,512 | 18,103 -> 24,500 | 73.8536% -> 99.9510% | 69.2104% -> 99.2663% | 24,544 -> 22,217 |
| NewFamily-pc | 16,467 | 14,639 -> 16,465 | 88.8990% -> 99.9879% | 63.3973% -> 98.5681% | 22,553 -> 15,224 |

The existing audit does not decode screen expressions when mapping strings. Supplemental AST provenance proves all 1,127 unmapped rows are literal text in typed screen fields; the five remaining compiled code flags are real UI labels/buttons H, S, hide, expression and back, coinciding with non-text fields elsewhere. The remaining other-field row is a visible x button. See `cycle98-screen-provenance.json` and `cycle98-remaining-flags-provenance.json`. The 265 loose Python literal false positives remain outside this change. After correcting the five demonstrated classification collisions, actual confirmed FPs are 265, with zero confirmed compiled code rows.

Field evidence: 310 inertly decoded protocol-2 pickles, 54,296 Say nodes, 580 Menu nodes, 966 Text/116 label/393 textbutton displayables and 324 tooltip fields. Static literal occurrences are 786/108/268/263 respectively; TL old-strings prove 6/1/2/7 occurrences. All 17 quoted Say.who occurrences were absent from TL old-strings, so who expressions are excluded. Translate/TranslateString were not present in this sample; block/old support is covered synthetically.

Typed traversal keeps Say.what (including empty and exact whitespace), menu captions/choices, Translate blocks and TranslateString.old, screen text/label/textbutton positional literals and text/tooltip keywords. It follows only structural blocks/branches/children and excludes Python/code, Define/Default, Image/ATL, raw UserStatement source, actions, conditions, filenames, and locators. Literal screen expressions are decoded without execution. No typed-reader fallback occurred in the real-game extraction logs.

Limits: 64 MiB inflation/input/string/output ceiling, 4,000,000 opcodes and edges, 1,000,000 nodes/memo entries, 32,768 stack entries, 256 graph depth. Arena IDs preserve memo identity/cycles without recursive destruction. Unknown opcodes, truncation and limit hits warn and use the conservative per-file string fallback. Shared PyExpr sources are charged/decoded once.

Validation: `cargo test --workspace`: 1,733 passed, 0 failed, 18 ignored. `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --all --check`, `cargo build -p locust-cli`, and `git diff --check` passed. Frontend gates skipped as requested.

The synthetic protocol-2 regression covers Say/Menu/screens, translation blocks, code/image/path/identifier rejection, short/markup text, exact whitespace/backslashes/quotes, memo aliases and slot state. Every prefix of the fixture, random garbage, giant claimed lengths, sparse memo IDs, cyclic containers, and stack/depth/op/node/string/edge/output limits are checked. Existing decompression-bomb and injection tests pass.

Negative test: temporarily restored the untyped old predicate; cargo exited 101, with 4 passed and 3 failed. Failures: typed_visible_fields, conservative_fallback, memo_identity_and_slot_state. The typed implementation was restored and its tests passed. Full output is `cycle98-negative-test.log`.

No commits, no dependency/Cargo changes, no frontend edits, and no writes to D:\juegos. Git status:

```text
 M crates/formats/src/lib.rs
 M crates/formats/src/renpy.rs
?? crates/formats/src/renpy_pickle.rs
```

Raw results: `report.txt`, `totals.json`, `summary.json`, `ast-summary.json`, `cycle98-comparison.json`, `cycle98-run-all.log`, `cycle98-workspace-tests.log`, `cycle98-clippy.log`, and `cycle98-negative-test.log` in this directory.
