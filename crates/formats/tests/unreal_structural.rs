//! Structural Unreal LocRes identity, conventional PAK priority, and
//! seek-based classic index reads. Tests are not executed in this snapshot.

use locust_core::extraction::FormatPlugin;
use locust_core::models::StringEntry;
use locust_formats::unreal::UnrealPlugin;
use locust_formats::unreal_locres::{
    str_crc32_ue, LocresFile, LocresNamespace, LocresString, LocresVersion,
};
use locust_formats::unreal_pak::{
    payload_offset, read_footer, read_index, read_index_from_reader, read_uncompressed_payload,
    write_pak, write_pak_marked_compressed, PakWriteFile, DEFAULT_MOUNT_POINT, PAK_MAGIC,
};
use std::fs::{self, File};
use std::io::{Cursor, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

fn locres(ns: &str, pairs: &[(&str, &str)]) -> LocresFile {
    LocresFile {
        version: LocresVersion::Compact,
        namespaces: vec![LocresNamespace {
            name: ns.into(),
            name_hash: 0,
            strings: pairs
                .iter()
                .map(|(key, value)| LocresString {
                    key: (*key).into(),
                    value: (*value).into(),
                    source_string_hash: str_crc32_ue(value),
                    key_hash: 0,
                })
                .collect(),
        }],
    }
}

fn inner(culture: &str, target: &str) -> String {
    format!("TestGame/Content/Localization/{target}/{culture}/{target}.locres")
}

fn write_pak_named(dir: &Path, file_name: &str, files: &[(&str, LocresFile)]) -> PathBuf {
    write_pak_named_with(dir, file_name, DEFAULT_MOUNT_POINT, files, false)
}

fn write_pak_named_with(
    dir: &Path,
    file_name: &str,
    mount: &str,
    files: &[(&str, LocresFile)],
    compressed: bool,
) -> PathBuf {
    let paks = dir.join("TestGame").join("Content").join("Paks");
    fs::create_dir_all(&paks).unwrap();
    let write_files: Vec<PakWriteFile> = files
        .iter()
        .map(|(name, file)| PakWriteFile {
            name: (*name).into(),
            data: file.serialize().unwrap(),
        })
        .collect();
    let bytes = if compressed {
        write_pak_marked_compressed(mount, 8, &write_files, file_name).unwrap()
    } else {
        write_pak(mount, 8, &write_files, file_name).unwrap()
    };
    let path = paks.join(file_name);
    fs::write(&path, bytes).unwrap();
    path
}

fn greeting_of(entries: &[StringEntry]) -> Vec<String> {
    let mut hits: Vec<String> = entries
        .iter()
        .filter(|e| e.metadata.get("locres_key").and_then(|v| v.as_str()) == Some("Greeting"))
        .map(|e| e.source.clone())
        .collect();
    hits.sort();
    hits
}

fn ids_for_key(entries: &[StringEntry], key: &str) -> Vec<String> {
    let mut ids: Vec<String> = entries
        .iter()
        .filter(|e| e.metadata.get("locres_key").and_then(|v| v.as_str()) == Some(key))
        .map(|e| e.id.clone())
        .collect();
    ids.sort();
    ids
}

struct ReadCounter<R> {
    inner: R,
    bytes_read: usize,
}

impl<R: Read> Read for ReadCounter<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.bytes_read += n;
        Ok(n)
    }
}

impl<R: Seek> Seek for ReadCounter<R> {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.inner.seek(pos)
    }
}

fn write_gapped_pak(path: &Path, gap: u64, files: &[PakWriteFile]) -> u64 {
    let compact = write_pak(DEFAULT_MOUNT_POINT, 8, files, "gapped.pak").unwrap();
    let footer = read_footer(&compact, "gapped.pak").unwrap();
    let data_end = footer.index_offset;
    let mut tail = compact[data_end as usize..].to_vec();
    let new_index = data_end + gap;
    let magic = PAK_MAGIC.to_le_bytes();
    let magic_pos = tail.windows(4).rposition(|w| w == magic).unwrap();
    tail[magic_pos + 8..magic_pos + 16].copy_from_slice(&new_index.to_le_bytes());
    let mut f = File::create(path).unwrap();
    f.write_all(&compact[..data_end as usize]).unwrap();
    f.seek(SeekFrom::Start(new_index)).unwrap();
    f.write_all(&tail).unwrap();
    f.metadata().unwrap().len()
}

