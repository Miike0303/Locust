//! Generated wire-format fixtures and opt-in read-only real archive replay.
use locust_core::extraction::FormatPlugin;
use locust_formats::unreal::UnrealPlugin;
use locust_formats::unreal_locres::{LocresFile, LocresNamespace, LocresString, LocresVersion};
use locust_formats::unreal_pak::*;
use std::io::{Cursor, Read, Seek, SeekFrom, Write};

fn u32b(v: &mut Vec<u8>, n: u32) {
    v.extend(n.to_le_bytes());
}
fn u64b(v: &mut Vec<u8>, n: u64) {
    v.extend(n.to_le_bytes());
}
fn string(v: &mut Vec<u8>, s: &str) {
    u32b(v, s.len() as u32 + 1);
    v.extend(s.as_bytes());
    v.push(0);
}
const DIR: &str = "Game/Content/Localization/Game/en/";
fn locres() -> Vec<u8> {
    LocresFile {
        version: LocresVersion::Compact,
        namespaces: vec![LocresNamespace {
            name: "Dialog".into(),
            name_hash: 0,
            strings: vec![LocresString {
                key: "Greeting".into(),
                value: "Hello traveler!".into(),
                source_string_hash: 42,
                key_hash: 0,
            }],
        }],
    }
    .serialize()
    .unwrap()
}

struct Fixture {
    bytes: Vec<u8>,
    primary: usize,
    directory: usize,
    phi: usize,
    encoded: usize,
    dir_location: usize,
}

fn fixture(
    version: u32,
    codec: &str,
    block_size: usize,
    ordinary: bool,
    path_hash: bool,
) -> Fixture {
    let data = locres();
    let compressed = !codec.is_empty();
    let method = if compressed { 1 } else { 0 };
    let mut blocks = Vec::new();
    let mut stored = Vec::new();
    if compressed {
        for chunk in data.chunks(block_size) {
            let bytes = if codec == "Gzip" {
                let mut w =
                    flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
                w.write_all(chunk).unwrap();
                w.finish().unwrap()
            } else {
                let mut w =
                    flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
                w.write_all(chunk).unwrap();
                w.finish().unwrap()
            };
            blocks.push(bytes.len());
            stored.extend(bytes);
        }
    } else {
        stored = data.clone();
    }
    let header_size = 53 + if compressed { 4 + 16 * blocks.len() } else { 0 };
    let mut header = Vec::new();
    u64b(&mut header, 0);
    u64b(&mut header, stored.len() as u64);
    u64b(&mut header, data.len() as u64);
    u32b(&mut header, method);
    header.extend(sha1_bytes(&stored));
    if compressed {
        u32b(&mut header, blocks.len() as u32);
        let mut start = header_size as u64;
        for size in &blocks {
            u64b(&mut header, start);
            start += *size as u64;
            u64b(&mut header, start);
        }
    }
    header.push(0);
    u32b(&mut header, if compressed { block_size as u32 } else { 0 });
    let mut bytes = header.clone();
    bytes.extend(stored);
    let mut encoded = Vec::new();
    if !ordinary {
        let bits = (1 << 31)
            | (1 << 30)
            | (1 << 29)
            | (method << 23)
            | ((blocks.len() as u32) << 6)
            | if compressed { 63 } else { 0 };
        u32b(&mut encoded, bits);
        if compressed {
            u32b(&mut encoded, block_size as u32);
        }
        u32b(&mut encoded, 0);
        u32b(&mut encoded, data.len() as u32);
        if compressed {
            u32b(&mut encoded, (bytes.len() - header_size) as u32);
            if blocks.len() > 1 {
                for n in &blocks {
                    u32b(&mut encoded, *n as u32);
                }
            }
        }
    }
    let mut directory_bytes = Vec::new();
    u32b(&mut directory_bytes, 1);
    string(&mut directory_bytes, DIR);
    u32b(&mut directory_bytes, 1);
    string(&mut directory_bytes, "Game.locres");
    let dir_location_relative = directory_bytes.len();
    u32b(&mut directory_bytes, if ordinary { u32::MAX } else { 0 });
    let directory = bytes.len();
    bytes.extend(&directory_bytes);
    let mut phi_bytes = Vec::new();
    u32b(&mut phi_bytes, 1);
    u64b(&mut phi_bytes, 1234);
    u32b(&mut phi_bytes, if ordinary { u32::MAX } else { 0 });
    u32b(&mut phi_bytes, 0);
    let phi = bytes.len();
    if path_hash {
        bytes.extend(&phi_bytes);
    }
    let primary = bytes.len();
    let mut index = Vec::new();
    string(&mut index, DEFAULT_MOUNT_POINT);
    u32b(&mut index, 1);
    u64b(&mut index, 0);
    u32b(&mut index, path_hash as u32);
    if path_hash {
        u64b(&mut index, phi as u64);
        u64b(&mut index, phi_bytes.len() as u64);
        index.extend(sha1_bytes(&phi_bytes));
    }
    u32b(&mut index, 1);
    u64b(&mut index, directory as u64);
    u64b(&mut index, directory_bytes.len() as u64);
    index.extend(sha1_bytes(&directory_bytes));
    u32b(&mut index, encoded.len() as u32);
    let encoded_offset = primary + index.len();
    index.extend(encoded);
    u32b(&mut index, ordinary as u32);
    if ordinary {
        index.extend(header);
    }
    bytes.extend(&index);
    bytes.extend([0; 17]);
    u32b(&mut bytes, PAK_MAGIC);
    u32b(&mut bytes, version);
    u64b(&mut bytes, primary as u64);
    u64b(&mut bytes, index.len() as u64);
    bytes.extend(sha1_bytes(&index));
    for method in [codec, "", "", "", ""] {
        let mut slot = [0; 32];
        slot[..method.len()].copy_from_slice(method.as_bytes());
        bytes.extend(slot);
    }
    Fixture {
        bytes,
        primary,
        directory,
        phi,
        encoded: encoded_offset,
        dir_location: directory + dir_location_relative,
    }
}

