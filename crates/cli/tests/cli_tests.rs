use predicates::prelude::*;
use serde_json::Value;

mod common;
use common::locust;

#[test]
fn command_roots_survive_commands_and_are_cleaned_up_at_thread_exit() {
    let roots = std::thread::spawn(|| {
        let mut roots = Vec::new();
        for _ in 0..2 {
            let command = locust();
            let paths: Vec<_> = command
                .get_envs()
                .filter(|(name, _)| *name == "LOCUST_DATA_DIR" || *name == "LOCUST_BACKUP_ROOT")
                .map(|(_, value)| std::path::PathBuf::from(value.unwrap()))
                .collect();
            assert_eq!(paths.len(), 2);
            for path in &paths {
                std::fs::create_dir_all(path).unwrap();
                std::fs::write(path.join("keep.txt"), b"earlier command").unwrap();
            }
            drop(command);
            roots.push(paths);
        }
        assert_ne!(roots[0], roots[1], "each command needs a fresh root");
        for path in roots.iter().flatten() {
            assert_eq!(
                std::fs::read(path.join("keep.txt")).unwrap(),
                b"earlier command"
            );
        }
        roots
    })
    .join()
    .unwrap();
    for path in roots.iter().flatten() {
        assert!(!path.parent().unwrap().exists(), "root leaked: {path:?}");
    }
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
    let dir_guard = tempfile::Builder::new()
        .prefix("locust_cli_test_")
        .tempdir()
        .unwrap();
    let dir = dir_guard.path().to_path_buf();
    locust()
        .args(["extract", &dir.to_string_lossy()])
        .assert()
        .failure()
        .stderr(predicate::str::contains("format not detected"));
}

#[test]
fn test_cli_glossary_add_and_list() {
    let dir_guard = tempfile::Builder::new()
        .prefix("locust_cli_gloss_")
        .tempdir()
        .unwrap();
    let dir = dir_guard.path().to_path_buf();
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

#[test]
fn validate_json_reports_issues_and_clean_projects() {
    use locust_core::database::Database;
    use locust_core::models::{StringEntry, StringStatus};

    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("validation.locust.db");
    let db = Database::open(&project).unwrap();
    let mut entry = StringEntry::new("line-1", "Hello", "script.txt".into());
    entry.translation = Some("Hello".into());
    entry.status = StringStatus::Translated;
    db.save_entries(&[entry.clone()]).unwrap();

    let output = locust()
        .args(["validate", project.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["count"], 1);
    assert_eq!(report["issues"][0]["entry_id"], "line-1");
    assert_eq!(report["issues"][0]["kind"], "IdenticalToSource");

    entry.translation = Some("Hola".into());
    db.save_entries(&[entry]).unwrap();
    let output = locust()
        .args(["validate", project.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["count"], 0);
    assert_eq!(report["issues"], serde_json::json!([]));
}

#[test]
fn format_profile_validate_uses_extracted_and_legacy_rpgmaker_provenance() {
    use locust_core::database::{Database, EntryFilter};
    use locust_core::models::{StringStatus, FORMAT_ID_METADATA_KEY};

    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("Actors.json");
    std::fs::write(&source, serde_json::to_vec(&serde_json::json!([
        null,
        {"name": r"\n<【貴族】リディア視点>　■解放条件　\n　・好感度１４以上\n　・「精力を集める」を選択"},
        {"name": r"\n<【冒険者】ルイ視点>　■解放条件　\n　・好感度６以上 /「精力を集める」を選択"}
    ])).unwrap()).unwrap();
    let project = dir.path().join("format.locust.db");
    locust()
        .arg("extract")
        .arg(&source)
        .args(["--format", "rpgmaker-mv", "--output"])
        .arg(&project)
        .assert()
        .success();
    let db = Database::open(&project).unwrap();
    let mut entries = db.get_entries(&EntryFilter::default()).unwrap();
    assert_eq!(entries.len(), 2);
    let translations = [
        r"\n<[Noble] Lidia> Unlock conditions\n Affection 14 or higher\n Select Gather energy",
        r"\n<[Adventurer] Louis> Unlock conditions\n Affection 6 or higher / Select Gather energy",
    ];
    for (entry, translation) in entries.iter_mut().zip(translations) {
        assert_eq!(entry.metadata[FORMAT_ID_METADATA_KEY], "rpgmaker-mv");
        entry.translation = Some(translation.into());
        entry.status = StringStatus::Translated;
    }
    // Validation of an archived project must not require the game files.
    std::fs::remove_file(&source).unwrap();
    for legacy in [false, true] {
        if legacy {
            for entry in &mut entries {
                entry.metadata.remove(FORMAT_ID_METADATA_KEY);
            }
        }
        db.save_entries(&entries).unwrap();
        let output = locust()
            .arg("validate")
            .arg(&project)
            .arg("--json")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
        let report: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(report["count"], 0);
    }
    for entry in &mut entries {
        entry
            .metadata
            .insert(FORMAT_ID_METADATA_KEY.into(), serde_json::json!("renpy"));
    }
    db.save_entries(&entries).unwrap();
    let output = locust()
        .arg("validate")
        .arg(&project)
        .arg("--json")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["count"], 2);
}
