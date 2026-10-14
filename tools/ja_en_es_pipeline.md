# Safe JA → EN → ES patch pipeline (PIPE-4D)

`ja_en_es_pipeline.py` is a Python standard-library CLI orchestrator for an
owner-authorized game. It never injects into the source folder. A successful run
returns both a verified Spanish-labelled patch ZIP and a separate translated
installation. It does **not** claim linguistic quality, font coverage, or game
runtime compatibility, and never launches the game.

## Requirements and invocation

- Python 3.9+ (both CI jobs use Python 3.12) and a trusted, current Locust CLI executable containing the strict
  `pivot --require-complete` and `inject --json` contracts (PIPE-3A `1db6bd1` or a
  compatible later build). The binary's SHA-256 is recorded; its source commit
  cannot be inferred from `locust --version` alone.
- An owner-authorized plain game directory and enough free disk space for several
  complete copies, plus Locust's injection backups/transaction originals.
- A **nonexistent** output directory whose parent already exists. Even an empty
  existing directory is refused, unless it is a verified resumable owned run.

Example in PowerShell (replace the explicit paths with your authorized paths):

```powershell
python tools/ja_en_es_pipeline.py `
  --cli C:/Projects/Locust/target/debug/locust.exe `
  --game D:/Games/AuthorizedGame `
  --output D:/TranslationRuns/AuthorizedGame-es-001 `
  --format renpy --provider google --batch-size 40 --concurrency 4 --timeout 900
```

The same flags work on POSIX with an absolute executable path. Supported formats
are `renpy`, `rpgmaker-mv`, and `kirikiri`. Providers are `google` (free) and
`antigravity` (the existing logged-in `agy` executable on PATH). There is no
API-key argument, paid-provider option, implicit fallback, or login operation.
`--fallback-provider google` is optional and valid **only** with primary
`--provider antigravity`. It applies to both JA→EN and EN→ES, never changes the
selected primary, and never selects an arbitrary provider.
Batch size is 1–200, concurrency 1–16, and per-child wall timeout 1–86400 seconds;
defaults are 40, 4, and 900 respectively. Large games may need a larger explicit
timeout; a timeout is not automatically retried.

`--test-endpoint http://127.0.0.1:PORT` replaces the provider with a custom
OpenAI-compatible scripted transport, fixed model `scripted-fixture`, and the
fixed **dummy**, noncredential key `pipeline-dummy-not-a-credential`. Only literal
loopback HTTP addresses with explicit ports are accepted, including `::1`;
DNS names, credentials, paths other than `/`, query strings and fragments are
refused. Use a trusted local fixture server, not a real translation service.
With explicit Google fallback, a scripted primary additionally requires a
**different** `--test-fallback-endpoint http://127.0.0.1:OTHER_PORT`. This test-only
seam uses a second fixed dummy OpenAI-compatible provider,
`pipeline-google-scripted`, to simulate Google without live requests. It is not
an override of Google's real transport. A fallback test route without a scripted
primary, an implicit fallback route, or conflicting primary/fallback URLs refuses.
This URL guard is not a network sandbox for an adversarial local server.
Inherited proxy and provider-key variables are not forwarded. The selected CLI
and the operator's PATH/login environment are conservatively trusted; this tool
is not an executable or network sandbox. **Untrusted resume data cannot grant
new executable or network destinations**: config is reconstructed from the
validated current invocation, not authorized by a manifest hash.

## Ownership, isolation and safety

The fresh output root contains:

| Path | Purpose |
| --- | --- |
| `pristine/` | Byte-verified baseline; files read-only, POSIX directories read-only |
| `work-game/` | Owned Direct injection target, including retained transaction evidence |
| `install-game/` | Owned strict patch installation; Spanish-labelled when completed |
| `proof-game/` | Resource-only post-injection copy for independent re-extraction |
| `english.db`, `spanish.db` | CLI-generated Japanese-source EN project and semantic EN-source ES pivot |
| `seed-snapshot.db`, `seed-english.xliff` | Optional exclusive seed copy and hash/ID/source/target-verified CLI catalog |
| `work-proof.db`, `install-proof.db`, `final-install-proof.db` | Separate re-extraction evidence; never semantic pivots |
| `spanish-patch.zip` | Verified patch artifact |
| `profile/`, `backups/`, `temp/` | Operation-local Locust profile, backups and temporary files |
| `pipeline-manifest.json` | Versioned ownership, input/settings hashes, durably published stage results and proofs |
| `pipeline-manifest.next` | Exclusive pending publication; retained on publication failure, never auto-adopted |
| `pipeline-publication-failure.json` | Small exclusive failure record when publication fails and safe recording is possible |
| `pipeline.lock` | Exclusive live-run marker; retained for crash/publication ambiguity |

