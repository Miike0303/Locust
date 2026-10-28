# Godot / Dialogic support guide

`godot-dialogic` is **Experimental** support for translating Dialogic timelines
inside a selected standalone Godot PCK. Start with Replace to create a separate
archive; review warnings and translations before installing it.

## Supported input

- A **standalone `.pck` file** at offset zero, little-endian and plain:
  **Godot 4.0–4.4 PCK v2** or **Godot 4.5+ PCK v3** (engine major 4).
- Both bare and `res://` member paths; canonical identities use `res://` while
  rebuilds preserve each original serialized spelling, member order, version,
  flags and reserved words.
- Dialogic `.dtl` timelines using the supported stock syntax.
- `.dch` file stems identify speakers; character display names are not translated.

Select the **`.pck` file**, not its folder or the game executable. Folder selection
is refused before backup or copying; this format never adopts the parent folder.
Embedded packs in executables, directory/member encryption, sparse bundles,
removal entries and unknown flags are unsupported. Unsafe or colliding paths,
overlapping payloads/metadata, out-of-bounds ranges, overflow and truncated
headers/directories are refused. Reader limits remain 100,000 entries, 16 KiB
per padded path and 16 MiB of directory records (including the v3 count).

Godot 4.4 v2 and 4.5+ v3 support is proven on synthetic fixtures only; no real
4.4/v3 game or engine launch has been verified. Independent hand-built fixtures
cover `.dtl`, `.dch`, opaque `.uid`/`.remap` sidecars and binary members. Sidecars
are preserved byte for byte; their presence does not establish that remapped
or custom-addon resources can be translated.

The compatible Dialogic reference is **2.0-alpha-16**. This does not identify the
installed addon version.

## Quick start: create a translated archive

Replace `<game>`, `<db>` and `<out>` with your archive name, project database path
and output directory. Commands below use Spanish (`es`) as the target language.

1. Extract the selected archive:

   ```text
   locust extract "<game>.pck" -o "<db>"
   ```

2. Translate the extracted rows or export, fill and import translations.
   Review protected tokens and any extraction warnings before injecting.
3. Create the translated archive with the default Replace mode:

   ```text
   locust inject "<game>.pck" -P "<db>" -l es -o "<out>"
   ```

The result is **`<out>/<game>.pck-es/<game>.pck`**, an **archive-only copy**.
The output folder is not a runnable game folder. Place the rewritten archive next
to the game executable in a game copy, or install it using the patch workflow below.
Review the injection report for skipped rows and warnings.

## What to translate and preserve

| Content | Behavior |
|---|---|
| Text events | Visible text is translated. |
| Continuations (`\` lines) | One row per continued event, with multiple segments. |
| Choices | Visible choice text is translated. |
| Speakers, commands, indentation, `#id:` | Preserved. |
| `[bbcode]` tags and `{variables}` | Shown literally in sources and preserved. |
| Character display names | Not translated. |

A translation must keep **every protected token verbatim and in order**, retain
the same continuation segment count, and introduce no new speaker prefix, command
or choice. A row that fails these checks is **skipped with a warning**; it does not
corrupt the archive. Correct the row and review the next injection report.

Unsupported lines produce extraction warnings and mark extraction **incomplete**.
For example, a line starting with an escaped event prefix (`\`) is unsupported.
An incomplete extraction preserves saved translations instead of deleting rows
that were not extracted.

### Provider output needs review

Generic provider placeholder protection covers some tokens, such as `[b]` and
`{player}`, but does not cover all Dialogic syntax: examples include `[/b]`,
`[color=red]` and `{Game.score}`. Check provider output against the literal source;
injection independently validates protected tokens and skips invalid rows.
The mock provider truncates output to source length and is unsuitable for this workflow.

## Modes and recovery

| Mode | Result |
|---|---|
| Replace (default) | Separate archive-only copy at `<output>/<name>.pck-<lang>/<name>.pck`. |
| Direct | Recoverable in-place update of the selected archive. Revisions rebuild from the verified original. |
| Add | Not supported for `godot-dialogic`. |

For Direct, use the selected PCK and the same project database:

```text
locust inject "<game>.pck" -P "<db>" -l es --direct
```

Each member read is MD5-checked. Rewriting streams the archive, verifies the
result before atomic installation, and retains the previous archive.
V2 keeps its directory-before-payload layout; v3 writes aligned payloads before
the trailing directory. Unchanged member payloads remain byte-identical;
container padding and offsets are regenerated. Recovery restores the exact
original archive from backup.

## Package, apply and roll back a patch

After Replace, pack the **output directory containing the rewritten PCK**:

```text
locust patch "<out>/<game>.pck-es" -P "<db>" -l es -o patch.zip
locust apply "<game folder>" patch.zip
locust patch-rollback "<game folder>"
```

`<game folder>` must be the directory containing the target `.pck`.
The patch contains the **whole rewritten PCK**, with a strict original hash;
it is not a delta of individual archive members. Sibling files are untouched.
Applying an already-applied patch is refused. Rollback restores the exact original
archive, after which the same patch can be applied again.

## Limitations and recorded verification

**Synthetic v3 verification:** independently assembled fixtures exercise
metadata-only reading, refusal cases, grow/shrink/empty rebuilds, an independent
byte/MD5 oracle, Replace, Direct revisions and no-ops, strict whole-PCK packing,
apply/already-applied refusal, exact rollback and reapply. The v2 lifecycle suite
also runs its revision, drift, provenance, scope and recovery checks for v3.

**Verified on 2026-10-15:** owned copies of one installed game; the original was
never modified. These results establish resource preservation and recovery:

- Extraction: **525 rows**, with **one unsupported-line warning**.
- XLIFF export → fill → import → Replace: **525/525** written and re-extracted.
- Independent byte oracle: **2,001 members identical; 111 timelines changed**.
- Strict patch pack → apply → already-applied refusal → rollback with the exact
  original hash → reapply passed.
- Direct A → B → unchanged revisions: **525/525** at each stage.

**Not verified:** running the game, fonts/glyph coverage, line length/layout,
linguistic quality, other Godot/Dialogic versions, or custom Dialogic indexers.
These checks do not establish that a translated game runs or displays correctly.

**Known XLIFF issue:** files saved with CRLF line endings currently fail to import
multi-line sources, reported as outdated. Save XLIFF with **LF** line endings;
the CRLF issue is being fixed separately.
