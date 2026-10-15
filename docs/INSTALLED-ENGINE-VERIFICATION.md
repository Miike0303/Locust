# Installed-engine verification: partial resource evidence

**REL-4A is delivered at verified source `77e5c96`; R01 remains `pending-policy` only for Add, not a failed Replace.** The fix preserves the trailing blank line lost in the historical REL-4 run and passes Replace packaging, installation, rollback and reapplication. Ren’Py Add still correctly refuses installation without structural-tier confirmation; its rollback and reapplication were not run. RPG Maker MZ and Unity retain their historical passing results, not verification at the latest commit.

These are **owned resource projections, not runnable game copies**. Public evidence uses aliases and counts; installed titles, paths, dialogue, databases and raw logs remain in ignored local artifacts.

## Results at a glance

Both records are dated 2026-10-15. Historical REL-4 used source checkpoint `d2a3fe715e3e60d337e227b2e2f4739f07c7d02a`. REL-4A replayed **only R01**, using the uncommitted Ren’Py fix candidate and release CLI SHA-256 `46289b196e09ee57a2cb81304169b60924b01aebef8ce846124f1c3ce3ab3954`. That source is now delivered to `main` as `77e5c96c252fb8780f87aaac1ffd905541068a92`, verified by [CI `38678826007`](https://github.com/Miike0303/Locust/actions/runs/38678826007). The actual replay reports remain precommit-candidate records with the same verified binary hash; their recorded source SHAs are not rewritten. The hash identifies the tested binary, not an application version bump.

The latest CI passed Rust Ubuntu **2415/0/21 ignored**, Windows **2426/0/21 ignored**, and **494 frontend tests on each**; Python recorded 64 total (one skipped on Windows), the oracle 10, and isolated Unicode checks 108 + 25 path checks. Build, fmt and strict Clippy passed. Ignored Rust tests were not run; 13 npm vulnerabilities remain unresolved. Local release CLI retained the replay hash above; desktop SHA-256 is `ec69ce9b596284d35e4b75594415addbdd5f9f315084ebf63b07100b4ab98a25`, also already hash-verified. These are parent-verified checkpoint results, not checks rerun for this passive update.

| Alias | Installed family and tested resources | Engine version | Extracted rows | Controlled targets | Evidence level and result |
|---|---|---|---:|---:|---|
| M01 | RPG Maker MV/MZ family; **MZ** JSON database resources | Unknown | 59 | 3 | Historical owned-projection verification at `d2a3fe7`: exact target fields, remaining JSON structure and Replace recovery passed; not replayed in REL-4A |
| R01 | Ren’Py; loose UTF-8 source scripts | Unknown | 31,594 | 3 | Owned-projection replay of the now-delivered fix: Replace structure, all rows and recovery passed; Add oracle passed, install refused; overall `pending-policy` |
| U01 | Unity; authored `SCRIPTS~` text and character declarations | Unknown | 169 | 3 | Historical owned-projection verification at `d2a3fe7`: declared-speaker targets, remaining script lines and Replace recovery passed; not replayed in REL-4A |

MV was not separately exercised. U01 does not cover Unity SerializedFile, UnityFS, IL2CPP or arbitrary Unity games. R01 does not cover compiled scripts or RPA archives. Optional KiriKiri, VX Ace and Unreal cases were not added.

### Integrity and selection

- Historical REL-4 verified matching before/after SHA-256 values for six original resources; none was injected or installed into. REL-4A read **no original resources** and performed no discovery. It verified the two retained R01 projection hashes against prior provenance and kept them unchanged. Its `originalHashesUnchanged: null` is not a fresh original-hash result.
- Historical selected input resources totalled **1,769,628 bytes**: M01 19,932; R01 1,739,758; U01 9,938. Each case contained two resources, below the 80 MiB/case and 256 MiB total input limits. REL-4A reused only R01’s 1,739,758-byte projection.
- Historical storage measured 73,340,337 bytes for the reference run and 181,989,227 bytes across retained REL-4 evidence. The initial REL-4A attempt was blocked by the **256 MiB preflight**. The explicitly authorized **512 MiB accumulated-evidence cap**, with a current **at least 2 GiB free-space guard**, passed: estimated total 276,347,800 bytes, actual retained total **251,665,208 bytes**, latest measured free space 338,732,617,728 bytes. No evidence was deleted to fit the budget. The estimate and post-operation checks are not an OS-enforced quota; future replays require a new storage check.
- Each case selected three plain, control-free display rows from independently mapped slots. The targets were generic ASCII Spanish test labels (`TEST Hola`, `TEST Si`, `TEST No`), not translations of the original prose.
- All selected resources are variable-length UTF-8 text. No binary capacity or non-UTF-8 encoding claim follows from this run.
- Each Replace result preserved its one unselected resource byte-for-byte. Re-extraction matched all 59, 31,594 and 169 expected rows respectively, including unchanged rows. The source DB text digests were unchanged; only the intended test translations and normal native injection records were written.
- Native reports classified the remaining 56, 31,591 and 166 rows as untranslated. Those are deliberate partial fixtures, not complete translations or unexpected injection skips.

### Packaging and recovery

| Alias / mode | Recorded patch contents | Pack | Install | Rollback | Reapply |
|---|---|---|---|---|---|
| M01 Replace | One replacement with original hash | Passed | Passed | Passed | Passed |
| R01 Replace | One replacement with original hash | Passed | Passed | Passed | Passed |
| U01 Replace | One replacement with original hash | Passed | Passed | Passed | Passed |
| R01 Add | Two added files, no original hashes | Passed | Safely refused | Not run | Not run |

M01 and U01 results above belong to historical REL-4; R01 results belong to the REL-4A candidate replay. Each successful installation used a **fresh owned projection** and matched the injected resource tree. Rollback restored the pristine resource tree; reapplication reproduced the injected tree. Internal `.locust` bookkeeping is excluded from resource-tree equality. Historical R01 Replace also passed this cycle, but that did not negate its separate script-preservation failure.

## Native workflow and what was exercised

For normal use, start with [the native workflow guide](NATIVE-VN-WORKFLOW.md), not this internal audit script.

1. Open the intended project and review its extracted rows and translations.
2. Prefer **Create translated copy**. Known binary-slot capacity checking is an early refusal guard, not evidence that every row is translated or that the game will run correctly.
3. Check reported skips, the actual output folder and format-appropriate output. Use the project’s native recording when packing a patch.
4. Verify installation and recovery on a separate owned copy before considering a real installation.

This audit used the native HTTP project-open and injection routes, with the format explicitly specified. Every Replace request set `require_binary_slot_fit: true`. Extraction and packaging used the release CLI with explicit output/database paths. This was **not** a Tauri window, UI interaction or managed-state IPC test.

### Ren’Py requires a different Add oracle

Replace re-extraction should expose the replaced source text. Add instead writes language-specific `game/tl/es` translation blocks and a language picker while preserving the original scripts. Finding the test targets in ordinary source re-extraction is therefore the wrong acceptance condition for Add.

The REL-4A R01 Add replay passed these separate checks:

- Three translation block identifiers matched the independent canonical-statement MD5 rule, and their original/target bodies matched the selected rows.
- Both source resource files in the owned projection stayed byte-identical; all 31,594 original source rows re-extracted unchanged, and the source DB text digest was unchanged.
- Native recording packed two added files. Because neither replaced an existing file, the patch had zero original-file hashes and was structural-tier.
- Ordinary apply refused that patch and left the fresh destination pristine, without a `.locust` directory. No `--force` or `--confirm-legacy` retry was made.

The strict capacity option is Replace-only and was deliberately absent from this separate Add request. Language activation was not executed in Ren’Py.

## Historical defect and remaining blocker

### 1. Ren’Py Replace: historical failure resolved in verified source

At `d2a3fe7`, the independent oracle expected 59,587 script lines after changing only the three selected literals; the output had 59,586. All common lines matched, and the missing final line was blank. All 31,594 re-extracted rows still matched. This was a resource-preservation failure, **not observed runtime breakage**.

The REL-4A candidate passed the unchanged independent script oracle: **59,587 lines preserved, zero line-ending changes**, all 31,594 re-extracted rows matched, and all three test targets matched their independent slots. The other resource remained byte-identical and the source DB text digest was unchanged. The report also records one negative oracle control. Replace pack/install/rollback/reapply all passed.

This documentation update changes no application or test code and reruns no audit. The recorded precommit result, now linked to verified source `77e5c96`, supersedes the historical Replace failure only for this bounded loose-script projection; it does not establish compiled-script, archive or runtime coverage. The code regression began at 3 passing/6 failing tests and finished with 9 tests covering 84 byte-exact combinations and 6 no-write controls; three old revision expectations were corrected before the passing workspace run. The earlier CI-watch network timeout was reconciled read-only, not a test failure.

### 2. Added-only Ren’Py patches require explicit confirmation

This is the remaining **validation-tier blocker**, not proof of a broken patch. `PatchManifest::supports_strict_tier` in `crates/core/src/patch/manifest.rs` requires at least one original hash. R01 Add has two new files and zero original hashes, so its validation tier is structural. `enforce_verify_gates` in `crates/core/src/patch/apply.rs` correctly refuses default, unconfirmed installation. The replay reports `pendingPolicy: 1`, `failed: 0`, `passed: 0`, `complete: false`; a passing Replace oracle is not an all-green R01 result.

Any follow-up belongs in a separately authorized native confirmation-flow investigation, with `crates/core/tests/patch_apply.rs` preserving the unconfirmed-refusal/no-mutation case. It must not silently add an original hash, force application, or reinterpret an added file as a replacement. Add install/rollback/reapply remain unverified here.

## Inspect retained local evidence

These paths are **ignored local evidence, not public links**. No replay is required to read this guide.

| Record | Retained local path | Authority |
|---|---|---|
| Historical REL-4 report | `tmp/reliable-installed-engines/rel-4/run-1792033780900-2cd5cc83/report.json` | M01/U01 results, original-resource hashes and the original R01 failure |
| Actual REL-4A report | `tmp/reliable-installed-engines/rel-4/verify-renpy-fix/report.json` | R01 precommit-candidate Replace pass (source now verified at `77e5c96`), Add policy refusal, unchanged binary hash and actual storage |
| Replay preparation | `tmp/reliable-installed-engines/rel-4/REPLAY.md` | Harness options and storage/isolation contract; its preparation-only status is not the executed result |

Detailed artifacts remain beside the corresponding report:

| Local artifact | Purpose |
|---|---|
| `report.json` | Alias-only counts, phase outcomes, refusals and original-hash result |
| `inventory-private.json` | Bounded candidate locators; not a complete game census |
| `original-hashes-private.json` | Exact selected original paths and pre/post SHA-256 values |
| `<alias>/manifest-private.json` | Exact source-resource projection mapping |
| `<alias>/selection-private.json` | Independent locators, test targets and expected row digests |
| `<alias>/*.locust.db` | Fresh private audit databases; never an existing user project |
| `<alias>/*.zip` and owned trees | Recorded patches, injection outputs and installation evidence |
| `logs/` | Retained native responses and command diagnostics; may contain private paths/text |

Earlier retained runs contain the inventory-only checkpoint and incremental failures, not additional installed cases. The two reports above have distinct authority; REL-4A does not rerun or upgrade M01/U01 evidence. The harness and oracle live under ignored `tmp/`; the guide alone is the public deliverable. Do not publish the private artifacts as part of the guide.

### Bounded discovery and isolation

Historical REL-4 discovery made 40 directory listings in the RPG Maker category, 40 in the Ren’Py category and 15 in the Unity category. The first two hit the cap. Candidate discovery was limited to two levels below category roots plus fixed resource directories; Unity inspected one known chapter directory. These are directory/candidate counts, **not counts of distinct games or a complete installed-engine inventory**.

The harness excludes output/mod/patch/`locust-work` paths, rejects linked projection paths, and checks selected originals’ Windows reparse attributes before copying. Only JSON, `.rpy` and `.txt` resources enter projections. It creates no executable markers, reads/copies/executes no game EXE or DLL, and launches no game. The existing VN inventory was consulted as context; its recursive whole-corpus runner was not executed.

Each run allocates fresh retained directories and uses explicit `--config`, `LOCUST_DATA_DIR`, `LOCUST_BACKUP_ROOT` and child-only scratch temp paths. OS profile/identity variables remain unchanged; normal GameLock behavior is allowed. Only the backend process spawned by that run is stopped. No cleanup, provider, credentials, registry, dependency, Git publication or global configuration operation is part of this harness.

The independent checks reuse the RPG Maker JSON and Ren’Py literal helpers under `tools/ground-truth/`; the Unity declared-speaker grammar is adapted from `unity/oot_audit.py` without importing its whole-corpus side effects. No game script is executed as Python or JavaScript.

## Read-only next research

REL-5 is in progress, not an implementation or installed-game verification. G01 confirms Godot 4.3/PCK v2 and Dialogic resources (112 `.dtl`, 7 `.dch`); the installed Dialogic version remains unknown. The primary alpha 16 reference is compatible with the observed format, not proof of the installed framework identity. Native support remains unavailable; no game runtime, font or linguistic result follows from this research.

For current Experimental support and verification, see the [Godot / Dialogic guide](GODOT-DIALOGIC.md).

## Key learnings

1. M01 and U01 retain historical projection-level passes at `d2a3fe7`; only R01 was replayed with the precommit binary whose source is now delivered and CI-verified at `77e5c96`.
2. R01 Replace now preserves all 59,587 script lines with zero line-ending changes and matches 31,594 re-extracted rows, including three controlled targets.
3. R01 Add verifies three translation targets while preserving both source files and all 31,594 original rows, but its structural-tier installation refusal leaves the case `pending-policy`.
4. Replace pack/install/rollback/reapply passed; Add rollback and reapply were not run because nothing was installed.
5. The authorized 512 MiB cap and at least 2 GiB free-space guard passed with 251,665,208 bytes retained and no evidence deletion.
6. Owned projections are not runnable games, and these checks establish no real-provider, linguistic, native-window, font, gameplay or engine-version result.
