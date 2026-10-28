# TMX project catalogs: the Locust plaintext profile

The CLI and core codec support a **restricted bilingual TMX 1.4b Level-1 project
catalog**, not general translation-memory interchange. Catalogs preserve exact
project identities, source text and review provenance. CLI import writes only the
selected project through the existing transactional importer, never global memory.
Generic CAT/Level-2 support remains outside this profile.

## Desktop and browser import preview

PO, XLIFF and TMX imports in the desktop/browser dialog first show a read-only
preview. Keep existing translations is selected initially; replacing existing
work is an explicit choice. The summary separates empty rows to fill,
replacements, unchanged rows, unknown IDs, outdated sources and protected local
translations. Replacement details include the file, previous text/status and new
text; at most 200 rows are shown, with a truncation notice when needed.

HTTP clients can POST `{content, keep_existing}` to
`/api/import/{format}/preview`, then POST the same fields plus the returned
`plan_token` to `/api/import/{format}/apply`. Native commands
`preview_import_translations` and `apply_import_preview` use the same parser,
preview and guarded apply with a catalog `path`. TMX languages come from the
catalog's declarations and still pass the strict bilingual codec validation.
Changing the project, catalog bytes, preservation choice or affected rows makes
the plan stale: apply returns HTTP 409 (or the same native error message), saves
nothing and asks for a new preview. Legacy direct-import APIs and CLI behavior
remain available.

## CLI quick path

```sh
locust export project.locust.db -f tmx --source-lang en -l es -o project-es.tmx
# Review/edit targets without changing encoded IDs, exact sources or languages.
locust import project.locust.db -f tmx --source-lang en -l es -i project-es.tmx --dry-run
locust import project.locust.db -f tmx --source-lang en -l es -i project-es.tmx --keep-existing
```

`--source-lang` and `-l`/`--lang` are explicit source and target tags for TMX.
Neither project history nor config supplies a missing TMX source language.
For PO/XLIFF, `--source-lang` is accepted but **ignored**, as the help states:
export still resolves its source language from project run history/config, and
import retains its existing language behavior. Omit that flag for PO/XLIFF.

| Operation | Behavior |
|---|---|
| Export selection | `--status` repeats form an ordered union; `--file` matches the exact stored path and intersects statuses. Empty selection fails without creating/replacing a catalog. |
| Export destination | Existing files require `--overwrite`; project DB, its SQLite sidecars and aliases remain protected, even with overwrite. Serialization/language errors do not replace an existing catalog. |
| Import validation | UTF-8 file reads are capped at 32 MiB + one detection byte before decoding. All languages, units and the document tail validate before any project write. Invalid UTF-8, ambiguous/malformed XML and exceeded limits fail the whole import. |
| Import matching | Decoded ID **and exact current source** must match. Unknown IDs and outdated sources are skipped/counted, never created or source-matched by fallback. No source trimming or Unicode normalization. |
| Preview | `--dry-run` shows replacements/confirmations and fill counts, with no saved row or counter changes. |
| Preserve local work | `--keep-existing` keeps differing nonblank saved translations; it still fills blank rows and permits explicit confirmation of identical pending text. |
| Status/provenance | New/replaced/confirmed text becomes `translated`, provider `import`, not `approved`. Identical non-pending rows count as imported/unchanged and preserve approval, timestamps and metadata. Reimport is idempotent. |

Successful imports print `Imported N translations from …`; N includes identical
non-pending rows. Additional `Unchanged`, `Kept` and `Skipped` lines explain the
result. `Skipped` separates outdated sources, unknown IDs and empty/missing/fuzzy
entries (the shared label includes TMX review-marked targets). Empty targets never
erase saved text. A whitespace-only target is nonempty and remains exact; this is
not a completeness or linguistic-quality gate. Preview uses `Would replace` and
`Would fill` instead of `Imported`, followed by `Dry run: nothing was saved.`