#[test]
fn base_patch_and_generated_override_by_resource() {
    let dir = tempfile::tempdir().unwrap();
    let en = inner("en", "Game");
    write_pak_named(
        dir.path(),
        "Game.pak",
        &[(
            en.as_str(),
            locres("Dialog", &[("Greeting", "Hello"), ("Farewell", "Bye")]),
        )],
    );
    write_pak_named(
        dir.path(),
        "Game_P.pak",
        &[(
            en.as_str(),
            locres("Dialog", &[("Greeting", "Hello patch")]),
        )],
    );
    write_pak_named(
        dir.path(),
        "Game_LOCUST_P.pak",
        &[(
            en.as_str(),
            locres("Dialog", &[("Greeting", "Hola generado")]),
        )],
    );

    let plugin = UnrealPlugin::new();
    let entries = plugin.extract(dir.path()).unwrap();
    assert_eq!(
        greeting_of(&entries),
        vec!["Hola generado".to_string()],
        "LOCUST patch must replace the whole resource, got {:?}",
        entries
            .iter()
            .map(|e| (e.id.as_str(), e.source.as_str()))
            .collect::<Vec<_>>()
    );
    assert!(
        entries
            .iter()
            .all(|e| e.source != "Bye" && e.source != "Hello" && e.source != "Hello patch"),
        "superseded resource keys must not mix into the winner"
    );
}

#[test]
fn shuffled_creation_order_does_not_change_winner() {
    let dir = tempfile::tempdir().unwrap();
    let en = inner("en", "Game");
    write_pak_named(
        dir.path(),
        "Game_LOCUST_P.pak",
        &[(en.as_str(), locres("Dialog", &[("Greeting", "Hola")]))],
    );
    write_pak_named(
        dir.path(),
        "Game.pak",
        &[(en.as_str(), locres("Dialog", &[("Greeting", "Hello")]))],
    );
    write_pak_named(
        dir.path(),
        "Game_P.pak",
        &[(en.as_str(), locres("Dialog", &[("Greeting", "Patched")]))],
    );

    let plugin = UnrealPlugin::new();
    let entries = plugin.extract(dir.path()).unwrap();
    assert_eq!(greeting_of(&entries), vec!["Hola".to_string()]);
}

#[test]
fn two_cultures_same_key_stay_separate() {
    let dir = tempfile::tempdir().unwrap();
    write_pak_named(
        dir.path(),
        "Game.pak",
        &[
            (
                inner("en", "Game").as_str(),
                locres("Dialog", &[("Greeting", "Hello")]),
            ),
            (
                inner("es", "Game").as_str(),
                locres("Dialog", &[("Greeting", "Hola")]),
            ),
        ],
    );
    let plugin = UnrealPlugin::new();
    let entries = plugin.extract(dir.path()).unwrap();
    let ids = ids_for_key(&entries, "Greeting");
    assert_eq!(ids.len(), 2, "ids={ids:?}");
    assert!(ids.iter().any(|id| id.contains("/en/")), "{ids:?}");
    assert!(ids.iter().any(|id| id.contains("/es/")), "{ids:?}");
    assert_ne!(ids[0], ids[1]);
    let cultures: Vec<_> = entries
        .iter()
        .filter_map(|e| {
            e.metadata
                .get("locres_culture")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        })
        .collect();
    assert!(cultures.contains(&"en".to_string()));
    assert!(cultures.contains(&"es".to_string()));
}

#[test]
fn same_key_different_resource_is_not_deduped() {
    let dir = tempfile::tempdir().unwrap();
    write_pak_named(
        dir.path(),
        "Game.pak",
        &[
            (
                inner("en", "Game").as_str(),
                locres("Dialog", &[("Greeting", "Hello")]),
            ),
            (
                inner("en", "Engine").as_str(),
                locres("Dialog", &[("Greeting", "Hello")]),
            ),
        ],
    );
    let plugin = UnrealPlugin::new();
    let entries = plugin.extract(dir.path()).unwrap();
    let ids = ids_for_key(&entries, "Greeting");
    assert_eq!(
        ids.len(),
        2,
        "same text must stay distinct by virtual path: {ids:?}"
    );
    assert!(ids.iter().any(|id| id.contains("/Game/")));
    assert!(ids.iter().any(|id| id.contains("/Engine/")));
}

