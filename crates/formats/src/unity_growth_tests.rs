//! Neutral format fixtures: no original game payloads or redistributed assets.
use super::*;
use crate::unity::UnityPlugin;
use crate::unity_fs::{build_test_bundle, StorageBlock, UnityFsArchive};
use locust_core::extraction::FormatPlugin;
use std::collections::BTreeMap;

fn u32e(out: &mut Vec<u8>, n: u32, be: bool) {
    out.extend_from_slice(&if be { n.to_be_bytes() } else { n.to_le_bytes() });
}
fn u64e(out: &mut Vec<u8>, n: u64, be: bool) {
    out.extend_from_slice(&if be { n.to_be_bytes() } else { n.to_le_bytes() });
}
fn align(out: &mut Vec<u8>, n: usize) {
    out.resize((out.len() + n - 1) & !(n - 1), 0);
}
fn string(out: &mut Vec<u8>, text: &str, be: bool) {
    u32e(out, text.len() as u32, be);
    out.extend_from_slice(text.as_bytes());
    align(out, 4);
}

fn fixture(version: u32, be: bool, tree: bool) -> Vec<u8> {
    let mut first = Vec::new();
    string(&mut first, "Dialogue", be);
    string(&mut first, "Hello traveler.", be);
    let opaque = b"\x8f\x0eopaque-resource\x00\xff".to_vec();
    let mut third = Vec::new();
    string(&mut third, "ManagedText", be);
    string(&mut third, "Menu.START: START\nMenu.EXIT: EXIT\n", be);
    let mut mesh = vec![0; 12];
    string(&mut mesh, "Welcome here", be);
    let payloads = [first, opaque, third, mesh];
    let mut data = Vec::new();
    let mut spans = Vec::new();
    for p in &payloads {
        align(&mut data, 8);
        spans.push((data.len() as u64, p.len() as u32));
        data.extend_from_slice(p);
        data.extend_from_slice(b"GAP!");
    }
    data.extend_from_slice(b"TRAILING-OPAQUE");
    let mut meta = b"6000.0.24f1\0".to_vec();
    u32e(&mut meta, 19, be);
    meta.push(tree as u8);
    u32e(&mut meta, 3, be);
    for class in [49, 999, 141] {
        u32e(&mut meta, class, be);
        meta.push(0);
        meta.extend_from_slice(&[255; 2]);
        meta.extend_from_slice(&[7; 16]);
        if tree {
            u32e(&mut meta, 0, be);
            u32e(&mut meta, 0, be);
            if version >= 21 {
                u32e(&mut meta, 2, be);
                u32e(&mut meta, 0, be);
                u32e(&mut meta, 1, be);
            }
        }
    }
    u32e(&mut meta, 4, be);
    // Object table order intentionally differs from physical data order.
    for i in [2, 0, 3, 1] {
        align(&mut meta, 4);
        u64e(&mut meta, i as u64 + 1, be);
        if version >= 22 {
            u64e(&mut meta, spans[i].0, be);
        } else {
            u32e(&mut meta, spans[i].0 as u32, be);
        }
        u32e(&mut meta, spans[i].1, be);
        u32e(&mut meta, [0, 1, 0, 2][i], be);
    }
    u32e(&mut meta, 0, be);
    u32e(&mut meta, 0, be);
    if version >= 20 {
        u32e(&mut meta, 0, be);
    }
    meta.extend_from_slice(b"preserved-user-information\0");
    let header_len = if version >= 22 { 48 } else { 20 };
    let offset = (header_len + meta.len() + 15) & !15;
    let size = offset + data.len();
    let mut out = Vec::new();
    for value in [
        if version >= 22 { 0 } else { meta.len() as u32 },
        if version >= 22 { 0 } else { size as u32 },
        version,
        if version >= 22 { 0 } else { offset as u32 },
    ] {
        out.extend_from_slice(&value.to_be_bytes());
    }
    out.extend_from_slice(&[be as u8, 0, 0, 0]);
    if version >= 22 {
        out.extend_from_slice(&(meta.len() as u32).to_be_bytes());
        out.extend_from_slice(&(size as u64).to_be_bytes());
        out.extend_from_slice(&(offset as u64).to_be_bytes());
        out.extend_from_slice(&[3; 8]);
    }
    out.extend_from_slice(&meta);
    out.resize(offset, 0x42);
    out.extend_from_slice(&data);
    out
}

