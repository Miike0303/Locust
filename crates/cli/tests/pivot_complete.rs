use std::path::{Path, PathBuf};

use locust_core::database::{Database, EntryFilter};
use locust_core::models::{StringEntry, StringStatus, STALE_TRANSLATION_METADATA_KEY};

mod common;
use common::locust;

const OUTPUT_FILES: [&str; 4] = [
    "output.db",
    "output.db-wal",
    "output.db-shm",
    "output.db-journal",
];

fn translated(id: &str, source: &str, translation: &str) -> StringEntry {
    let mut row = StringEntry::new(id, source, "script.rpy".into());
    row.translation = Some(translation.into());
    row.status = StringStatus::Translated;
    row
}

fn fixture(rows: &[StringEntry]) -> (tempfile::TempDir, PathBuf) {
    let root = tempfile::tempdir().unwrap();
    let project = root.path().join("source.db");
    Database::open(&project)
        .unwrap()
        .save_entries(rows)
        .unwrap();
    (root, project)
}

fn snapshot(project: &Path) -> serde_json::Value {
    let db = Database::open(project).unwrap();
    serde_json::to_value(db.get_entries(&EntryFilter::default()).unwrap()).unwrap()
}

fn pivot_command(project: &Path, output: &Path, strict: bool) -> assert_cmd::Command {
    let mut command = locust();
    command.arg("pivot").arg(project).arg("-o").arg(output);
    if strict {
        command.arg("--require-complete");
    }
    command
}

fn assert_refused(rows: &[StringEntry], reason: &str) {
    let (root, project) = fixture(rows);
    let before = snapshot(&project);
    let output = root.path().join("output.db");
    pivot_command(&project, &output, true)
        .assert()
        .failure()
        .stderr(predicates::str::contains(reason));
    for name in OUTPUT_FILES {
        assert!(!root.path().join(name).exists(), "unexpected {name}");
    }
    assert_eq!(snapshot(&project), before, "source rows must be unchanged");
}

#[test]
fn pivot_require_complete_refuses_incomplete_inventory() {
    for translation in [None, Some(""), Some("\u{a0}\u{2003}\u{3000}")] {
        let mut missing = translated("z", "別の行", "unused");
        missing.translation = translation.map(str::to_owned);
        assert_refused(
            &[translated("a", "日本語", "English"), missing],
            "missing or blank translation",
        );
    }
    for status in [StringStatus::Pending, StringStatus::Error] {
        let mut retained = translated("z", "別の行", "Retained English");
        retained.status = status;
        assert_refused(
            &[translated("a", "日本語", "English"), retained],
            "not ready",
        );
    }
}

#[test]
fn pivot_require_complete_preserves_all_ready_rows_and_visible_names() {
    let mut name = translated("a", r"\n<錬金術師>こんにちは", r"\n<Alchemist>Hello");
    name.file_path = "data/Map001.json".into();
    name.metadata
        .insert("format_id".into(), serde_json::json!("rpgmaker-mv"));
    let mut reviewed = translated("b", "こんにちは [name]", "Hello [name]");
    reviewed.status = StringStatus::Reviewed;
    let mut approved = translated("c", "開始", "Start");
    approved.status = StringStatus::Approved;
    let (root, project) = fixture(&[approved, reviewed, name]);
    let before = snapshot(&project);
    let output = root.path().join("output.db");
    pivot_command(&project, &output, true).assert().success();
    let db = Database::open(&output).unwrap();
    let rows = db.get_entries(&EntryFilter::default()).unwrap();
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0].source, r"\n<Alchemist>Hello");
    assert_eq!(
        rows[0].injection_source().unwrap(),
        r"\n<錬金術師>こんにちは"
    );
    assert_eq!(rows[1].source, "Hello [name]");
    assert_eq!(rows[2].source, "Start");
    assert!(rows
        .iter()
        .all(|row| row.translation.is_none() && row.status == StringStatus::Pending));
    assert_eq!(snapshot(&project), before);
}

#[test]
fn pivot_require_complete_keeps_controls_stale_and_empty_preflights() {
    assert_refused(&[], "no translated entries");
    assert_refused(
        &[
            translated("a", "日本語", "English"),
            translated("z", "こんにちは [name]", "Hello"),
        ],
        "changed protected controls",
    );
    let mut stale = translated("z", "新しい原文", "Old English");
    stale.metadata.insert(
        STALE_TRANSLATION_METADATA_KEY.into(),
        serde_json::json!({
            "version": 1,
            "translated_source_sha256": "0".repeat(64),
            "current_source_sha256": stale.source_hash(),
        }),
    );
    assert_refused(
        &[translated("a", "日本語", "English"), stale],
        "stale translation",
    );
}

#[test]
fn pivot_require_complete_preserves_existing_output_and_sidecars() {
    let (root, project) = fixture(&[translated("a", "日本語", "English")]);
    let before = snapshot(&project);
    let output = root.path().join("output.db");
    for name in OUTPUT_FILES {
        std::fs::write(root.path().join(name), b"existing output").unwrap();
    }
    pivot_command(&project, &output, true)
        .assert()
        .failure()
        .stderr(predicates::str::contains("refusing to overwrite"));
    for name in OUTPUT_FILES {
        assert_eq!(
            std::fs::read(root.path().join(name)).unwrap(),
            b"existing output"
        );
    }
    assert_eq!(snapshot(&project), before);
}

#[test]
fn pivot_default_still_allows_partial_and_pending_nonempty_translations() {
    let mut retained = translated("b", "別の行", "Retained English");
    retained.status = StringStatus::Pending;
    let missing = StringEntry::new("c", "未翻訳", "script.rpy".into());
    let blank = translated("d", "空白", "\u{a0}\u{2003}\u{3000}");
    let (root, project) = fixture(&[
        translated("a", "日本語", "English"),
        retained,
        missing,
        blank,
    ]);
    let before = snapshot(&project);
    let output = root.path().join("partial.db");
    pivot_command(&project, &output, false).assert().success();
    let db = Database::open(&output).unwrap();
    let rows = db.get_entries(&EntryFilter::default()).unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].source, "English");
    assert_eq!(rows[1].source, "Retained English");
    assert_eq!(snapshot(&project), before);
}
