use assert_cmd::Command;
use locust_core::{
    database::Database,
    models::{StringEntry, StringStatus},
};
use predicates::prelude::PredicateBooleanExt;
use std::{fs, path::Path};

fn command(root: &Path, database: &Path, input: &Path) -> Command {
    let config = root.join("offline.json");
    fs::write(&config, r#"{"providers":{},"default_provider":null}"#).unwrap();
    let mut command = Command::cargo_bin("locust").unwrap();
    command
        .env("LOCUST_DATA_DIR", root.join("profile"))
        .env("LOCUST_BACKUP_ROOT", root.join("backups"))
        .arg("--config")
        .arg(config)
        .arg("import")
        .arg(database)
        .args(["--format", "xliff", "--lang", "es", "--input"])
        .arg(input);
    command
}

#[test]
fn prefixed_xliff_updates_the_exact_id_and_rejects_incomplete_documents_atomically() {
    let temp = tempfile::tempdir().unwrap();
    let database = temp.path().join("project.locust.db");
    let db = Database::open(&database).unwrap();
    let mut entry = StringEntry::new("entry&日本語", "Source", "story.html".into());
    entry.translation = Some("Existing translation".into());
    entry.status = StringStatus::Translated;
    db.save_entries(&[entry]).unwrap();
    drop(db);
    let valid = r#"<x:xliff xmlns:x="urn:oasis:names:tc:xliff:document:1.2" version="1.2"><x:file><x:body>
      <x:trans-unit id="entry&amp;日本語"><x:source>Source</x:source><x:target><![CDATA[ Español <texto> ]]>&amp; 日</x:target></x:trans-unit>
    </x:body></x:file></x:xliff>"#;
    let input = temp.path().join("translations.xlf");
    let before = serde_json::to_value(
        Database::open(&database)
            .unwrap()
            .get_entries(&Default::default())
            .unwrap(),
    )
    .unwrap();
    for (document, message) in [
        (
            valid.replace("</x:body></x:file></x:xliff>", ""),
            "empty or incomplete XLIFF document",
        ),
        (
            format!("{valid}<xliff/>"),
            "expected one XLIFF 1.2 document",
        ),
        (
            format!("{valid}late text"),
            "text outside the document root",
        ),
        (
            valid.replace("</x:body>", "<q:trans-unit/></x:body>"),
            "undeclared XML namespace prefix",
        ),
    ] {
        fs::write(&input, document).unwrap();
        for extra in [vec![], vec!["--dry-run"], vec!["--keep-existing"]] {
            command(temp.path(), &database, &input)
                .args(extra)
                .assert()
                .code(1)
                .stdout("")
                .stderr(format!(
                    "Error: parse error in xliff: XLIFF parse error: {message}\n"
                ));
            let after = Database::open(&database)
                .unwrap()
                .get_entries(&Default::default())
                .unwrap();
            assert_eq!(serde_json::to_value(after).unwrap(), before);
        }
    }
    fs::write(&input, valid).unwrap();
    command(temp.path(), &database, &input)
        .assert()
        .success()
        .stdout(predicates::str::contains("Imported 1 translations"));
    let db = Database::open(&database).unwrap();
    assert_eq!(
        db.get_entry("entry&日本語")
            .unwrap()
            .unwrap()
            .translation
            .as_deref(),
        Some(" Español <texto> & 日")
    );
}

#[test]
fn old_source_from_xliff_is_skipped_instead_of_replacing_a_current_translation() {
    let temp = tempfile::tempdir().unwrap();
    let database = temp.path().join("project.locust.db");
    let db = Database::open(&database).unwrap();
    let mut entry = StringEntry::new("entry", "Current source", "story.html".into());
    entry.translation = Some("Current translation".into());
    entry.status = StringStatus::Approved;
    db.save_entries(&[entry]).unwrap();
    drop(db);
    let input = temp.path().join("old.xlf");
    fs::write(&input, r#"<xliff><file><body><trans-unit id="entry"><source>Old source</source><target>Obsolete translation</target></trans-unit></body></file></xliff>"#).unwrap();
    command(temp.path(), &database, &input)
        .assert()
        .success()
        .stdout(predicates::str::contains("1 outdated source(s)"));
    let db = Database::open(&database).unwrap();
    let entry = db.get_entry("entry").unwrap().unwrap();
    assert_eq!(entry.translation.as_deref(), Some("Current translation"));
    assert_eq!(entry.status, StringStatus::Approved);
}

fn exported_crlf_catalog_roundtrips_counts(format: &str) {
    let temp = tempfile::tempdir().unwrap();
    let database = temp.path().join("project.locust.db");
    let config = temp.path().join("offline.json");
    fs::write(&config, r#"{"providers":{},"default_provider":null}"#).unwrap();
    let db = Database::open(&database).unwrap();
    // Reproduce the reported shape: 525 rows, including 57 multiline sources.
    let entries: Vec<_> = (0..525)
        .map(|index| {
            let source = if index < 57 {
                format!("First line {index}\nSecond line")
            } else {
                format!("Single line {index}")
            };
            let mut entry = StringEntry::new(
                format!("story#{index}#message"),
                source,
                "story.json".into(),
            );
            entry.translation = Some(format!("Traducción {index}\nSegunda línea"));
            entry.status = StringStatus::Approved;
            entry
        })
        .collect();
    db.save_entries(&entries).unwrap();
    drop(db);
    let catalog = temp.path().join(format!("translations.{format}"));
    for operation in ["export", "import"] {
        let mut command = Command::cargo_bin("locust").unwrap();
        command
            .env("LOCUST_DATA_DIR", temp.path().join("profile"))
            .env("LOCUST_BACKUP_ROOT", temp.path().join("backups"))
            .arg("--config")
            .arg(&config)
            .arg(operation)
            .arg(&database)
            .args(["-f", format, "-l", "es"])
            .arg(if operation == "export" { "-o" } else { "-i" })
            .arg(&catalog);
        if operation == "export" {
            command.assert().success();
            let content = fs::read_to_string(&catalog).unwrap();
            assert!(!content.contains('\r'));
            fs::write(&catalog, content.replace('\n', "\r\n")).unwrap();
        } else {
            command
                .assert()
                .success()
                .stdout(predicates::str::contains("Imported 525 translations"))
                .stdout(predicates::str::contains(
                    "Unchanged 525 (already identical)",
                ))
                .stdout(predicates::str::contains("Skipped").not());
        }
    }
    let db = Database::open(&database).unwrap();
    for expected in entries {
        assert_eq!(
            serde_json::to_value(db.get_entry(&expected.id).unwrap().unwrap()).unwrap(),
            serde_json::to_value(expected).unwrap()
        );
    }
}

#[test]
fn exported_crlf_xliff_roundtrips_all_525_rows() {
    exported_crlf_catalog_roundtrips_counts("xliff");
}

#[test]
fn exported_crlf_po_roundtrips_all_525_rows() {
    exported_crlf_catalog_roundtrips_counts("po");
}
