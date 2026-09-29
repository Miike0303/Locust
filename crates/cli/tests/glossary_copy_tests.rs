use std::path::{Path, PathBuf};

use locust_core::database::{sha256_path, Database, GlossaryEntry};
use predicates::prelude::*;

mod common;
use common::locust;

fn entry(term: &str, translation: &str, lang_pair: &str) -> GlossaryEntry {
    GlossaryEntry {
        term: term.into(),
        translation: translation.into(),
        lang_pair: lang_pair.into(),
        context: None,
        case_sensitive: false,
    }
}

fn source_entries() -> [GlossaryEntry; 4] {
    [
        GlossaryEntry {
            context: Some("Organization title".into()),
            case_sensitive: true,
            ..entry("Guild", "Gremio", "en-es")
        },
        GlossaryEntry {
            context: Some("Combat stat".into()),
            case_sensitive: true,
            ..entry("HP", "PV", "en-es")
        },
        entry("HP", "Vie", "en-fr"),
        entry("MP", "Magie", "en-fr"),
    ]
}

fn tuned_entry() -> GlossaryEntry {
    entry("HP", "Puntos de vida", "en-es")
}

struct Fixture {
    _dir: tempfile::TempDir,
    source: PathBuf,
    dest: PathBuf,
    source_hash: String,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source project.db");
        let dest = dir.path().join("destination project.db");
        let source_db = Database::open(&source).unwrap();
        for entry in source_entries() {
            source_db.save_glossary_entry(&entry).unwrap();
        }
        // Close both connections so hashes cover checkpointed file contents.
        drop(source_db);
        let dest_db = Database::open(&dest).unwrap();
        for entry in [tuned_entry(), entry("HP", "Local French", "en-fr")] {
            dest_db.save_glossary_entry(&entry).unwrap();
        }
        drop(dest_db);
        let source_hash = sha256_path(&source).unwrap();
        Self {
            _dir: dir,
            source,
            dest,
            source_hash,
        }
    }

    fn assert_source_unchanged(&self) {
        assert_eq!(sha256_path(&self.source).unwrap(), self.source_hash);
    }
}

fn copy(source: &Path, dest: &Path) -> assert_cmd::Command {
    let mut command = locust();
    command
        .args(["glossary", "copy"])
        .arg(source)
        .arg(dest)
        .args(["--lang-pair", "en-es"]);
    command
}

fn assert_glossary(path: &Path, pair: &str, expected: &[GlossaryEntry]) {
    let db = Database::open(path).unwrap();
    let mut actual = db.get_glossary(pair).unwrap();
    actual.sort_by(|a, b| a.term.cmp(&b.term));
    let mut expected = expected.to_vec();
    expected.sort_by(|a, b| a.term.cmp(&b.term));
    assert_eq!(
        serde_json::to_value(actual).unwrap(),
        serde_json::to_value(expected).unwrap()
    );
}

#[test]
fn copy_preserves_tuned_conflict_and_all_fields() {
    let fixture = Fixture::new();
    let assertion = copy(&fixture.source, &fixture.dest).assert().success();

    // Check the actual destination before the counts so unconditional upserts
    // fail on the translator's lost edits, even if the report looks correct.
    assert_glossary(
        &fixture.dest,
        "en-es",
        &[source_entries()[0].clone(), tuned_entry()],
    );
    assert_glossary(
        &fixture.dest,
        "en-fr",
        &[entry("HP", "Local French", "en-fr")],
    );
    fixture.assert_source_unchanged();
    assertion.stdout(predicate::str::contains(
        "added=1 overwritten=0 kept_existing=1 unchanged=0",
    ));

    let before = sha256_path(&fixture.dest).unwrap();
    copy(&fixture.source, &fixture.dest)
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "added=0 overwritten=0 kept_existing=1 unchanged=1",
        ));
    assert_eq!(sha256_path(&fixture.dest).unwrap(), before);
    fixture.assert_source_unchanged();
}

#[test]
fn copy_overwrites_conflict_and_identical_repeat_does_not_write() {
    let fixture = Fixture::new();
    copy(&fixture.source, &fixture.dest)
        .arg("--overwrite")
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "added=1 overwritten=1 kept_existing=0 unchanged=0",
        ));
    assert_glossary(&fixture.dest, "en-es", &source_entries()[..2]);
    assert_glossary(
        &fixture.dest,
        "en-fr",
        &[entry("HP", "Local French", "en-fr")],
    );
    fixture.assert_source_unchanged();

    // A byte hash alone can miss redundant updates; reject even attempted
    // glossary writes on both repeat paths.
    let conn = rusqlite::Connection::open(&fixture.dest).unwrap();
    conn.execute_batch(
        "CREATE TRIGGER no_glossary_insert BEFORE INSERT ON glossary
         BEGIN SELECT RAISE(ABORT, 'unexpected glossary insert'); END;
         CREATE TRIGGER no_glossary_update BEFORE UPDATE ON glossary
         BEGIN SELECT RAISE(ABORT, 'unexpected glossary update'); END;
         CREATE TRIGGER no_glossary_delete BEFORE DELETE ON glossary
         BEGIN SELECT RAISE(ABORT, 'unexpected glossary delete'); END;",
    )
    .unwrap();
    drop(conn);
    let before = sha256_path(&fixture.dest).unwrap();
    for overwrite in [false, true] {
        let mut command = copy(&fixture.source, &fixture.dest);
        if overwrite {
            command.arg("--overwrite");
        }
        command.assert().success().stdout(predicate::str::contains(
            "added=0 overwritten=0 kept_existing=0 unchanged=2",
        ));
        assert_eq!(sha256_path(&fixture.dest).unwrap(), before);
        fixture.assert_source_unchanged();
    }
}

#[test]
fn copy_empty_pair_has_zero_counts_and_changes_neither_database() {
    let fixture = Fixture::new();
    let before = sha256_path(&fixture.dest).unwrap();
    locust()
        .args(["glossary", "copy"])
        .arg(&fixture.source)
        .arg(&fixture.dest)
        .args(["--lang-pair", "ja-es"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "added=0 overwritten=0 kept_existing=0 unchanged=0",
        ));
    assert_eq!(sha256_path(&fixture.dest).unwrap(), before);
    fixture.assert_source_unchanged();
}

#[test]
fn copy_rejects_same_database_including_alternate_spelling() {
    let fixture = Fixture::new();
    for dest in [
        fixture.source.clone(),
        fixture.source.canonicalize().unwrap(),
        fixture
            .source
            .parent()
            .unwrap()
            .join(".")
            .join("source project.db"),
    ] {
        copy(&fixture.source, &dest)
            .arg("--overwrite")
            .assert()
            .failure()
            .stderr(predicate::str::contains("same database"));
        fixture.assert_source_unchanged();
    }
}

#[test]
fn copy_rejects_missing_source_or_destination_without_creating_files() {
    let fixture = Fixture::new();
    let missing = fixture.source.parent().unwrap().join("missing project.db");
    let dest_before = sha256_path(&fixture.dest).unwrap();
    for (source, dest) in [(&missing, &fixture.dest), (&fixture.source, &missing)] {
        copy(source, dest)
            .assert()
            .failure()
            .stderr(predicate::str::contains("project database not found"));
        for suffix in ["", "-wal", "-shm", "-journal"] {
            assert!(!missing
                .with_file_name(format!("missing project.db{suffix}"))
                .exists());
        }
        fixture.assert_source_unchanged();
        assert_eq!(sha256_path(&fixture.dest).unwrap(), dest_before);
    }
}
