# Unity independent ground-truth audit

All final outputs, packages and builds live in this directory. No repo edits or game injection is permitted. The Python interpreter does not need global Unity packages.

Run everything from any directory:

```powershell
py -3.13 C:\Users\Mike\AppData\Local\Temp\locust-research-unity\run_all.py
```

Prerequisites: Python 3.13, Cargo/Rust toolchain, existing Cargo registry cache, and the local games under `D:\juegos\unity`. The first run downloads pinned independent parser wheels into a scratch-only environment. The installer redirects temporary files into `pip-temp/` and works around restrictive Windows temporary-directory creation. It does not install globally. `run_all.py` uses `--without-pip` for a new venv.

The default build archives the pinned commit in `snapshot-commit.txt` into `workspace/`, uses `--manifest-path`, `--locked --offline`, and sets `CARGO_TARGET_DIR=target/`. `--live-unity` starts from current committed HEAD and overlays only the stable Unity files. Missing third-party `.rlib/.rmeta/.lib/.d` artifacts are copied from the cycle101 cache into scratch; that cache is never written. Every Locust invocation sets `LOCUST_DATA_DIR=locust-data/`. Extraction reads originals; injection uses uniquely named scratch copies and fresh databases so previous injection manifests cannot interfere with reruns. Other sessions changed HEAD and began editing unity.rs before delivery; baseline line references concern the pinned commit, with archived source preserved for review.

Boundary disclosure: the initial uninstrumented CLI used standard persistent lock files outside this directory. Its lock namespace intentionally ignores temporary/profile overrides (`C:\Projects\Locust\crates\core\src\patch\lock.rs:30`). The final runner applies `isolate_cli.py` ONLY to the scratch core copy, adding the opt-in `LOCUST_RESEARCH_LOCK_DIR` redirect under `locks/`. Unity extraction/injection code is unchanged. Initial shell profile/pip setup also attempted default user/Temp locations and failed; see `provenance.json`. The strict write-only boundary was therefore not met by those initial runs; it is enforced for the final runner. No repo/game files were changed.

Useful rerun switches:

```powershell
# Stable writer edits: overlay only unity*.rs onto the committed scratch snapshot.
py -3.13 C:\Users\Mike\AppData\Local\Temp\locust-research-unity\run_all.py --live-unity
# Reuse existing extraction DBs; rebuild and repeat the independent comparisons/probes.
py -3.13 C:\Users\Mike\AppData\Local\Temp\locust-research-unity\run_all.py --reuse-extractions
# Reuse an already built scratch CLI and extraction DBs.
py -3.13 C:\Users\Mike\AppData\Local\Temp\locust-research-unity\run_all.py --no-build --reuse-extractions
```

`--reuse-extractions` is for reproducing the baseline, not validating changed extraction code. A writer must refresh the DBs after modifying the plugin. The databases merge rather than delete old entries; Locust's extraction records removed rows according to its normal behavior.

Sequence: inventory/snapshot/build → five CLI extractions → independent TextAsset walk (`probe.py`) → independent Object.m_Name headers (`headers.py`) → DLL-derived field trees (`trees.py`) → script oracle/inject (`oot_audit.py`) → binary copy/inject (`binary_copy_probe.py`) → typed-field comparison (`binary_audit.py`) → five writer briefs → regressions → report. The default run exits successfully while recording expected old-code regression failures in `regression-results.json`. Run `venv\Scripts\python.exe regressions.py` separately for a conventional nonzero assertion exit.

Outputs:

- `last-report.md`: denominators, limitations, counts, complete real examples and ranked defects.
- `writer-01-controls.md` … `writer-05-typed-fields.md`: dispatch-ready Unity-only briefs.
- `class-examples.json`, `*-truth.json`, `*-misses.json`, `*-fps.json`, `*-unclassified.json`: complete evidence, never truncated.
- `oot-injection.json`, `binary-injection.json`, `oot-original-hashes.json`: independent copy/integrity checks.
- `*-trees.json`: successfully parsed field values and explicit parser errors. Failed custom object schemas are excluded from the recall denominator.
- `inventory.json`, `provenance.json`, `cache-reuse.json`: roots, source identity, boundaries and cache reuse.

Interpretation: recall concerns the successfully parsed shipped physical text fields, including renderer defaults; it is not observed runtime coverage. Precision is reported for classified rows plus conservative whole-output bounds; most unknown custom fields are not falsely declared technical. Spanish translation-pair provenance is unknown, so pairs and Locust DBs are not truth. The oracle uses the game's runtime field schemas/tables and an independently installed parser.

The research used the pattern in `C:\Projects\Locust\tools\ground-truth\README.md`, `rpgm\audit.py`, `rpgm\injection.py`, and the brief shape in `tmp\cycle104-brief.md`. No tmp/ log search was used to interpret the stop rule. Stop only for a real Windows sandbox-helper error returned by your own command-runner call.
