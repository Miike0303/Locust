//! Durable regression: binary inject engines tag `metadata.binary_slot`, and
//! `locust validate` / core helpers detect oversize translations before inject.

use locust_core::extraction::FormatPlugin;
use locust_core::models::ValidationKind;
use locust_core::validation::{count_binary_slot_oversize, Validator};
use locust_formats::unreal::UnrealPlugin;
use locust_formats::wolf_rpg::{build_test_fixture, WolfRpgPlugin};
use std::fs;
use std::path::PathBuf;

fn tmp(prefix: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("locust_{prefix}_{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn wolf_extract_tags_sjis_binary_slot_and_validate_catches_oversize() {
    let dir = tmp("wolf_slot");
    let data = dir.join("Data");
    fs::create_dir_all(&data).unwrap();
    fs::write(data.join("BasicData.wolf"), build_test_fixture()).unwrap();

    let plugin = WolfRpgPlugin::new();
    let mut entries = plugin.extract(&dir).unwrap();
    assert!(!entries.is_empty());
    for e in &entries {
        assert_eq!(
            e.metadata.get("binary_slot").and_then(|v| v.as_str()),
            Some("sjis"),
            "wolf entry {} missing binary_slot",
            e.id
        );
    }

    // Force an oversize translation on the first entry.
    entries[0].translation = Some("この文字列は元の文字列よりもはるかに長いです！！！！".into());
    let n = count_binary_slot_oversize(&entries);
    assert!(n >= 1, "expected at least one ExceedsBinarySlot, got {n}");
    let issues = Validator::validate_entry(&entries[0]);
    assert!(
        issues
            .iter()
            .any(|i| matches!(i.kind, ValidationKind::ExceedsBinarySlot { .. })),
        "validate_entry: {issues:?}"
    );
}

#[test]
fn unreal_extract_tags_utf16le_binary_slot() {
    let dir = tmp("ue_slot");
    let paks = dir.join("TestGame").join("Content").join("Paks");
    fs::create_dir_all(&paks).unwrap();

    // Minimal PAK fixture with embedded UTF-16LE "Hello World"
    let mut data: Vec<u8> = vec![0; 32];
    for ch in "Hello World".encode_utf16() {
        data.extend_from_slice(&ch.to_le_bytes());
    }
    data.extend_from_slice(&[0, 0]);
    data.extend_from_slice(&[0xFF; 16]);
    for ch in "Press Start".encode_utf16() {
        data.extend_from_slice(&ch.to_le_bytes());
    }
    data.extend_from_slice(&[0, 0, 0, 0]);
    // Footer magic — pak discovery requires it to reject Chromium/NW.js packs.
    data.extend_from_slice(&locust_formats::unreal_pak::PAK_MAGIC.to_le_bytes());
    data.extend_from_slice(&[0; 40]);
    fs::write(paks.join("TestGame.pak"), &data).unwrap();

    let plugin = UnrealPlugin::new();
    assert!(plugin.detect(&dir));
    let entries = plugin.extract(&dir).unwrap();
    assert!(!entries.is_empty());
    for e in &entries {
        assert_eq!(
            e.metadata.get("binary_slot").and_then(|v| v.as_str()),
            Some("utf16le"),
            "unreal entry {} missing binary_slot",
            e.id
        );
    }

    let mut long = entries[0].clone();
    long.translation = Some("This translation is deliberately much longer than source".into());
    assert!(count_binary_slot_oversize(std::slice::from_ref(&long)) >= 1);
}