The source is read only for copying/hashing, is never passed to a Locust child,
and its complete file/directory inventory is checked repeatedly. No source
permissions are changed. Copying uses independent regular files, not hard links.
Executable bits are preserved; copied work/install files are made owner-writable.
“Byte-exact rollback” means file bytes and directory inventory, not source ACLs,
timestamps or every filesystem attribute. Read-only pristine permissions plus
repeated hashes protect the baseline operationally, not against an administrator
who can change permissions.

The tool rejects source/output containment **in either direction**, including
source-tree/destination-ancestor directory identity aliases, parent traversal
paths, symlink/reparse components and tree members, hard-linked tree
files, special files, unsafe/ambiguous filesystem names, occupied fresh outputs,
and existing `.locust`, `.locust-injections`, or `zzz_locust*` installed state.
A filename such as `.locust-not-metadata.bin` remains ordinary game content.
Credential-like names (`.env`, `.ssh`, `xai-oauth.json`, private-key extensions,
etc.) are refused **before reading**; this is not a universal secret detector.
Do not select a user profile, credential/config directory, or personal-data tree
as a game. Source changes during copying/hashing refuse the run. Exclusive owned
run markers and repeated checks assume a trusted, non-concurrently-edited tree;
this is not a hostile-filesystem/TOCTOU-proof sandbox.

All Locust commands receive an explicit generated `--config`, isolated
`LOCUST_DATA_DIR`, `LOCUST_BACKUP_ROOT`, temporary directories and cwd. Inherited
`LOCUST_CONFIG`, unknown provider overrides and proxy variables are excluded.
Ordinary commands also use synthetic HOME/USERPROFILE/XDG/AppData paths under the
owned profile, including Windows KnownFolder directories used by game locks.
Only **Antigravity translation children** preserve inherited home/profile
variables to let agy use its existing subscription login. Python does not open,
copy or print that login/configuration. Other stages remain synthetically
profile-isolated. Locust's unrelated Grok registration does not probe the owner's
token file: `crates/providers/src/xai_oauth.rs::token_path()` delegates to
`AppConfig::config_dir()`, and `crates/core/src/config.rs` checks the nonempty
isolated `LOCUST_DATA_DIR` before any OS/home fallback. No live agy authentication
was exercised in this implementation's tests.

Failed output roots are retained. There is no broad deletion, backup purge,
overwrite of an occupied game/project, or force/legacy apply path. The sole
ordinary cleanup performed by Python is its own live `pipeline.lock` marker on
orderly exit **without publication ambiguity**. Publication failures retain that
marker, the old durable manifest and any pending `.next`. Locust itself performs its normal owned transaction staging cleanup
and the explicitly requested installation rollback. Tests use disposable
synthetic fixtures under `.test-tmp/`; fixture guards clean those test-only roots.

## Manifest publication and failure

Each save exclusively creates `pipeline-manifest.next`, writes the intended
canonical bytes, flushes/fsyncs and closes it, then replaces only the known owned
`pipeline-manifest.json`. Replacement retries apply **only to `PermissionError`
from `os.replace`**: one initial attempt plus at most three retries, with delays
of **50, 100 and 200 ms** (350 ms total requested sleep, not an overall IO deadline).
No other filesystem operation/error is retried. The observed Windows
`WinError 5` defect is real; its environmental cause is unproven, and no manifest
handle leak has been established. This policy does not assume a particular cause.

Before the initial replacement and again **after each retry delay**, checks prove:
- The owned root and its parent still have the recorded directory identities,
  with plain lexical path components (no symlink/reparse traversal).
- The **same exclusively created `.next` inode/device and expected hash/size**
  remain present, regular, single-link and nonreparse. It is never unlinked or
  recreated for a retry, and a preoccupied/foreign `.next` is never claimed.
- The previous durable target has the same recorded identity and hash/size and
  remains regular, single-link and nonreparse; initial target absence must also
  remain absence. A new foreign target is never overwritten.

