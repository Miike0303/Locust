use locust_core::database::Database;
use locust_core::models::{StringEntry, StringStatus};
use predicates::prelude::*;

mod common;
use common::locust;

const MISSING_PROJECT: &str = "missing project.locust.db";

fn assert_missing_project(args: &[&str]) {
    let dir = tempfile::tempdir().unwrap();
    // Valid inputs ensure the project guard is reached, including by inject/import.
    std::fs::create_dir_all(dir.path().join("game/game")).unwrap();
    std::fs::write(
        dir.path().join("game/game/script.rpy"),
        "label start:\n    e \"Hello, world!\"\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("input.po"),
        "msgctxt \"hello\"\nmsgid \"Hello\"\nmsgstr \"Hola\"\n",
    )
    .unwrap();

    let output = locust()
        .current_dir(dir.path())
        .args(args)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    // Check artifacts first so a regression reports the unintended creation,
    // even for commands that already failed after opening an empty database.
    for suffix in ["", "-wal", "-shm"] {
        let artifact = dir.path().join(format!("{MISSING_PROJECT}{suffix}"));
        assert!(
            !artifact.try_exists().unwrap(),
            "{args:?} created unexpected file: {}\nstatus: {}\nstderr: {stderr}",
            artifact.display(),
            output.status
        );
    }
    assert!(!output.status.success(), "{args:?} succeeded unexpectedly");
    assert!(
        stderr.contains(&format!("project database not found: {MISSING_PROJECT}")),
        "{args:?}: {stderr}"
    );
    assert!(stderr.contains("locust extract <game> -o"), "{stderr}");
    assert!(!dir.path().join("output").exists());
}

macro_rules! missing_project_test {
    ($name:ident, [$($arg:expr),* $(,)?]) => {
        #[test]
        fn $name() {
            assert_missing_project(&[$($arg),*]);
        }
    };
}

missing_project_test!(stats_rejects_missing_project, ["stats", MISSING_PROJECT]);
missing_project_test!(
    validate_rejects_missing_project,
    ["validate", MISSING_PROJECT]
);
missing_project_test!(
    export_rejects_missing_project,
    [
        "export",
        MISSING_PROJECT,
        "-f",
        "po",
        "-l",
        "es",
        "-o",
        "output"
    ]
);
missing_project_test!(
    import_rejects_missing_project,
    [
        "import",
        MISSING_PROJECT,
        "-f",
        "po",
        "-l",
        "es",
        "-i",
        "input.po"
    ]
);
missing_project_test!(
    translate_rejects_missing_project,
    ["translate", MISSING_PROJECT, "-p", "mock"]
);
missing_project_test!(
    backup_project_rejects_missing_project,
    ["backup-project", MISSING_PROJECT, "-o", "output"]
);
missing_project_test!(
    glossary_list_rejects_missing_project,
    ["glossary", "list", MISSING_PROJECT, "--lang-pair", "en-es"]
);
missing_project_test!(
    glossary_add_rejects_missing_project,
    [
        "glossary",
        "add",
        MISSING_PROJECT,
        "--term",
        "Hello",
        "--translation",
        "Hola",
        "--lang-pair",
        "en-es"
    ]
);
missing_project_test!(
    glossary_delete_rejects_missing_project,
    [
        "glossary",
        "delete",
        MISSING_PROJECT,
        "--term",
        "Hello",
        "--lang-pair",
        "en-es"
    ]
);
missing_project_test!(
    inject_rejects_missing_project,
    [
        "inject",
        "game",
        "-P",
        MISSING_PROJECT,
        "-l",
        "es",
        "-o",
        "output"
    ]
);
missing_project_test!(
    inject_direct_rejects_missing_project,
    [
        "inject",
        "game",
        "-P",
        MISSING_PROJECT,
        "--direct",
        "-l",
        "es"
    ]
);
missing_project_test!(
    patch_rejects_missing_project,
    [
        "patch",
        "game",
        "-P",
        MISSING_PROJECT,
        "-l",
        "es",
        "-o",
        "output"
    ]
);
missing_project_test!(
    pivot_rejects_missing_project,
    ["pivot", MISSING_PROJECT, "-o", "output"]
);
missing_project_test!(
    replace_rejects_missing_project,
    [
        "replace",
        MISSING_PROJECT,
        "--find",
        "Hello",
        "--replace",
        "Hola"
    ]
);

#[test]
fn directory_is_not_a_project_database() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project.locust.db");
    std::fs::create_dir(&project).unwrap();

    locust()
        .arg("stats")
        .arg(&project)
        .assert()
        .failure()
        .stderr(predicate::str::contains(format!(
            "project database path is a directory: {}",
            project.display()
        )));
    assert!(project.is_dir());
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    assert_eq!(std::fs::read_dir(&project).unwrap().count(), 0);
}

#[test]
fn stats_accepts_an_existing_project() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project.locust.db");
    let db = Database::open(&project).unwrap();
    db.save_entries(&[StringEntry::new("hello", "Hello", "script.rpy".into())])
        .unwrap();
    drop(db);

    locust()
        .arg("translate")
        .arg(&project)
        .args(["-p", "mock", "-s", "en", "-t", "es"])
        .assert()
        .success();
    locust()
        .arg("stats")
        .arg(&project)
        .assert()
        .success()
        .stdout(predicate::str::contains("mock"));
}

#[test]
fn replace_updates_an_existing_project() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project.locust.db");
    let db = Database::open(&project).unwrap();
    let mut entry = StringEntry::new("hello", "Hello", "script.rpy".into());
    entry.translation = Some("Hola".into());
    entry.status = StringStatus::Translated;
    db.save_entries(&[entry]).unwrap();
    drop(db);

    locust()
        .arg("replace")
        .arg(&project)
        .args(["--find", "Hola", "--replace", "Buenas"])
        .assert()
        .success();
    assert_eq!(
        Database::open(&project)
            .unwrap()
            .get_entry("hello")
            .unwrap()
            .unwrap()
            .translation
            .as_deref(),
        Some("Buenas")
    );
}

#[test]
fn glossary_delete_updates_an_existing_project() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project.locust.db");
    drop(Database::open(&project).unwrap());
    locust()
        .args(["glossary", "add"])
        .arg(&project)
        .args([
            "--term",
            "Hello",
            "--translation",
            "Hola",
            "--lang-pair",
            "en-es",
        ])
        .assert()
        .success();
    assert_eq!(
        Database::open(&project)
            .unwrap()
            .get_glossary("en-es")
            .unwrap()
            .len(),
        1
    );

    locust()
        .args(["glossary", "delete"])
        .arg(&project)
        .args(["--term", "Hello", "--lang-pair", "en-es"])
        .assert()
        .success();
    assert!(Database::open(&project)
        .unwrap()
        .get_glossary("en-es")
        .unwrap()
        .is_empty());
}