#[test]
fn extract_rejects_encrypted_index_and_sha1_mismatch() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_pak_named(
        dir.path(),
        "Game.pak",
        &[(
            inner("en", "Game").as_str(),
            locres("Dialog", &[("Greeting", "Hello")]),
        )],
    );
    let plugin = UnrealPlugin::new();

    let mut encrypted = fs::read(&path).unwrap();
    let footer = read_footer(&encrypted, "Game.pak").unwrap();
    encrypted[footer.magic_offset - 1] = 1;
    fs::write(&path, &encrypted).unwrap();
    let err = plugin.extract(dir.path()).unwrap_err();
    assert!(err.to_string().contains("encrypted"), "{err}");

    fs::write(
        &path,
        write_pak(
            DEFAULT_MOUNT_POINT,
            8,
            &[PakWriteFile {
                name: inner("en", "Game"),
                data: locres("Dialog", &[("Greeting", "Hello")])
                    .serialize()
                    .unwrap(),
            }],
            "Game.pak",
        )
        .unwrap(),
    )
    .unwrap();
    let mut bad_hash = fs::read(&path).unwrap();
    let footer = read_footer(&bad_hash, "Game.pak").unwrap();
    bad_hash[footer.magic_offset + 24] ^= 0xFF;
    fs::write(&path, &bad_hash).unwrap();
    let err = plugin.extract(dir.path()).unwrap_err();
    assert!(err.to_string().contains("SHA-1"), "{err}");
}

#[test]
fn extract_rejects_index_offset_past_eof() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_pak_named(
        dir.path(),
        "Game.pak",
        &[(
            inner("en", "Game").as_str(),
            locres("Dialog", &[("Greeting", "Hello")]),
        )],
    );
    let mut bytes = fs::read(&path).unwrap();
    let footer = read_footer(&bytes, "Game.pak").unwrap();
    let huge = (bytes.len() as u64 + 1_000_000).to_le_bytes();
    bytes[footer.magic_offset + 8..footer.magic_offset + 16].copy_from_slice(&huge);
    fs::write(&path, &bytes).unwrap();
    let plugin = UnrealPlugin::new();
    let err = plugin.extract(dir.path()).unwrap_err();
    assert!(
        err.to_string().contains("past EOF") || err.to_string().contains("index"),
        "{err}"
    );
}

#[test]
fn extract_rejects_forged_v9_footer_without_heuristic_slurp() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_pak_named(
        dir.path(),
        "Game.pak",
        &[(
            inner("en", "Game").as_str(),
            locres("Dialog", &[("Greeting", "Hello")]),
        )],
    );
    let mut bytes = fs::read(&path).unwrap();
    let footer = read_footer(&bytes, "Game.pak").unwrap();
    bytes[footer.magic_offset + 4..footer.magic_offset + 8].copy_from_slice(&9u32.to_le_bytes());
    fs::write(&path, &bytes).unwrap();
    let plugin = UnrealPlugin::new();
    let err = plugin.extract(dir.path()).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("invalid frozen-index flag") || msg.contains("unsupported"),
        "{msg}"
    );
}

