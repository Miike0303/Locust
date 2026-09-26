use locust_core::{extraction::FormatPlugin, models::StringEntry};
use locust_formats::{
    unreal::UnrealPlugin,
    unreal_locres::{
        LocresFile, LocresNamespace, LocresString, LocresVersion, LOCRES_TUPLE_ID_PREFIX,
    },
};
use std::{collections::HashSet, fs, path::Path};

#[path = "support/locres_identity_containers.rs"]
mod containers;

fn source(rows: &[(&str, &str, &str, u32)]) -> LocresFile {
    LocresFile {
        version: LocresVersion::Compact,
        namespaces: rows
            .iter()
            .map(|(ns, key, value, hash)| LocresNamespace {
                name: (*ns).into(),
                name_hash: 0,
                strings: vec![LocresString {
                    key: (*key).into(),
                    value: (*value).into(),
                    source_string_hash: *hash,
                    key_hash: 0,
                }],
            })
            .collect(),
    }
}
fn loose(root: &Path, rows: &[(&str, &str, &str, u32)]) -> std::path::PathBuf {
    let path = root.join("Game.locres");
    fs::write(&path, source(rows).serialize().unwrap()).unwrap();
    path
}

#[test]
fn loose_slash_collisions_have_distinct_ids_and_translate_each_tuple() {
    let dir = tempfile::tempdir().unwrap();
    let path = loose(
        dir.path(),
        &[
            ("a", "b/c", "one", 11),
            ("a/b", "c", "two", 22),
            ("plain", "key", "three", 33),
        ],
    );
    let plugin = UnrealPlugin::new();
    let mut entries = plugin.extract(&path).unwrap();
    assert_eq!(
        entries.iter().map(|e| &e.id).collect::<HashSet<_>>().len(),
        3
    );
    assert_eq!(
        entries
            .iter()
            .filter(|e| e.id.starts_with(LOCRES_TUPLE_ID_PREFIX))
            .count(),
        2
    );
    assert!(entries.iter().any(|e| e.id == "plain/key"));
    for entry in &mut entries {
        entry.translation = Some(format!("translated {}", entry.source));
    }
    let report = plugin.inject(&path, &entries).unwrap();
    assert_eq!(report.strings_written, 3);
    assert_eq!(report.strings_skipped, 0);
    let read = LocresFile::parse_path(&path).unwrap();
    let rows: Vec<_> = read.iter_entries().collect();
    assert_eq!(
        rows,
        vec![
            ("a", "b/c", "translated one", 11),
            ("a/b", "c", "translated two", 22),
            ("plain", "key", "translated three", 33)
        ]
    );
}

#[test]
fn exact_duplicate_tuple_is_explicit_extract_error_and_counts_each_rejected_row() {
    let dir = tempfile::tempdir().unwrap();
    let path = loose(dir.path(), &[("a", "b", "one", 11), ("a", "b", "two", 22)]);
    let before = fs::read(&path).unwrap();
    let plugin = UnrealPlugin::new();
    assert!(plugin
        .extract(&path)
        .unwrap_err()
        .to_string()
        .contains("duplicate"));
    let entries: Vec<_> = [("one", 11), ("two", 22)]
        .into_iter()
        .map(|(value, hash)| {
            let mut e = StringEntry::new("a/b", value, path.clone());
            e.translation = Some("translated".into());
            e.metadata
                .insert("locres_namespace".into(), serde_json::json!("a"));
            e.metadata
                .insert("locres_key".into(), serde_json::json!("b"));
            e.metadata
                .insert("locres_source_hash".into(), serde_json::json!(hash));
            e
        })
        .collect();
    let report = plugin.inject(&path, &entries).unwrap();
    assert_eq!(report.strings_written, 0);
    assert_eq!(report.strings_skipped, 2);
    assert_eq!(fs::read(path).unwrap(), before);
}