fn payload(bytes: &[u8]) -> Result<Vec<u8>, PakError> {
    let index = read_index(bytes, "fixture")?;
    read_payload(
        &mut Cursor::new(bytes),
        &index.records[0],
        index.footer.version,
        bytes.len() as u64,
        "fixture",
    )
}

fn rehash_primary(f: &mut Fixture) {
    let footer = read_footer(&f.bytes, "fixture").unwrap();
    let hash = sha1_bytes(&f.bytes[f.primary..f.primary + footer.index_size as usize]);
    f.bytes[footer.magic_offset + 24..footer.magic_offset + 44].copy_from_slice(&hash);
}
fn rehash_directory(f: &mut Fixture, path_hash: bool) {
    let end = if path_hash { f.phi } else { f.primary };
    let hash = sha1_bytes(&f.bytes[f.directory..end]);
    let hash_pos = f.primary
        + 4
        + DEFAULT_MOUNT_POINT.len()
        + 1
        + 4
        + 8
        + 4
        + if path_hash { 36 } else { 0 }
        + 4
        + 8
        + 8;
    f.bytes[hash_pos..hash_pos + 20].copy_from_slice(&hash);
    rehash_primary(f);
}

#[test]
fn v10_v11_encoded_and_ordinary_zlib_gzip_block_roundtrips() {
    for version in [10, 11] {
        for ordinary in [false, true] {
            for codec in ["", "Zlib", "Gzip"] {
                for size in [32, 65536] {
                    let f = fixture(version, codec, size, ordinary, true);
                    assert_eq!(
                        payload(&f.bytes).unwrap(),
                        locres(),
                        "v{version} {codec} block={size} ordinary={ordinary}"
                    );
                }
            }
        }
    }
}

#[test]
fn full_directory_without_path_hash_supported() {
    assert_eq!(
        payload(&fixture(11, "", 65536, false, false).bytes).unwrap(),
        locres()
    );
}

