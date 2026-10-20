# Research note: Ren'Py compiled-only .rpyc

Read-only Codex research, 2026-10-19 (day plan [L23..L25](../../odd/tasks/day-plan-2026-10-19.md)). Repository facts cite file:line at the time of writing; sections labelled as format background are external knowledge, not repository facts. Nothing here was verified by running tests.

## Current state

**Compiled-only support already exists for archived scripts, but loose `.rpyc` support is incomplete.**

- **Detection:** individual files match only `.rpy`/`.rpa`; game roots require immediate `game/` `.rpy` or `.rpa` files. Loose compiled-only games therefore detect **false**. Forced directory extraction ignores loose `.rpyc` and returns zero rows if nothing else qualifies. Forced single-file extraction reads it as UTF-8 source, normally producing a decoding error. See [renpy.rs:2440](../../crates/formats/src/renpy.rs), [renpy.rs:2472](../../crates/formats/src/renpy.rs), [renpy.rs:574](../../crates/formats/src/renpy.rs).
- **Archived compiled scripts:** `.rpa` detects true. Extraction includes orphan `.rpyc`, prefers exact `.rpy` twins, skips `tl/`, harvests compiled strings, and tags rows `dialogue,rpyc`. Thus compiled-only archives can already produce rows. Unharvestable scripts return zero rows with a warning; archive errors propagate for direct extraction but become warnings during directory scans. See [renpy.rs:544](../../crates/formats/src/renpy.rs), [renpy.rs:64](../../crates/formats/src/renpy.rs), [renpy.rs:117](../../crates/formats/src/renpy.rs), [renpy.rs:2521](../../crates/formats/src/renpy.rs).
- **Two distinct pickle readers:** the RPA index decoder near line 2381 supports containers, memo operations, integers, string/bytes variants and selected protocol-4 operations; unknown opcodes error. It is not the bounded AST reader. RPA index inflation is unbounded, and member paths are joined directly to the temporary directory. See [renpy.rs:1995](../../crates/formats/src/renpy.rs), [renpy.rs:2381](../../crates/formats/src/renpy.rs), [renpy.rs:511](../../crates/formats/src/renpy.rs), [renpy.rs:558](../../crates/formats/src/renpy.rs).
- **AST harvesting:** RPC2 slot 1, 64-MiB inflation cap, inert object graph, resource limits. Supported operations include containers, memoization, `GLOBAL`/`STACK_GLOBAL`, `NEWOBJ`, inert `REDUCE`, and `BUILD`. Protocol headers above 4 fail; **`FRAME` is unsupported**, so protocol-4 support is partial. Known fields include Say, Menu, TranslateString and selected screen literals; UserStatement is excluded. Failure triggers heuristic scanning; output deduplicates by text and loses statement identity. See [renpy.rs:1706](../../crates/formats/src/renpy.rs), [renpy.rs:1758](../../crates/formats/src/renpy.rs), [renpy_pickle.rs:4](../../crates/formats/src/renpy_pickle.rs), [renpy_pickle.rs:182](../../crates/formats/src/renpy_pickle.rs), [renpy_pickle.rs:465](../../crates/formats/src/renpy_pickle.rs).
- **Reinjection:** compiled rows generate `game/zzz_locust_translate.rpy`, chaining `config.say_menu_text_filter`; compiled files remain unchanged. Identical source text shares one mapping, and screen/UI coverage is unproven. See [renpy.rs:133](../../crates/formats/src/renpy.rs), [renpy.rs:212](../../crates/formats/src/renpy.rs), [renpy.rs:2649](../../crates/formats/src/renpy.rs).
- Tests cover typed ASTs, malformed inputs, memo identity, bombs and filter generation ([renpy.rs:3631](../../crates/formats/src/renpy.rs)). Read-only inspection found `c127-scripts.rpa` contains source `scripts/story.rpy`, not compiled dialogue; its CLI test proves source-overlay restoration ([backup_recovery_tests.rs:11](../../crates/cli/tests/backup_recovery_tests.rs)).