#[test]
fn reserved_prefix_and_json_characters_cannot_alias_raw_keys() {
    let dir = tempfile::tempdir().unwrap();
    let escaped = locust_formats::unreal_locres::encoded_identity("a", "b/c");
    let key = format!("{LOCRES_TUPLE_ID_PREFIX}arbitrary/\"quote\\newline\n");
    let path = loose(
        dir.path(),
        &[
            ("a", "b/c", "same", 11),
            ("a/b", "c", "same", 11),
            ("", &escaped, "same", 11),
            ("", &key, "same", 11),
        ],
    );
    let plugin = UnrealPlugin::new();
    let mut entries = plugin.extract(&path).unwrap();
    assert_eq!(
        entries.iter().map(|e| &e.id).collect::<HashSet<_>>().len(),
        4
    );
    assert!(entries
        .iter()
        .all(|e| e.id.starts_with(LOCRES_TUPLE_ID_PREFIX)));
    for (i, entry) in entries.iter_mut().enumerate() {
        entry.translation = Some(format!("translated {i}"));
    }
    assert_eq!(plugin.inject(&path, &entries).unwrap().strings_written, 4);
    let reread = plugin.extract(&path).unwrap();
    assert_eq!(
        reread.iter().map(|e| &e.id).collect::<Vec<_>>(),
        entries.iter().map(|e| &e.id).collect::<Vec<_>>()
    );
    for (i, entry) in reread.iter().enumerate() {
        assert_eq!(entry.source, format!("translated {i}"));
    }
}

#[test]
fn legacy_metadata_matches_the_tuple_without_requiring_id_migration() {
    let dir = tempfile::tempdir().unwrap();
    let path = loose(
        dir.path(),
        &[("a", "b/c", "one", 11), ("a/b", "c", "two", 22)],
    );
    let plugin = UnrealPlugin::new();
    let mut entries = plugin.extract(&path).unwrap();
    for entry in &mut entries {
        entry.id = "a/b/c".into();
        entry.translation = Some(format!("translated {}", entry.source));
    }
    let report = plugin.inject(&path, &entries).unwrap();
    assert_eq!(report.strings_written, 2);
    assert_eq!(report.strings_skipped, 0);
    assert_eq!(
        plugin
            .extract(&path)
            .unwrap()
            .iter()
            .map(|e| e.source.as_str())
            .collect::<Vec<_>>(),
        vec!["translated one", "translated two"]
    );
}

#[test]
fn metadata_free_legacy_or_escaped_ids_never_pivot_between_physical_tuples() {
    let dir = tempfile::tempdir().unwrap();
    let encoded = locust_formats::unreal_locres::encoded_identity("a", "b");
    let path = loose(
        dir.path(),
        &[
            ("a", "b/c", "same", 7),
            ("a/b", "c", "same", 7),
            ("a", "b", "same", 7),
            ("", &encoded, "same", 7),
            ("", "unique#key", "same", 7),
        ],
    );
    let plugin = UnrealPlugin::new();
    let before = fs::read(&path).unwrap();
    for id in ["a/b/c", encoded.as_str()] {
        let mut e = StringEntry::new(id, "same", path.clone());
        e.translation = Some("wrong target".into());
        let report = plugin.inject(&path, &[e]).unwrap();
        assert_eq!(report.strings_written, 0);
        assert_eq!(report.strings_skipped, 1);
        assert_eq!(fs::read(&path).unwrap(), before);
    }
    let mut e = StringEntry::new("unique#key", "same", path.clone());
    e.translation = Some("right target".into());
    assert_eq!(plugin.inject(&path, &[e]).unwrap().strings_written, 1);
    let tuple_id = locust_formats::unreal_locres::encoded_identity("a", "b/c");
    let mut e = StringEntry::new(tuple_id, "same", path.clone());
    e.translation = Some("structured target".into());
    assert_eq!(plugin.inject(&path, &[e]).unwrap().strings_written, 1);
    let actual = LocresFile::parse_path(&path).unwrap();
    let rows: Vec<_> = actual.iter_entries().collect();
    assert_eq!(rows[0].2, "structured target");
    assert_eq!(rows[1].2, "same");
    assert_eq!(rows[3].2, "same");
}

