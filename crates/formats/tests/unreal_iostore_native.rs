//! Independently constructed TOC/UCAS fixtures: these writers do not call any
//! native reader helper and serialize the reference field layout explicitly.
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::PathBuf;

use locust_core::extraction::FormatPlugin;
use locust_formats::unreal::UnrealPlugin;
use locust_formats::unreal_iostore_native::{read_index, MAX_LOCRES_BYTES};
use locust_formats::unreal_locres::{LocresFile, LocresNamespace, LocresString, LocresVersion};
use locust_formats::unreal_pak::{write_pak, PakWriteFile, DEFAULT_MOUNT_POINT};

const NONE: u32 = u32::MAX;
const INNER: &str = "Game/Content/Localization/Game/en/Game.locres";

fn locres(text: &str) -> Vec<u8> {
    LocresFile {
        version: LocresVersion::Compact,
        namespaces: vec![LocresNamespace {
            name: "Menu".into(),
            name_hash: 0,
            strings: vec![LocresString {
                key: "Greeting".into(),
                value: text.into(),
                source_string_hash: 0x12345678,
                key_hash: 0,
            }],
        }],
    }
    .serialize()
    .unwrap()
}
fn put32(out: &mut Vec<u8>, n: u32) {
    out.extend(n.to_le_bytes());
}
fn string(out: &mut Vec<u8>, text: &str, utf16: bool) {
    if utf16 {
        let units: Vec<_> = text.encode_utf16().collect();
        out.extend((-((units.len() + 1) as i32)).to_le_bytes());
        for n in units {
            out.extend(n.to_le_bytes());
        }
        out.extend([0, 0]);
    } else {
        put32(out, (text.len() + 1) as u32);
        out.extend(text.as_bytes());
        out.push(0);
    }
}
fn set32(out: &mut [u8], off: usize, n: u32) {
    out[off..off + 4].copy_from_slice(&n.to_le_bytes());
}
fn set64(out: &mut [u8], off: usize, n: u64) {
    out[off..off + 8].copy_from_slice(&n.to_le_bytes());
}
fn set_be40(out: &mut [u8], off: usize, n: u64) {
    out[off..off + 5].copy_from_slice(&n.to_be_bytes()[3..]);
}
fn set_le40(out: &mut [u8], off: usize, n: u64) {
    out[off..off + 5].copy_from_slice(&n.to_le_bytes()[..5]);
}