A `x-locust-needs-review=true` property always skips that TU, even when its target
was edited. After reviewing against the current source, remove the **whole
property** to submit the text for ordinary exact-source matching. See the review
contract below before editing catalogs with an external tool.

## CLI verification scope

Local integrated checks passed: 2,747 Rust tests (25 ignored), 759 frontend tests,
production build/typecheck, format and strict workspace Clippy. Python gates ran
84 pipeline cases (one Windows-only FIFO skip), 13 replay, 14 RGSS and 12 updater
cases successfully. These counts describe local verification, not hosted CI.

The independent CLI suite passed 11 tests. Retained same-path baseline/candidate
PO/XLIFF comparisons passed 28 ordinary cases with exact stdout, stderr and exit
codes (and matching export bytes), without normalization. Four additional cases
verify the documented acceptance/ignore behavior of the new `--source-lang` flag.
The independent evidence lives locally under
`.test-tmp/tmx-cli-independent-evidence/`; it is not shipped in Git.

The writer reported a failing pre-implementation CLI test (missing flag), followed
by GREEN. Its original runtime-owned raw logs were removed at task teardown;
that historical RED remains writer-attributed, not independently reproduced or
claimed retained. The independently recreated compatibility evidence above has
its own count and provenance. Native gameplay, providers and arbitrary external
CAT round-trips are not certified by these checks.

## Public API and safe application order

Use the reexports in `locust_core::export`:

1. `export_tmx(entries, source_lang, target_lang) -> Result<String>` validates
   everything before returning a UTF-8 document. Write it only after success and
   apply the existing export-destination guard before opening an output file.
2. `import_tmx(content, source_lang, target_lang) -> Result<Vec<TmxUnit>>` validates
   the complete document, including its tail. No partial units escape on error.
3. `tmx_units_for_batch(&units) -> (Vec<ImportedTranslation>, usize)` preserves
   exact ID/source/target text and counts stale or empty targets as skipped.
4. Pass those updates to the existing project preview/transactional importer.
   That layer, not TMX, owns exact-source matching, unknown-ID accounting,
   keep-existing, dry-run and destination guards. Never apply units incrementally.

`TmxUnit` exposes `id`, `source`, `target` and `needs_review`. An empty target is
not a request to erase saved text; a whitespace-only target is nonempty and exact.
No translation-language selection, provider call or database write is implicit.
PO and XLIFF implementations are unchanged.

## Wire contract

| Area | Accepted profile |
|---|---|
| Envelope | One unnamespaced `<tmx version="1.4">`, one header followed by one body; an empty body is valid. |
| Header | Required nonempty `creationtool`, `creationtoolversion`, `o-tmf`, `adminlang`, `srclang`, `segtype`, `datatype`. Export uses Locust/package version, `o-tmf="Locust"`, `adminlang="en"`, `segtype="block"`, `datatype="plaintext"`. Import also accepts sentence/paragraph/phrase segmentation; datatype must be plaintext. Header has no child elements. |
| Identity | Every TU requires `tuid="locust-v1-"` followed by hex UTF-8 bytes of the **entire exact nonempty project ID**. Export uses lowercase hex; import accepts either hex case and rejects duplicate decoded IDs. No source-only fallback, positional matching or source deduplication. |
| Languages | Caller supplies two concrete distinct tags. Accepted syntax: 2–8 ASCII letters, followed by zero or more hyphen-separated 1–8 ASCII alphanumeric subtags; `und`, `mul`, `zxx` bases are excluded. Comparison is ASCII case-insensitive exact-tag comparison, not locale guessing: `es` differs from `es-ES`. This is a bounded syntax profile, not a complete BCP-47 registry validator. |
| Variants | Exactly one source and one target `<tuv xml:lang="…"><seg>…</seg></tuv>`, in either order. Missing, repeated or third languages fail. Header `srclang` must match the selected source; an optional TU `srclang` must also match. `*all*` is not supported. |
| Text | Plain XML text, predefined/numeric character references and CDATA. Spacing, tabs, Unicode, newlines, literal game controls and repeated sources are significant. No inline elements, even TMX Level-2 codes. |
| Review | At most one TU `<prop type="x-locust-needs-review">true</prop>`, before the variants; see below. |
| Attributes | Closed allowlist: root version; the seven header fields; TU tuid/srclang; TUV xml:lang; prop type. No attributes on body/seg. Unknown/duplicate attributes and namespace declarations/prefix aliases fail, including an explicit declaration of the otherwise implicit `xml` prefix. |
| XML | XML 1.0 legal characters in raw XML, decoded attributes/segments and decoded IDs. Optional initial XML 1.0 declaration (UTF-8 only if encoding is specified; standalone yes/no); comments allowed, no DTD or other processing instructions. Trailing comments/literal XML whitespace allowed, but no second root, trailing text, reference or incomplete XML. |

