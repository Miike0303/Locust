use locust_core::database::Database;
use predicates::prelude::*;

mod common;
use common::locust;

#[test]
fn glossary_add_preserves_case_sensitive_and_context() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project.db");
    drop(Database::open(&project).unwrap());

    for context in ["  Organization title\nUSA  ", ""] {
        locust()
            .args(["glossary", "add"])
            .arg(&project)
            .args([
                "--term",
                "US",
                "--translation",
                "EEUU",
                "--lang-pair",
                "en-es",
                "--case-sensitive",
                "--context",
                context,
            ])
            .assert()
            .success();
        locust()
            .args(["glossary", "list"])
            .arg(&project)
            .args(["--lang-pair", "en-es"])
            .assert()
            .success()
            .stdout(predicate::str::contains("US"));

        let db = Database::open(&project).unwrap();
        let entries = db.get_glossary("en-es").unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].term, "US");
        assert_eq!(entries[0].translation, "EEUU");
        assert!(entries[0].case_sensitive);
        assert_eq!(entries[0].context.as_deref(), Some(context));
    }
}

#[test]
fn glossary_add_defaults_to_case_insensitive_without_context() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project.db");
    drop(Database::open(&project).unwrap());

    locust()
        .args(["glossary", "add"])
        .arg(&project)
        .args([
            "--term",
            "US",
            "--translation",
            "EEUU",
            "--lang-pair",
            "en-es",
        ])
        .assert()
        .success();

    let db = Database::open(&project).unwrap();
    let entries = db.get_glossary("en-es").unwrap();
    assert_eq!(entries.len(), 1);
    assert!(!entries[0].case_sensitive);
    assert_eq!(entries[0].context, None);
}
