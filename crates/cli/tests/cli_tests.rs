use assert_cmd::Command;
use predicates::prelude::*;

fn locust() -> Command {
    Command::cargo_bin("locust").unwrap()
}

#[test]
fn test_cli_help_exits_0() {
    locust().arg("--help").assert().success();
}

#[test]
fn test_cli_version() {
    locust()
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::contains("locust"));
}

#[test]
fn test_cli_formats() {
    locust()
        .arg("formats")
        .assert()
        .success()
        .stdout(predicate::str::contains("RPG Maker"));
}

#[test]
fn test_cli_providers() {
    locust().arg("providers").assert().success();
}

#[test]
fn test_cli_extract_missing_path() {
    locust()
        .args(["extract", "/nonexistent_path_xyz_123"])
        .assert()
        .failure();
}

#[test]
fn test_cli_extract_unknown_format() {
    let dir = std::env::temp_dir().join(format!("locust_cli_test_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    locust()
        .args(["extract", &dir.to_string_lossy()])
        .assert()
        .failure()
        .stderr(predicate::str::contains("format not detected"));
}

#[test]
fn test_cli_glossary_add_and_list() {
    let dir = std::env::temp_dir().join(format!("locust_cli_gloss_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let db_path = dir.join("test.locust.db");

    // Create the db first with an extract-like operation (or just open it)
    let db = locust_core::database::Database::open(&db_path).unwrap();
    drop(db);

    // Add a term
    locust()
        .args([
            "glossary",
            "add",
            &db_path.to_string_lossy(),
            "--term",
            "HP",
            "--translation",
            "PV",
            "--lang-pair",
            "en-es",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Added"));

    // List
    locust()
        .args([
            "glossary",
            "list",
            &db_path.to_string_lossy(),
            "--lang-pair",
            "en-es",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("HP"));
}