#[test]
fn changed_physical_source_or_hash_rejects_only_its_exact_tuple() {
    for hash_only in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let path = loose(
            dir.path(),
            &[("a", "b/c", "same", 7), ("a/b", "c", "same", 7)],
        );
        let plugin = UnrealPlugin::new();
        let mut entries = plugin.extract(&path).unwrap();
        for entry in &mut entries {
            entry.id = "a/b/c".into();
            entry.translation = Some("translated".into());
        }
        fs::write(
            &path,
            source(&[
                (
                    "a",
                    "b/c",
                    if hash_only { "same" } else { "changed" },
                    if hash_only { 8 } else { 7 },
                ),
                ("a/b", "c", "same", 7),
            ])
            .serialize()
            .unwrap(),
        )
        .unwrap();
        let report = plugin.inject(&path, &entries).unwrap();
        assert_eq!(report.strings_written, 1);
        assert_eq!(report.strings_skipped, 1);
        let data = LocresFile::parse_path(&path).unwrap();
        let rows: Vec<_> = data.iter_entries().collect();
        assert_eq!(rows[0].2, if hash_only { "same" } else { "changed" });
        assert_eq!(rows[0].3, if hash_only { 8 } else { 7 });
        assert_eq!(rows[1].2, "translated");
    }
}

#[test]
fn repeated_requests_are_rejected_and_counted_without_last_row_wins() {
    let dir = tempfile::tempdir().unwrap();
    let path = loose(dir.path(), &[("a", "b", "one", 7)]);
    let before = fs::read(&path).unwrap();
    let plugin = UnrealPlugin::new();
    let baseline = plugin.extract(&path).unwrap();
    let entries: Vec<_> = (0..3)
        .map(|i| {
            let mut e = baseline[0].clone();
            e.translation = Some(format!("conflict {i}"));
            e
        })
        .collect();
    let report = plugin.inject(&path, &entries).unwrap();
    assert_eq!(report.strings_written, 0);
    assert_eq!(report.strings_skipped, 3);
    assert_eq!(fs::read(path).unwrap(), before);
}

#[test]
fn classic_modern_zlib_and_native_overlays_preserve_structured_identities() {
    for kind in ["classic", "modern", "native"] {
        let dir = tempfile::tempdir().unwrap();
        let raw = locust_formats::unreal_locres::encoded_identity("a", "b/c");
        let payload = source(&[
            ("a", "b/c", "one", 11),
            ("a/b", "c", "two", 22),
            ("", &raw, "three", 33),
            ("plain", "key", "four", 44),
        ])
        .serialize()
        .unwrap();
        let path = containers::container(dir.path(), kind, payload);
        let before = fs::read(&path).unwrap();
        let ucas = if kind == "native" {
            Some(fs::read(path.with_extension("ucas")).unwrap())
        } else {
            None
        };
        let plugin = UnrealPlugin::new();
        let mut entries = plugin.extract(&path).unwrap();
        assert_eq!(entries.len(), 4);
        assert_eq!(
            entries.iter().map(|e| &e.id).collect::<HashSet<_>>().len(),
            4
        );
        assert!(entries.iter().all(|e| e.id.contains("#")));
        assert!(entries.iter().any(|e| e.id.ends_with("#plain/key")));
        for e in &mut entries {
            e.translation = Some(format!("translated {}", e.source));
        }
        let report = plugin.inject(&path, &entries).unwrap();
        assert_eq!(report.strings_written, 4, "{kind}: {:?}", report.warnings);
        assert_eq!(report.strings_skipped, 0);
        let reread = plugin.extract(dir.path()).unwrap();
        assert_eq!(reread.len(), 4);
        for e in &reread {
            assert!(e.source.starts_with("translated "), "{kind}: {}", e.source);
            assert!(entries.iter().any(|old| old.id == e.id));
        }
        assert_eq!(fs::read(&path).unwrap(), before);
        if let Some(bytes) = ucas {
            assert_eq!(fs::read(path.with_extension("ucas")).unwrap(), bytes);
        }
    }
}

