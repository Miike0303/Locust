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
