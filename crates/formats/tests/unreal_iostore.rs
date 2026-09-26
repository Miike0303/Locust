//! Synthetic header/companion-layout tests, not real IoStore chunk decoding.
use locust_core::extraction::FormatPlugin;
use locust_formats::unreal::UnrealPlugin;
use locust_formats::unreal_iostore::{find_toc, is_toc, toc_for_container, TOC_MAGIC};
use locust_formats::unreal_locres::{
    str_crc32_ue, LocresFile, LocresNamespace, LocresString, LocresVersion,
};
use locust_formats::unreal_pak::{write_pak, PakWriteFile, DEFAULT_MOUNT_POINT, PAK_MAGIC};
use sha1::{Digest, Sha1};
use std::fs;
use std::path::{Path, PathBuf};

fn header() -> Vec<u8> {
    let mut bytes = vec![0u8; 144];
    bytes[..16].copy_from_slice(TOC_MAGIC);
    bytes[16] = 5; // PerfectHashWithOverflow (identity only)
    bytes[20..24].copy_from_slice(&144u32.to_le_bytes());
    bytes
}

fn toc(dir: &Path, name: &str) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, header()).unwrap();
    path
}

fn locres(text: &str) -> Vec<u8> {
    LocresFile {
        version: LocresVersion::Compact,
        namespaces: vec![LocresNamespace {
            name: "Menu".into(),
            name_hash: 0,
            strings: vec![LocresString {
                key: "Greeting".into(),
                value: text.into(),
                source_string_hash: str_crc32_ue("Welcome aboard"),
                key_hash: 0,
            }],
        }],
    }
    .serialize()
    .unwrap()
}

fn pak(dir: &Path, name: &str, text: Option<&str>) -> PathBuf {
    let files: Vec<_> = text
        .map(|text| PakWriteFile {
            name: "Game/Content/Localization/Game/en/Game.locres".into(),
            data: locres(text),
        })
        .into_iter()
        .collect();
    let path = dir.join(name);
    let bytes = if files.is_empty() {
        // A valid empty v3 classic PAK. The patch writer intentionally disallows
        // creating empty patches, but IoStore companion PAKs can be empty.
        let mut index = ((DEFAULT_MOUNT_POINT.len() + 1) as u32)
            .to_le_bytes()
            .to_vec();
        index.extend_from_slice(DEFAULT_MOUNT_POINT.as_bytes());
        index.push(0);
        index.extend_from_slice(&0u32.to_le_bytes());
        let mut pak = index.clone();
        pak.extend_from_slice(&PAK_MAGIC.to_le_bytes());
        pak.extend_from_slice(&3u32.to_le_bytes());
        pak.extend_from_slice(&0u64.to_le_bytes());
        pak.extend_from_slice(&(index.len() as u64).to_le_bytes());
        pak.extend_from_slice(&Sha1::digest(&index));
        pak
    } else {
        write_pak(DEFAULT_MOUNT_POINT, 8, &files, name).unwrap()
    };
    fs::write(&path, bytes).unwrap();
    path
}

#[test]
fn recognizes_toc_only_directory_and_gives_actionable_diagnosis() {
    let dir = tempfile::tempdir().unwrap();
    toc(dir.path(), "game.utoc");
    let plugin = UnrealPlugin::new();
    assert!(plugin.detect(dir.path()));
    let error = plugin.extract(dir.path()).unwrap_err().to_string();
    assert!(error.contains("IoStore container detected"), "{error}");
    assert!(error.contains("complete game folder"), "{error}");
    assert!(
        error.contains("does not mean the game contains no text"),
        "{error}"
    );
}

#[test]
fn mixed_game_extracts_companion_pak_even_when_toc_flags_are_encrypted() {
    let dir = tempfile::tempdir().unwrap();
    let toc = toc(dir.path(), "game.utoc");
    let mut bytes = header();
    bytes[80] = 2; // encrypted IoStore, unrelated unencrypted companion PAK
    fs::write(toc, bytes).unwrap();
    let pak = pak(dir.path(), "game.pak", Some("Welcome aboard"));
    let entries = UnrealPlugin::new().extract(dir.path()).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].source, "Welcome aboard");
    assert_eq!(entries[0].file_path, pak);
}

#[test]
fn direct_toc_opens_sibling_paks_with_whole_resource_patch_priority() {
    let dir = tempfile::tempdir().unwrap();
    let toc = toc(dir.path(), "game.utoc");
    pak(dir.path(), "game.pak", Some("Old welcome"));
    pak(dir.path(), "game_2_P.pak", Some("Updated welcome"));
    let winner = pak(dir.path(), "game_10_P.pak", Some("Final welcome"));
    let plugin = UnrealPlugin::new();
    assert!(plugin.detect(&toc));
    let entries = plugin.extract(&toc).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].source, "Final welcome");
    assert_eq!(entries[0].file_path, winner);
}