#[test]
fn native_incremental_overlay_keeps_prior_translation_of_the_other_slash_tuple() {
    let dir = tempfile::tempdir().unwrap();
    let path = containers::native(
        dir.path(),
        &source(&[("a", "b/c", "one", 11), ("a/b", "c", "two", 22)])
            .serialize()
            .unwrap(),
    );
    let plugin = UnrealPlugin::new();
    let baseline = plugin.extract(&path).unwrap();
    for old in &baseline {
        let mut entry = old.clone();
        entry.id = "old-flat-id".into();
        entry.translation = Some(format!("translated {}", entry.source));
        let report = plugin.inject(&path, &[entry]).unwrap();
        assert_eq!(report.strings_written, 1, "{:?}", report.warnings);
    }
    let reread = plugin.extract(&path).unwrap();
    assert_eq!(reread.len(), 2);
    assert!(reread.iter().all(|e| e.source.starts_with("translated ")));
}

#[test]
fn all_container_paths_reject_exact_duplicates_and_report_every_old_project_row() {
    for kind in ["classic", "modern", "native"] {
        let dir = tempfile::tempdir().unwrap();
        let plugin = UnrealPlugin::new();
        let path = containers::container(
            dir.path(),
            kind,
            source(&[("a", "b1", "one", 11), ("a", "b2", "two", 22)])
                .serialize()
                .unwrap(),
        );
        let mut entries = plugin.extract(&path).unwrap();
        containers::container(
            dir.path(),
            kind,
            source(&[("a", "b", "one", 11), ("a", "b", "two", 22)])
                .serialize()
                .unwrap(),
        );
        let before = fs::read(&path).unwrap();
        assert!(plugin
            .extract(&path)
            .unwrap_err()
            .to_string()
            .contains("duplicate"));
        for e in &mut entries {
            e.id = "a/b".into();
            e.metadata
                .insert("locres_key".into(), serde_json::json!("b"));
            e.translation = Some("must not write".into());
        }
        let report = plugin.inject(&path, &entries).unwrap();
        assert_eq!(report.strings_written, 0, "{kind}");
        assert_eq!(report.strings_skipped, 2, "{kind}: {:?}", report.warnings);
        assert!(report.files_written.is_empty());
        assert_eq!(fs::read(&path).unwrap(), before);
        assert!(!dir.path().join("game_LOCUST_P.pak").exists());
    }
}

#[test]
fn native_prior_overlay_revalidates_source_hash_per_tuple() {
    let dir = tempfile::tempdir().unwrap();
    let initial = source(&[("a", "b/c", "one", 11), ("a/b", "c", "two", 22)])
        .serialize()
        .unwrap();
    let path = containers::native(dir.path(), &initial);
    let plugin = UnrealPlugin::new();
    let baseline = plugin.extract(&path).unwrap();
    let mut first = baseline.iter().find(|e| e.source == "one").unwrap().clone();
    first.translation = Some("old A translation".into());
    assert_eq!(plugin.inject(&path, &[first]).unwrap().strings_written, 1);
    containers::native(
        dir.path(),
        &source(&[("a", "b/c", "updated A", 99), ("a/b", "c", "two", 22)])
            .serialize()
            .unwrap(),
    );
    let mut second = baseline.iter().find(|e| e.source == "two").unwrap().clone();
    second.translation = Some("new B translation".into());
    assert_eq!(plugin.inject(&path, &[second]).unwrap().strings_written, 1);
    let reread = plugin.extract(&path).unwrap();
    assert!(reread.iter().any(|e| e.source == "updated A"));
    assert!(reread.iter().any(|e| e.source == "new B translation"));
    assert!(!reread.iter().any(|e| e.source == "old A translation"));
}

