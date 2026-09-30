# Cycle 96 verification

Only `C:\Projects\Locust\crates\formats\src\renpy.rs` was edited in the repository. No commit, Cargo/dependency changes, desktop source edits, or writes under `D:\juegos`.

The extractor now keeps a stack of actual menu-header and choice indents. Tabs and spaces use the existing Python tracker's one-byte convention. Blank/comment lines leave scopes intact. Choice-level headers remain menu strings; deeper branch statements use normal extraction, including dialogue context and label metadata. Nested menus restore their enclosing scope. Conditional/argument choices extract just their literal, with balanced expression delimiters and a required terminating colon.

## Final rebuilt CLI audit: 16 games

The audit denominator remains 686,682 raw statements / 580,543 unique statements.

| Metric | Before | After |
| --- | ---: | ---: |
| Overall recall | 65.96% (382,932/580,543) | 95.83% (556,344/580,543) |
| Precision-ish | 88.15% (369,297/418,947) | 91.51% (535,816/585,520) |
| Missed while menu flag remains active, as classified by the audit's old tracer | 165,910 | 1,529 |
| Supplemental menu-choice misses | 924 (917 conditional, 7 arguments) | 0 |
| Loose visible markup/text mistaken for paths | 1,573 | 1,760 |
| Loose escaped no-space text mistaken for paths | 21 | 21 |
| Confirmed false positives: compiled code/expression/locator | 35,055 | 35,055 |
| Confirmed false positives: loose Python literals | 265 | 265 |
| Total confirmed false positives | 35,320 | 35,320 |

Precision-ish improved by 3.36 percentage points. All 16 extractions exited 0. The audit's tracer still models the old menu boolean: all 1,529 remaining rows in its menu class were inspected against raw current literals. 1,528 are rejected by the deliberately unchanged path heuristic; one is empty. They are not still gated by menu state. The separately classified markup/path count rose because recovered branch narration now follows the normal say extractor and its existing path heuristic.

`python run_all.py` was rerun against the final rebuilt binary. `report.txt` and `totals.json` were read. The runner exits 1 solely because its final guard expects the regression to fail, while it now passes. Its generated narrative paragraphs are hardcoded baseline descriptions; the dynamic numbers above are the final measurements.

## Caption decision

The corpus check found 60 bare choice-level captions, zero dialogue translate blocks referencing the caption line, and 45 captions with `old`/`new` strings blocks referencing that caption or its menu header. Same-text dialogue blocks elsewhere were inspected and belong to ordinary narration outside the menu. Caption strings therefore keep their existing `menu` tag and go to strings blocks in Add mode.

Examples: CelebrityHunter `game/script.rpy:50` is translated as an `old` string in `game/tl/Paloslios/CG7.rpy:15` with source reference `script.rpy:48` (the menu header). Eimis `game/script.rpy:2069` similarly has an `old` string at `game/tl/Paloslios/CG7.rpy:111` referencing menu line 2067. See `cycle96-caption-evidence.json`.

## Tests and gates

- Five new regression tests pass, covering the minimal repro and Add blocks, nested/control-flow menus, tabs and custom indents, adjacent menus, conditional/argument choices and rejected non-choices, Python skipping, blank/comment lines and branch underscore calls.
- Negative test restored the original `extract_content` (including its old fixed-indent exit and say gate), retaining the new tests. Output: `test result: FAILED. 0 passed; 5 failed; 0 ignored`; Cargo exit 101. The fixed source was restored byte-for-byte. See `cycle96-negative-test.log`.
- `python regression.py`: PASS, exit 0, all four say lines tagged dialogue with `label=start` and the expected character/narrator context; choices and caption retain the menu tag.
- `cargo build -p locust-cli`: PASS.
- `cargo test --workspace`: PASS on the final source. Existing no-menu extraction/order/translation-ID tests pass.
- `cargo clippy --workspace --all-targets -- -D warnings`: PASS.
- `cargo fmt --all --check`: PASS.
- `git diff --check`: PASS.
- Final `git status --short`: only ` M crates/formats/src/renpy.rs`.

Hash/ID functions, compiled pickle extraction and path-heuristic code were not changed. Frontend gates were skipped as requested.
