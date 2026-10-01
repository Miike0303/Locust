# Ground-truth audits

Scripts that measure Locust's extractors against data a third party produced, instead of against our own tests. They found the largest defects of the 2026-09-29 goal session (Ren'Py recall 65.96% -> 97.92%, precision 88.15% -> 98.45%).

The idea: find a real artifact that lists what the engine itself considers translatable, then compute recall (what Locust finds) and precision (what Locust extracts that is not text).

## Ren'Py (`renpy/`)

Ground truth: the `game/tl/<lang>/*.rpy` files that Ren'Py itself writes (`translate <lang> <label>_<hash>:` blocks with the original statement in a `# ...` comment). 5,382 such files, 1.65 million dialogue blocks, exist under `D:\juegos\renpy` (this machine only).

Facts derived from that data (all verified, all now implemented in `crates/formats/src/renpy.rs`):

- Translation id = `<label>_<md5(canonical say statement + "\r\n")[:8]>`, repeats get `_1`, `_2`... numbered over the whole game and language in source path then script line order, and untranslated repeats still reserve their number (full-id agreement 99.47%).
- Menu branches, conditional choices (`"Text" if cond:`), python block headers, markup/escapes that look like paths and typed compiled-script (`.rpyc`) fields all matter for recall/precision.

### Run it

These scripts write large outputs (databases, jsonl) next to themselves, so copy the directory to a scratch location first:

```powershell
Copy-Item -Recurse C:\Projects\Locust\tools\ground-truth\renpy $env:TEMP\ground
cd C:\Projects\Locust; cargo build -p locust-cli          # target\debug\locust.exe
cd $env:TEMP\ground; python run_all.py
```

`run_all.py` runs, in order: `inventory.py` (finds Ren'Py games with tl directories), `extract.py` (runs `locust extract` on each), `audit.py`, `ast_audit.py`, `menu_audit.py`, `classify.py`, `classify_extra.py`, `report.py`, then `regression.py`. `report.txt` has the per-game table and the class counts; `evidence.txt` has three real `path:line` examples per class. The final guard message "Expected the menu regression assertion to fail" is stale since cycle 96 (the regression now passes); read `report.txt` regardless.

Latest numbers (cycle 99, 16 games, 580,543 unique tl statements): recall 97.92%, precision-ish 98.45%, no confirmed false-positive class left. See `last-report.txt`, `last-evidence.txt`, `cycle96-verification.md`, `cycle98-verification.md`.

Paths are hard-coded to this machine: `C:\Projects\Locust\target\debug\locust.exe` (`extract.py`, `regression.py`) and `D:\juegos\renpy` (`inventory.py`, `report.py`). Never modify anything under `D:\juegos`; always use `LOCUST_DATA_DIR=<scratch>` for locust runs (the scripts do).

## Rules that made this work

- Measure before writing a brief; give the writer the measuring script and ask for BEFORE/AFTER numbers.
- Claude re-runs the audit himself before accepting a writer's numbers.
- A researcher must not modify the repo or `D:\juegos`; check `git status --short` afterwards.
- Write briefs with a file tool, not a shell heredoc: an unbalanced backtick or quote in a briefing broke the shell twice.

## RPG Maker (`rpgm/`)

Original-vs-translated pairs plus an independent JSON parse over the deployed MV/MZ games in `D:\juegos\rpgm`. Drove cycles 101-105 (encoding, speaker names, D_TEXT, non-CJK plugin text, blank lines). Latest numbers: pair visible recall 100%, 0 unexpected injected-copy changes, 0 source-value mismatches. `verify_additional.py` checks raw layout (blank lines, scroll groups) after inject.

## Unity (`unity/`, added 2026-09-30)

Ground truth: the game-authored script lines of Out of Touch (`SCRIPTS~`, speaker set from `Characters.txt`) and the serialized text fields read by **UnityPy 1.25.2 + TypeTreeGeneratorAPI 0.0.10** using each game's shipped `Managed/*.dll` schemas (CCTV, CCTV-USSR, Sunkissed, BOXMAN, Out of Touch under `D:\juegos\unity`). The venv is NOT in the repo: `bootstrap.py` / `install_parser.py` recreate it in scratch. Run `py -3.13 run_all.py --live-unity` from a scratch copy (paths are hard-coded to this machine; `isolate_cli.py` redirects the patch-lock directory into scratch for audit CLIs). `last-report.md` is the first audit (before cycles 107-112); `writer-0N-*.md` are the five writer briefs it produced (all five are merged: cycles 107, 108, 110, 109, 112). Combined recall went 89.26% -> 97.84% (+typed fields 99.99% classified precision), confirmed false positives 792 -> 6.

## Visual-novel engines (`vn/`, added 2026-09-30)

KiriKiri/KAG, YU-RIS, TyranoBuilder, NScripter. Ground truth: the KAG runtime grammar parsed by an independent reader (`readers.py`), the real `ysc.ybn` opcode table with an independent YSTB reader, the engine author's public Tyrano sample scripts and a public Japanese NScripter demo (URLs and SHA256 in the report; fetched into scratch by `fetch_specs.py`). `last-report.md` has the table of misses/false positives with real examples; `regressions/` has minimal failing fixtures; `writer-01..04-*.md` are the writer briefs. Only `writer-01-kirikiri.md` is merged (cycle 111); `writer-02-yuris.md`, `writer-03-tyrano.md` and `writer-04-nscripter.md` are still to do (see `docs/HANDOFF-2026-09-30.md`).

Lessons that cost time on 2026-09-30: never put a literal stop phrase in a brief that a researcher may grep out of other briefs; tell researchers not to search the repo's `tmp/`; keep at most 2 Codex sessions at once and do not run your own cargo builds beside them.