struct Fixture {
    dir: tempfile::TempDir,
    toc: PathBuf,
    bytes: Vec<u8>,
    chunk_offsets: usize,
    blocks: usize,
    methods: usize,
    directory: usize,
    directory_nodes: usize,
    file_nodes: usize,
    strings: usize,
    expected: Vec<Vec<u8>>,
    partitions: Vec<Vec<u8>>,
}
impl Fixture {
    fn new(version: u8, codec: &str, partition_size: u64, utf16: bool, names: &[&str]) -> Self {
        Self::build(version, codec, partition_size, utf16, names, None)
    }
    fn build(
        version: u8,
        codec: &str,
        partition_size: u64,
        utf16: bool,
        names: &[&str],
        payloads: Option<&[Vec<u8>]>,
    ) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let toc = dir.path().join("game.utoc");
        let block_size = 128u32;
        let mut logical = Vec::new();
        let mut offsets = Vec::new();
        let mut expected = Vec::new();
        for (i, _) in names.iter().enumerate() {
            logical.extend([0x56; 17]); // deliberately unaligned chunk start
            let data = payloads.map(|p| p[i].clone()).unwrap_or_else(|| {
                locres(&format!(
                    "Hello traveler {i}; a fairly long greeting across blocks"
                ))
            });
            offsets.push((logical.len() as u64, data.len() as u64));
            logical.extend(&data);
            logical.extend([0xa5; 9]);
            expected.push(data);
        }
        let mut blocks_data = Vec::new();
        let mut partitions = vec![Vec::new()];
        for raw in logical.chunks(block_size as usize) {
            let compressed = match codec {
                "Zlib" => {
                    let mut z =
                        flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
                    z.write_all(raw).unwrap();
                    z.finish().unwrap()
                }
                "LZ4" => lz4_flex::block::compress(raw),
                _ => raw.to_vec(),
            };
            if partitions.last().unwrap().len() as u64 + compressed.len() as u64 > partition_size {
                partitions.push(Vec::new());
            }
            let physical = (partitions.len() - 1) as u64 * partition_size
                + partitions.last().unwrap().len() as u64;
            blocks_data.extend_from_slice(&physical.to_le_bytes()[..5]);
            blocks_data.extend_from_slice(&(compressed.len() as u32).to_le_bytes()[..3]);
            blocks_data.extend_from_slice(&(raw.len() as u32).to_le_bytes()[..3]);
            blocks_data.push(if codec == "None" { 0 } else { 1 });
            partitions.last_mut().unwrap().extend(compressed);
        }
        let mut names_table = Vec::<String>::new();
        let mut intern = |name: &str| -> u32 {
            if let Some(i) = names_table.iter().position(|n| n == name) {
                i as u32
            } else {
                names_table.push(name.into());
                names_table.len() as u32 - 1
            }
        };
        let mut dirs = vec![[NONE; 4]];
        let mut files = Vec::<[u32; 3]>::new();
        let mut dirs_map = BTreeMap::new();
        for (chunk, name) in names.iter().enumerate() {
            let parts: Vec<_> = name.split('/').collect();
            let mut parent = 0usize;
            for part in &parts[..parts.len() - 1] {
                let key = (parent, (*part).to_owned());
                parent = if let Some(&i) = dirs_map.get(&key) {
                    i
                } else {
                    let index = dirs.len();
                    dirs.push([intern(part), NONE, NONE, NONE]);
                    if dirs[parent][1] == NONE {
                        dirs[parent][1] = index as u32;
                    } else {
                        let mut child = dirs[parent][1] as usize;
                        while dirs[child][2] != NONE {
                            child = dirs[child][2] as usize;
                        }
                        dirs[child][2] = index as u32;
                    }
                    dirs_map.insert(key, index);
                    index
                };
            }
            let file_index = files.len() as u32;
            files.push([intern(parts.last().unwrap()), NONE, chunk as u32]);
            if dirs[parent][3] == NONE {
                dirs[parent][3] = file_index;
            } else {
                let mut last = dirs[parent][3] as usize;
                while files[last][1] != NONE {
                    last = files[last][1] as usize;
                }
                files[last][1] = file_index;
            }
        }
        let mut directory = Vec::new();
        string(&mut directory, "../../../", utf16);
        put32(&mut directory, dirs.len() as u32);
        let directory_nodes = directory.len();
        for row in dirs {
            for n in row {
                put32(&mut directory, n);
            }
        }
        put32(&mut directory, files.len() as u32);
        let file_nodes = directory.len();
        for row in files {
            for n in row {
                put32(&mut directory, n);
            }
        }
        put32(&mut directory, names_table.len() as u32);
        let strings = directory.len();
        for name in names_table {
            string(&mut directory, &name, utf16);
        }
        let mut bytes = vec![0u8; 144];
        bytes[..16].copy_from_slice(b"-==--==--==--==-");
        bytes[16] = version;
        set32(&mut bytes, 20, 144);
        set32(&mut bytes, 24, names.len() as u32);
        set32(&mut bytes, 28, (blocks_data.len() / 12) as u32);
        set32(&mut bytes, 32, 12);
        set32(&mut bytes, 36, if codec == "None" { 0 } else { 1 });
        set32(&mut bytes, 40, 32);
        set32(&mut bytes, 44, block_size);
        set32(&mut bytes, 48, directory.len() as u32);
        set32(&mut bytes, 52, partitions.len() as u32);
        bytes[80] = 8 | if codec == "None" { 0 } else { 1 };
        set32(&mut bytes, 84, 1); // perfect hash seed and overflow list intentionally present
        set64(&mut bytes, 88, partition_size);
        set32(&mut bytes, 96, 1);
        for chunk in 0..names.len() {
            let mut id = [0u8; 12];
            id[..8].copy_from_slice(&(chunk as u64 + 1).to_le_bytes());
            id[11] = 7;
            bytes.extend(id);
        }
        let chunk_offsets = bytes.len();
        for (offset, len) in offsets {
            bytes.extend_from_slice(&offset.to_be_bytes()[3..]);
            bytes.extend_from_slice(&len.to_be_bytes()[3..]);
        }
        bytes.extend((-1i32).to_le_bytes());
        bytes.extend(0u32.to_le_bytes());
        let blocks = bytes.len();
        bytes.extend(blocks_data);
        let methods = bytes.len();
        if codec != "None" {
            let mut name = [0u8; 32];
            name[..codec.len()].copy_from_slice(codec.as_bytes());
            bytes.extend(name);
        }
        let directory_pos = bytes.len();
        bytes.extend(directory);
        for raw in &expected {
            let mut meta = vec![0u8; if version == 8 { 24 } else { 33 }];
            meta[..20].copy_from_slice(&blake3::hash(raw).as_bytes()[..20]);
            bytes.extend(meta);
        }
        let f = Self {
            dir,
            toc,
            bytes,
            chunk_offsets,
            blocks,
            methods,
            directory: directory_pos,
            directory_nodes: directory_pos + directory_nodes,
            file_nodes: directory_pos + file_nodes,
            strings: directory_pos + strings,
            expected,
            partitions,
        };
        f.save();
        f
    }
    fn simple() -> Self {
        Self::new(5, "None", u64::MAX, false, &[INNER])
    }
    fn save(&self) {
        fs::write(&self.toc, &self.bytes).unwrap();
        for (i, data) in self.partitions.iter().enumerate() {
            let name = if i == 0 {
                "game.ucas".into()
            } else {
                format!("game_s{i}.ucas")
            };
            fs::write(self.dir.path().join(name), data).unwrap();
        }
    }
    fn error(&self) -> String {
        match read_index(&self.toc) {
            Err(e) => e.message,
            Ok(_) => panic!("unexpected valid index"),
        }
    }
    fn read_error(&self) -> String {
        let index = read_index(&self.toc).unwrap();
        index.read_locres(&index.records[0]).unwrap_err().message
    }
}