Exhaustion, changed identities/bytes, unsafe paths, or any other publication IO
failure aborts the run. **No next child launches until the preceding stage's
result is durably published.** Ordinary exception handling does not call `save()`
again with occupied `.next`. The original publication exception remains the
cause of a sanitized publication diagnostic; if publication fails while recording
an earlier gate/child failure, that earlier failure still wins.

A separate, at-most-4-KiB `pipeline-publication-failure.json` is attempted once
with exclusive creation (`xb`), never replacement, alternate filenames or retry.
It records run ownership, failure kind/codes and attempt count, expected pending/
durable fingerprints, and small attempted-versus-durable phase/active-command/
last-stage summaries with `status: failed` and `resume_safe: false`. It contains
no raw exception message, provider response or game text. This distinction
preserves ambiguity when a child succeeded but its result was not published.
Expected fingerprints are evidence, not a claim that a foreign-mutated file is
still intact. Unsafe root ownership, a foreign existing record, or a denied or
partial record write cannot mask the original failure; owned partial artifacts
remain for inspection, and a failure record is not guaranteed in those cases.

Resume refuses pending publication/failure markers. Neither `.next` nor the
failure record can authorize promotion, cleanup, recovery or a completed result,
even if pending bytes contain `status: completed`. Retain the run for independent
inspection and use a fresh output root from the untouched source when ambiguous.

These identity/hash checks assume a trusted, non-concurrently-edited filesystem.
There is still a check-to-replace/check-to-create race; inode reuse and a hostile
actor changing paths between checks are not solved by these guards. No general
filesystem retry, permission relaxation, forced replacement, durable-target
rollback or cleanup of foreign files is performed.

## Release gates and sequence

1. Hash the executable and entire source, create verified pristine/work/install
   copies, and extract the work copy into a fresh EN database.
2. Optionally borrow verified ready English targets as described below, then
   translate JA→EN using `--no-memory`. Require the exact original inventory,
   sources, tags, context, metadata and physical identity; every row must have a
   nonblank current translation, ready status, timestamp and expected provider.
   Extraction warnings must be present as a valid empty project array. Run
   `validate --json` and check both its structured count and exit status.
3. Run `pivot --require-complete` into a fresh ES database. Prove that every pivot
   source equals its EN translation, every immutable injection source equals the
   original JA source, and other static row properties are preserved. EN must
   remain unchanged. Translate EN→ES with memory disabled and repeat readiness,
   exact-inventory, provenance and validation gates. Raw KAG argument tags are
   additionally compared exactly against both semantic and physical sources.
4. Persist `injection_started: true` **before** starting
   `inject work-game -P spanish.db --direct -l es --json`.
5. Verify the report, completed transaction identity/phase/plan, all result and
   ordinal-original hashes, and the full contained V2 pre-write backup payload
   and inventory against pristine. Written + skipped must equal the complete
   database inventory. Only explicit `unchanged` skips with physical equality
   can qualify; unknown skips, mismatched counts, unrecorded/kept-previous
   outcomes, warning-bearing partial results and malformed metadata refuse.
6. Re-extract `proof-game/` into a separate DB and prove all physical resource
   identities and expected ES database translations. Package the recorded work
   with `--pristine pristine`, then independently stream-check the ZIP's complete
   inventory, manifest, pristine hashes, payload hashes and sizes against the
   exact game delta. Deleted files and structural/added-only patches refuse.
7. Strict apply dry-run to `install-game/`, prove no byte changes, then strict
   apply **without** `--force` or `--confirm-legacy`. Check the strict/pristine,
   nonforced receipt, exact installed files and original backups. Re-extract into
   `install-proof.db` and compare every expected resource.
8. Roll back this owned installation and prove byte/directory-exact pristine
   restoration. Reapply the **same hashed ZIP once**, verify strict receipt and
   bytes again, and perform final complete re-extraction into
   `final-install-proof.db`. Recheck EN/ES rows and injection backup provenance.
   Only then mark the run completed. Failed final reapply, receipt, hashes or
   re-extraction never produce a completed result.

### Two narrowly classified informational cases

- `validate --json`'s `IdenticalToSource` issue can represent an unchanged proper
  name. Only that exact issue kind/message, a unique known row, and proven
  nonblank source/translation equality qualify. All other validation issues
  refuse. This classification is **not** a linguistic judgment that an unchanged
  title or sentence is good Spanish; PIPE-4/human quality audit remains needed.
