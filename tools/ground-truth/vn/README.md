# Visual-novel ground-truth audit

Read-only audit of commit `b0a17d2fd2fa91c069cd6b536185ddc5a8b193a5`.
All scripts, databases, snapshots, builds, downloads and injection copies stay here.

## Run

```powershell
Set-Location C:\Users\Mike\AppData\Local\Temp\locust-research-vn
py -3.13 run_all.py
```

Use a writer's stable CLI without modifying this baseline snapshot:

```powershell
py -3.13 run_all.py --cli C:\Users\Mike\AppData\Local\Temp\writer-scratch\target\debug\locust.exe
py -3.13 regressions.py --require-fixed
```

Set `LOCUST_AUDIT_CLI` to the same executable when running individual scripts or `regressions.py`. `run_all.py --cli` passes it automatically to its children. If a fix changes row locators, adapt the scratch output-to-slot mapper/probe selection only; preserve the independent oracle and corpus unchanged, and document the adapter difference. Keep a separate copy of this audit for AFTER numbers; rerunning replaces reports/databases and refreshes injection copies, never game originals. Never point `--cli` at a concurrently compiled live-tree build.

Python 3.13 standard library only; no global installs. Rust/Cargo/MSVC are needed only to build. The existing pinned HEAD snapshot is reused. If absent, run_all creates it using `git archive HEAD` without a worktree or repository writes. `--build` builds only scratch `snapshot/`; `CARGO_TARGET_DIR`, `TEMP`, `TMP`, `LOCUST_DATA_DIR` are scratch paths. Existing cargo registry cache is read/reused; the other session's target cache is not used. No game runtime is launched.

## Pipeline (same measuring/reporting shape as tools/ground-truth/rpgm)

| Script | Output / purpose |
|---|---|
| inventory.py | Local engine/container inventory; before/after SHA256 of 3,310 original files |
| prepare.py | Byte-for-byte script copies, independent XP3 scenario decode; cached public Tyrano scenarios and NS demo |
| readers.py | Independent KAG grammar, YSTB/YSCM, XP3/YPF transport; imports no Locust code |
| audit.py | CLI extraction vs static game/runtime oracle; misses/FP and explicit precision bounds |
| ns_audit.py | Separate NS public-demo measurement |
| pairs.py | Secondary existing third-party JSON pair/lexical check, distinct denominator |
| injection.py | Deterministic prefixes on scratch copies; compare untouched data and re-extract; distinguish safe rejection |
| archive_injection.py | Real XP3/YPF copies; compare every original member; identify destructive patch overwrite and safe YPF install failure |
| regressions.py | Eight minimal old-code failing checks; --require-fixed returns nonzero until fixed |
| report.py | last-report.md, all-stats.json, all-class-examples.json |

`last-report.md` puts integrity first, then inventory, denominators, three full real examples per class, and ranked disjoint writer briefs. The four writer files each own one plugin; shared archive_replace/core staging work is excluded from their edit scope. All source citations reference the pinned commit. Eight plugin source hashes match the live checkout after LF normalization; Unity writers' edits were excluded from the build.

## Oracle limits

* Static literal/attribute coverage, not executed path coverage. Known speaker/choice attributes are typed slots. Five Tyrano ruby readings are present inside opaque rich rows but not separately addressable; reported as strict attribute misses.
* Yuris has 2,225 unclassified rows; the precision interval retains them. Existing output.ja.bak files are English, not assumed Japanese originals or human translations.
* Motto's six initialization scripts are not a gameplay denominator. No local Tyrano/NS game was identified: public third-party sample numbers are explicitly separate.
* No XP3 cipher keys or gameplay-payload decompiler. Motto archive names are hashed/extensionless; their payloads remain unexamined. Transport copies only readable .ks from Ochiru patch2. My/Taimanin measurements cover selected loose scripts, excluding their additional archive scripts (see archive-inventory.json). Huge media files are excluded from the original hash sample.
* Source URLs and cached specification bytes are in specs/ and source manifests, including final-spec-sources.json. `fetch_specs.py` is a source-discovery helper, not required for an offline rerun; its initial missing-path errors are recorded. The corrected source files are cached. The NS GBK candidate was rejected and is not part of the pipeline.

## Baseline integrity

XP3: translating three rows replaces a pre-existing 172-member patch with one member, then 0/3 translations re-extract because patch2 outranks it. Mixed-newline KAG: three writes normalize 5,381 bare CR, change 15 untouched locators and expand 18 extracted rows to 2,022. YPF: installation aborts on retained shared staging data; all 572 original members survive. Loose Yuris/Tyrano/NS normal prefixes preserve untouched data. Missing KAG/Tyrano controls are rejected before installing files.

Do not edit `C:\Projects\Locust` or anything under `D:\juegos` from this audit. Check `original-hashes-check.json`, not only CLI write totals. The game corpus, public scripts and eight baseline failures are the oracle/evidence; Locust test expectations never define ground truth.