## Format background

**External format knowledge, checked against upstream—not repository facts:**

RPC2 has a ten-byte `RENPY RPC2` header and 12-byte slot/offset/length records, normally little-endian, ending with slot zero. Slots contain zlib-compressed pickles of `(metadata, statements)`: slot 1 precedes static transformations; slot 2 follows them. Legacy files can be bare zlib pickles. [Upstream script implementation](https://raw.githubusercontent.com/renpy/renpy/master/renpy/script.py).

`renpy.ast.Say.what` carries dialogue; `Menu.items` carries captions/choices; `TranslateString.old/new` carries translation pairs. `UserStatement` contains extension-defined data requiring custom interpretation. Screen-language nodes contain expressions, only some statically recoverable. [Upstream AST](https://raw.githubusercontent.com/renpy/renpy/master/renpy/ast.py).

Ren’Py 6.99/7 commonly write Python-2 protocol 2. Ren’Py 8 uses Python 3 but initially retained protocol 2; newer upstream uses highest available protocol. Consequently “8 means protocol 4” is unsafe. Python-2 byte strings, Unicode, class names and reconstruction conventions need explicit handling. [6.99](https://raw.githubusercontent.com/renpy/renpy/6.99.14.3.3347/renpy/script.py), [7](https://raw.githubusercontent.com/renpy/renpy/7.4.11.2266/renpy/script.py), [early 8](https://raw.githubusercontent.com/renpy/renpy/8.0.3.22090809/renpy/compat/pickle.py), [current](https://raw.githubusercontent.com/renpy/renpy/master/renpy/compat/pickle.py).

## Options

**(a) Extend the restricted VM.** Highly feasible: it already exists. Preserve occurrences, classes, fields and translation-unit metadata; never resolve globals or execute constructors. Binary rewriting additionally requires faithful serialization, slot consistency, metadata/digest handling and version compatibility. Proof requires engine-generated fixtures and load/behavior equivalence across supported versions.

**(b) Generate native `tl/<lang>/*.rpy`.** Preferred reinjection direction. Original scripts need no recompilation; the engine compiles added translation files. Read stored `Translate.identifier` from transformed slots where available. Otherwise reproduce version-specific statement canonicalization, grouping, label normalization, explicit IDs and collision numbering. Existing Add mode supplies scaffolding but explicitly loses attributes and grouped statements ([renpy.rs:2959](../../crates/formats/src/renpy.rs)). Translation identifiers and compilation digests are different mechanisms. Proof: compare identifiers against engine-generated translations, then verify dialogue, menu behavior, voice/NVL and saves. [Translation model](https://www.renpy.org/doc/html/translation.html).

**(c) External unrpyc.** Enables decompile-to-existing-source-path integration, but introduces Python, dependency/version management and reconstruction risk. It fails the no-Python requirement; subprocess isolation alone does not establish safety. Require pinned-tool security review and engine equivalence tests. [unrpyc requirements](https://github.com/CensoredUsername/unrpyc).

## Recommendation

Extend the existing reader and use native translation overlays. Keep the runtime filter as an explicitly limited fallback. Defer binary rewriting and external dependencies.

## First unit

Implement **loose RPC2 parity only**, reusing harvesting and filter injection.

Acceptance checks: direct-file/root detection; nested discovery; exact source-twin preference; generated/`tl` exclusions; archived/loose extraction equivalence; deterministic IDs; unchanged original bytes; escaped overlay output; explicit malformed/unsupported diagnostics; bounded resource use.

Extend existing Rust fixture builders ([renpy.rs:3383](../../crates/formats/src/renpy.rs)) with synthetic Say/Menu graphs, memo aliases, Unicode and malicious reductions, wrapped loose and inside synthetic RPA. Later add tiny author-owned scripts compiled by pinned Ren’Py versions for compatibility proof.

No files edited; tests were inspected, not run.