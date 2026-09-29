use std::path::{Path, PathBuf};

use locust_core::database::{Database, EntryFilter, GlossaryEntry, TranslationRun};
use locust_core::models::{StringEntry, StringStatus};
use serde_json::json;

mod common;
use common::locust;

fn backup_command(project: &Path, output: &Path) -> assert_cmd::Command {
    let mut command = locust();
    command
        .arg("backup-project")
        .arg(project)
        .arg("-o")
        .arg(output);
    command
}

fn rows(db: &Database) -> serde_json::Value {
    serde_json::to_value(db.get_entries(&EntryFilter::default()).unwrap()).unwrap()
}

fn live_project() -> (tempfile::TempDir, PathBuf, Database) {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project.locust.db");
    let db = Database::open(&project).unwrap();
    db.save_entries(&[StringEntry::new("hello", "Hello", "story.rpy".into())])
        .unwrap();
    (dir, project, db)
}

#[tokio::test]
async fn backup_project_preserves_live_wal_state() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project.locust.db");
    // Close once to checkpoint the empty schema. Keep the next connection alive
    // through the CLI call so its committed writes remain in the source WAL.
    drop(Database::open(&project).unwrap());
    let db = Database::open(&project).unwrap();
    let main_before_writes = std::fs::read(&project).unwrap();
    let mut entries = Vec::new();
    for (id, source, translation) in [
        ("hello", "Hello", "¡Hola!"),
        ("goodbye", "Goodbye", "Adiós"),
    ] {
        let mut entry = StringEntry::new(id, source, "story.rpy".into())
            .with_context("Menu dialogue")
            .with_tags(vec!["menu".into()]);
        entry.translation = Some(translation.into());
        entry.status = StringStatus::Approved;
        entry.provider_used = Some("mock".into());
        entry.translated_at = Some(chrono::Utc::now());
        entry.reviewed_at = entry.translated_at;
        entries.push(entry);
    }
    db.save_entries(&entries).unwrap();
    db.save_glossary_entry(&GlossaryEntry {
        term: "Guild".into(),
        translation: "Gremio".into(),
        lang_pair: "en-es".into(),
        context: Some("Organization".into()),
        case_sensitive: true,
    })
    .unwrap();
    db.record_translation_run(&TranslationRun {
        started_at: "2026-09-28T12:00:00Z".into(),
        duration_secs: 1.25,
        provider: "mock".into(),
        source_lang: "en".into(),
        target_lang: "es".into(),
        strings_translated: 2,
        tokens_used: 30,
        input_tokens: 20,
        output_tokens: 10,
        cost_usd: 0.125,
        cost_is_complete: true,
        ..Default::default()
    })
    .await
    .unwrap();
    let metadata = json!({"source_lang": "en", "target_lang": "es", "title": "Prueba 日本語"});
    db.set_project_metadata("project", &metadata).unwrap();

    let expected_rows = rows(&db);
    let expected_glossary = serde_json::to_value(db.get_glossary("en-es").unwrap()).unwrap();
    let expected_runs = serde_json::to_value(db.get_translation_runs().unwrap()).unwrap();
    assert_eq!(expected_glossary.as_array().unwrap().len(), 1);
    assert_eq!(expected_runs.as_array().unwrap().len(), 1);
    assert_eq!(std::fs::read(&project).unwrap(), main_before_writes);
    assert!(
        std::fs::metadata(dir.path().join("project.locust.db-wal"))
            .unwrap()
            .len()
            > 32,
        "fixture writes must still be in the WAL"
    );

    let output = dir.path().join("translator's checkpoint.locust.db");
    let assertion = backup_command(&project, &output).assert().success();
    let size = std::fs::metadata(&output).unwrap().len();
    assert!(size > 0);
    // Move ONLY the main checkpoint file to an empty directory: no source or
    // checkpoint sidecar can provide missing data when it is opened here.
    let standalone_dir = tempfile::tempdir().unwrap();
    let standalone = standalone_dir.path().join("checkpoint.locust.db");
    std::fs::rename(&output, &standalone).unwrap();
    let checkpoint = Database::open(&standalone).unwrap();
    let loaded = checkpoint.get_entries(&EntryFilter::default()).unwrap();
    assert_eq!(
        loaded.len(),
        2,
        "checkpoint must include both entries committed to the live WAL"
    );
    assert!(loaded
        .iter()
        .all(|entry| entry.status == StringStatus::Approved));
    assert_eq!(serde_json::to_value(loaded).unwrap(), expected_rows);
    assert_eq!(
        serde_json::to_value(checkpoint.get_glossary("en-es").unwrap()).unwrap(),
        expected_glossary
    );
    assert_eq!(
        serde_json::to_value(checkpoint.get_translation_runs().unwrap()).unwrap(),
        expected_runs
    );
    assert_eq!(
        checkpoint.get_project_metadata("project").unwrap(),
        Some(metadata.clone())
    );
    assertion.stdout(predicates::str::contains(format!(
        "Checkpoint: {} ({size} bytes, 2 strings)",
        output.display()
    )));

    assert_eq!(rows(&db), expected_rows);
    assert_eq!(
        serde_json::to_value(db.get_glossary("en-es").unwrap()).unwrap(),
        expected_glossary
    );
    assert_eq!(
        serde_json::to_value(db.get_translation_runs().unwrap()).unwrap(),
        expected_runs
    );
    assert_eq!(db.get_project_metadata("project").unwrap(), Some(metadata));
}