#[test]
fn native_reads_named_external_locres_versions_and_codecs() {
    for version in [5, 7, 8] {
        for codec in ["None", "Zlib", "LZ4"] {
            let f = Fixture::new(version, codec, u64::MAX, false, &[INNER]);
            let index = read_index(&f.toc).unwrap();
            assert_eq!(index.records.len(), 1);
            assert_eq!(index.records[0].name, INNER);
            assert_eq!(
                index.read_locres(&index.records[0]).unwrap(),
                f.expected[0],
                "version {version}, codec {codec}"
            );
        }
    }
}
#[test]
fn native_reads_multiple_files_and_partitioned_unaligned_blocks() {
    let f = Fixture::new(
        8,
        "Zlib",
        140,
        true,
        &[INNER, "Game/Content/Localization/Game/fr/Game.locres"],
    );
    assert!(f.partitions.len() > 1);
    let index = read_index(&f.toc).unwrap();
    for record in &index.records {
        assert_eq!(
            index.read_locres(record).unwrap(),
            f.expected[record.chunk_index as usize]
        );
    }
}
#[test]
fn native_skips_non_locres_and_rejects_wrong_chunk_type() {
    let mut f = Fixture::new(
        5,
        "None",
        u64::MAX,
        false,
        &["Game/Content/Foo.uasset", INNER],
    );
    assert_eq!(read_index(&f.toc).unwrap().records.len(), 1);
    f.bytes[144 + 12 + 11] = 1;
    f.save();
    assert!(f.read_error().contains("not an ExternalFile"));
}
#[test]
fn native_rejects_encryption_signatures_and_unsupported_versions_precisely() {
    let mut f = Fixture::simple();
    let base = f.bytes.clone();
    for (flag, word) in [(2, "encrypted"), (4, "signed"), (16, "unknown")] {
        f.bytes = base.clone();
        f.bytes[80] |= flag;
        f.save();
        let error = read_index(&f.toc).err().unwrap();
        assert!(error.unsupported);
        assert!(error.message.contains(word));
    }
    for version in [0, 1, 2, 3, 4, 6, 9, 255] {
        f.bytes = base.clone();
        f.bytes[16] = version;
        f.save();
        assert!(f.error().contains("TOC version"));
    }
}
#[test]
fn native_requires_directory_index_and_does_not_guess_paths() {
    let mut f = Fixture::simple();
    f.bytes[80] &= !8;
    f.save();
    assert!(f.error().contains("directory index is absent"));
}
#[test]
fn native_rejects_truncated_headers_tables_and_metadata() {
    let mut f = Fixture::simple();
    let full = f.bytes.clone();
    for length in [
        0,
        15,
        143,
        144,
        f.chunk_offsets + 9,
        f.blocks + 11,
        f.directory + 5,
        full.len() - 1,
    ] {
        f.bytes = full[..length].to_vec();
        f.save();
        assert!(f.error().contains("truncated"));
    }
}
#[test]
fn native_rejects_header_allocation_and_partition_attacks() {
    let mut f = Fixture::simple();
    let base = f.bytes.clone();
    for (offset, value) in [
        (20, 100),
        (24, u32::MAX),
        (28, u32::MAX),
        (32, 13),
        (36, 256),
        (44, 0),
        (44, 3),
        (44, 32 * 1024 * 1024),
        (48, u32::MAX),
        (52, 0),
        (52, 1025),
        (84, u32::MAX),
        (96, 2),
    ] {
        f.bytes = base.clone();
        set32(&mut f.bytes, offset, value);
        f.save();
        assert!(!f.error().is_empty());
    }
    f.bytes = base;
    set64(&mut f.bytes, 88, 0);
    f.save();
    assert!(f.error().contains("partition"));
}
#[test]
fn native_rejects_directory_cycles_indices_and_unsafe_names() {
    let mut f = Fixture::simple();
    let base = f.bytes.clone();
    for (offset, value) in [
        (f.directory_nodes + 4, 0),
        (f.directory_nodes + 4, 50_000),
        (f.file_nodes + 4, 0),
        (f.file_nodes + 8, 50_000),
        (f.file_nodes, 50_000),
        (f.directory_nodes + 16, 50_000),
    ] {
        f.bytes = base.clone();
        set32(&mut f.bytes, offset, value);
        f.save();
        assert!(!f.error().is_empty());
    }
    f.bytes = base;
    f.bytes[f.strings + 4] = b'/';
    f.save();
    assert!(f.error().contains("unsafe"));
}
#[test]
fn native_rejects_fstring_minimum_integer_and_missing_terminator() {
    let mut f = Fixture::simple();
    let base = f.bytes.clone();
    set32(&mut f.bytes, f.strings, 0x80000000);
    f.save();
    assert!(f.error().contains("length limit"));
    f.bytes = base;
    f.bytes[f.strings + 4 + "Game".len()] = b'!';
    f.save();
    assert!(f.error().contains("terminator"));
}
#[test]
fn native_rejects_duplicate_paths() {
    let f = Fixture::new(5, "None", u64::MAX, false, &[INNER, INNER]);
    assert!(f.error().contains("duplicate"));
}
#[test]
fn native_rejects_chunk_oversize_and_block_range_attacks() {
    let mut f = Fixture::simple();
    let base = f.bytes.clone();
    for size in [0, MAX_LOCRES_BYTES + 1, (1u64 << 40) - 1] {
        f.bytes = base.clone();
        set_be40(&mut f.bytes, f.chunk_offsets + 5, size);
        f.save();
        assert!(f.read_error().contains("size"));
    }
    f.bytes = base;
    set_be40(&mut f.bytes, f.chunk_offsets, (1u64 << 40) - 1);
    f.save();
    assert!(f.read_error().contains("block range"));
}
#[test]
fn native_rejects_physical_offsets_and_missing_partitions() {
    let mut f = Fixture::new(5, "None", 128, false, &[INNER]);
    let base = f.bytes.clone();
    set_le40(&mut f.bytes, f.blocks, 127);
    f.save();
    assert!(f.read_error().contains("partition bounds"));
    f.bytes = base.clone();
    set_le40(&mut f.bytes, f.blocks, 50_000);
    f.save();
    assert!(f.read_error().contains("partition bounds"));
    f.bytes = base;
    f.save();
    fs::remove_file(f.dir.path().join("game_s1.ucas")).unwrap();
    assert!(f.read_error().contains("cannot open"));
}
#[test]
fn native_rejects_truncated_ucas_and_false_uncompressed_size() {
    let mut f = Fixture::simple();
    f.partitions[0].truncate(5);
    f.save();
    assert!(f.read_error().contains("partition file"));
    let mut f = Fixture::simple();
    f.bytes[f.blocks + 8] -= 1;
    f.save();
    assert!(f.read_error().contains("sizes differ"));
}
#[test]
fn native_rejects_unknown_codecs_and_codec_indices() {
    let mut f = Fixture::new(5, "Oodle", u64::MAX, false, &[INNER]);
    assert!(f.read_error().contains("codec Oodle"));
    f.bytes[f.blocks + 11] = 2;
    f.save();
    assert!(f.read_error().contains("method index"));
    f.bytes[f.blocks + 11] = 1;
    f.bytes[f.methods..f.methods + 32].fill(b'X');
    f.save();
    assert!(f.error().contains("unterminated"));
}
#[test]
fn native_rejects_zlib_corruption_and_declared_output_mismatch() {
    let mut f = Fixture::new(5, "Zlib", u64::MAX, false, &[INNER]);
    f.partitions[0][0] ^= 0xff;
    f.save();
    assert!(f.read_error().contains("Zlib"));
    let mut f = Fixture::new(5, "Zlib", u64::MAX, false, &[INNER]);
    f.bytes[f.blocks + 8] -= 1;
    f.save();
    assert!(f.read_error().contains("Zlib"));
}
#[test]
fn native_rejects_lz4_corruption_and_declared_output_mismatch() {
    let mut f = Fixture::new(5, "LZ4", u64::MAX, false, &[INNER]);
    f.partitions[0][0] = 0;
    f.partitions[0][1] = 0;
    f.partitions[0][2] = 0;
    f.save();
    assert!(f.read_error().contains("LZ4"));
    let mut f = Fixture::new(5, "LZ4", u64::MAX, false, &[INNER]);
    f.bytes[f.blocks + 8] -= 1;
    f.save();
    assert!(f.read_error().contains("LZ4"));
}

