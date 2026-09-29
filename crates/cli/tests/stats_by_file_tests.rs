use locust_core::database::{Database, EntryFilter};
use locust_core::models::{StringEntry, StringStatus};

mod common;
use common::locust;

#[test]
fn stats_by_file_without_runs_counts_statuses_and_preserves_rows() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("stats.locust.db");
    let db = Database::open(&project).unwrap();
    let entries: Vec<_> = [
        (
            "b-translated",
            "b.rpy",
            StringStatus::Translated,
            Some("Hola"),
        ),
        ("b-error", "b.rpy", StringStatus::Error, None),
        (
            "a-pending",
            "a.rpy",
            StringStatus::Pending,
            Some("Stale text"),
        ),
        (
            "a-approved",
            "a.rpy",
            StringStatus::Approved,
            Some("Aprobado"),
        ),
    ]
    .into_iter()
    .map(|(id, file, status, translation)| {
        let mut entry = StringEntry::new(id, id, file.into());
        entry.status = status;
        entry.translation = translation.map(str::to_owned);
        entry
    })
    .collect();
    db.save_entries(&entries).unwrap();
    assert!(db.get_translation_runs().unwrap().is_empty());
    let before = serde_json::to_value(db.get_entries(&EntryFilter::default()).unwrap()).unwrap();

    let result = locust()
        .arg("stats")
        .arg(&project)
        .arg("--by-file")
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&result.get_output().stdout);
    let rows: Vec<Vec<&str>> = stdout
        .lines()
        .filter(|line| line.starts_with("| "))
        .map(|line| line.trim_matches('|').split('|').map(str::trim).collect())
        .collect();
    assert_eq!(
        rows,
        vec![
            vec![
                "File",
                "Total",
                "Pending",
                "Translated",
                "Reviewed",
                "Approved",
                "Error"
            ],
            vec!["a.rpy", "2", "1", "0", "0", "1", "0"],
            vec!["b.rpy", "2", "0", "1", "0", "0", "1"],
            vec!["TOTAL", "4", "1", "1", "0", "1", "1"],
        ],
        "{stdout}"
    );
    locust()
        .arg("stats")
        .arg(&project)
        .assert()
        .success()
        .stdout("No translation runs recorded yet for this project.\n");
    let after = serde_json::to_value(db.get_entries(&EntryFilter::default()).unwrap()).unwrap();
    assert_eq!(after, before);
    assert!(db.get_translation_runs().unwrap().is_empty());
}

#[test]
fn stats_by_file_empty_project_has_no_table() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("empty.locust.db");
    Database::open(&project).unwrap();
    locust()
        .arg("stats")
        .arg(&project)
        .arg("--by-file")
        .assert()
        .success()
        .stdout("No strings in this project yet.\n");
}