#[test]
fn path_hash_only_and_deleted_locres_are_explicitly_unsupported() {
    let mut f = fixture(11, "", 65536, false, true);
    let has_fdi = f.primary + 4 + DEFAULT_MOUNT_POINT.len() + 1 + 4 + 8 + 4 + 36;
    f.bytes[has_fdi..has_fdi + 4].copy_from_slice(&0u32.to_le_bytes());
    rehash_primary(&mut f);
    assert!(read_index(&f.bytes, "bad")
        .unwrap_err()
        .message
        .contains("full directory index required"));
    let mut f = fixture(11, "", 65536, false, true);
    f.bytes[f.dir_location..f.dir_location + 4].copy_from_slice(&i32::MIN.to_le_bytes());
    rehash_directory(&mut f, true);
    assert!(read_index(&f.bytes, "bad")
        .unwrap_err()
        .message
        .contains("deleted/pruned LocRes"));
}

#[test]
fn compressed_bomb_stops_at_declared_output_and_rejects_bad_blocks() {
    let mut f = fixture(11, "Zlib", 65536, false, false);
    f.bytes[16..24].copy_from_slice(&1u64.to_le_bytes());
    f.bytes[f.encoded + 12..f.encoded + 16].copy_from_slice(&1u32.to_le_bytes());
    rehash_primary(&mut f);
    assert!(payload(&f.bytes)
        .unwrap_err()
        .message
        .contains("decompressed block size"));
    let f = fixture(11, "Zlib", 32, true, false);
    let mut index = read_index(&f.bytes, "f").unwrap();
    let rec = &mut index.records[0];
    rec.compression_blocks[0].0 = u64::MAX;
    assert!(read_payload(
        &mut Cursor::new(&f.bytes),
        rec,
        11,
        f.bytes.len() as u64,
        "f"
    )
    .is_err());
}

#[test]
fn v9_non_frozen_classic_supported_but_frozen_refused() {
    let bytes = write_pak(
        DEFAULT_MOUNT_POINT,
        8,
        &[PakWriteFile {
            name: format!("{DIR}Game.locres"),
            data: locres(),
        }],
        "v8",
    )
    .unwrap();
    let footer = read_footer(&bytes, "v8").unwrap();
    let mut bytes = bytes;
    bytes[footer.magic_offset + 4..footer.magic_offset + 8].copy_from_slice(&9u32.to_le_bytes());
    bytes.insert(footer.magic_offset + 44, 0);
    assert_eq!(payload(&bytes).unwrap(), locres());
    bytes[footer.magic_offset + 44] = 1;
    assert!(read_index(&bytes, "frozen")
        .unwrap_err()
        .message
        .contains("unsupported v9 frozen"));
}

#[test]
fn hostile_fstring_and_directory_count_return_errors_without_panics() {
    let mut f = fixture(11, "", 65536, false, false);
    f.bytes[f.primary..f.primary + 4].copy_from_slice(&i32::MIN.to_le_bytes());
    rehash_primary(&mut f);
    assert!(read_index(&f.bytes, "bad").is_err());
    let mut f = fixture(11, "", 65536, false, false);
    f.bytes[f.directory..f.directory + 4].copy_from_slice(&u32::MAX.to_le_bytes());
    rehash_directory(&mut f, false);
    assert!(read_index(&f.bytes, "bad")
        .unwrap_err()
        .message
        .contains("count"));
}

#[test]
fn classic_compressed_absolute_and_relative_block_offsets() {
    for version in [3, 5, 7, 8] {
        for codec in ["Zlib", "Gzip"] {
            let f = fixture(11, codec, 32, false, false);
            let idx = read_index(&f.bytes, "modern").unwrap();
            let rec = &idx.records[0];
            let hdr = payload_offset(rec, 11) as usize;
            let mut header = f.bytes[..hdr].to_vec();
            let offset = 19usize;
            if version < 5 {
                for n in 0..rec.compression_blocks.len() * 2 {
                    let p = 52 + n * 8;
                    let value =
                        u64::from_le_bytes(header[p..p + 8].try_into().unwrap()) + offset as u64;
                    header[p..p + 8].copy_from_slice(&value.to_le_bytes());
                }
            }
            if version < 8 && codec == "Gzip" {
                header[24..28].copy_from_slice(&2u32.to_le_bytes());
            }
            let mut bytes = vec![0; offset];
            bytes.extend(&header);
            bytes.extend(&f.bytes[hdr..f.directory]);
            let mut index = Vec::new();
            string(&mut index, DEFAULT_MOUNT_POINT);
            u32b(&mut index, 1);
            string(&mut index, &format!("{DIR}Game.locres"));
            header[..8].copy_from_slice(&(offset as u64).to_le_bytes());
            index.extend(header);
            let index_offset = bytes.len();
            bytes.extend(&index);
            if version >= 7 {
                bytes.extend([0; 16]);
            }
            if version >= 4 {
                bytes.push(0);
            }
            u32b(&mut bytes, PAK_MAGIC);
            u32b(&mut bytes, version);
            u64b(&mut bytes, index_offset as u64);
            u64b(&mut bytes, index.len() as u64);
            bytes.extend(sha1_bytes(&index));
            if version >= 8 {
                for method in [codec, "", "", "", ""] {
                    let mut slot = [0; 32];
                    slot[..method.len()].copy_from_slice(method.as_bytes());
                    bytes.extend(slot);
                }
            }
            assert_eq!(payload(&bytes).unwrap(), locres(), "v{version} {codec}");
        }
    }
}