#[test]
fn native_verifies_uncompressed_chunk_integrity() {
    let mut f = Fixture::simple();
    f.partitions[0][50] ^= 1;
    f.save();
    assert!(f.read_error().contains("BLAKE3 chunk hash mismatch"));
}

fn add_pak(f: &Fixture, name: &str, text: &str) -> PathBuf {
    let path = f.dir.path().join(name);
    let bytes = write_pak(
        DEFAULT_MOUNT_POINT,
        8,
        &[PakWriteFile {
            name: INNER.into(),
            data: locres(text),
        }],
        name,
    )
    .unwrap();
    fs::write(&path, bytes).unwrap();
    path
}

#[test]
fn independent_retoc_writer_fixtures_extract_with_native_reader_and_plugin() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/iostore-native");
    let expected = fs::read(root.join("expected.locres")).unwrap();
    for version in [5, 7, 8] {
        let toc = root.join(format!("retoc-v{version}.utoc"));
        let index = read_index(&toc).unwrap();
        assert_eq!(index.records.len(), 1);
        assert_eq!(
            index.records[0].name,
            "FixtureGame/Content/Localization/Game/en/Game.locres"
        );
        assert_eq!(index.read_locres(&index.records[0]).unwrap(), expected);
        let tmp = tempfile::tempdir().unwrap();
        let selected = tmp.path().join("game.utoc");
        fs::copy(&toc, &selected).unwrap();
        fs::copy(toc.with_extension("ucas"), selected.with_extension("ucas")).unwrap();
        let entries = UnrealPlugin::new().extract(&selected).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].source, "Independent retoc fixture greeting");
        assert_eq!(entries[0].file_path, selected);
    }
}