Unknown elements/properties and foreign namespaces are errors, not metadata to
silently discard. The codec does not load DTDs, resolve external entities, perform
network access or flatten protected game controls.

### Line endings and identity

Physical CRLF and CR normalize to LF **before** XML reference decoding, including
inside CDATA. Export uses the existing XML escaping helper, which writes CR as
`&#13;`; references decode after normalization, preserving content CR/CRLF exactly.
Attribute whitespace normalizes before references are decoded. Hex IDs have no
literal whitespace, so IDs containing spaces, tabs, CR/LF, quotes, delimiters and
Unicode are reversible. XML-illegal characters in decoded IDs are rejected even
though hex could physically transport them. No Unicode normalization occurs.

### Stale provenance is not approval

Export emits `x-locust-needs-review=true` whenever the row contains the existing
stale-translation metadata key, regardless of its stored value. The old target
text remains visible for review. Import retains that marker as `needs_review`;
the adapter skips it even if its target text was edited. Empty targets also skip.

After reviewing against the current source, a translator must **remove the whole
property** to submit a nonempty target for ordinary exact-source validation.
`false`, blank, duplicate, unknown and misplaced properties are rejected rather
than interpreted as approval. Removing the marker can confirm unchanged stale
text through the existing database importer; that is an explicit review action,
not automatic evidence that the old translation fits a changed source.

## Hard limits (inclusive)

| Resource | Maximum |
|---|---:|
| Input UTF-8 document / exported UTF-8 document | 32 MiB (33,554,432 bytes), before input line-ending normalization |
| Translation units | 100,000 |
| Each decoded source or target segment | 1 MiB (1,048,576 UTF-8 bytes), accumulated across text/CDATA events |
| Each decoded project ID | 16 KiB (16,384 UTF-8 bytes); hex payload at most 32 KiB |
| Element nesting | 5 (`tmx/body/tu/tuv/seg`); unsupported nesting fails immediately |
| Review-property decoded value | Exactly four bytes: `true` |

The API takes `&str`, so the caller must enforce a bounded UTF-8 file read before
allocating input; the codec checks again before parsing/normalizing. Limits bound
catalog processing, not total process memory: parsed units, identity sets,
escaping and normalized copies also consume memory. Export errors rather than
truncating, splitting segments, dropping rows or emitting a partial catalog.

## Deliberate external CAT limitations

General TMX does not require Locust's reversible unique IDs or exactly two
variants. CAT tools may regenerate IDs, merge repeated sources, insert inline
codes/notes, add standard optional attributes, declare namespaces, change tags,
remove custom properties or emit DTDs. Such output is either rejected by this
profile or, if a review marker is removed, submitted as explicitly reviewed text.
Do not assume arbitrary CAT round-trips preserve stale provenance. Preserve IDs,
exact source text, selected language tags and review properties when editing.
There is no generic TMX/Level-2 support or external-CAT compatibility certification.

