use locust_core::extraction::FormatPlugin;
use locust_formats::{unreal::UnrealPlugin, unreal_locres::*, unreal_pak::*};
use std::{fs, path::Path};

#[allow(dead_code)]
#[path = "support/locres_identity_containers.rs"]
mod containers;

fn locres() -> Vec<u8> {
    let loc = LocresFile {
        version: LocresVersion::Compact,
        namespaces: vec![LocresNamespace {
            name: "Menu".into(),
            name_hash: 0,
            strings: vec![LocresString {
                key: "Hello".into(),
                value: "Hello".into(),
                source_string_hash: 7,
                key_hash: 0,
            }],
        }],
    };
    loc.serialize().unwrap()
}
fn fixture(path: &Path) {
    let bytes = write_pak(
        DEFAULT_MOUNT_POINT,
        8,
        &[
            PakWriteFile {
                name: "Game/Content/Localization/Game/en/Game.locres".into(),
                data: locres(),
            },
            PakWriteFile {
                name: "Game/Content/Other.bin".into(),
                data: b"unselected original asset".to_vec(),
            },
        ],
        "fixture",
    )
    .unwrap();
    fs::write(path, bytes).unwrap();
}

#[test]
fn unrelated_old_sidecar_survives_repeated_overlay_replacement() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("Game.pak");
    fixture(&source);
    let original = fs::read(&source).unwrap();
    let plugin = UnrealPlugin::new();
    let mut entries = plugin.extract(&source).unwrap();
    entries[0].translation = Some("First".into());
    let report = plugin.inject(root.path(), &entries).unwrap();
    assert_eq!(report.strings_written, 1);
    let output = &report.files_written[0];
    let previous = fs::read(output).unwrap();
    let sidecar = output.with_extension("pak.locust-old");
    fs::write(&sidecar, b"unrelated exact bytes").unwrap();
    entries[0].translation = Some("Second".into());
    let report = plugin.inject(root.path(), &entries).unwrap();
    assert_eq!(report.strings_written, 1);
    let backup = report
        .warnings
        .iter()
        .find_map(|w| w.strip_prefix("previous overlay retained at "))
        .unwrap();
    assert_eq!(fs::read(backup).unwrap(), previous);
    assert!(Path::new(backup).extension().is_none());
    assert_eq!(fs::read(&sidecar).unwrap(), b"unrelated exact bytes");
    assert_eq!(fs::read(source).unwrap(), original);
}

#[test]
fn reserved_suffix_source_pak_is_never_its_own_output() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("Game_LOCUST_P.pak");
    fixture(&source);
    let original = fs::read(&source).unwrap();
    let plugin = UnrealPlugin::new();
    let mut entries = plugin.extract(&source).unwrap();
    entries[0].translation = Some("Translated".into());
    let report = plugin.inject(root.path(), &entries).unwrap();
    assert_eq!(report.strings_written, 1);
    assert_eq!(fs::read(&source).unwrap(), original);
    assert_ne!(report.files_written[0], source);
}

#[test]
fn game_root_lock_blocks_nested_native_and_classic_before_writes() {
    let root = tempfile::tempdir().unwrap();
    let paks = root.path().join("Game/Content/Paks");
    fs::create_dir_all(&paks).unwrap();
    let source = paks.join("Game.pak");
    fixture(&source);
    let plugin = UnrealPlugin::new();
    let mut entries = plugin.extract(&source).unwrap();
    entries[0].translation = Some("Translated".into());
    let lock = locust_core::patch::GameLock::acquire(root.path()).unwrap();
    assert!(plugin
        .inject(root.path(), &entries)
        .unwrap_err()
        .to_string()
        .contains("game busy"));
    assert!(!patch_pak_path(&source).exists());
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "overlay_lock_child", "--ignored", "--nocapture"])
        .env("LOCUST_OVERLAY_TEST_ROOT", root.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    drop(lock);
    assert_eq!(
        plugin
            .inject(root.path(), &entries)
            .unwrap()
            .strings_written,
        1
    );
}

