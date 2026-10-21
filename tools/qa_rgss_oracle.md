# Bounded Ace wire preservation proofs (V1 and V3)

The standalone oracle proves two fixed, independently constructed Ruby Marshal 4.8
VX Ace fixtures through the existing public CLI. It does **not** admit RGSS to
`ja_en_es_pipeline.py`, implement a Marshal runtime, or certify a game/provider.
V1 maps to S1 (another engine's synthetic pipeline), S2 (continue bounded work),
and S3 (no real games/providers/publication). V3 acceptance is defined by
[the owner feature, L19–L20](../odd/tasks/engine-pipeline-expansion.md), not replaced
by this guide; day-plan F4 links to that owner. Ledgers remain parent-owned.

## Quick path

From the checkout root, using the existing Python and `target/debug/locust.exe`:

```bash
python -B -m unittest discover -s tools -p test_qa_rgss_oracle.py -v
python -B tools/qa_rgss_oracle.py --help
python -B tools/qa_rgss_oracle.py fixture tmp/engine-pipeline-expansion/rgss/example-ja
python -B tools/qa_rgss_oracle.py check tmp/engine-pipeline-expansion/rgss/example-ja --phase ja
python -B tools/qa_rgss_oracle.py fixture tmp/engine-pipeline-expansion/rgss/f4-example --profile ace-common-shrink
python -B tools/qa_rgss_oracle.py check tmp/engine-pipeline-expansion/rgss/f4-example --phase ja --profile ace-common-shrink
```

Use a **new** fixture directory. `fixture ROOT` refuses an existing directory;
`check ROOT --phase {ja,en,es}` is read-only and requires the exact fixed physical
language. Both commands accept `--profile {ace-v1,ace-common-shrink}`; omission
selects the original `ace-v1` fixture without changing its bytes or guards.
The profile option is listed in subcommand help; the legacy usage synopsis is
retained so existing invalid-input stderr stays byte-identical.
`-h`/`--help` works globally and on both subcommands. Success exits 0,
stdout `RGSS fixture OK` or `RGSS check OK`, stderr empty. Integrity/path/IO
refusals exit 1, stdout empty, stderr starts `RGSS refused:`. Argparse misuse
exits 2 with usage/error on stderr. Examples of exact integrity messages:

- `RGSS refused: artifact graph differs: Data/Actors.rvdata2`
- `RGSS refused: artifact wire differs: Data/Actors.rvdata2`
- `RGSS refused: existing fixture refused`

Both commands confine paths beneath `tmp/engine-pipeline-expansion/rgss/`, refuse
lexical traversal and observed symlink/reparse components, and never overwrite
an existing fixture. These are local, trusted single-writer QA tools, not a
race-proof filesystem sandbox. Checks compare only the selected profile's named
resources (four in V1, three in V3); they do not audit arbitrary extra files,
receipts, or unknown game trees.

## V1 physical proof, not re-extraction or hash transfer

| Resource | Selected writes | Complete preservation comparison |
|---|---|---|
| `Data/Actors.rvdata2` | Name, multiline description | Ordered ivars, opaque invalid-UTF8 bytes, backward shared-string alias, `E=false` ASCII sentinel |
| `Data/Map001.rvdata2` | First message, later message, two choices | Nontext 101 header parameters, all command codes/indents, map sentinel, hash keys, page/event layout |
| `Data/System.rvdata2` | None | Entire original bytes |
| `Graphics/sentinel.bin` | None | Entire original bytes |

Fixture and expected EN/ES files come from explicit independent wire-building
vocabulary in `qa_rgss_oracle.py`, **never** production `MarshalValue` or a
production serializer. There is no production-module import in that oracle.
The test imports only the existing wrapper's read-only database utility for row
provenance; it never uses that utility to derive expected artifacts.

`check` decodes every byte of Actors/Map, compares the complete ordered graph,
then compares **full expected document bytes**. It does not mask selected
ranges, normalize observed output, scan binary with regex, or trust re-extracted
text as integrity proof. The extra wire comparison rejects even graph-equivalent
packed-length spellings. Symbol references resolve by symbol identity; object
references retain their structural target path. Full-byte equality additionally
proves the exact symbol/object-link spelling and encoding metadata for this
fixture. System/binary are compared directly. Unsupported input is refused
before comparison; nothing is written by `check`.

EN/ES expectations contain three explicitly authored <=40-character 401 lines
where JA has one. The later message ID moves `cmd_3` -> `cmd_5`; both choices
move `cmd_4` -> `cmd_6`. The expected physical lines are literal, not calculated
by the production rewrapper. Unit tests verify the fixed wrap boundaries.

## Ace/CommonEvents shrink profile (V3)

`ace-common-shrink` selects exactly two unshared UTF-8 messages in
`Data/CommonEvents.rvdata2`, with an array `[nil, RPG::CommonEvent]` and two
101/401 blocks. The first JA block has three literal 401 lines, reduced to one
literal line in EN and ES. The later message is independently translated:

| Identity | JA source / pivot DB identity | EN/ES physical identity |
|---|---|---|
| First message | `CommonEvents.rvdata2#1#cmd_1#msg` | Same `cmd_1` |
| Later message | `CommonEvents.rvdata2#1#cmd_5#msg` | `cmd_3` |

The two DB IDs stay original through strict pivot. Pivot semantic source is EN;
`locust_injection_source` stays JA, not the shortened physical EN anchor. The
public XLIFF exports corroborate the actual source texts before each import.
The ordered event ivars (`@id`, `@trigger`, `@switch_id`, `@list`, `@opaque`,
`@ascii`), every command code/indent/header parameter, raw invalid-UTF8 opaque
bytes, `E=false` ASCII, System document and binary sentinel are all covered by
independent **whole-wire and ordered-graph** equality, not masked ranges.
Expected EN/ES line contents are authored constants, not production rewrapping.
Both injected copies require two writes and zero skips. ES strict pack, apply,
rollback and reapply compare full resources; source files and inventory stay
unchanged. EN installation lifecycle is still not claimed.

Rejection tests mutate opaque bytes, 401 code, indent, ASCII encoding flag and
graph-equivalent noncanonical packed length; exact error streams/exits and
unchanged artifact bytes are asserted. Outside, existing, observed linked roots
and invalid profile are rejected without fixture creation. Missing symlink
capability is a failed proof, never a silent skip. These are observed path guards
under the trusted single-writer contract, not race-proof containment.

**Explicitly excluded in V3:** XP/VX, named/alternate encodings (including
Shift-JIS/Windows-31J), selected-field aliases or selected-to-nondisplay aliases,
container links, shared or re-homed continuations, arbitrary Marshal wires,
games/runtime/fonts/linguistic quality. The existing V1 opaque-string alias is
not selected-field alias proof. These exclusions describe QA evidence only;
they add no product preflight and certify no wrapper admission profile.

## Public CLI test and retained evidence

`PublicCliTests.test_owned_public_lifecycle` (V1) and
`test_shrink_public_lifecycle` (V3) create separate unique retained `public-*`
roots. Every subprocess has an explicit 30-second timeout, runs synchronously,
and records exact argv/exit/stdout/stderr to `commands.json`, with original stream
bytes in `cli-NN.stdout`/`cli-NN.stderr` (strict UTF-8 decoding, no replacement).
A timeout is fatal;
there is no retry, migration, build, cleanup, provider, or network command.
Without an override, a missing local checkout CLI is still an explicit unittest
skip; a missing CI checkout CLI is still a failure. Tests additionally accept
`LOCUST_QA_RGSS_CLI` as an explicit candidate file path. Missing/empty explicit
paths fail even outside CI; they never skip or fall back. This selector is test
code only: it does not build, install or copy binaries. For example in Git Bash:

```bash
LOCUST_QA_RGSS_CLI="$PWD/.test-tmp/e7d-parent-proof/e7d-candidate.exe" python -B -m unittest discover -s tools -p test_qa_rgss_oracle.py -v
```

That retained candidate is attributed to parent E7d checks (day-plan L59), not
asserted to match the exact current plugin ancestry by its hash.
All config/profile/backups/temp paths are isolated under that root; only OS/PATH
environment keys are inherited. Never supply a real game or credentials.

Both profiles use this lifecycle (six selected rows in V1, two in V3):

1. Extract the exact JA rows; reject incomplete strict pivot (exit 1, no output DB,
   source database bytes/counters/rows unchanged; no destination or destination
   sidecars). Read-only SQLite may retire transient source SHM/WAL files; their
   presence is not asserted as persistent project state.
2. Export XLIFF, set the fixed neutral EN targets, production import, validate.
3. Replace-inject an owned EN copy; compare every expected resource byte.
4. Strict pivot; prove EN semantic source and JA `locust_injection_source`.
5. Export/import ES targets, validate, Replace-inject; six/two writes, no skips;
   compare every expected resource byte, including accented UTF-8 and metadata.
6. Re-extract to corroborate moving IDs; package strict patch with pristine source;
   apply to another owned copy; compare ES; rollback and compare JA; final reapply
   and compare ES. Verify the source fixture never changed.

`proof.json` records original/pivot/re-extracted rows, write report, full-byte
equality results, and executable SHA-256.
`expected-en/` and `expected-es/` retain independent expected bytes alongside
actual copies, DBs, catalogs, patch, receipt, and backups. The executable hash
identifies the tested binary, **not** its correspondence to current Rust sources.
Do not delete these proof roots as generic scratch.

## Strict profile and limits

Supported: nil/booleans, bounded fixnums, arrays, ordinary hashes, ordered ordinary
objects with unique ASCII ivar names, unique ASCII symbols/references, raw byte
strings, and string-only `I` metadata with exactly `E=true` UTF-8 or `E=false`
ASCII. Only backward links to previously registered strings are accepted.
Limits: 64 KiB document, 4096-byte blobs/counts, 64-byte symbol names, maximum
nesting depth 32 (root depth zero), 2048 decoded values. Complete input consumption is
mandatory. These ceilings protect the fixture, not arbitrary RGSS workloads.

Refused: forward/container/cyclic links, duplicate symbol definitions/ivars,
non-ASCII symbols, named encodings, non-string wrappers, invalid UTF-8/ASCII,
unknown tags, floats/Bignum, structs/classes/modules/regexp, user data (`u` Table
included), hash defaults, arbitrary sharing/re-homing of command objects, and
truncated/negative/oversized lengths. XP/VX, arbitrary CommonEvents/shrinking blocks,
shared selected display strings, selected-to-nondisplay alias side effects,
noncanonical original wires, and real runtime/font/linguistic behavior are unproved
outside the exact profiles. CommonEvents/shrinking are now proved only for the
V3 fixed profile above, not arbitrary layouts.

Reader refusal diagnostics identify the boundary that failed: `header/size refused`,
`truncated input`, `length refused`, `symbol tag refused`, `symbol link refused`,
`symbol encoding refused`, `duplicate symbol definition refused`, `graph limit refused`,
`object link refused`, `container/cyclic link refused`, `encoding wrapper refused`,
`encoding metadata refused`, `encoding flag refused`, `string encoding refused`,
`unsupported tag`, `duplicate ivar refused`, and `trailing input refused`.
Fixture/path diagnostics are `fixture profile refused` (Python API),
`fixture integer refused`, `owned path refused`,
`linked path refused`, `existing fixture refused`, and `fixture file/size refused`.
Comparison diagnostics name the relative resource after `artifact graph differs:`,
`artifact wire differs:`, or `unchanged resource differs:`. OS failures retain the
platform IO description after the same `RGSS refused:` prefix; nothing is retried.

## Read-only production source map

| Source | Relevant behavior (not independent proof) |
|---|---|
| `crates/formats/src/rpgmaker_vxa.rs:467-586` | Definition registration, object/symbol links and encoding wrappers |
| `crates/formats/src/rpgmaker_vxa.rs:955-1152` | Actor fields, map 101/401 grouping and choices |
| `crates/formats/src/rpgmaker_vxa.rs:1212-1478` | Rewrap, descending command operations, growing 401 runs |
| `crates/formats/src/rpgmaker_vxa.rs:1565-1718` | Extract, pristine read, source guards, serialize/write report |
| `crates/formats/tests/rpgmaker_marshal_lossless.rs:1-220` | Existing hand-wire tests; broader tags/encodings than this oracle |
| `crates/formats/tests/rpgmaker_marshal_lossless.rs:241-427` | Existing XP links, deleted-command re-homing, Ace continuation metadata and malformed-input tests |
| `tools/ja_en_es_pipeline.py:710` | Existing read-only database snapshot utility used by tests only |

The older production `build_test_fixture` uses `MarshalValue::serialize`; this
QA deliberately does not call it. Existing broader Rust tests were read, not run.

## Parent checks and next decision

V3 writer verification includes focused profile/selector tests, the complete
RGSS QA suite (both public lifecycles), existing replay-oracle tests, structural
self-checks and an OS-scratch comparison against `5d268f9`. The differential
compares exact default command exits/stdout/stderr and full V1 JA/EN/ES wires;
it is an external evidence helper, never Git-history access in committed tests.
Raw receipts, failures, artifacts and the handoff are retained under the unique
`f4-*` roots. Parent independently verifies Risk item 2 (owned fixture/candidate
boundary) and owns cleanup/Git/ledger/index/memory.
Parent can run the existing Python suites separately:

```bash
python -B -m unittest discover -s tools -p test_ja_en_es_pipeline.py -v
python -B -m unittest discover -s tools -p test_qa_replay_oracle.py -v
python -B -m unittest discover -s tools -p test_qa_rgss_oracle.py -v
```

No Rust/frontend build or suite was run; cleanup of shared/unclassified artifacts
remains withheld. This bounded profile needs no new graph work to repeat its own
proof. Any generic VXA wrapper admission **does** need new independent layout/
graph work beyond these fixed layouts (selected aliasing, continuation
sharing/re-homing, arbitrary shrinking and alternate encodings), or a separately
authorized product boundary that excludes them. The QA exclusions here are not
such a product preflight.
Do not infer admission, general Marshal integrity, or provider semantics here.
