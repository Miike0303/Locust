# Research note: Godot PCK v3 and Godot 4.4

Read-only Codex research, 2026-10-19 (day plan [L23..L25](../../odd/tasks/day-plan-2026-10-19.md)). Repository facts cite file:line at the time of writing; sections labelled as format background are external knowledge, not repository facts. Nothing here was verified by running tests.

**Current state — repo facts**

- [`godot_pck.rs:15`](../../crates/formats/src/godot_pck.rs) bounds support to standalone, offset-zero, little-endian PCK v2 / Godot 4.0–4.3. Its 100-byte header contains six `u32`s, `file_base:u64`, sixteen reserved words, then count. Directory/member encryption and unknown flags are refused; header bit 1 is supported.
- A sufficiently long v3 archive with `GDPC` at zero fails at [`godot_pck.rs:207`](../../crates/formats/src/godot_pck.rs): `InvalidData`, `"PCK format version unsupported (requires v2)"`. Godot 4.4 **v2** independently fails the engine check at line 211. Embedded packs fail the magic check first.
- [`godot_dialogic.rs:80`](../../crates/formats/src/godot_dialogic.rs) requires a regular, link-free `.pck` containing `.dtl`; it wraps reader errors. Detection returns false; validation/extraction return errors. Character authority uses unique `.dch` stems; custom identifiers/addon compatibility remain unproven ([line 3](../../crates/formats/src/godot_dialogic.rs)).
- Rebuild regenerates layout with 32-byte alignment, preserves metadata/raw paths/order, authenticates untouched payloads, and independently verifies output. Installation uses staging, `GameLock`, and retained backup ([lines 53–77](../../crates/formats/src/godot_pck.rs)).
- [`godot_dialogic_tests.rs:15`](../../crates/formats/tests/godot_dialogic_tests.rs) constructs v2/4.3 fixtures: two timelines, a character and binary member. Tests cover selection/refusal, locators, authentication, codec/token protection, ambiguous characters, warnings, links, no-op identity, locking and stale/tampered rows. Line 78 explicitly rejects format 3 and engine 4.4. Separate [index tests](../../crates/formats/tests/godot_pck_index.rs) cover malformed/rebuild cases; [lifecycle tests](../../crates/formats/tests/godot_dialogic_lifecycle.rs) cover strict pack/apply/rollback/reapply. An [ignored G01 test](../../crates/formats/tests/godot_pck_index.rs) uses `LOCUST_GODOT_PCK_FIXTURE`. No valid v3 fixture exists.

**Format background — my analysis, verified against upstream**

The premise needs correction: [Godot 4.4 declares format 2](https://github.com/godotengine/godot/blob/4.4/core/io/file_access_pack.h#L37-L49); [4.5 introduces/defaults to v3](https://github.com/godotengine/godot/blob/4.5/core/io/file_access_pack.h#L37-L54).

V3 retains the first 32 bytes, adds `directory_offset:u64` at byte 32, then sixteen reserved words: a 104-byte fixed header. Count lives at the referenced directory. Stock writers place aligned payloads first, directory last. Entry fields remain padded path, offset, size, MD5, flags; paths use four-byte padding and packer alignment defaults to 32. [PCKPacker](https://github.com/godotengine/godot/blob/4.5/core/io/pck_packer.cpp#L83-L114)

Entry offsets remain relative to `file_base`. V3 makes `file_base` relative to PCK start unconditionally; v2 does so with `PACK_REL_FILEBASE`. V3 directory offsets also reference PCK start. Godot locates embedded packs through executable sections or a size/`GDPC` footer. Directory encryption is header bit 0; the count precedes encrypted records. Member bit 0 indicates encryption; member bit 1 means removal. Header bit 2 denotes sparse bundles whose payloads are external files. Patch packs contain changed entries/removals and depend on base packs/load order. [Reader](https://github.com/godotengine/godot/blob/4.5/core/io/file_access_pack.cpp#L177-L304), [sparse access](https://github.com/godotengine/godot/blob/4.5/core/io/file_access_pack.cpp#L439-L465)

Godot 4.4’s [`.uid` sidecars](https://godotengine.org/article/uid-changes-coming-to-godot-4-4/) concern resource identity. `.remap` redirects resource loading. Stock Dialogic still loads [text `.dtl`](https://github.com/dialogic-godot/dialogic/blob/main/addons/dialogic/Resources/TimelineResourceLoader.gd) and [Variant-text `.dch`](https://github.com/dialogic-godot/dialogic/blob/main/addons/dialogic/Resources/CharacterResourceLoader.gd); container support alone cannot prove export/custom-addon compatibility.

Another compatibility gap: upstream [stores paths without `res://`](https://github.com/godotengine/godot/blob/4.5/editor/export/editor_export_platform.cpp#L304-L306), while [Locust requires it](../../crates/formats/src/godot_pck.rs).

**Options**

Extend the v2 engine allowance for 4.4 separately; stage v3 metadata reading first; then implement native v3 rebuilding. Silent v3→v2 conversion complicates provenance and existing revision-format checks.

**Recommendation**

Support plain, standalone v3 with explicit version profiles. Model header/directory extents separately; replace the current “payload after index” rule with checked interval exclusion. Canonicalize bare paths internally while preserving serialized spelling.

The writer must preplan header→payloads→directory, recalculate offsets/positions/length, preserve versions/flags/reserved words/order, and extend independent verification. Retain streaming hashes, staging, locks, pristine provenance and exact backup restoration. Relayout changes bytes; reversibility comes from backups. Main risks are overflow, directory overlap, path collisions, remapped resources and changing Dialogic identifiers. Defer embedded, encryption, sparse bundles and removals.

**First unit**

Implement bounded v3 index reading only; explicitly keep v3 refused by plugin selection and rebuild until writer/lifecycle gates pass.

Build independent byte fixtures without production `rebuild`: 104-byte header, payloads, trailing directory, real MD5s, `.dtl`/`.dch`, bare/prefixed paths, opaque `.uid`/`.remap` members.

Acceptance: correct metadata; no payload reads; retained caps; rejection of truncation, overflow, directory/payload overlap, aliases, unsafe/colliding paths and unsupported flags; unchanged v2 behavior. Subsequent release gates: grow/shrink rebuilds, untouched-byte equality, independent verification, revisions, and exact strict rollback/reapply.

No files changed; tests were inspected, not run.