#[test]
fn native_plugin_extracts_toc_only_and_ucas_selections() {
    let f = Fixture::simple();
    for selected in [
        &f.toc,
        &f.toc.with_extension("ucas"),
        &f.dir.path().to_path_buf(),
    ] {
        let plugin = UnrealPlugin::new();
        assert!(plugin.detect(selected));
        let entries = plugin.extract(selected).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].file_path, f.toc);
        assert_eq!(
            entries[0].metadata.get("iostore_external_file"),
            Some(&serde_json::json!(true))
        );
        assert!(!entries[0].metadata.contains_key("locres_offset"));
    }
}

#[test]
fn native_and_pak_resources_follow_shared_conventional_priority() {
    let f = Fixture::simple();
    add_pak(&f, "game.pak", "Companion value");
    assert_eq!(
        UnrealPlugin::new().extract(f.dir.path()).unwrap()[0].source,
        "Companion value"
    );
    fs::rename(&f.toc, f.dir.path().join("game_2_P.utoc")).unwrap();
    fs::rename(
        f.toc.with_extension("ucas"),
        f.dir.path().join("game_2_P.ucas"),
    )
    .unwrap();
    assert!(UnrealPlugin::new().extract(f.dir.path()).unwrap()[0]
        .source
        .starts_with("Hello traveler"));
    add_pak(&f, "game_3_P.pak", "New PAK patch");
    assert_eq!(
        UnrealPlugin::new().extract(f.dir.path()).unwrap()[0].source,
        "New PAK patch"
    );
}

