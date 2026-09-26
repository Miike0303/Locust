# IoStore localization boundary

**Historical companion-support pass:** the bounded native subset is now documented
in [UNREAL-IOSTORE-NATIVE.md](UNREAL-IOSTORE-NATIVE.md). Statements below describing
native decoding as pending refer to the initial implementation and its tests.

Locust can identify `.utoc` containers and route `.utoc` / `.ucas` selections to
localization in sibling PAKs or loose LocRes files. This is companion-file support,
not a decoder for IoStore chunks or Zen assets.

## Why companion PAKs matter

Epic's [Zen Loader documentation](https://dev.epicgames.com/documentation/en-us/unreal-engine/zen-loader-in-unreal-engine)
describes the normal split: package, bulk and shader data reside in UTOC/UCAS;
loose files remain in PAKs. Mounting a PAK also mounts its corresponding container.
Thus an IoStore game can have usable localization in its PAKs even though Locust
does not decode the associated assets. The presence of an encrypted IoStore
container does not make a separate, unencrypted localization PAK unreadable.

This does **not** prove every game's LocRes files must be in PAKs. Epic's
[EIoChunkType API](https://dev.epicgames.com/documentation/unreal-engine/API/Runtime/Core/EIoChunkType)
includes `ExternalFile`; no universal restriction excluding LocRes from all custom
IoStore arrangements has been established. Text inside assets and custom containers
still requires a compatible external exporter or future native decoding.

## Implemented behavior

- Direct UTOC opens sibling PAKs together, preserving existing whole-resource patch
  priority. Direct UCAS requires its matching UTOC; `_sN.ucas` partitions also
  resolve the base UTOC. Case-insensitive TOC names are supported.
- Detection reads a fixed 144-byte TOC header. It checks magic, nonzero version
  and declared header bounds; it does not allocate from TOC counts. Unknown versions
  can identify a container because localization comes from a separate PAK. This
  identification does not validate the chunk tables or guarantee container validity.
- UCAS payloads are never opened for reading. Directory discovery is capped at the
  existing PAK walk depth of five and does not follow directory symlinks.
- An IoStore-only directory, or an empty localization result with a recognized
  companion TOC, gives an actionable error. It recommends opening the complete game
  folder or exporting LocRes with UnrealPak/a compatible tool; it explicitly does
  not claim the game contains no text.
- Translation output remains the existing sibling `*_LOCUST_P.pak`. No UTOC/UCAS
  content is rewritten; successful extraction is not proof of runtime mounting.

Header layout/magic were checked against the original
[retoc implementation at 885a8dae](https://github.com/trumank/retoc/blob/885a8dae740cb1ce1e41ff2e74f67f9f0c118237/retoc/src/lib.rs#L1189).
The same project's [writer](https://github.com/trumank/retoc/blob/885a8dae740cb1ce1e41ff2e74f67f9f0c118237/retoc_cli/src/main.rs#L937)
creates an empty companion PAK when needed. Partition naming was checked against
[CUE4Parse's IoStoreReader](https://github.com/FabianFG/CUE4Parse/blob/master/CUE4Parse/UE4/IO/IoStoreReader.cs#L84).

## Evidence and remaining work

`crates/formats/tests/unreal_iostore.rs` uses synthetic TOC headers and real-format
classic PAK/LocRes fixtures. It tests detection, malformed headers, empty companion
PAKs, UCAS pairing, patches, extraction and translation/reextraction with original
files unchanged. These fixtures do not validate IoStore directory indexes,
compression, encryption, signatures, chunk mapping, native text extraction or game
runtime behavior. No real UTOC/UCAS was found in the read-only `D:/juegos` inventory
on 2026-09-11. Native IoStore chunk extraction remains pending.