#[test]
fn bounded_sparse_index_read_does_not_slurp_the_gap() {
    let dir = tempfile::tempdir().unwrap();
    let paks = dir.path().join("TestGame").join("Content").join("Paks");
    fs::create_dir_all(&paks).unwrap();
    let loc = locres("Dialog", &[("Greeting", "Sparse hello")]);
    let files = [PakWriteFile {
        name: inner("en", "Game"),
        data: loc.serialize().unwrap(),
    }];
    let pak_path = paks.join("Game.pak");
    let gap = 4 * 1024 * 1024;
    let file_len = write_gapped_pak(&pak_path, gap, &files);
    assert!(file_len > gap);

    let file = File::open(&pak_path).unwrap();
    let mut reader = ReadCounter {
        inner: file,
        bytes_read: 0,
    };
    let index = read_index_from_reader(&mut reader, file_len, "gapped.pak").unwrap();
    assert_eq!(index.records.len(), 1);
    let rec = &index.records[0];
    let payload = read_uncompressed_payload(
        &mut reader,
        rec,
        index.footer.version,
        file_len,
        "gapped.pak",
    )
    .unwrap();
    let parsed = LocresFile::parse(&payload, "sparse").unwrap();
    assert!(parsed
        .iter_entries()
        .any(|(_, _, v, _)| v == "Sparse hello"));
    assert!(
        reader.bytes_read as u64 + 64 * 1024 < file_len,
        "expected a sparse index/payload read, read {} of {file_len}",
        reader.bytes_read
    );

    let plugin = UnrealPlugin::new();
    let entries = plugin.extract(dir.path()).unwrap();
    assert_eq!(greeting_of(&entries), vec!["Sparse hello".to_string()]);
}

#[test]
fn inject_roundtrip_combined_extract_shows_translated_values() {
    let dir = tempfile::tempdir().unwrap();
    let en = inner("en", "Game");
    let base = write_pak_named(
        dir.path(),
        "Game.pak",
        &[(
            en.as_str(),
            locres("Dialog", &[("Greeting", "Hello traveler")]),
        )],
    );

    let plugin = UnrealPlugin::new();
    let mut entries = plugin.extract(dir.path()).unwrap();
    assert!(
        entries
            .iter()
            .any(|e| e.id.contains("Game.locres#Dialog/Greeting")),
        "virtual path identity, got {:?}",
        entries.iter().map(|e| &e.id).collect::<Vec<_>>()
    );
    for e in &mut entries {
        if e.source == "Hello traveler" {
            e.translation = Some("Hola viajero".into());
        }
    }
    let report = plugin.inject(dir.path(), &entries).unwrap();
    assert!(report.strings_written >= 1, "{report:?}");
    let patch = dir
        .path()
        .join("TestGame")
        .join("Content")
        .join("Paks")
        .join("Game_LOCUST_P.pak");
    assert!(patch.is_file(), "missing {}", patch.display());
    assert!(!fs::read(&base).unwrap().is_empty());

    let again = plugin.extract(dir.path()).unwrap();
    assert_eq!(
        greeting_of(&again),
        vec!["Hola viajero".to_string()],
        "combined reextract must expose the generated translation"
    );

    let patch_bytes = fs::read(&patch).unwrap();
    let idx = read_index(&patch_bytes, "patch").unwrap();
    assert_eq!(idx.records.len(), 1);
    let rec = &idx.records[0];
    let poff = payload_offset(rec, idx.footer.version) as usize;
    let payload = &patch_bytes[poff..poff + rec.size as usize];
    let parsed = LocresFile::parse(payload, "patch-locres").unwrap();
    assert!(parsed
        .iter_entries()
        .any(|(_, _, v, _)| v.contains("Hola viajero")));
}

#[test]
fn numbered_patch_suffix_beats_bare_p() {
    let dir = tempfile::tempdir().unwrap();
    let en = inner("en", "Game");
    write_pak_named(
        dir.path(),
        "Game_P.pak",
        &[(en.as_str(), locres("Dialog", &[("Greeting", "P0")]))],
    );
    write_pak_named(
        dir.path(),
        "Game_P2.pak",
        &[(en.as_str(), locres("Dialog", &[("Greeting", "P2")]))],
    );
    let plugin = UnrealPlugin::new();
    let entries = plugin.extract(dir.path()).unwrap();
    assert_eq!(greeting_of(&entries), vec!["P2".to_string()]);
}

