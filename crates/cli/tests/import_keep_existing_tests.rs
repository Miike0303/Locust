use locust_core::database::Database;
use locust_core::export;
use locust_core::models::{StringEntry, StringStatus, STALE_TRANSLATION_METADATA_KEY};
use std::path::Path;

mod common;
use common::locust;

fn entry(id: &str, source: &str, translation: Option<&str>) -> StringEntry {
    let mut entry = StringEntry::new(id, source, "story.json".into());
    entry.translation = translation.map(str::to_owned);
    entry
}

fn approved(id: &str, source: &str, translation: &str) -> StringEntry {
    let mut entry = entry(id, source, Some(translation));
    entry.status = StringStatus::Approved;
    entry.provider_used = Some("local-provider".into());
    entry.translated_at = Some("2025-01-01T00:00:00Z".parse().unwrap());
    entry.reviewed_at = Some("2025-01-02T00:00:00Z".parse().unwrap());
    entry.metadata.insert(
        STALE_TRANSLATION_METADATA_KEY.into(),
        serde_json::json!("preserve on no-op"),
    );
    entry
}

fn import(project: &Path, input: &Path, format: &str, keep_existing: bool) -> String {
    let mut command = locust();
    command
        .arg("import")
        .arg(project)
        .args(["--format", format, "--lang", "es", "--input"])
        .arg(input);
    if keep_existing {
        command.arg("--keep-existing");
    }
    let assertion = command.assert().success();
    String::from_utf8(assertion.get_output().stdout.clone()).unwrap()
}

fn write_catalog(input: &Path, format: &str, entries: &[StringEntry]) {
    let content = match format {
        "po" => export::export_po(entries, "en", "es"),
        "xliff" => export::export_xliff(entries, "en", "es"),
        _ => unreachable!(),
    };
    std::fs::write(input, content).unwrap();
}

#[test]
fn cli_import_keep_existing_preserves_approved_and_fills_empty() {
    for format in ["po", "xliff"] {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("project.db");
        let input = dir.path().join(format!("catalog.{format}"));
        let db = Database::open(&project).unwrap();
        let kept = approved("kept", "Hello", "Local correction");
        let identical = approved("same", "Welcome", "Bienvenido");
        let stale = approved("stale", "Current source", "Current translation");
        let missing_target = entry("missing-target", "Empty target", None);
        db.save_entries(&[
            kept.clone(),
            identical.clone(),
            entry("empty", "Goodbye", None),
            entry("blank", "Thanks", Some(" \t\r\n\u{2003}")),
            stale.clone(),
            missing_target.clone(),
        ])
        .unwrap();
        drop(db);
        write_catalog(
            &input,
            format,
            &[
                entry("kept", "Hello", Some("Collaborator correction")),
                identical.clone(),
                entry("empty", "Goodbye", Some("Adiós")),
                entry("blank", "Thanks", Some("Gracias")),
                entry("stale", "Old source", Some("Current translation")),
                entry("unknown", "Unknown", Some("Missing")),
                missing_target.clone(),
            ],
        );

        let output = import(&project, &input, format, true);

        assert!(output.contains("Imported 3 translations from "), "{output}");
        assert!(
            output.contains("Unchanged 1 (already identical)"),
            "{output}"
        );
        assert!(
            output.contains("Kept 1 existing translation(s) (--keep-existing)"),
            "{output}"
        );
        assert!(
            output.contains(
                "Skipped 3: 1 outdated source(s), 1 unknown id(s), 1 empty/missing entries"
            ),
            "{output}"
        );
        let db = Database::open(&project).unwrap();
        for before in [kept, identical, stale, missing_target] {
            assert_eq!(
                serde_json::to_value(db.get_entry(&before.id).unwrap().unwrap()).unwrap(),
                serde_json::to_value(before).unwrap(),
                "{format}"
            );
        }
        for (id, translation) in [("empty", "Adiós"), ("blank", "Gracias")] {
            let filled = db.get_entry(id).unwrap().unwrap();
            assert_eq!(filled.translation.as_deref(), Some(translation));
            assert_eq!(filled.status, StringStatus::Translated);
            assert_eq!(filled.provider_used.as_deref(), Some("import"));
            assert!(filled.translated_at.is_some());
        }
        assert!(db.get_entry("unknown").unwrap().is_none());
    }
}

#[test]
fn cli_import_default_preserves_identical_and_overwrites_differing() {
    for format in ["po", "xliff"] {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("project.db");
        let input = dir.path().join(format!("catalog.{format}"));
        let db = Database::open(&project).unwrap();
        let same = approved("same", "Hello", "Hola");
        let differing = approved("different", "Goodbye", "Local correction");
        db.save_entries(&[same.clone(), differing]).unwrap();
        drop(db);
        write_catalog(
            &input,
            format,
            &[same.clone(), entry("different", "Goodbye", Some("Adiós"))],
        );

        let output = import(&project, &input, format, false);

        assert!(output.contains("Imported 2 translations from "), "{output}");
        assert!(
            output.contains("Unchanged 1 (already identical)"),
            "{output}"
        );
        assert!(!output.contains("Skipped"), "{output}");
        assert!(!output.contains("Kept"), "{output}");
        let db = Database::open(&project).unwrap();
        assert_eq!(
            serde_json::to_value(db.get_entry("same").unwrap().unwrap()).unwrap(),
            serde_json::to_value(same).unwrap()
        );
        let after = db.get_entry("different").unwrap().unwrap();
        assert_eq!(after.translation.as_deref(), Some("Adiós"));
        assert_eq!(after.status, StringStatus::Translated);
        assert_eq!(after.provider_used.as_deref(), Some("import"));
    }
}