- Current Direct injection emits this exact informational message in `warnings`:
  `injection <id> completed; verified originals retained at <owned originals path>`.
  Parent disposition A explicitly permits classification of this one notice.
  Its text is **derived from independently verified owned transaction state**,
  never parsed to obtain a trusted path or accepted merely by prefix. Both report
  warning arrays must be empty or contain only that exact notice. Any additional
  or altered warning refuses. Completion additionally requires complete work,
  installed and final re-extraction equivalence; the manifest records this
  distinction. An unfinished transaction, foreign backup path, bad original
  hash, incomplete counter, or failed equivalence cannot be hidden by the notice.

## Bounded format/resource constraints

This slice intentionally rejects common archives/compiled scripts by extension,
including RPA, RPYC, XP3, ZIP, 7z, RAR, ASAR, NW and JSONO. KiriKiri is
**loose `.ks` only**, never an encrypted/XP3 fallback. It is not a general archive
sniffer or a claim of complete extraction from arbitrary deployments.

Current loose Ren'Py/KAG extraction rows may lack explicit `format_id` metadata;
absence is accepted only for recognized `.rpy#<line>` / `.ks#kag:<line>...` physical
locators. An explicit malformed/wrong format stamp always refuses. Duplicate
texts remain separate resources. Locust extraction/filtering is the inventory
boundary: exact preservation of extracted slots does not prove every visible
runtime string was discovered.

MV command offsets can move when the injector rewrites author-wrapped dialogue.
Following the bounded `qa_replay_oracle.py` pattern, the comparison groups rows by
physical file and structural event/page prefix, orders `(command offset, kind)`,
and compares ordered slot/kind identities. Missing, extra, swapped-text or wrong
page/file resources refuse. Only `#msg`/`#description` rows whose original JA
source was multiline may replace re-extracted LF with single spaces to match
exact target text. There is no generic whitespace folding, choice/name exception,
arbitrary command rearrangement migration, or language/markup correction.

**Known conservative KiriKiri refusal:** a modified loose file containing both
changed and unchanged rows currently omits an unchanged row from CLI written/
skipped counters. This pipeline refuses that incomplete report even if text
looks correct. The homogeneous six-slot KAG positive fixture avoids unchanged
rows; a seven-slot mixed-file negative proves the omission remains rejected.
No Rust counter repair or silent inferred skip is included here.

## Explicit pending-only Google fallback

After the primary returns **exit 0** and that result is durably published, the
wrapper checks the entire static inventory and every row column. Rows already
ready before the child must remain byte-for-byte logically identical, including
status, provider, translation and timestamps. Newly ready rows must have the
exact dispatched provider and are bound by full-row hash to its typed successful
stage. Pending rows must remain unchanged if they were not accepted. The complete
resource inventory is never filtered down to provider successes.

If pending rows remain and fallback was explicitly requested, run classified CLI
validation of accepted targets first, then dispatch Google once with the same
language pair and `--no-memory`. Locust's pending-only selector—not SQL edits—
selects its work. Google uses isolated HOME/USERPROFILE/XDG/AppData paths, never
the inherited agy login profile. Its result is durably published and the same
inventory/column/provenance checks repeat. Prior-ready rows cannot be reattributed
or overwritten, even if the fallback returns exit 0. The manifest records exact
`ja-en-fallback-google` / `en-es-fallback-google` stages and mixed attribution in
`accepted_rows`. Test-route attribution remains distinct from real Google.

No fallback runs after a nonzero primary exit, timeout, active/uncertain child,
or publication failure. It is not a watchdog recovery mechanism. A nonzero or
timed-out fallback never advances readiness; an orderly exit 0 with pending rows
still fails the original full-readiness gate. Partial-validation stages cannot
release a pivot, injection, or patch. All final recording, ZIP and strict-install
gates still operate on **the whole inventory**, not just the borrowed/Google rows.

## Fresh-only verified English seed

An optional `--english-seed /absolute/closed-owned-run/english.db` can reuse ready
JA→EN rows without resuming that run. Example additions to the fresh invocation:

```text
--provider antigravity --fallback-provider google --english-seed D:/TranslationRuns/closed-run/english.db
```

The input must be a **closed**, known owned failed run at `english-pending` or
`english-ready`, with no active/uncertain child or injection marker. Interrupted
runs are not seeds: never forge an active marker, take over an old lock, or resume
the interrupted first Lydia run. The input and its parent components must be
plain/nonreparse and the file must be single-link. Reject all actual SQLite
WAL/SHM/journal sidecars, publication remnants/failure records, locks, Spanish
pivots/patches, and overlapping source/output/seed roots. A historical checkpoint
may record an empty WAL (exact empty SHA/zero size) only if it is now absent;
nonempty recorded WAL refuses. The operator must independently establish closure;
the wrapper never checkpoints, migrates, removes sidecars, or repairs the owner.