#[test]
fn modern_rejects_secondary_hash_corruption() {
    for directory in [false, true] {
        let mut f = fixture(11, "", 65536, false, true);
        let offset = if directory { f.directory } else { f.phi };
        f.bytes[offset] ^= 1;
        assert!(read_index(&f.bytes, "bad")
            .unwrap_err()
            .message
            .contains("SHA-1"));
    }
}

#[test]
fn modern_rejects_out_of_bounds_and_middle_of_entry_locations() {
    for location in [1, 999999, i32::MAX, i32::MIN + 1] {
        let mut f = fixture(11, "", 65536, false, true);
        f.bytes[f.dir_location..f.dir_location + 4].copy_from_slice(&location.to_le_bytes());
        rehash_directory(&mut f, true);
        assert!(read_index(&f.bytes, "bad")
            .unwrap_err()
            .message
            .contains("location"));
    }
}

#[test]
fn modern_rejects_oversized_secondary_and_primary_overlap() {
    for size in [MAX_INDEX_SIZE + 1, u64::MAX] {
        let mut f = fixture(11, "", 65536, false, false);
        let size_pos = f.primary + 4 + DEFAULT_MOUNT_POINT.len() + 1 + 4 + 8 + 4 + 4 + 8;
        f.bytes[size_pos..size_pos + 8].copy_from_slice(&size.to_le_bytes());
        rehash_primary(&mut f);
        assert!(read_index(&f.bytes, "bad").is_err());
    }
    let mut f = fixture(11, "", 65536, false, false);
    let offset_pos = f.primary + 4 + DEFAULT_MOUNT_POINT.len() + 1 + 4 + 8 + 4 + 4;
    f.bytes[offset_pos..offset_pos + 8].copy_from_slice(&(f.primary as u64).to_le_bytes());
    rehash_primary(&mut f);
    assert!(read_index(&f.bytes, "bad")
        .unwrap_err()
        .message
        .contains("overlap"));
}

#[test]
fn modern_rejects_encoded_payload_overflow() {
    let mut f = fixture(11, "", 65536, false, false);
    // Force the encoded entry to read a 64-bit offset near u64::MAX.
    let bits =
        u32::from_le_bytes(f.bytes[f.encoded..f.encoded + 4].try_into().unwrap()) & !(1 << 31);
    f.bytes[f.encoded..f.encoded + 4].copy_from_slice(&bits.to_le_bytes());
    f.bytes[f.encoded + 4..f.encoded + 12].copy_from_slice(&u64::MAX.to_le_bytes());
    rehash_primary(&mut f);
    assert!(read_index(&f.bytes, "bad").is_err());
}

#[test]
fn payload_checks_header_and_stored_hash() {
    let mut f = fixture(11, "Zlib", 32, false, true);
    f.bytes[8] ^= 1;
    assert!(payload(&f.bytes)
        .unwrap_err()
        .message
        .contains("header/index"));
    let mut f = fixture(11, "Zlib", 32, false, true);
    let i = read_index(&f.bytes, "f").unwrap();
    let off = payload_offset(&i.records[0], 11) as usize;
    f.bytes[off] ^= 1;
    assert!(payload(&f.bytes).unwrap_err().message.contains("SHA-1"));
}