fn object<'a>(sf: &'a SerializedFile<'_>, id: i64) -> &'a [u8] {
    let o = sf.objects.iter().find(|o| o.path_id == id).unwrap();
    &sf.data[o.data_abs as usize..o.data_abs as usize + o.byte_size as usize]
}

#[test]
fn all_versions_endians_and_type_trees_grow_and_shrink_without_touching_opaque_data() {
    for version in 17..=22 {
        for be in [false, true] {
            for tree in [false, true] {
                let before =
                    SerializedFile::parse(fixture(version, be, tree), "fixture.assets").unwrap();
                let opaque = object(&before, 2).to_vec();
                let mesh = object(&before, 4).to_vec();
                assert_eq!(before.rewriteable_text_assets().unwrap().len(), 2);
                let old_offsets: BTreeMap<_, _> = before
                    .objects
                    .iter()
                    .map(|o| (o.path_id, o.data_abs % 8))
                    .collect();
                let source_len = before.data.len();
                let mut metadata_before =
                    before.data[..before.header.data_offset as usize].to_vec();
                let mut ignored: Vec<std::ops::Range<usize>> = before
                    .objects
                    .iter()
                    .map(|o| o.table_offset..o.table_offset + if version >= 22 { 12 } else { 8 })
                    .collect();
                ignored.push(if version >= 22 { 24..32 } else { 4..8 });
                for r in &ignored {
                    metadata_before[r.clone()].fill(0);
                }
                let long = "多语言 dialogue with plenty of room. ".repeat(30);
                let plan = BTreeMap::from([
                    (1, long.clone()),
                    (
                        3,
                        "Menu.START: Iniciar una nueva partida\nMenu.EXIT: Salir del juego\n"
                            .into(),
                    ),
                ]);
                let grown = before.rewrite_text_assets(&plan).unwrap();
                assert!(grown.len() > source_len);
                let grown = SerializedFile::parse(grown, "grown.assets").unwrap();
                let mut metadata_after = grown.data[..grown.header.data_offset as usize].to_vec();
                for r in &ignored {
                    metadata_after[r.clone()].fill(0);
                }
                assert_eq!(
                    metadata_before, metadata_after,
                    "opaque metadata must survive"
                );
                assert_eq!(grown.read_text_asset(1).unwrap().script, long);
                assert_eq!(object(&grown, 2), opaque);
                assert_eq!(object(&grown, 4), mesh);
                assert!(grown.data.ends_with(b"TRAILING-OPAQUE"));
                for o in &grown.objects {
                    assert_eq!(o.data_abs % 8, old_offsets[&o.path_id]);
                }
                let grown_len = grown.data.len();
                let short = grown
                    .rewrite_text_assets(&BTreeMap::from([
                        (1, "Hola".into()),
                        (3, "Menu.START: Sí\n".into()),
                    ]))
                    .unwrap();
                assert!(short.len() < grown_len);
                let short = SerializedFile::parse(short, "short.assets").unwrap();
                assert_eq!(short.read_text_asset(1).unwrap().script, "Hola");
                assert_eq!(object(&short, 2), opaque);
                assert_eq!(object(&short, 4), mesh);
            }
        }
    }
}

#[test]
fn malformed_layouts_never_gain_resize_capability() {
    let valid = fixture(22, false, true);
    let sf = SerializedFile::parse(valid.clone(), "fixture.assets").unwrap();
    let first = sf.objects.iter().find(|o| o.path_id == 1).unwrap();
    let second = sf.objects.iter().find(|o| o.path_id == 2).unwrap();
    let mut overlap = valid.clone();
    overlap[second.table_offset..second.table_offset + 8]
        .copy_from_slice(&(first.data_abs - sf.header.data_offset).to_le_bytes());
    assert!(SerializedFile::parse(overlap, "bad.assets")
        .unwrap()
        .rewriteable_text_assets()
        .is_err());
    let mut duplicate = valid.clone();
    duplicate[second.table_offset - 8..second.table_offset].copy_from_slice(&1i64.to_le_bytes());
    assert!(SerializedFile::parse(duplicate, "bad.assets")
        .unwrap()
        .rewriteable_text_assets()
        .is_err());
    let mut trailing = valid.clone();
    trailing[first.table_offset + 8..first.table_offset + 12]
        .copy_from_slice(&(first.byte_size + 4).to_le_bytes());
    assert!(!SerializedFile::parse(trailing, "tail.assets")
        .unwrap()
        .rewriteable_text_assets()
        .unwrap()
        .contains_key(&1));
    assert!(sf
        .rewrite_text_assets(&BTreeMap::from([(
            1,
            "X".repeat(MAX_REBUILT_TEXT_ASSET_BYTES + 1)
        )]))
        .is_err());
    for cut in 0..valid.len() {
        assert!(SerializedFile::parse(valid[..cut].to_vec(), "truncated.assets").is_err());
    }
}