#[test]
fn backup_project_refuses_existing_destination_without_changing_bytes() {
    let (dir, project, db) = live_project();
    let before_rows = rows(&db);
    let output = dir.path().join("existing.locust.db");
    // SQLite permits VACUUM INTO an empty file; the explicit guard must refuse it.
    for bytes in [b"translator checkpoint\0\xff\n".as_slice(), b""] {
        std::fs::write(&output, bytes).unwrap();
        backup_command(&project, &output)
            .assert()
            .failure()
            .stderr(predicates::str::contains(
                "checkpoint destination already exists",
            ));
        assert_eq!(std::fs::read(&output).unwrap(), bytes);
        assert_eq!(rows(&db), before_rows);
    }
}

#[test]
fn backup_project_refuses_project_database() {
    let (_dir, project, db) = live_project();
    let before_rows = rows(&db);
    let before_bytes = std::fs::read(&project).unwrap();
    backup_command(&project, &project)
        .assert()
        .failure()
        .stderr(predicates::str::contains("the project database itself"));
    assert_eq!(std::fs::read(&project).unwrap(), before_bytes);
    assert_eq!(rows(&db), before_rows);
    drop(db);
    assert_eq!(rows(&Database::open(&project).unwrap()), before_rows);
}

#[test]
fn backup_project_refuses_sqlite_sidecars() {
    let (dir, project, db) = live_project();
    let before_rows = rows(&db);
    let before_bytes = std::fs::read(&project).unwrap();
    let wal = dir.path().join("project.locust.db-wal");
    let before_wal = std::fs::read(&wal).unwrap();
    for suffix in ["-wal", "-shm", "-journal"] {
        let output = dir.path().join(format!("project.locust.db{suffix}"));
        backup_command(&project, &output)
            .assert()
            .failure()
            .stderr(predicates::str::contains(format!(
                "SQLite {suffix} sidecar"
            )));
        assert_eq!(std::fs::read(&project).unwrap(), before_bytes);
        assert_eq!(std::fs::read(&wal).unwrap(), before_wal);
        assert_eq!(rows(&db), before_rows);
    }
    drop(db);
    assert_eq!(rows(&Database::open(&project).unwrap()), before_rows);
}

#[test]
fn backup_project_help_explains_checkpoint_usage() {
    locust()
        .args(["backup-project", "--help"])
        .assert()
        .success()
        .stdout(predicates::str::contains("opens like any project database"));
}