#[test]
fn decoder_checks_stream_checksum_and_block_geometry_after_pak_hashes_pass() {
    for codec in ["Zlib", "Gzip"] {
        let mut f = fixture(11, codec, 65536, false, false);
        let index = read_index(&f.bytes, "fixture").unwrap();
        let start = payload_offset(&index.records[0], 11) as usize;
        // Break the codec checksum, then refresh the PAK SHA-1 so the codec
        // decoder (rather than the outer integrity check) must reject it.
        f.bytes[f.directory - if codec == "Gzip" { 8 } else { 1 }] ^= 1;
        let hash = sha1_bytes(&f.bytes[start..f.directory]);
        f.bytes[28..48].copy_from_slice(&hash);
        assert!(payload(&f.bytes)
            .unwrap_err()
            .message
            .contains("block decode"));
    }
    let mut f = fixture(11, "Zlib", 32, true, false);
    let index = read_index(&f.bytes, "fixture").unwrap();
    let header_size = payload_offset(&index.records[0], 11) as usize;
    let ordinary_header = f.primary + index.footer.index_size as usize - header_size;
    // Both authenticated index and data header agree, but the block points
    // into the header rather than into the stored payload.
    f.bytes[52..60].copy_from_slice(&0u64.to_le_bytes());
    f.bytes[ordinary_header + 52..ordinary_header + 60].copy_from_slice(&0u64.to_le_bytes());
    rehash_primary(&mut f);
    assert!(payload(&f.bytes)
        .unwrap_err()
        .message
        .contains("invalid compressed block range"));
}

#[test]
fn oodle_diagnostic_and_decompression_cap() {
    let f = fixture(11, "Oodle", 32, false, true);
    assert!(payload(&f.bytes).unwrap_err().message.contains("Oodle"));
    let f = fixture(11, "Zlib", 32, false, true);
    let index = read_index(&f.bytes, "f").unwrap();
    let mut rec = index.records[0].clone();
    rec.uncompressed_size = MAX_LOCRES_PAYLOAD + 1;
    assert!(read_payload(
        &mut Cursor::new(&f.bytes),
        &rec,
        11,
        f.bytes.len() as u64,
        "f"
    )
    .unwrap_err()
    .message
    .contains("safety limit"));
}

#[test]
fn compressed_modern_overlay_injection_and_priority_roundtrip() {
    let temp = tempfile::tempdir().unwrap();
    let paks = temp.path().join("Game/Content/Paks");
    std::fs::create_dir_all(&paks).unwrap();
    let f = fixture(11, "Gzip", 32, false, true);
    let base = paks.join("Game.pak");
    std::fs::write(&base, &f.bytes).unwrap();
    let plugin = UnrealPlugin::new();
    let mut entries = plugin.extract(temp.path()).unwrap();
    assert_eq!(entries.len(), 1);
    entries[0].translation = Some("Hola viajero, bienvenido de nuevo.".into());
    let report = plugin.inject(temp.path(), &entries).unwrap();
    assert_eq!(report.strings_written, 1);
    assert_eq!(std::fs::read(&base).unwrap(), f.bytes);
    let after = plugin.extract(temp.path()).unwrap();
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].id, entries[0].id);
    assert_eq!(after[0].source, entries[0].translation.clone().unwrap());
}

struct Counted<T> {
    inner: T,
    read: u64,
}
impl<T: Read> Read for Counted<T> {
    fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(b)?;
        self.read += n as u64;
        Ok(n)
    }
}
impl<T: Seek> Seek for Counted<T> {
    fn seek(&mut self, p: SeekFrom) -> std::io::Result<u64> {
        self.inner.seek(p)
    }
}