#[test]
fn unityfs_resize_preserves_multiple_nodes_blocks_ress_and_storage_tail() {
    let sf = fixture(22, false, true);
    let ress = b"NEUTRAL streaming resource\0\xff";
    for lz4 in [false, true] {
        for version in [6, 7, 8] {
            for end in [false, true] {
                for pad in [false, true] {
                    let packed = build_test_bundle(
                        &[
                            ("CAB-main", &sf),
                            ("CAB-main.resS", ress),
                            ("nested/CAB-other", &sf),
                        ],
                        lz4,
                        version,
                        pad,
                        end,
                    );
                    let mut archive = UnityFsArchive::parse(packed, "fixture.unity3d").unwrap();
                    let total = archive.uncompressed.len();
                    archive.blocks = [17, 41, total - 58]
                        .into_iter()
                        .map(|n| StorageBlock {
                            uncompressed_size: n as u32,
                            compressed_size: 0,
                            flags: if lz4 { 2 } else { 0 },
                        })
                        .collect();
                    let other = archive
                        .node_bytes(archive.node("nested/CAB-other").unwrap())
                        .unwrap()
                        .to_vec();
                    let bigger = SerializedFile::parse(sf.clone(), "main")
                        .unwrap()
                        .rewrite_text_assets(&BTreeMap::from([(
                            1,
                            "Larger translated script. ".repeat(50),
                        )]))
                        .unwrap();
                    archive.resize_node("CAB-main", &bigger).unwrap();
                    let mut again =
                        UnityFsArchive::parse(archive.write_bytes().unwrap(), "again").unwrap();
                    assert_eq!(
                        again
                            .node_bytes(again.node("CAB-main.resS").unwrap())
                            .unwrap(),
                        ress
                    );
                    assert_eq!(
                        again
                            .node_bytes(again.node("nested/CAB-other").unwrap())
                            .unwrap(),
                        other
                    );
                    assert_eq!(
                        again.node_bytes(again.node("CAB-main").unwrap()).unwrap(),
                        bigger
                    );
                    again.resize_node("CAB-main", &sf).unwrap();
                    let shrink =
                        UnityFsArchive::parse(again.write_bytes().unwrap(), "shrink").unwrap();
                    assert_eq!(
                        shrink.node_bytes(shrink.node("CAB-main").unwrap()).unwrap(),
                        sf
                    );
                    assert_eq!(
                        shrink
                            .node_bytes(shrink.node("CAB-main.resS").unwrap())
                            .unwrap(),
                        ress
                    );
                }
            }
        }
    }
}

#[test]
fn plugin_edits_original_offsets_before_textasset_relayout_standalone_and_bundle() {
    for bundled in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let original = fixture(22, false, true);
        let path = temp.path().join(if bundled {
            "data.unity3d"
        } else {
            "resources.assets"
        });
        let bytes = if bundled {
            build_test_bundle(
                &[("CAB-main", &original), ("CAB-main.resS", b"resource")],
                true,
                8,
                true,
                true,
            )
        } else {
            original.clone()
        };
        std::fs::write(&path, bytes).unwrap();
        let plugin = UnityPlugin::new();
        let mut entries = plugin.extract(&path).unwrap();
        let mut expected = 0;
        for e in &mut entries {
            let translated = match e.source.as_str() {
                "Hello traveler." => {
                    Some("Hola viajero, bienvenido a esta aventura mucho más extensa.")
                }
                "START" => Some("Iniciar una nueva partida desde el principio"),
                "EXIT" => Some("Salir del juego y volver al escritorio"),
                "Welcome here" => Some("Bienvenido"),
                _ => None,
            };
            if let Some(text) = translated {
                e.translation = Some(text.into());
                expected += 1;
            }
            if e.metadata.get("extraction_method").and_then(|v| v.as_str()) == Some("textasset") {
                assert!(e.metadata.contains_key("textasset_rewrite"));
                assert_eq!(e.char_limit, None);
            }
        }
        assert_eq!(expected, 4, "{entries:?}");
        let report = plugin.inject(temp.path(), &entries).unwrap();
        assert_eq!(report.strings_written, 4, "{report:?}");
        let data = std::fs::read(&path).unwrap();
        let rewritten = if bundled {
            let a = UnityFsArchive::parse(data, "bundle").unwrap();
            assert_eq!(
                a.node_bytes(a.node("CAB-main.resS").unwrap()).unwrap(),
                b"resource"
            );
            a.node_bytes(a.node("CAB-main").unwrap()).unwrap().to_vec()
        } else {
            data
        };
        let sf = SerializedFile::parse(rewritten, "result").unwrap();
        assert!(sf
            .read_text_asset(1)
            .unwrap()
            .script
            .contains("mucho más extensa"));
        assert!(sf
            .read_text_asset(3)
            .unwrap()
            .script
            .contains("volver al escritorio"));
        assert!(sf.read_text_mesh(4).unwrap().text.starts_with("Bienvenido"));
        let original = SerializedFile::parse(original, "before").unwrap();
        assert_eq!(object(&sf, 2), object(&original, 2));
    }
}