Seed acceptance binds a closed version-1 or version-2 ownership manifest, its
English fingerprint and logical snapshot, the independently hashed current
Japanese source, and the old pristine/work/install Japanese copies. Manifest
paths, binary/tool/config hashes and network/executable settings are **data
only**, never command authority. The old profile/config/executable and its
claimed source-game path are never opened or invoked. Current invocation's CLI,
config, source and tool checks remain independent and strict. Seed reports are
provenance evidence, not authenticated signatures or a hostile-FS sandbox.

Python exclusively copies the seed into `seed-snapshot.db` and inspects/exports
**only that copy**. It checks the original file/manifest identities and hashes
before/after and throughout the run. SQLite reads enforce integrity, empty
extraction warnings/memory, no injection recording, and JA→EN translation-run
provider/language evidence. Full Japanese inventory must match a **new actual CLI
extraction**: IDs, relative file paths, source, context, tags, format, static
metadata and character limits all match. Only the known root in `file_path` is
mapped; IDs and other strings are not normalized. Foreign/stale/shrunk/pivoted
inventories refuse. Version-2 ready rows must also match their owner stage bindings.

The existing CLI exports only `translated`, `reviewed` and `approved` targets to
XLIFF. The wrapper requires the exact emitted plain-text XLIFF structure,
declared source `ja` / target `en`, no inline/DTD/unknown structures, and exact
unique IDs, Japanese sources and English snapshot targets. Missing/damaged ready
rows or a partial catalog refuse; pending/invalid nonready targets are never
borrowed and never reduce inventory. CLI `import --keep-existing` then fills the
**new** English DB. Before accepting imports, production CLI validation checks
controls in that fresh DB, and the wrapper proves exactly the verified targets
were filled, other pending rows remained unchanged, and fresh JA physical metadata
and immutable timestamps survived. No Python SQL translation writes occur.

Imported rows retain the CLI's `provider_used=import` / translated status (not
an invented Google/Antigravity or review attribution). `seed.targets` separately
records original provider/status and hash/ID-bound source/target provenance;
`accepted_rows` binds the new row to `seed-import-english`. Remaining pending
rows still go through the primary and optional Google fallback. No seed path
becomes a game/injection root. The retained snapshot/catalog are private game-text
artifacts; failed fresh runs are retained rather than cleaned or adopted.

## Conservative resume

Repeat the **same** invocation with `--resume` and the same output root only after
an orderly, verified failure before injection. Version/owner, tool and binary
hashes, input source/copy hashes, settings, generated config, logical DB inventory
and DB/WAL/journal hashes must all match. Before acquiring a run lock or launching
any resumed child, the entire pre-injection manifest is checked against a closed
schema: exact required/optional keys at every level, typed status/phase/run ID,
canonical resource identities, SHA-256/size and DB row counts, and an ordered
known-stage graph with typed exit codes/durations and classified validation
counts. Unknown fields (including premature installation proofs), malformed
histories and all nonresumable states refuse without republishing the input.
Only the known pending-only retry paths are allowed; extraction/pivot cannot
repeat. This is structural/drift validation, not manifest authentication.

Independently, the permitted config is reconstructed from the **current validated
options**. Both JSON semantics and exact canonical bytes must match. Ordinary
Google/Antigravity config must be exactly `{"providers":{}}`; scripted config may
contain only the fixed dummy-key/model/rate-limit fields and the exact current
literal-loopback URL. Unexpected providers/keys, `extra.agy_path`, another
loopback destination, external `base_url`, and even semantically identical
noncanonical config bytes refuse. Changing config and its recorded hash/size
together grants no authority. The stored config fingerprint must also match the
independently reconstructed fingerprint. SQLite reads use URI `mode=ro`,
`query_only`, an explicit read transaction and integrity checks, **not**
`immutable=1` (which can hide WAL contents). No Python SQL translation seeding or
DB repair occurs. SHM is link-checked but not fingerprinted because it is mutable
coordination state; logical snapshots and WAL hashes remain mandatory.

