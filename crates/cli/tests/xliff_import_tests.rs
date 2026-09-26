use assert_cmd::Command;
use locust_core::{
    database::Database,
    models::{StringEntry, StringStatus},
};
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
    fs::write(&input, valid.replace("</x:body></x:file></x:xliff>", "")).unwrap();
    command(temp.path(), &database, &input).assert().failure();
    let db = Database::open(&database).unwrap();
    assert_eq!(
        db.get_entry("entry&日本語")
            .unwrap()
            .unwrap()
            .translation
            .as_deref(),
        Some("Existing translation")
    );
    drop(db);
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