fn lzma_unity(data: &[u8]) -> Vec<u8> {
    let mut encoded = Vec::new();
    lzma_rs::lzma_compress(&mut std::io::Cursor::new(data), &mut encoded).unwrap();
    encoded.drain(5..13);
    encoded
}

fn lzma_bundle(main: &[u8]) -> Vec<u8> {
    let mut plain = main.to_vec();
    plain.extend_from_slice(b"resource");
    let compressed = lzma_unity(&plain);
    let mut info = vec![0; 16];
    u32e(&mut info, 1, true);
    u32e(&mut info, plain.len() as u32, true);
    u32e(&mut info, compressed.len() as u32, true);
    info.extend_from_slice(&1u16.to_be_bytes());
    u32e(&mut info, 2, true);
    for (offset, size, name) in [
        (0, main.len(), "CAB-main"),
        (main.len(), 8, "CAB-main.resS"),
    ] {
        u64e(&mut info, offset as u64, true);
        u64e(&mut info, size as u64, true);
        u32e(&mut info, 4, true);
        info.extend_from_slice(name.as_bytes());
        info.push(0);
    }
    let packed_info = lzma_unity(&info);
    let mut out = b"UnityFS\0".to_vec();
    u32e(&mut out, 8, true);
    out.extend_from_slice(b"5.x.x\0neutral\0");
    let size_at = out.len();
    u64e(&mut out, 0, true);
    u32e(&mut out, packed_info.len() as u32, true);
    u32e(&mut out, info.len() as u32, true);
    u32e(&mut out, 0x41, true);
    align(&mut out, 16);
    out.extend_from_slice(&packed_info);
    out.extend_from_slice(&compressed);
    let total = out.len() as u64;
    out[size_at..size_at + 8].copy_from_slice(&total.to_be_bytes());
    out
}

#[test]
fn lzma_info_and_storage_rebuild_to_supported_compression() {
    let source = fixture(21, true, true);
    let mut archive = UnityFsArchive::parse(lzma_bundle(&source), "lzma.bundle").unwrap();
    let grown = SerializedFile::parse(source, "main")
        .unwrap()
        .rewrite_text_assets(&BTreeMap::from([(
            1,
            "A longer translated multilingual line: 日本語".repeat(30),
        )]))
        .unwrap();
    archive.resize_node("CAB-main", &grown).unwrap();
    let after = UnityFsArchive::parse(archive.write_bytes().unwrap(), "repacked").unwrap();
    assert_eq!(
        after.node_bytes(after.node("CAB-main").unwrap()).unwrap(),
        grown
    );
    assert_eq!(
        after
            .node_bytes(after.node("CAB-main.resS").unwrap())
            .unwrap(),
        b"resource"
    );
    assert!(after
        .blocks
        .iter()
        .all(|b| matches!(b.compression(), 0 | 2)));
}

#[test]
fn hostile_node_layouts_and_unknown_bundle_modes_refuse_relayout_atomically() {
    let bytes = build_test_bundle(
        &[("CAB-a", b"abc"), ("CAB-b", b"def")],
        false,
        8,
        false,
        false,
    );
    for kind in 0..5 {
        let mut archive = UnityFsArchive::parse(bytes.clone(), "source").unwrap();
        match kind {
            0 => archive.nodes[1].offset = 1,
            1 => archive.nodes[1].path = "CAB-a".into(),
            2 => archive.nodes[1].size = i64::MAX,
            3 => archive.header.version = 99,
            _ => archive.header.flags |= 0x1000,
        }
        let original = archive.uncompressed.clone();
        assert!(!archive.supports_relayout());
        assert!(archive.resize_node("CAB-a", b"growth").is_err());
        assert_eq!(archive.uncompressed, original);
    }
    for cut in 0..bytes.len() {
        let result =
            std::panic::catch_unwind(|| UnityFsArchive::parse(bytes[..cut].to_vec(), "cut"));
        assert!(result.is_ok(), "panic at {cut}");
        assert!(result.unwrap().is_err(), "accepted truncation at {cut}");
    }
}