#[test]
fn seek_index_matches_slice_on_cursor() {
    let files = [PakWriteFile {
        name: inner("en", "Game"),
        data: locres("Dialog", &[("Greeting", "Hello")])
            .serialize()
            .unwrap(),
    }];
    let bytes = write_pak(DEFAULT_MOUNT_POINT, 3, &files, "t.pak").unwrap();
    let mut cursor = Cursor::new(bytes.as_slice());
    let from_reader = read_index_from_reader(&mut cursor, bytes.len() as u64, "t.pak").unwrap();
    let from_slice = read_index(&bytes, "t.pak").unwrap();
    assert_eq!(from_reader.records.len(), from_slice.records.len());
    assert_eq!(from_reader.records[0].name, from_slice.records[0].name);
}

#[test]
fn conventional_n_p_suffix_is_numeric_not_lexical() {
    let dir = tempfile::tempdir().unwrap();
    let en = inner("en", "Game");
    write_pak_named(
        dir.path(),
        "Game_2_P.pak",
        &[(en.as_str(), locres("Dialog", &[("Greeting", "from-2")]))],
    );
    write_pak_named(
        dir.path(),
        "Game_10_P.pak",
        &[(en.as_str(), locres("Dialog", &[("Greeting", "from-10")]))],
    );
    write_pak_named(
        dir.path(),
        "Game_1_P.pak",
        &[(en.as_str(), locres("Dialog", &[("Greeting", "from-1")]))],
    );
    let plugin = UnrealPlugin::new();
    let entries = plugin.extract(dir.path()).unwrap();
    assert_eq!(
        greeting_of(&entries),
        vec!["from-10".to_string()],
        "Game_10_P must beat Game_2_P; lexical 10<2 would pick from-2"
    );
}

#[test]
fn compressed_overlay_does_not_return_stale_base() {
    let dir = tempfile::tempdir().unwrap();
    let en = inner("en", "Game");
    write_pak_named(
        dir.path(),
        "Game.pak",
        &[(en.as_str(), locres("Dialog", &[("Greeting", "Hello base")]))],
    );
    write_pak_named_with(
        dir.path(),
        "Game_P.pak",
        DEFAULT_MOUNT_POINT,
        &[(
            en.as_str(),
            locres("Dialog", &[("Greeting", "Hello compressed overlay")]),
        )],
        true,
    );
    let plugin = UnrealPlugin::new();
    let err = plugin.extract(dir.path()).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("compressed") || msg.contains("unreadable"),
        "{msg}"
    );
    assert!(!msg.is_empty(), "must not succeed with stale base Greeting");
}

#[test]
fn uncompressed_overlay_still_wins_over_compressed_base() {
    let dir = tempfile::tempdir().unwrap();
    let en = inner("en", "Game");
    write_pak_named_with(
        dir.path(),
        "Game.pak",
        DEFAULT_MOUNT_POINT,
        &[(en.as_str(), locres("Dialog", &[("Greeting", "Hello base")]))],
        true,
    );
    write_pak_named(
        dir.path(),
        "Game_P.pak",
        &[(
            en.as_str(),
            locres("Dialog", &[("Greeting", "Hello patch")]),
        )],
    );
    let plugin = UnrealPlugin::new();
    let entries = plugin.extract(dir.path()).unwrap();
    assert_eq!(greeting_of(&entries), vec!["Hello patch".to_string()]);
}

#[test]
fn alias_mount_name_splits_dedupe_to_one_resource() {
    let dir = tempfile::tempdir().unwrap();
    let plugin = UnrealPlugin::new();
    write_pak_named_with(
        dir.path(),
        "Game.pak",
        "../../../",
        &[(
            "TestGame/Content/Localization/Game/en/Game.locres",
            locres("Dialog", &[("Greeting", "Hello")]),
        )],
        false,
    );
    write_pak_named_with(
        dir.path(),
        "Game_P.pak",
        "../../",
        &[(
            "../TestGame/Content/./Localization/Game/en/Game.locres",
            locres("Dialog", &[("Greeting", "Hello patch")]),
        )],
        false,
    );
    let entries = plugin.extract(dir.path()).unwrap();
    let greetings = greeting_of(&entries);
    assert_eq!(
        greetings,
        vec!["Hello patch".to_string()],
        "aliases must be one resource, overlay wins: {greetings:?}"
    );
    let ids: Vec<_> = entries.iter().map(|entry| &entry.id).collect();
    assert_eq!(ids.len(), 1, "alias split must not duplicate ids: {ids:?}");
}