Reference: [TMX 1.4b specification](https://www.ttt.org/oscarStandards/tmx/tmx14b.html),
notably 3.1.1, 3.2.1 and 3.2.2. Restrictions above are Locust policy, not claims
about all standards-valid TMX documents.

## T1 verification evidence

These retained diagnostic/result logs are embedded here because this work unit's
allowed paths include no separate log directory. Dependency-compilation chatter
is omitted. This is local synthetic codec evidence, not CLI, native-game, provider,
external-CAT or hosted-CI proof. S1–S3 are covered **for T1**; T2/T3 remain separate.

All Cargo invocations used the existing installed toolchain, checkout `target/`,
PATH prefixed with `/c/Users/Mike/.cargo/bin:/c/msys64/mingw64/bin`, and:

```sh
export CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_INCREMENTAL=0
```

### RED log

Command: `cargo test --locked -p locust-core --test tmx_catalog_tests`
Baseline with public tests written and no TMX implementation: **exit 101**, zero
tests executed. Observed diagnostic excerpt:

```text
error[E0432]: unresolved imports `locust_core::export::export_tmx`, `locust_core::export::import_tmx`, `locust_core::export::tmx_units_for_batch`
 --> crates\core\tests\tmx_catalog_tests.rs:3:14
  |
3 |     export::{export_tmx, import_tmx, tmx_units_for_batch, ImportedTranslation},
  |              ^^^^^^^^^^  ^^^^^^^^^^  ^^^^^^^^^^^^^^^^^^^ no `tmx_units_for_batch` in `export`
  |              |           |
  |              |           no `import_tmx` in `export`
  |              no `export_tmx` in `export`
error[E0308]: mismatched types
   --> crates\core\tests\tmx_catalog_tests.rs:224:59
    | expected `Vec<ImportedTranslation>`, found `[ImportedTranslation]`
error: could not compile `locust-core` (test "tmx_catalog_tests") due to 2 previous errors
Command exited with code 101
```

The secondary diagnostic disappeared when the API existed; no conversion workaround
was added to the test. First implementation run compiled and passed all 10 tests
(exit 0; 0 failed/ignored, 6.20s). Subsequent self-check added declaration-order and
outside-root reference negatives. Formatting checks initially returned exit 1
(only the two new Rust files); the correction batch applied those formatting diffs.

### Size-guard RED log

Final self-check added an empty-catalog/oversized-header regression. With the final
output-size guard absent, the same exact public-test command exited **101**:

```text
---- document_and_unit_caps_apply_on_import_and_export stdout ----
thread 'document_and_unit_caps_apply_on_import_and_export' (41056) panicked at crates\core\tests\tmx_catalog_tests.rs:266:5:
assertion failed: export_tmx(&[], &long_language, "es").is_err()
test result: FAILED. 9 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.59s
error: test failed, to rerun pass `-p locust-core --test tmx_catalog_tests`
```

A final output-length check now covers empty catalogs as well as the existing
per-unit checks. All required commands were rerun on that final Rust candidate.

### GREEN log

`cargo fmt --check`: **exit 0**, no output. Earlier formatting-only attempts
returned 1 twice (the second identified three remaining wrapped test expressions);
both are resolved. No workspace-wide formatter was run and no unrelated file was
rewritten.

`cargo test --locked -p locust-core --test tmx_catalog_tests`: **exit 0**:

```text
   Compiling locust-core v0.2.0 (C:\Projects\Locust\crates\core)
    Finished `test` profile [unoptimized] target(s) in 9.36s
     Running tests\tmx_catalog_tests.rs (target\debug\deps\tmx_catalog_tests-fbd384b128584adc.exe)

running 10 tests
test exact_public_roundtrip_and_required_header ... ok
test complete_document_validation_rejects_late_or_malformed_xml ... ok
test cdata_references_and_physical_line_endings ... ok
test xml_10_character_contract_is_fallible_both_ways ... ok
test languages_are_explicit_concrete_distinct_and_exact ... ok
test validates_every_attribute_namespace_entity_and_header ... ok
test rejects_ambiguous_identity_and_structure ... ok
test stale_empty_and_exact_source_guards_reuse_existing_batch_contract ... ok
test decoded_segment_and_id_limits_are_inclusive ... ok
test document_and_unit_caps_apply_on_import_and_export ... ok

test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 7.69s
```

`cargo test --locked -p locust-core export::`: **exit 0**:

```text
   Compiling locust-core v0.2.0 (C:\Projects\Locust\crates\core)
    Finished `test` profile [unoptimized] target(s) in 19.11s
     Running unittests src\lib.rs (target\debug\deps\locust_core-0e211b97bde02c81.exe)
running 42 tests
test result: ok. 42 passed; 0 failed; 0 ignored; 0 measured; 731 filtered out; finished in 0.02s
```

The latter command also compiled all core integration-test binaries, but their
tests were filtered out by `export::`; it is not evidence of a full core-suite
run. It covers the existing PO/XLIFF unit tests and export-destination guards,
not the T2 public-command differential checks.

### Cleanup log

Both pre- and post-batch commands exited **0**:

```sh
powershell -NoProfile -File tools/cleanup-scratch.ps1 -Apply -MinAgeHours 876000 -Prefix locust-owned-tmx-20261021-
```

The preceding `git worktree prune --dry-run` had no output. Cleanup skipped all
prior test/evidence entries, with `freed GB: 0` both times. Checkout target was
reported as 0.01 GB before and 0.7 GB after (under its 30 GB cap); no alternate
Cargo target was created. Existing ledger/index changes were preserved.

### Self-check and remaining boundary

- **S1:** Fallible core export/import and public reexports implemented; no CLI,
  UI, provider, schema, global-memory or real-game changes.
- **S2:** The separate TMX module reuses `ImportedTranslation`, XML escaping and
  physical-line-ending normalization. PO/XLIFF bodies are unchanged; their 42
  selected tests pass. The explicit parser, public tests and contract/evidence
  docs exceed the approximate 400-line slice target; none were compressed away.
- **S3:** Public tests cover exact whitespace/Unicode IDs, literal control text,
  CR references/physical endings, language/identity ambiguity, malformed late
  XML, attributes/namespaces/entities, XML legality, inclusive byte/unit caps,
  stale/empty skips and explicit stale confirmation through the existing importer.
  Typechecking succeeded in both test commands. Database snapshots remain equal
  for skipped stale rows, dry-run previews and a rejected tail.

The parser's internal `unwrap`s depend on the closed grammar: segments/properties
can only occur inside a current TU, and TU closure consumes it once. External CAT
interoperability, CLI counters/keep-existing/destination integration, broad tests,
independent verification and hosted CI remain parent/T2/T3 responsibilities.

## T1 correction evidence: required attribute separators

The independent probe identified three S3 violations accepted by the original
candidate: adjacent header creationtool/creationtoolversion attributes, adjacent
header segtype/o-tmf attributes, and adjacent XML-declaration version/encoding
pseudo-attributes. The parent's job-1 was deliberately **stopped/incomplete**, not
passed; the parent then granted this correction an exclusive source-write/Cargo
slot. No compilation or source writes occurred before that grant.

The correction checks raw attribute bytes for XML whitespace before the first
attribute and after each closing value quote. It tracks single/double quotes so
an opposite quote inside a value remains content. The same helper protects all
TMX start tags and the declaration; quick-xml still validates the remaining
attribute syntax. No regex, dependencies, PO/XLIFF edits or profile relaxation
were introduced. This section adds evidence only; the wire contract above stands.

### Separator RED log

With the three public negative tests and a positive whitespace test added **before
the implementation change**, the actual candidate reproduced all three defects.
Command: `cargo test --locked -p locust-core --test tmx_catalog_tests`; exit **101**.
Retained diagnostic/result excerpts:

```text
   Compiling locust-core v0.2.0 (C:\Projects\Locust\crates\core)
    Finished `test` profile [unoptimized] target(s) in 1.25s
running 14 tests
test rejects_missing_separator_after_creationtool ... FAILED
test rejects_missing_separator_in_xml_declaration ... FAILED
test rejects_missing_separator_after_segtype ... FAILED
test accepts_xml_whitespace_between_attributes ... ok

---- rejects_missing_separator_after_creationtool stdout ----
accepted: <?xml version="1.0" encoding="UTF-8"?>
<tmx version="1.4">
<header creationtool="Locust"creationtoolversion="0.2.0" segtype="block" o-tmf="Locust" adminlang="en

---- rejects_missing_separator_in_xml_declaration stdout ----
accepted: <?xml version="1.0"encoding="UTF-8"?>
<tmx version="1.4">
<header creationtool="Locust" creationtoolversion="0.2.0" segtype="block" o-tmf="Locust" adminlang="en

---- rejects_missing_separator_after_segtype stdout ----
accepted: <?xml version="1.0" encoding="UTF-8"?>
<tmx version="1.4">
<header creationtool="Locust" creationtoolversion="0.2.0" segtype="block"o-tmf="Locust" adminlang="en

test result: FAILED. 11 passed; 3 failed; 0 ignored; 0 measured; 0 filtered out; finished in 8.63s
error: test failed, to rerun pass `-p locust-core --test tmx_catalog_tests`
```

### Separator GREEN log

After the shared-helper fix, the positive test was also expanded to cover first
attribute separators, mixed quote styles and self-closing headers. Both quote
styles reject each missing-separator case. All commands used the same PATH,
checkout target and no-debug/no-incremental settings documented above.

`cargo test --locked -p locust-core --test tmx_catalog_tests`: exit **0**:

```text
    Finished `test` profile [unoptimized] target(s) in 8.79s
running 14 tests
test rejects_missing_separator_in_xml_declaration ... ok
test rejects_missing_separator_after_segtype ... ok
test rejects_missing_separator_after_creationtool ... ok
test xml_10_character_contract_is_fallible_both_ways ... ok
test cdata_references_and_physical_line_endings ... ok
test exact_public_roundtrip_and_required_header ... ok
test languages_are_explicit_concrete_distinct_and_exact ... ok
test validates_every_attribute_namespace_entity_and_header ... ok
test rejects_ambiguous_identity_and_structure ... ok
test complete_document_validation_rejects_late_or_malformed_xml ... ok
test accepts_xml_whitespace_between_attributes ... ok
test stale_empty_and_exact_source_guards_reuse_existing_batch_contract ... ok
test decoded_segment_and_id_limits_are_inclusive ... ok
test document_and_unit_caps_apply_on_import_and_export ... ok

test result: ok. 14 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 8.29s
```

`cargo test --locked -p locust-core export::`: exit **0**:

```text
    Finished `test` profile [unoptimized] target(s) in 19.35s
running 42 tests
test result: ok. 42 passed; 0 failed; 0 ignored; 0 measured; 731 filtered out; finished in 0.01s
```

All core integration-test binaries compiled; their tests were filtered out by
`export::` (including 14 TMX cases), so this is not full-suite execution.
`cargo fmt --check`: exit **0**, no output; no formatter rewrite was needed.

The exact cleanup command below ran before RED and after GREEN, exit **0** both
times; prior scratch/evidence was skipped, `freed GB: 0`, checkout target 2.5 GB
(under its 30 GB cap). The preceding worktree-prune preview had no output.

```sh
powershell -NoProfile -File tools/cleanup-scratch.ps1 -Apply -MinAgeHours 876000 -Prefix locust-owned-tmx-20261021-
```

Correction self-check: the raw-byte pass recognizes only space/tab/LF/CR as XML
whitespace, never decoded references, and never strips segment/attribute content.
Positive public cases exercise space, tab, LF, CR, CRLF and mixed whitespace,
whitespace around equals and before tag closure, literal opposite quotes inside
values, mixed attribute quote styles, and empty-header syntax. S1/S2 boundaries
remain unchanged; all three reported S3 blockers are addressed. Parent full gates
and independent re-verification remain outstanding; job-1 is not evidence of a pass.