#[test]
fn forged_resize_target_does_not_write_any_asset() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("resources.assets");
    let bytes = fixture(22, false, true);
    std::fs::write(&path, &bytes).unwrap();
    let plugin = UnityPlugin::new();
    let mut entries = plugin.extract(&path).unwrap();
    entries.retain(|e| e.source == "Hello traveler.");
    assert_eq!(entries.len(), 1);
    entries[0].translation = Some("Hola".into());
    entries[0]
        .metadata
        .insert("path_id".into(), serde_json::json!(999));
    let report = plugin.inject(temp.path(), &entries).unwrap();
    assert_eq!(report.strings_written, 0);
    assert_eq!(report.skip_reasons.get("invalid_target"), Some(&1));
    assert_eq!(std::fs::read(path).unwrap(), bytes);
}

#[test]
fn csv_cells_grow_the_complete_blob_without_padding_or_per_cell_limits() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("resources.assets");
    let script = "ITEM_NAME,DESCRIPTION,ID\nSword,Sharp,1\nShield,Strong,2\n";
    let bytes = write_v17_fixture("Items", script);
    std::fs::write(&path, bytes).unwrap();
    let plugin = UnityPlugin::new();
    let mut entries = plugin.extract(&path).unwrap();
    entries.retain(|e| {
        e.metadata.get("extraction_method").and_then(|v| v.as_str()) == Some("textasset_csv_cell")
    });
    assert_eq!(entries.len(), 4);
    for e in &mut entries {
        assert!(e.metadata.contains_key("textasset_rewrite"));
        if e.source == "Sword" {
            e.translation = Some("Espada muy larga de los guardianes antiguos".into());
        } else if e.source == "Strong" {
            e.translation = Some("Resistente contra los ataques más poderosos".into());
        }
    }
    let report = plugin.inject(temp.path(), &entries).unwrap();
    assert_eq!(report.strings_written, 2, "{report:?}");
    let result = SerializedFile::parse_path(&path)
        .unwrap()
        .read_text_asset(1)
        .unwrap()
        .script;
    assert!(result.len() > script.len());
    assert!(result.contains("Espada muy larga"));
    assert!(result.contains("Sharp,1\n"));
    assert!(result.contains("más poderosos,2\n"));
}

#[test]
fn serialized_metadata_and_script_corruption_corpus_has_no_panics() {
    let mut cases = 0;
    let mut accepted = 0;
    for version in [17, 22] {
        let source = fixture(version, false, true);
        let original = SerializedFile::parse(source.clone(), "source").unwrap();
        // Payload and table mutations stay small; deliberately huge counts are
        // separately rejected by remaining-byte checks before allocation.
        let table = original.objects[0].table_offset - 8;
        for at in table..source.len() {
            for mask in [1u8, 0x80, 0xff] {
                let mut mutated = source.clone();
                mutated[at] ^= mask;
                cases += 1;
                let result = std::panic::catch_unwind(|| {
                    if let Ok(sf) = SerializedFile::parse(mutated, "mutated") {
                        if let Ok(assets) = sf.rewriteable_text_assets() {
                            if assets.contains_key(&1) {
                                let rewritten = sf
                                    .rewrite_text_assets(&BTreeMap::from([(
                                        1,
                                        "Neutral expanded dialogue for corpus".into(),
                                    )]))
                                    .unwrap();
                                assert_eq!(
                                    SerializedFile::parse(rewritten, "rebuilt")
                                        .unwrap()
                                        .read_text_asset(1)
                                        .unwrap()
                                        .script,
                                    "Neutral expanded dialogue for corpus"
                                );
                                return true;
                            }
                        }
                    }
                    false
                });
                assert!(
                    result.is_ok(),
                    "panic version={version} offset={at} mask={mask}"
                );
                accepted += usize::from(result.unwrap());
            }
        }
    }
    println!("UNITY_REWRITE_CORPUS cases={cases} accepted_rebuilt={accepted} rejected_or_no_capability={}",cases-accepted);
}