#[test]
fn encoded_tuple_ids_retain_distinct_resource_and_culture_prefixes() {
    use locust_formats::unreal_pak::{write_pak, PakWriteFile, DEFAULT_MOUNT_POINT};
    let dir = tempfile::tempdir().unwrap();
    let payload = source(&[("a", "b/c", "one", 11), ("a/b", "c", "two", 22)])
        .serialize()
        .unwrap();
    let files: Vec<_> = ["en", "fr"]
        .into_iter()
        .map(|culture| PakWriteFile {
            name: format!("Game/Content/Localization/Game/{culture}/Game.locres"),
            data: payload.clone(),
        })
        .collect();
    let path = dir.path().join("game.pak");
    fs::write(
        &path,
        write_pak(DEFAULT_MOUNT_POINT, 8, &files, "fixture").unwrap(),
    )
    .unwrap();
    let plugin = UnrealPlugin::new();
    let mut entries = plugin.extract(&path).unwrap();
    assert_eq!(
        entries.iter().map(|e| &e.id).collect::<HashSet<_>>().len(),
        4
    );
    for e in &mut entries {
        let culture = e.metadata["locres_culture"].as_str().unwrap();
        assert!(e.id.contains(&format!("/{culture}/Game.locres#")));
        e.translation = Some(format!("{culture}:{}", e.source));
    }
    assert_eq!(plugin.inject(&path, &entries).unwrap().strings_written, 4);
    for e in plugin.extract(dir.path()).unwrap() {
        assert!(e
            .source
            .starts_with(e.metadata["locres_culture"].as_str().unwrap()));
    }
}

#[test]
fn newer_pak_can_supersede_unreadable_duplicate_native_resource() {
    use locust_formats::unreal_pak::{write_pak, PakWriteFile, DEFAULT_MOUNT_POINT};
    for kind in ["classic", "modern", "native"] {
        let dir = tempfile::tempdir().unwrap();
        containers::container(
            dir.path(),
            kind,
            source(&[("a", "b", "one", 11), ("a", "b", "two", 22)])
                .serialize()
                .unwrap(),
        );
        let file = PakWriteFile {
            name: "Game/Content/Localization/Game/en/Game.locres".into(),
            data: source(&[("a", "b", "current", 33)]).serialize().unwrap(),
        };
        fs::write(
            dir.path().join("game_3_P.pak"),
            write_pak(DEFAULT_MOUNT_POINT, 8, &[file], "fixture").unwrap(),
        )
        .unwrap();
        let entries = UnrealPlugin::new().extract(dir.path()).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].source, "current");
    }
}

#[test]
fn unique_legacy_slash_ids_remain_unchanged_and_malformed_metadata_is_rejected() {
    for (ns, key) in [("a/b", "c"), ("a", "b/c"), ("", "a/b/c")] {
        let file = source(&[(ns, key, "one", 11)]);
        assert_eq!(file.extraction_ids("fixture").unwrap(), vec!["a/b/c"]);
    }
    let dir = tempfile::tempdir().unwrap();
    let path = loose(dir.path(), &[("a", "b", "one", 11)]);
    let plugin = UnrealPlugin::new();
    let before = fs::read(&path).unwrap();
    for mutation in 0..3 {
        let mut e = plugin.extract(&path).unwrap().remove(0);
        e.translation = Some("should reject".into());
        match mutation {
            0 => {
                e.metadata
                    .insert("locres_namespace".into(), serde_json::json!(3));
            }
            1 => {
                e.metadata.insert("locres_key".into(), serde_json::json!(3));
            }
            _ => {
                e.metadata.remove("locres_key");
            }
        }
        let report = plugin.inject(&path, &[e]).unwrap();
        assert_eq!(report.strings_written, 0);
        assert_eq!(report.strings_skipped, 1);
        assert_eq!(fs::read(&path).unwrap(), before);
    }
}
