use locust_core::database::{Database, EntryFilter};
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

fn import(
    project: &Path,
    input: &Path,
    format: &str,
    keep_existing: bool,
    dry_run: bool,
) -> String {
    let mut command = locust();
    command
        .arg("import")
        .arg(project)
        .args(["--format", format, "--lang", "es", "--input"])
        .arg(input);
    if keep_existing {
        command.arg("--keep-existing");
    }
    if dry_run {
        command.arg("--dry-run");
    }
    let assertion = command.assert().success();
    String::from_utf8(assertion.get_output().stdout.clone()).unwrap()
}

fn write_catalog(input: &Path, format: &str, entries: &[StringEntry]) {
    // A translator's catalog carries no project metadata; stale markers on the
    // fixtures describe DB state and would otherwise export as fuzzy entries.
    let entries: Vec<StringEntry> = entries
        .iter()
        .cloned()
        .map(|mut entry| {
            entry.metadata.clear();
            entry
        })
        .collect();
    let content = match format {
        "po" => export::export_po(&entries, "en", "es"),
        "xliff" => export::export_xliff(&entries, "en", "es"),
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

        let output = import(&project, &input, format, true, false);

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
                "Skipped 3: 1 outdated source(s), 1 unknown id(s), 1 empty/missing/fuzzy entries"
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

        let output = import(&project, &input, format, false, false);

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

#[test]
fn cli_import_po_fuzzy_preserves_approved_and_imports_normal() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project.db");
    let input = dir.path().join("catalog.po");
    let db = Database::open(&project).unwrap();
    let reviewed = approved("fuzzy", "Hello", "Reviewed translation");
    let normal = entry("normal", "Goodbye", None);
    db.save_entries(&[reviewed.clone(), normal.clone()])
        .unwrap();
    drop(db);
    std::fs::write(
        &input,
        concat!(
            "#, fuzzy\nmsgctxt \"fuzzy\"\nmsgid \"Hello\"\nmsgstr \"Unreviewed guess\"\n\n",
            "msgctxt \"normal\"\nmsgid \"Goodbye\"\nmsgstr \"Adiós\"",
        ),
    )
    .unwrap();

    for dry_run in [true, false] {
        let output = import(&project, &input, "po", false, dry_run);
        assert!(
            output.contains(
                "Skipped 1: 0 outdated source(s), 0 unknown id(s), 1 empty/missing/fuzzy entries"
            ),
            "{output}"
        );
        let db = Database::open(&project).unwrap();
        let after = db.get_entry("fuzzy").unwrap().unwrap();
        assert_eq!(after.translation, reviewed.translation);
        assert_eq!(after.status, StringStatus::Approved);
        assert_eq!(
            serde_json::to_value(after).unwrap(),
            serde_json::to_value(&reviewed).unwrap()
        );
        let after = db.get_entry("normal").unwrap().unwrap();
        if dry_run {
            assert!(
                output.contains("Would replace 0 translation(s):"),
                "{output}"
            );
            assert!(output.contains("Would fill 1 empty row(s)"), "{output}");
            assert_eq!(
                serde_json::to_value(after).unwrap(),
                serde_json::to_value(&normal).unwrap()
            );
        } else {
            assert!(output.contains("Imported 1 translations from "), "{output}");
            assert_eq!(after.translation.as_deref(), Some("Adiós"));
            assert_eq!(after.status, StringStatus::Translated);
            assert_eq!(after.provider_used.as_deref(), Some("import"));
        }
    }
}

fn rows_bytes(db: &Database) -> Vec<u8> {
    let rows = serde_json::to_value(db.get_entries(&EntryFilter::default()).unwrap()).unwrap();
    serde_json::to_vec(&rows).unwrap()
}

fn check_dry_run_then_import(format: &str) {
    for keep_existing in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("project.db");
        let input = dir.path().join(format!("catalog.{format}"));
        let db = Database::open(&project).unwrap();
        let same = approved("same", "Hello", "Hola");
        let different = approved("different", "Welcome", "Local correction");
        let stale = approved("stale", "Current source", "Current translation");
        let missing_target = entry("missing-target", "Empty target", None);
        let untouched = approved("untouched", "Outside the catalog", "Leave alone");
        let mut confirm = approved("confirm", "Hello again", "Hola de nuevo");
        confirm.status = StringStatus::Pending;
        db.save_entries(&[
            same.clone(),
            different.clone(),
            stale.clone(),
            missing_target.clone(),
            untouched.clone(),
            confirm.clone(),
            entry("empty", "Goodbye", None),
            entry("blank", "Thanks", Some(" \t\r\n\u{2003}")),
        ])
        .unwrap();
        let before = rows_bytes(&db);
        drop(db);
        write_catalog(
            &input,
            format,
            &[
                same.clone(),
                entry("different", "Welcome", Some("Collaborator correction")),
                confirm,
                entry("empty", "Goodbye", Some("Adiós")),
                entry("blank", "Thanks", Some("Gracias")),
                entry("stale", "Old source", Some("Wrong translation")),
                entry("unknown", "Unknown", Some("Missing")),
                missing_target.clone(),
            ],
        );

        let output = import(&project, &input, format, keep_existing, true);

        let replacements = if keep_existing { 1 } else { 2 };
        assert!(
            output.contains(&format!("Would replace {replacements} translation(s):")),
            "{output}"
        );
        let replacement_line =
            "different: \"Local correction\" (approved) -> \"Collaborator correction\"";
        assert_eq!(
            output.contains(replacement_line),
            !keep_existing,
            "{output}"
        );
        assert!(
            output.contains("confirm: \"Hola de nuevo\" (pending) -> \"Hola de nuevo\""),
            "{output}"
        );
        assert!(output.contains("Would fill 2 empty row(s)"), "{output}");
        assert!(output.contains("Unchanged 1"), "{output}");
        assert_eq!(
            output.contains("Kept 1 (--keep-existing)"),
            keep_existing,
            "{output}"
        );
        let skip_line =
            "Skipped 3: 1 outdated source(s), 1 unknown id(s), 1 empty/missing/fuzzy entries";
        assert!(output.contains(skip_line), "{output}");
        assert!(
            output.trim_end().ends_with("Dry run: nothing was saved."),
            "{output}"
        );
        assert!(!output.contains("Imported "), "{output}");
        let db = Database::open(&project).unwrap();
        assert_eq!(
            rows_bytes(&db),
            before,
            "{format}, keep_existing={keep_existing}"
        );
        drop(db);

        let output = import(&project, &input, format, keep_existing, false);

        let imported = if keep_existing { 4 } else { 5 };
        assert!(
            output.contains(&format!("Imported {imported} translations from ")),
            "{output}"
        );
        assert!(
            output.contains("Unchanged 1 (already identical)"),
            "{output}"
        );
        assert!(output.contains(skip_line), "{output}");
        assert!(!output.contains("Dry run"), "{output}");
        assert!(!output.contains("Would "), "{output}");
        let db = Database::open(&project).unwrap();
        let mut preserved = vec![same, stale, missing_target, untouched];
        if keep_existing {
            preserved.push(different.clone());
        }
        for entry in preserved {
            assert_eq!(
                serde_json::to_value(db.get_entry(&entry.id).unwrap().unwrap()).unwrap(),
                serde_json::to_value(entry).unwrap()
            );
        }
        let mut changed = vec![
            ("confirm", "Hola de nuevo"),
            ("empty", "Adiós"),
            ("blank", "Gracias"),
        ];
        if !keep_existing {
            changed.push(("different", "Collaborator correction"));
        }
        for (id, translation) in changed {
            let after = db.get_entry(id).unwrap().unwrap();
            assert_eq!(after.translation.as_deref(), Some(translation));
            assert_eq!(after.status, StringStatus::Translated);
            assert_eq!(after.provider_used.as_deref(), Some("import"));
            assert!(after.translated_at.is_some());
            assert!(!after.metadata.contains_key(STALE_TRANSLATION_METADATA_KEY));
        }
        assert_eq!(
            db.get_entry("different").unwrap().unwrap().reviewed_at,
            different.reviewed_at
        );
        assert_eq!(db.get_entries(&EntryFilter::default()).unwrap().len(), 8);
        assert!(db.get_entry("unknown").unwrap().is_none());
    }
}

