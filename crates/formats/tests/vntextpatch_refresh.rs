use std::fs;

use locust_core::{
    database::Database,
    error::LocustError,
    extraction::{FormatPlugin, FormatRegistry},
    models::StringStatus,
    project::{open_project, saved_extraction_warnings},
};
use locust_formats::vntextpatch::VnTextPatchPlugin;

#[test]
fn vntextpatch_incomplete_refresh_preserves_translations_until_repaired() {
    let dir = tempfile::tempdir().unwrap();
    let game = dir.path().join("game");
    fs::create_dir(&game).unwrap();
    let a = game.join("a.json");
    let b = game.join("b.json");
    fs::write(
        &a,
        r#"[{"message":"A kept"},{"message":"A removed later"}]"#,
    )
    .unwrap();
    fs::write(&b, r#"[{"message":"B original"}]"#).unwrap();
    let db = Database::open_in_memory().unwrap();
    let mut registry = FormatRegistry::new();
    registry.register(Box::new(VnTextPatchPlugin::new()));
    let first = open_project(&db, &registry, &game, Some("vntextpatch")).unwrap();
    assert_eq!(first.total_strings, 3);
    assert!(first.extraction_warnings.is_empty());

    let mut saved = Vec::new();
    for (id, translation) in [
        ("a.json#0#message", "A guardada"),
        ("a.json#1#message", "A para eliminar después"),
        ("b.json#0#message", "B guardada"),
    ] {
        let mut entry = db.get_entry(id).unwrap().unwrap();
        entry.translation = Some(translation.into());
        entry.status = StringStatus::Approved;
        entry.provider_used = Some("manual".into());
        saved.push(entry);
    }
    db.save_entries(&saved).unwrap();

    fs::write(&a, r#"[{"message":"half-written""#).unwrap();
    fs::write(&b, r#"[{"message":"B updated"},{"message":"B added"}]"#).unwrap();
    let partial = open_project(&db, &registry, &game, Some("vntextpatch")).unwrap();
    assert_eq!(partial.total_strings, 4);
    assert_eq!(partial.removed, 0);
    assert_eq!(partial.added, 1);
    assert_eq!(partial.stale_source_reset, 1);
    for entry in &saved[..2] {
        assert_eq!(
            serde_json::to_value(db.get_entry(&entry.id).unwrap().unwrap()).unwrap(),
            serde_json::to_value(entry).unwrap()
        );
    }
    let updated = db.get_entry("b.json#0#message").unwrap().unwrap();
    assert_eq!(updated.source, "B updated");
    assert_eq!(updated.translation.as_deref(), Some("B guardada"));
    assert_eq!(updated.status, StringStatus::Pending);
    assert_eq!(
        db.get_entry("b.json#1#message").unwrap().unwrap().source,
        "B added"
    );
    assert_eq!(partial.extraction_warnings.len(), 1);
    let warning = &partial.extraction_warnings[0];
    assert!(warning.contains("a.json"), "{warning}");
    assert!(warning.contains("EOF"), "{warning}");
    assert_eq!(
        saved_extraction_warnings(&db).unwrap(),
        partial.extraction_warnings
    );

    // Once readable again, the script really has one fewer row.
    fs::write(&a, r#"[{"message":"A kept"}]"#).unwrap();
    let complete = open_project(&db, &registry, &game, Some("vntextpatch")).unwrap();
    assert_eq!(complete.total_strings, 3);
    assert_eq!(complete.removed, 1);
    assert!(db.get_entry("a.json#1#message").unwrap().is_none());
    assert_eq!(
        serde_json::to_value(db.get_entry(&saved[0].id).unwrap().unwrap()).unwrap(),
        serde_json::to_value(&saved[0]).unwrap()
    );
    assert!(complete.extraction_warnings.is_empty());
    assert!(saved_extraction_warnings(&db).unwrap().is_empty());
}

#[test]
fn vntextpatch_valid_non_script_json_is_complete_without_warnings() {
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("script.json");
    fs::write(
        &script,
        "\u{feff}[{\"name\":\"Speaker\",\"message\":\"Hello\"}]",
    )
    .unwrap();
    fs::write(dir.path().join("config.json"), r#"{"x":1}"#).unwrap();
    fs::write(
        dir.path().join("data.json"),
        r#"[{"name":"Not a script"},{"message":42}]"#,
    )
    .unwrap();
    let plugin = VnTextPatchPlugin::new();
    let extraction = plugin.extract_with_diagnostics(dir.path()).unwrap();
    assert!(extraction.complete);
    assert!(extraction.warnings.is_empty());
    assert_eq!(extraction.entries.len(), 2);
    assert_eq!(extraction.entries[0].id, "script.json#0#name");
    assert_eq!(extraction.entries[1].context.as_deref(), Some("Speaker"));
    assert_eq!(plugin.extract(dir.path()).unwrap().len(), 2);

    fs::remove_file(script).unwrap();
    let extraction = plugin.extract_with_diagnostics(dir.path()).unwrap();
    assert!(extraction.complete);
    assert!(extraction.entries.is_empty());
    assert!(extraction.warnings.is_empty());
}

#[test]
fn vntextpatch_unreadable_directory_candidates_are_incomplete_even_without_rows() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.json");
    let plugin = VnTextPatchPlugin::new();
    for (bytes, reason) in [(b"[".as_slice(), "EOF"), (&[0xff], "UTF-8")] {
        fs::write(&path, bytes).unwrap();
        let extraction = plugin.extract_with_diagnostics(dir.path()).unwrap();
        assert!(!extraction.complete, "{reason}");
        assert!(extraction.entries.is_empty());
        assert_eq!(extraction.warnings.len(), 1);
        let warning = &extraction.warnings[0];
        assert!(warning.contains("a.json"), "{warning}");
        assert!(warning.contains(reason), "{warning}");
        assert!(plugin.extract(dir.path()).unwrap().is_empty());
    }
}

#[test]
fn vntextpatch_unparseable_single_file_returns_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.json");
    fs::write(&path, r#"[{"message":"unfinished""#).unwrap();
    let plugin = VnTextPatchPlugin::new();
    let error = plugin.extract_with_diagnostics(&path).unwrap_err();
    match error {
        LocustError::ParseError { file, message } => {
            assert_eq!(file, path.display().to_string());
            assert!(message.contains("EOF"), "{message}");
        }
        error => panic!("expected a JSON parse error, got {error}"),
    }
    assert!(plugin.extract(&path).is_err());

    fs::write(&path, [0xff]).unwrap();
    assert!(matches!(
        plugin.extract_with_diagnostics(&path),
        Err(LocustError::IoError(_))
    ));
    assert!(plugin.extract(&path).is_err());
}