#[test]
#[ignore = "requires LOCUST_UNREAL_FIXTURE_DIR; originals opened read-only"]
fn real_archives_read_only_extraction_and_small_overlay_roundtrip() {
    let root = std::path::PathBuf::from(
        std::env::var_os("LOCUST_UNREAL_FIXTURE_DIR").expect("fixture directory"),
    );
    let mut rows = Vec::new();
    for name in [
        "LastHope-WindowsNoEditor.pak",
        "LastHope-WindowsNoEditor_P.pak",
    ] {
        let path = root.join(name);
        let file = std::fs::File::open(&path).unwrap();
        let len = file.metadata().unwrap().len();
        let mut reader = Counted {
            inner: file,
            read: 0,
        };
        let index = read_index_from_reader(&mut reader, len, name).unwrap();
        let mut resources = 0;
        let mut strings = 0;
        let mut first = None;
        for rec in index
            .records
            .iter()
            .filter(|r| is_locres_record_name(&r.name))
        {
            let data = read_payload(&mut reader, rec, index.footer.version, len, name).unwrap();
            let loc = LocresFile::parse(&data, &rec.name).unwrap();
            resources += 1;
            strings += loc.iter_entries().count();
            if first.is_none() && loc.iter_entries().next().is_some() {
                first = Some((rec.name.clone(), data));
            }
        }
        assert!(resources > 0 && strings > 0);
        if len > 1_000_000_000 {
            assert!(
                reader.read < 64 * 1024 * 1024,
                "unexpected archive read volume {}",
                reader.read
            );
        }
        let temp = tempfile::tempdir().unwrap();
        let paks = temp.path().join("Game/Content/Paks");
        std::fs::create_dir_all(&paks).unwrap();
        let (inner, data) = first.unwrap();
        // The small real _P is copied exactly to exercise injection against its
        // actual v11 directory index. For the multi-GB base, retain only one
        // extracted resource in a small v8 QA archive.
        let small = if len < 2 * 1024 * 1024 {
            std::fs::read(&path).unwrap()
        } else {
            write_pak(
                &index.mount_point,
                8,
                &[PakWriteFile { name: inner, data }],
                "small QA base",
            )
            .unwrap()
        };
        let small_base = paks.join("QA.pak");
        std::fs::write(&small_base, &small).unwrap();
        let plugin = UnrealPlugin::new();
        let mut entries = plugin.extract(temp.path()).unwrap();
        let chosen = entries
            .iter_mut()
            .find(|e| !e.source.trim().is_empty())
            .unwrap();
        chosen.translation = Some("Locust QA: traducción de prueba — 中文".into());
        let selected = chosen.clone();
        let report = plugin
            .inject(temp.path(), std::slice::from_ref(&selected))
            .unwrap();
        assert_eq!(report.strings_written, 1);
        let reread = plugin.extract(temp.path()).unwrap();
        assert_eq!(
            reread.iter().find(|e| e.id == selected.id).unwrap().source,
            selected.translation.unwrap()
        );
        assert_eq!(std::fs::read(small_base).unwrap(), small);
        if let Some(destination) = std::env::var_os("LOCUST_UNREAL_ARTIFACT_DIR") {
            let destination = std::path::PathBuf::from(destination);
            std::fs::create_dir_all(&destination).unwrap();
            std::fs::copy(
                paks.join("QA_LOCUST_P.pak"),
                destination.join(format!("{name}.qa-overlay.pak")),
            )
            .unwrap();
        }
        rows.push(serde_json::json!({"archive":name,"file_bytes":len,"version":index.footer.version,"index_records":index.records.len(),"locres_resources":resources,"locres_strings":strings,"bytes_read":reader.read,"overlay_roundtrip":true,"overlay_source":if len < 2*1024*1024 {"exact copy of original v11 _P"} else {"one extracted resource in small v8 QA base"}}));
    }
    // Exercise actual format-plugin extraction over both original archives,
    // including resource identity and conventional _P priority, read-only.
    let entries = UnrealPlugin::new().extract(&root).unwrap();
    assert!(!entries.is_empty());
    let ids: std::collections::HashSet<_> = entries.iter().map(|e| &e.id).collect();
    assert_eq!(ids.len(), entries.len());
    let report = serde_json::json!({"archives":rows,"combined_plugin_strings":entries.len(),"unique_ids":ids.len()});
    println!("{}", serde_json::to_string_pretty(&report).unwrap());
    if let Some(path) = std::env::var_os("LOCUST_UNREAL_REPORT") {
        std::fs::write(path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    }
}
