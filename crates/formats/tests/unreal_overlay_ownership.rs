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

fn reset_fixture(path: &Path, separate_resources: bool) {
    let mut first = LocresFile::parse(&locres(), "fixture").unwrap();
    let second = LocresString {
        key: "Goodbye".into(),
        value: "Goodbye".into(),
        source_string_hash: 8,
        key_hash: 0,
    };
    let mut files = Vec::new();
    if separate_resources {
        let mut other = first.clone();
        other.namespaces[0].strings = vec![second];
        files.push(PakWriteFile {
            name: "Game/Content/Localization/Other/en/Other.locres".into(),
            data: other.serialize().unwrap(),
        });
    } else {
        first.namespaces[0].strings.push(second);
    }
    files.push(PakWriteFile {
        name: "Game/Content/Localization/Game/en/Game.locres".into(),
        data: first.serialize().unwrap(),
    });
    fs::write(
        path,
        write_pak(DEFAULT_MOUNT_POINT, 8, &files, "fixture").unwrap(),
    )
    .unwrap();
}

fn active_values(plugin: &UnrealPlugin, root: &Path) -> Vec<String> {
    let mut values: Vec<_> = plugin
        .extract(root)
        .unwrap()
        .into_iter()
        .map(|entry| entry.source)
        .collect();
    values.sort();
    values
}

fn assert_all_reset(kind: &str) {
    let root = tempfile::tempdir().unwrap();
    let source = containers::container(root.path(), kind, locres());
    let original = fs::read(&source).unwrap();
    let plugin = UnrealPlugin::new();
    let mut entries = plugin.extract(&source).unwrap();
    entries[0].translation = Some("Translated".into());
    let first = plugin.inject(root.path(), &entries).unwrap();
    assert_eq!(first.strings_written, 1);
    let output = &first.files_written[0];
    let previous = fs::read(output).unwrap();
    assert_eq!(active_values(&plugin, root.path()), ["Translated"]);

    entries[0].translation = Some(entries[0].source.clone());
    let reset = plugin.inject(root.path(), &entries).unwrap();
    assert_eq!(reset.strings_written, 0);
    assert_eq!(reset.strings_skipped, 1);
    assert_eq!(reset.skip_reasons.get("unchanged"), Some(&1));
    // Re-extract the game with its active overlays, rather than only the base PAK.
    assert_eq!(active_values(&plugin, root.path()), ["Hello"]);
    let backup = reset
        .warnings
        .iter()
        .find_map(|w| w.strip_prefix("previous overlay retained at "))
        .unwrap();
    assert_eq!(fs::read(backup).unwrap(), previous);
    assert!(Path::new(backup).extension().is_none());
    assert!(reset.files_written.is_empty());
    assert!(!output.exists());
    assert_eq!(fs::read(source).unwrap(), original);
}

#[test]
fn all_reset_retires_classic_overlay() {
    assert_all_reset("classic");
}

#[test]
fn all_reset_retires_modern_overlay() {
    assert_all_reset("modern");
}

fn assert_partial_reset(separate_resources: bool) {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("Game.pak");
    reset_fixture(&source, separate_resources);
    let original = fs::read(&source).unwrap();
    let plugin = UnrealPlugin::new();
    let mut entries = plugin.extract(&source).unwrap();
    assert_eq!(entries.len(), 2);
    for entry in &mut entries {
        entry.translation = Some(format!("Translated {}", entry.source));
    }
    assert_eq!(
        plugin
            .inject(root.path(), &entries)
            .unwrap()
            .strings_written,
        2
    );
    assert_eq!(
        active_values(&plugin, root.path()),
        ["Translated Goodbye", "Translated Hello"]
    );
    let reset = entries.iter_mut().find(|e| e.source == "Hello").unwrap();
    reset.translation = Some(reset.source.clone());
    let report = plugin.inject(root.path(), &entries).unwrap();
    assert_eq!(report.strings_written, 1);
    assert_eq!(report.strings_skipped, 1);
    assert_eq!(report.skip_reasons.get("unchanged"), Some(&1));
    assert_eq!(
        active_values(&plugin, root.path()),
        ["Hello", "Translated Goodbye"]
    );
    assert_eq!(fs::read(source).unwrap(), original);
}

#[test]
fn partial_reset_in_same_resource_keeps_other_translation() {
    assert_partial_reset(false);
}

#[test]
fn reset_resource_keeps_other_resource_translated() {
    assert_partial_reset(true);
}

#[test]
fn source_equal_translations_without_overlay_create_nothing() {
    for kind in ["classic", "modern"] {
        let root = tempfile::tempdir().unwrap();
        let source = containers::container(root.path(), kind, locres());
        let original = fs::read(&source).unwrap();
        let plugin = UnrealPlugin::new();
        let mut entries = plugin.extract(&source).unwrap();
        for entry in &mut entries {
            entry.translation = Some(entry.source.clone());
        }
        let report = plugin.inject(root.path(), &entries).unwrap();
        assert_eq!(report.strings_written, 0);
        assert_eq!(report.strings_skipped, 1);
        assert_eq!(report.skip_reasons.get("unchanged"), Some(&1));
        assert!(report.files_written.is_empty());
        assert!(report.warnings.is_empty());
        assert!(!patch_pak_path(&source).exists());
        assert_eq!(active_values(&plugin, root.path()), ["Hello"]);
        assert_eq!(fs::read(source).unwrap(), original);
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
    }
}

#[test]
fn empty_output_without_reset_preserves_existing_overlay() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("Game.pak");
    fixture(&source);
    let plugin = UnrealPlugin::new();
    let mut entries = plugin.extract(&source).unwrap();
    entries[0].translation = Some("Translated".into());
    let first = plugin.inject(root.path(), &entries).unwrap();
    let output = &first.files_written[0];
    let previous = fs::read(output).unwrap();
    for translation in [None, Some("Rejected".into())] {
        entries[0].translation = translation;
        entries[0]
            .metadata
            .insert("locres_source_hash".into(), serde_json::json!(999));
        let report = plugin.inject(root.path(), &entries).unwrap();
        assert_eq!(report.strings_written, 0);
        assert!(report.files_written.is_empty());
        assert_eq!(fs::read(output).unwrap(), previous);
        assert_eq!(active_values(&plugin, root.path()), ["Translated"]);
    }
}