#[test]
fn unreadable_native_overlay_does_not_expose_stale_pak_base() {
    let f = Fixture::new(5, "Oodle", u64::MAX, false, &[INNER]);
    add_pak(&f, "base.pak", "Stale value");
    let error = UnrealPlugin::new()
        .extract(f.dir.path())
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("Oodle") && error.contains("superseded"),
        "{error}"
    );
    add_pak(&f, "game_3_P.pak", "Readable newer overlay");
    assert_eq!(
        UnrealPlugin::new().extract(f.dir.path()).unwrap()[0].source,
        "Readable newer overlay"
    );
}

#[test]
fn unrelated_encrypted_container_preserves_companion_extraction_with_diagnosis() {
    let mut f = Fixture::simple();
    f.bytes[80] |= 2;
    f.save();
    add_pak(&f, "game.pak", "Readable companion");
    let entries = UnrealPlugin::new().extract(f.dir.path()).unwrap();
    assert_eq!(entries[0].source, "Readable companion");
    let warnings = entries[0]
        .metadata
        .get("iostore_unread_containers")
        .unwrap()
        .to_string();
    assert!(warnings.contains("encrypted"));
    fs::remove_file(f.dir.path().join("game.pak")).unwrap();
    assert!(UnrealPlugin::new()
        .extract(f.dir.path())
        .unwrap_err()
        .to_string()
        .contains("encrypted"));
}

#[test]
fn malformed_supported_native_tables_are_not_hidden_by_companion_pak() {
    let mut f = Fixture::simple();
    set32(&mut f.bytes, 24, u32::MAX);
    f.save();
    add_pak(&f, "game.pak", "Readable companion");
    assert!(UnrealPlugin::new()
        .extract(f.dir.path())
        .unwrap_err()
        .to_string()
        .contains("count exceeds"));
}

#[test]
fn native_injection_overlay_roundtrip_preserves_originals_and_native_paths() {
    let f = Fixture::new(8, "LZ4", 140, false, &[INNER]);
    let plugin = UnrealPlugin::new();
    let mut entries = plugin.extract(&f.toc).unwrap();
    entries[0].translation = Some("Bienvenue à bord, voyageur".into());
    let report = plugin.inject(&f.toc, &entries).unwrap();
    assert_eq!(report.strings_written, 1, "{:?}", report.warnings);
    assert_eq!(
        report.files_written,
        vec![f.dir.path().join("game_LOCUST_P.pak")]
    );
    assert_eq!(fs::read(&f.toc).unwrap(), f.bytes);
    for (i, data) in f.partitions.iter().enumerate() {
        let name = if i == 0 {
            "game.ucas".into()
        } else {
            format!("game_s{i}.ucas")
        };
        assert_eq!(fs::read(f.dir.path().join(name)).unwrap(), *data);
    }
    let reread = plugin.extract(&f.toc).unwrap();
    assert_eq!(reread.len(), 1);
    assert_eq!(reread[0].source, "Bienvenue à bord, voyageur");
    assert_eq!(reread[0].id, entries[0].id);
}