#[test]
fn cli_import_po_dry_run_previews_changes_without_saving() {
    check_dry_run_then_import("po");
}

#[test]
fn cli_import_xliff_dry_run_previews_changes_without_saving() {
    check_dry_run_then_import("xliff");
}

#[test]
fn cli_import_dry_run_truncates_unicode_text_and_escapes_newlines() {
    for format in ["po", "xliff"] {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("project.db");
        let input = dir.path().join(format!("catalog.{format}"));
        let db = Database::open(&project).unwrap();
        let old = format!("{}old tail", "界".repeat(90));
        let new = format!("{}new tail", "é".repeat(90));
        db.save_entries(&[
            approved("long", "Long", &old),
            approved("escaped", "Escaped", "Old\n\"line\""),
        ])
        .unwrap();
        let before = rows_bytes(&db);
        drop(db);
        write_catalog(
            &input,
            format,
            &[
                entry("long", "Long", Some(&new)),
                entry("escaped", "Escaped", Some("New\n\"line\"")),
            ],
        );

        let output = import(&project, &input, format, false, true);

        assert!(
            output.contains(&format!(
                "long: \"{}…\" (approved) -> \"{}…\"",
                "界".repeat(79),
                "é".repeat(79)
            )),
            "{output}"
        );
        assert!(!output.contains("old tail"), "{output}");
        assert!(!output.contains("new tail"), "{output}");
        assert!(
            output.contains(r#"escaped: "Old\n\"line\"" (approved) -> "New\n\"line\"""#),
            "{output}"
        );
        assert_eq!(rows_bytes(&Database::open(&project).unwrap()), before);
    }
}
