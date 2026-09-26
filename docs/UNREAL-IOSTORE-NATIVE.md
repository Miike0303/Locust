# Native IoStore LocRes subset

Locust now extracts localization bytes directly from UCAS when a supported UTOC
directory explicitly maps a `.locres` filename to an `ExternalFile` chunk. This
extends the original [companion-PAK workflow](UNREAL-IOSTORE.md).

## Supported input and output

- TOC versions **5, 7 and 8**, unsigned and unencrypted, with a directory index.
- `ExternalFile` chunk type **7** only, and only directory-mapped `.locres` paths.
- Uncompressed, **Zlib** and raw **LZ4** blocks, including partial logical blocks
  and physical UCAS partitions (`name.ucas`, `name_s1.ucas`, etc.).
- First 20 bytes of the BLAKE3 chunk digest are checked against TOC entry metadata.
  This detects changed/corrupt payloads; it does not authenticate unsigned files.
- Native resources and PAK resources share Locust's conventional whole-resource
  ranking. A same-stem PAK wins a tie; numbered patches and generated Locust PAKs
  retain their existing priority. This policy is not certified runtime mount order.
- Translation writes a sibling `*_LOCUST_P.pak` using the real mapped virtual path.
  UTOC/UCAS and original companion PAK bytes stay unchanged. Reserved-suffix input
  names receive a separate `_NATIVE_LOCUST_P.pak` output to avoid a collision.
  Output symlinks are rejected. Existing overlay resources and earlier translated
  keys are preserved while their namespace/key/source hash still matches.

Resource IDs remain identical between native extraction and PAK-overlay reextraction.
Native entries contain `iostore_external_file`, `iostore_chunk_index` and
`iostore_toc_version`, plus the existing LocRes path/culture/source-hash metadata.
No fabricated physical LocRes offset is emitted.

## Limits and diagnosis

Header tables are bounded before allocation: 128 MiB metadata, one million chunks,
four million compression blocks, and at most 1024 partitions. Directory walking
validates links, cycles, ownership, reachable entries, FString encoding, path
components and duplicates; depth is capped at 128 and total expanded paths at
16 MiB. A resource is limited to 64 MiB decoded data, 256 MiB compressed reads and
65,536 blocks. A block is limited to 16 MiB. Native extraction and PAK-overlay
contents each have a 512 MiB aggregate budget.

The reader checks physical partition ranges and file lengths before seeking. Zlib
must consume the entire compressed stream and produce the exact declared size;
LZ4 must decode to the exact block length. Only blocks intersecting a selected
LocRes chunk are read. Package assets and unrelated UCAS data are not scanned.

A directory-mapped winning LocRes that cannot be decoded causes an error instead
of exposing a superseded copy. A container whose native directory is unavailable
(encrypted, signed, unsupported version or absent directory index) may coexist with
readable companion files. Those entries carry `iostore_unread_containers`, and a
warning is logged; this explicitly records partial coverage. Without readable
localization, the error names the unsupported condition and recommends an external
exporter. Malformed tables of supported containers are never silently ignored.

Still unsupported: TOC v1–4, v6 on-demand metadata, unknown versions/codecs,
encrypted/signed containers, filenames absent from directory metadata, Zen package
FText/string tables, custom mount logic and native UTOC/UCAS rewriting. No real
installed-game IoStore sample was available, so native game loading/font behavior
is not certified.

## Independent evidence and references

`tests/fixtures/iostore-native` contains neutral synthetic localization containers
emitted by **unmodified retoc 0.1.5**, commit
`885a8dae740cb1ce1e41ff2e74f67f9f0c118237`. Retoc's own `verify` and `list --path`
passed for all three versions, and Locust extracts their expected LocRes bytes and
text. The included example regenerates these fixtures; provenance includes SHA-256
hashes. No game assets, encryption keys or proprietary decompression libraries are
included or loaded. These independent fixtures exercise uncompressed chunks;
separate handcrafted tests cover compressed/partitioned and hostile layouts.

The reader was independently authored from format behavior, without copying source
implementations. Format authorities used:

- [Epic: EIoChunkType](https://dev.epicgames.com/documentation/unreal-engine/API/Runtime/Core/EIoChunkType)
  defines `ExternalFile`; the [Zen Loader overview](https://dev.epicgames.com/documentation/en-us/unreal-engine/zen-loader-in-unreal-engine)
  explains normal companion PAK/container packaging.
- [Retoc TOC/directory structures and reader](https://github.com/trumank/retoc/blob/885a8dae740cb1ce1e41ff2e74f67f9f0c118237/retoc/src/lib.rs),
  [writer](https://github.com/trumank/retoc/blob/885a8dae740cb1ce1e41ff2e74f67f9f0c118237/retoc/src/iostore_writer.rs)
  and [integrity verifier](https://github.com/trumank/retoc/blob/885a8dae740cb1ce1e41ff2e74f67f9f0c118237/retoc_cli/src/main.rs#L438).
  Its source license is MIT.
- [CUE4Parse IoStoreReader](https://github.com/FabianFG/CUE4Parse/blob/master/CUE4Parse/UE4/IO/IoStoreReader.cs)
  provides partition and partial-block addressing behavior. Its source license is
  Apache-2.0. No CUE4Parse code is bundled.