#[test]
fn native_injection_rejects_stale_values_and_forged_paths_without_output() {
    let f = Fixture::simple();
    let plugin = UnrealPlugin::new();
    let base = plugin.extract(&f.toc).unwrap();
    for forge_path in [false, true] {
        let mut entries = base.clone();
        entries[0].translation = Some("Different value".into());
        if forge_path {
            entries[0].metadata.insert(
                "locres_resource".into(),
                serde_json::json!("../../outside.locres"),
            );
        } else {
            entries[0].source = "Stale source".into();
        }
        let report = plugin.inject(&f.toc, &entries).unwrap();
        assert_eq!(report.strings_written, 0);
        assert_eq!(report.strings_skipped, 1);
        assert!(!f.dir.path().join("game_LOCUST_P.pak").exists());
    }
    assert_eq!(fs::read(&f.toc).unwrap(), f.bytes);
}

#[test]
fn native_incremental_injection_preserves_previous_keys_and_resources() {
    let mut source = LocresFile::parse(&locres("Hello traveler"), "fixture").unwrap();
    source.namespaces[0].strings.push(LocresString {
        key: "Farewell".into(),
        value: "See you soon".into(),
        source_string_hash: 42,
        key_hash: 0,
    });
    let source = source.serialize().unwrap();
    let f = Fixture::build(
        5,
        "Zlib",
        u64::MAX,
        false,
        &[INNER, "Game/Content/Localization/Game/fr/Game.locres"],
        Some(&[source.clone(), source]),
    );
    let plugin = UnrealPlugin::new();
    let baseline = plugin.extract(&f.toc).unwrap();
    assert_eq!(baseline.len(), 4);
    for (i, mut entry) in baseline.into_iter().enumerate() {
        entry.translation = Some(format!("Translated {i}"));
        let report = plugin.inject(&f.toc, &[entry]).unwrap();
        assert_eq!(report.strings_written, 1, "{:?}", report.warnings);
    }
    let reread = plugin.extract(&f.toc).unwrap();
    assert_eq!(reread.len(), 4);
    assert!(reread.iter().all(|e| e.source.starts_with("Translated")));
}

#[test]
fn native_reserved_suffix_input_does_not_overwrite_original_companion() {
    let f = Fixture::simple();
    let toc = f.dir.path().join("game_LOCUST_P.utoc");
    fs::rename(&f.toc, &toc).unwrap();
    fs::rename(f.toc.with_extension("ucas"), toc.with_extension("ucas")).unwrap();
    let companion = toc.with_extension("pak");
    let companion_bytes = write_pak(
        DEFAULT_MOUNT_POINT,
        8,
        &[PakWriteFile {
            name: "Game/Config/DefaultGame.ini".into(),
            data: b"[Game]\nFixture=true\n".to_vec(),
        }],
        "game_LOCUST_P.pak",
    )
    .unwrap();
    fs::write(&companion, &companion_bytes).unwrap();
    let plugin = UnrealPlugin::new();
    let mut entries = plugin.extract(&toc).unwrap();
    assert_eq!(entries[0].file_path, toc);
    entries[0].translation = Some("Translated native resource".into());
    let report = plugin.inject(&toc, &entries).unwrap();
    assert_eq!(report.strings_written, 1, "{:?}", report.warnings);
    assert_ne!(report.files_written[0], companion);
    assert_eq!(fs::read(companion).unwrap(), companion_bytes);
    assert_eq!(fs::read(&toc).unwrap(), f.bytes);
    assert_eq!(
        plugin.extract(&toc).unwrap()[0].source,
        "Translated native resource"
    );
}