Manifest **version 2** records the exact fallback/seed/test-route settings and
closed typed seed/fallback stage graph. Version-1 runs **cannot resume**, even
though a separately verified closed failed English DB can be borrowed as a seed.
Changing explicit fallback/defaults or a test destination refuses resume.
`--english-seed` itself is fresh-run-only: when resuming a seeded pre-injection
run, omit that flag. Its recorded path remains historical normalization data;
resume independently verifies the retained snapshot/catalog/full inventory and
imported target bindings without reopening/exporting/importing the old input.
An incomplete seed-import checkpoint is not resumable. Ready checkpoint counts
must be backed by complete successful stage-bound row attribution.

Only EN-pending/ready and ES-pending/ready checkpoints qualify. Retrying
translation sends pending work with `--no-memory`; it never re-extracts the
semantic pivot, rebuilds a valid pivot, or overwrites an ambiguous pivot artifact.
All runs whose injection-start marker was ever set refuse resume, including
successful completed runs. Interrupted command/manifest/lock state or a timed-out
child also refuses automatic takeover.

On timeout, Python terminates its immediate subprocess, but **does not guarantee
termination of descendants such as agy**. Independently ensure descendants have
stopped before operating on retained paths. Inspect owned `work-game` using
Locust `inject-status`/`inject-recover` as needed; recovery is not run automatically.
Use a fresh output root from the untouched source when state is ambiguous.

## Output and verification

Stdout ends with one JSON summary. Success has `status: completed`, `patch`,
`translated_copy`, `patch_sha256`, `resources_verified`, `rollback_verified`,
`report`, and `runtime_verified: false`. Failure returns exit 1 with a sanitized
JSON diagnostic; invalid command syntax uses argparse's standard exit 2.
Provider response/source text and keys are not dumped. The
manifest retains nonsecret settings, file/resource identities, hashes, stage exit
codes/durations and hashes of stdout/stderr rather than raw command output.
Databases and copies necessarily retain game text inside the private owned root;
reports are not an anonymity guarantee for resource filenames.

Focused checks from the checkout root, after `cargo build -p locust-cli` using
the normal `target` directory (not an alternate verification target). Production
transport tests **fail**, rather than skip, if `target/debug/locust[.exe]` is
missing. Both Ubuntu and Windows CI jobs retain their frontend/fmt/Clippy/Rust
gates and additionally set up Python 3.12, build that CLI, and run each Python
suite as a separate mandatory step with `PYTHONDONTWRITEBYTECODE=1`.

```bash
PYTHONDONTWRITEBYTECODE=1 python -m unittest discover -s tools -p test_ja_en_es_pipeline.py
PYTHONDONTWRITEBYTECODE=1 python tools/test_qa_replay_oracle.py
```

In PowerShell, set `$env:PYTHONDONTWRITEBYTECODE = '1'` before the same two Python
commands.

Tests use mocked subprocess orchestration plus a threaded stdlib loopback HTTP
server and the actual production CLI. Known neutral JA/EN/ES answers exercise
Ren'Py, MV wrapping/shifted offsets, homogeneous loose KAG, duplicate resources,
proper-name identity, strict receipts, rollback and final Spanish installation.
Negative coverage includes missing answers, corrupt placeholders/raw KAG tags,
unsafe roots, warnings, incomplete counts, corrupt/foreign backup provenance,
malformed metadata, conservative resume and failed final installation.
Coordinated config/manifest endpoint and `agy_path` mutations plus unknown/nested
schema fields and malformed stage histories must refuse with **zero child
launches** and without rewriting the forged manifest. Monkeypatched publication
tests cover transient/permanent permission denials, exact retry budgets, unchanged
pending inode/bytes, foreign replacement during retry delays, reparse/special/
multiple-link refusal, retained stage ambiguity, and secondary failure-record
creation/write denial without masking the primary error. PIPE-4D tests additionally
exercise primary/fallback failure/timeout/publication, immutable accepted columns,
mixed stage/import provenance, isolated Google profiles, version/default refusal,
seed links/sidecars/locks/remnants/foreign or stale inventories, warning/memory/
recording/language evidence, forged catalogs and invented ready controls. Actual
CLI export/import (no SQL translation seeding), legacy seed-data-only authority,
owned-snapshot-only inspection, seed before/after hashes, seeded resume and
loopback pending completion all reach strict Spanish patch/install/rollback.
The existing Windows FIFO-only skip remains; missing CLI is a failure, not a skip.
Real
Google/Antigravity requests, purchases, owner credential reads, full workspace
suites, native UI, game runtime and linguistic evaluation are outside these tests.