#[test]
fn direct_ucas_and_partition_resolve_case_insensitive_toc() {
    let dir = tempfile::tempdir().unwrap();
    let toc = toc(dir.path(), "GAME.UTOC");
    pak(dir.path(), "game.pak", Some("Welcome aboard"));
    for name in ["game.UCAS", "game_s2.ucas"] {
        let ucas = dir.path().join(name);
        fs::write(&ucas, b"payload is deliberately not an IoStore header").unwrap();
        assert_eq!(
            toc_for_container(&ucas).unwrap().canonicalize().unwrap(),
            toc.canonicalize().unwrap()
        );
        assert!(UnrealPlugin::new().detect(&ucas));
        let entries = UnrealPlugin::new().extract(&ucas).unwrap();
        assert_eq!(entries.len(), 1);
    }
}

#[test]
fn unpaired_ucas_is_not_claimed_and_has_explicit_error() {
    let dir = tempfile::tempdir().unwrap();
    let ucas = dir.path().join("game.ucas");
    fs::write(&ucas, header()).unwrap(); // even accidental magic cannot pair it
    assert!(!UnrealPlugin::new().detect(&ucas));
    let error = UnrealPlugin::new().extract(&ucas).unwrap_err().to_string();
    assert!(error.contains("missing matching IoStore .utoc"), "{error}");
}

#[test]
fn malformed_and_truncated_headers_do_not_misidentify_a_game() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("game.utoc");
    let valid = header();
    for len in [0, 15, 16, 24, 143] {
        fs::write(&path, &valid[..len]).unwrap();
        assert!(!is_toc(&path));
        assert!(!UnrealPlugin::new().detect(dir.path()));
    }
    for (offset, value) in [(0, b'x'), (16, 0), (20, 255)] {
        let mut invalid = valid.clone();
        invalid[offset] = value;
        fs::write(&path, invalid).unwrap();
        assert!(!is_toc(&path));
    }
    let mut invalid = valid.clone();
    invalid[20..24].copy_from_slice(&24u32.to_le_bytes());
    fs::write(&path, invalid).unwrap();
    assert!(!is_toc(&path));
}

#[test]
fn unknown_toc_version_can_route_to_independent_supported_pak() {
    let dir = tempfile::tempdir().unwrap();
    let path = toc(dir.path(), "game.utoc");
    let mut bytes = header();
    bytes[16] = 255; // no claim that we decode this version's chunk format
    bytes[24..80].fill(255); // no allocations from untrusted TOC table counts
    fs::write(&path, bytes).unwrap();
    pak(dir.path(), "game.pak", Some("Welcome aboard"));
    assert_eq!(UnrealPlugin::new().extract(&path).unwrap().len(), 1);
}

#[test]
fn empty_companion_pak_does_not_silently_return_success() {
    let dir = tempfile::tempdir().unwrap();
    let path = toc(dir.path(), "game.utoc");
    let pak = pak(dir.path(), "game.pak", None);
    for selected in [&path, &dir.path().to_path_buf(), &pak] {
        let error = UnrealPlugin::new()
            .extract(selected)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("No readable localization strings"),
            "{error}"
        );
    }
}

#[test]
fn loose_locres_remains_usable_without_companion_pak() {
    let dir = tempfile::tempdir().unwrap();
    let path = toc(dir.path(), "game.utoc");
    fs::write(dir.path().join("Game.locres"), locres("Welcome aboard")).unwrap();
    assert_eq!(UnrealPlugin::new().extract(&path).unwrap().len(), 1);
}

#[test]
fn discovery_stops_at_same_depth_as_pak_walk() {
    let dir = tempfile::tempdir().unwrap();
    let nested = dir.path().join("a/b/c/d/e");
    fs::create_dir_all(&nested).unwrap();
    toc(&nested, "too-deep.utoc");
    assert!(find_toc(dir.path()).is_none());
    let shallow = toc(nested.parent().unwrap(), "within-depth.utoc");
    assert_eq!(find_toc(dir.path()), Some(shallow));
}

#[test]
fn companion_extraction_injection_roundtrip_writes_only_patch_pak() {
    let dir = tempfile::tempdir().unwrap();
    let toc = toc(dir.path(), "game.utoc");
    let ucas = dir.path().join("game.ucas");
    fs::write(&ucas, b"original UCAS bytes").unwrap();
    let source_pak = pak(dir.path(), "game.pak", Some("Welcome aboard"));
    let original_pak = fs::read(&source_pak).unwrap();
    let plugin = UnrealPlugin::new();
    let mut entries = plugin.extract(&toc).unwrap();
    entries[0].translation = Some("Bienvenue à bord".into());
    let report = plugin.inject(&toc, &entries).unwrap();
    assert_eq!(report.strings_written, 1);
    assert_eq!(fs::read(&source_pak).unwrap(), original_pak);
    assert_eq!(fs::read(&toc).unwrap(), header());
    assert_eq!(fs::read(&ucas).unwrap(), b"original UCAS bytes");
    let reread = plugin.extract(&toc).unwrap();
    assert_eq!(reread.len(), 1);
    assert_eq!(reread[0].source, "Bienvenue à bord");
}