#[test]
#[ignore = "child process invoked by game_root_lock_blocks_nested_native_and_classic_before_writes"]
fn overlay_lock_child() {
    let root = std::path::PathBuf::from(std::env::var_os("LOCUST_OVERLAY_TEST_ROOT").unwrap());
    let source = root.join("Game/Content/Paks/Game.pak");
    let plugin = UnrealPlugin::new();
    let mut entries = plugin.extract(&source).unwrap();
    entries[0].translation = Some("Competing".into());
    assert!(plugin
        .inject(&root, &entries)
        .unwrap_err()
        .to_string()
        .contains("game busy"));
    assert!(!patch_pak_path(&source).exists());
}

#[test]
fn outside_game_entry_rejects_entire_injection_before_writes() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let source = root.path().join("Game.pak");
    let other = outside.path().join("Other.pak");
    fixture(&source);
    fixture(&other);
    let plugin = UnrealPlugin::new();
    let mut entries = plugin.extract(&source).unwrap();
    entries.extend(plugin.extract(&other).unwrap());
    for e in &mut entries {
        e.translation = Some("Translated".into());
    }
    assert!(plugin.inject(root.path(), &entries).is_err());
    assert!(!patch_pak_path(&source).exists());
    assert!(!patch_pak_path(&other).exists());
}

#[test]
fn native_overlay_replacement_preserves_unique_backups_and_container_bytes() {
    let root = tempfile::tempdir().unwrap();
    let toc = containers::native(root.path(), &locres());
    let toc_bytes = fs::read(&toc).unwrap();
    let ucas_bytes = fs::read(toc.with_extension("ucas")).unwrap();
    let plugin = UnrealPlugin::new();
    let mut entries = plugin.extract(&toc).unwrap();
    entries[0].translation = Some("First".into());
    let lock = locust_core::patch::GameLock::acquire(root.path()).unwrap();
    assert!(plugin
        .inject(root.path(), &entries)
        .unwrap_err()
        .to_string()
        .contains("game busy"));
    assert!(!patch_pak_path(&toc.with_extension("pak")).exists());
    drop(lock);
    let first = plugin.inject(root.path(), &entries).unwrap();
    assert_eq!(first.strings_written, 1);
    let previous = fs::read(&first.files_written[0]).unwrap();
    let sidecar = first.files_written[0].with_extension("pak.locust-old");
    fs::create_dir(&sidecar).unwrap();
    fs::write(sidecar.join("sentinel"), b"recursive unrelated").unwrap();
    entries[0].translation = Some("Second".into());
    let second = plugin.inject(root.path(), &entries).unwrap();
    assert_eq!(second.strings_written, 1);
    let backup = second
        .warnings
        .iter()
        .find_map(|w| w.strip_prefix("previous overlay retained at "))
        .unwrap();
    assert_eq!(fs::read(backup).unwrap(), previous);
    assert_eq!(
        fs::read(sidecar.join("sentinel")).unwrap(),
        b"recursive unrelated"
    );
    assert_eq!(fs::read(toc.with_extension("ucas")).unwrap(), ucas_bytes);
    assert_eq!(fs::read(toc).unwrap(), toc_bytes);
}

#[test]
fn compressed_modern_overlay_replacement_preserves_each_previous_generation() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("Modern.pak");
    let original = containers::modern(locres());
    fs::write(&source, &original).unwrap();
    let plugin = UnrealPlugin::new();
    let mut entries = plugin.extract(&source).unwrap();
    let mut previous = None;
    let mut backups = std::collections::HashSet::new();
    for text in ["First", "Second", "Third"] {
        entries[0].translation = Some(text.into());
        let report = plugin.inject(root.path(), &entries).unwrap();
        assert_eq!(report.strings_written, 1);
        if let Some(bytes) = previous {
            let path = report
                .warnings
                .iter()
                .find_map(|w| w.strip_prefix("previous overlay retained at "))
                .unwrap();
            assert_eq!(fs::read(path).unwrap(), bytes);
            assert!(backups.insert(path.to_owned()));
        }
        previous = Some(fs::read(&report.files_written[0]).unwrap());
    }
    assert_eq!(backups.len(), 2);
    assert_eq!(fs::read(source).unwrap(), original);
    // Extensionless backups do not reappear as active localization resources.
    let active = plugin.extract(root.path()).unwrap();
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].source, "Third");
}
