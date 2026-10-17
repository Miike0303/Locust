# Bounded Ace wire preservation proof (V1 only)

The standalone oracle proves one independently constructed Ruby Marshal 4.8
VX Ace fixture through the existing public CLI. It does **not** admit RGSS to
`ja_en_es_pipeline.py`, implement a Marshal runtime, or certify a game/provider.
V1 maps to S1 (another engine's synthetic pipeline), S2 (continue bounded work),
and S3 (no real games/providers/publication). V2 and the ledgers remain parent-owned.

## Quick path

From the checkout root, using the existing Python and `target/debug/locust.exe`:

```bash
python -B -m unittest discover -s tools -p test_qa_rgss_oracle.py -v
python -B tools/qa_rgss_oracle.py --help
python -B tools/qa_rgss_oracle.py fixture tmp/engine-pipeline-expansion/rgss/example-ja
python -B tools/qa_rgss_oracle.py check tmp/engine-pipeline-expansion/rgss/example-ja --phase ja
```

Use a **new** fixture directory. `fixture ROOT` refuses an existing directory;
`check ROOT --phase {ja,en,es}` is read-only and requires the exact fixed physical
language. `-h`/`--help` works globally and on both subcommands. Success exits 0,
stdout `RGSS fixture OK` or `RGSS check OK`, stderr empty. Integrity/path/IO
refusals exit 1, stdout empty, stderr starts `RGSS refused:`. Argparse misuse
exits 2 with usage/error on stderr. Examples of exact integrity messages:

- `RGSS refused: artifact graph differs: Data/Actors.rvdata2`
- `RGSS refused: artifact wire differs: Data/Actors.rvdata2`
- `RGSS refused: existing fixture refused`

Both commands confine paths beneath `tmp/engine-pipeline-expansion/rgss/`, refuse
lexical traversal and observed symlink/reparse components, and never overwrite
an existing fixture. These are local, trusted single-writer QA tools, not a
race-proof filesystem sandbox. Checks compare the four named fixture resources;
they do not audit arbitrary extra files, receipts, or unknown game trees.

## Physical proof, not re-extraction or hash transfer

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

## Public CLI test and retained evidence

`PublicCliTests.test_owned_public_lifecycle` creates a unique retained `public-*`
root. Every subprocess has an explicit 30-second timeout, runs synchronously,
and records exact argv/exit/stdout/stderr to `commands.json`. A timeout is fatal;
there is no retry, migration, build, cleanup, provider, or network command.
No CLI means an explicit unittest skip, not a successful pipeline proof.
All config/profile/backups/temp paths are isolated under that root; only OS/PATH
environment keys are inherited. Never supply a real game or credentials.

The lifecycle is:

1. Extract six exact JA rows; reject incomplete strict pivot (exit 1, no output DB,
   source database bytes/rows unchanged).
2. Export XLIFF, set six fixed neutral EN targets, production import, validate.
3. Replace-inject an owned EN copy; compare every expected resource byte.
4. Strict pivot; prove EN semantic source and JA `locust_injection_source`.
5. Export/import six ES targets, validate, Replace-inject; six writes/no skips;
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
truncated/negative/oversized lengths. XP/VX/CommonEvents, shrinking blocks,
shared selected display strings, selected-to-nondisplay alias side effects,
noncanonical original wires, and real runtime/font/linguistic behavior are unproved.

Reader refusal diagnostics identify the boundary that failed: `header/size refused`,
`truncated input`, `length refused`, `symbol tag refused`, `symbol link refused`,
`symbol encoding refused`, `duplicate symbol definition refused`, `graph limit refused`,
`object link refused`, `container/cyclic link refused`, `encoding wrapper refused`,
`encoding metadata refused`, `encoding flag refused`, `string encoding refused`,
`unsupported tag`, `duplicate ivar refused`, and `trailing input refused`.
Fixture/path diagnostics are `fixture integer refused`, `owned path refused`,
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

Writer verification is the focused eight-test command above, plus public oracle
commands. Existing-command differential is inapplicable: no existing module or
wrapper admission was edited. Parent can run the existing Python suites separately:

```bash
python -B -m unittest discover -s tools -p test_ja_en_es_pipeline.py -v
python -B -m unittest discover -s tools -p test_qa_replay_oracle.py -v
python -B -m unittest discover -s tools -p test_qa_rgss_oracle.py -v
```

No Rust/frontend build or suite was run; cleanup of shared/unclassified artifacts
remains withheld. This bounded profile needs no new graph work to repeat its own
proof. Any generic VXA wrapper admission **does** need new independent layout/
graph work (at least selected aliasing, continuation sharing/re-homing, shrinking
and alternate encodings), or an explicit hard preflight that excludes them.
Do not infer admission, general Marshal integrity, or provider semantics here.
