use locust_core::database::Database;
use locust_core::models::{StringEntry, StringStatus, INJECTION_SOURCE_METADATA_KEY};

mod common;
use common::locust;

#[test]
fn translate_errors_are_visible_without_a_terminal_and_grouped() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("errors.db");
    let db = Database::open(&project).unwrap();
    // Mock reverses short free text: n\ becomes \n, adding a protected control.
    let entries: Vec<_> = (0..8)
        .map(|i| StringEntry::new(format!("line-{i}"), r"n\", "Map001.json".into()))
        .collect();
    db.save_entries(&entries).unwrap();
    drop(db);

    let assertion = locust()
        .args([
            "translate",
            project.to_str().unwrap(),
            "--provider",
            "mock",
            "--no-memory",
        ])
        .assert()
        .success();
    let output = assertion.get_output();
    let stderr = String::from_utf8_lossy(&output.stderr);
    let reason = "translation changed protected placeholders; entry remains pending";
    assert_eq!(
        stderr.matches(&format!("Error: {reason}")).count(),
        3,
        "{stderr}"
    );
    assert!(stderr.contains(&format!("8 x {reason}")), "{stderr}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Batch errors"), "{stdout}");
    assert!(stdout.contains("8 strings still pending"), "{stdout}");
}

#[test]
fn validate_accepts_translated_nameboxes_including_pivot_baseline() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("names.db");
    let db = Database::open(&project).unwrap();
    let mut entries = Vec::new();
    for (i, escape) in [r"\n", r"\nc", r"\nr", r"\NC"].iter().enumerate() {
        let mut entry = StringEntry::new(
            format!("name-{i}"),
            format!("{escape}<Lydia>Hello"),
            "Map001.json".into(),
        );
        entry.translation = Some(format!("{escape}<Lidia>Hola"));
        entry.status = StringStatus::Translated;
        entry.metadata.insert(
            INJECTION_SOURCE_METADATA_KEY.into(),
            serde_json::json!(format!("{escape}<リディア>こんにちは")),
        );
        entries.push(entry);
    }
    db.save_entries(&entries).unwrap();
    drop(db);

    let assertion = locust()
        .args(["validate", project.to_str().unwrap(), "--json"])
        .assert()
        .success();
    let report: serde_json::Value = serde_json::from_slice(&assertion.get_output().stdout).unwrap();
    assert_eq!(report["count"], 0);
    assert_eq!(report["issues"], serde_json::json!([]));
}
