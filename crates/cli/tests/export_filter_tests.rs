use std::path::{Path, PathBuf};

use locust_core::config::AppConfig;
use locust_core::database::{Database, EntryFilter};
use locust_core::export::{export_po, export_xliff, import_po, import_xliff};
use locust_core::models::{StringEntry, StringStatus, STALE_TRANSLATION_METADATA_KEY};

mod common;
use common::locust;

const PENDING: (&str, &str, &str) = ("01-pending", "Untranslated", "");
const APPROVED: (&str, &str, &str) = ("02-approved", "Approved source", "Aprobado");
const STALE: (&str, &str, &str) = ("03-stale", "Changed <source>", "Texto anterior & válido");

fn project_fixture() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project.locust.db");
    let db = Database::open(&project).unwrap();
    let pending = StringEntry::new(PENDING.0, PENDING.1, "story.rpy".into());
    let mut approved = StringEntry::new(APPROVED.0, APPROVED.1, "story.rpy".into());
    approved.translation = Some(APPROVED.2.into());
    approved.status = StringStatus::Approved;
    let mut stale = StringEntry::new(STALE.0, "Original source", "story.rpy".into());
    stale.translation = Some(STALE.2.into());
    stale.status = StringStatus::Translated;
    // Deliberately insert out of ID order, with the approved ID between pending IDs.
    db.save_entries(&[stale, approved, pending]).unwrap();
    let stats = db
        .merge_entries_preserving_missing(&[StringEntry::new(STALE.0, STALE.1, "story.rpy".into())])
        .unwrap();
    assert_eq!(stats.stale_source_reset, 1);
    let stale = db.get_entry(STALE.0).unwrap().unwrap();
    assert_eq!(stale.status, StringStatus::Pending);
    assert_eq!(stale.translation.as_deref(), Some(STALE.2));
    assert!(stale.metadata.contains_key(STALE_TRANSLATION_METADATA_KEY));
    (dir, project)
}

fn export_command(project: &Path, format: &str, output: &Path) -> assert_cmd::Command {
    let mut command = locust();
    command
        .arg("export")
        .arg(project)
        .args(["-f", format, "-l", "es", "-o"])
        .arg(output);
    command
}

fn assert_catalog(output: &Path, format: &str, expected: &[(&str, &str, &str)]) {
    let content = std::fs::read_to_string(output).unwrap();
    let rows: Vec<(String, String, String)> = match format {
        "po" => import_po(&content)
            .unwrap()
            .into_iter()
            .map(|row| (row.id.unwrap(), row.source, row.translation))
            .collect(),
        "xliff" => import_xliff(&content)
            .unwrap()
            .into_iter()
            .map(|row| (row.id, row.source, row.target))
            .collect(),
        _ => unreachable!(),
    };
    let rows: Vec<(&str, &str, &str)> = rows
        .iter()
        .map(|(id, source, translation)| (id.as_str(), source.as_str(), translation.as_str()))
        .collect();
    assert_eq!(rows, expected, "{format} catalog rows in ID order");
}

fn pending_only(format: &str) {
    let (dir, project) = project_fixture();
    let output = dir.path().join(format!("pending.{format}"));
    let assertion = export_command(&project, format, &output)
        .args(["--status", "pending"])
        .assert()
        .success();
    assert_catalog(&output, format, &[PENDING, STALE]);
    assertion.stdout(predicates::str::contains("Exported 2 entries to"));
}

#[test]
fn po_pending_only_preserves_stale_text() {
    pending_only("po");
}

#[test]
fn xliff_pending_only_preserves_stale_text() {
    pending_only("xliff");
}

fn repeated_statuses(format: &str) {
    let (dir, project) = project_fixture();
    for states in [
        vec!["pending", "approved"],
        vec!["approved", "pending", "approved", "pending"],
        vec!["error", "reviewed", "translated", "approved", "pending"],
    ] {
        let output = dir.path().join(format!("union.{format}"));
        let mut command = export_command(&project, format, &output);
        command.arg("--overwrite");
        for state in states {
            command.args(["--status", state]);
        }
        command
            .assert()
            .success()
            .stdout(predicates::str::contains("Exported 3 entries to"));
        assert_catalog(&output, format, &[PENDING, APPROVED, STALE]);
    }
}

#[test]
fn po_repeated_statuses_form_an_ordered_union() {
    repeated_statuses("po");
}

#[test]
fn xliff_repeated_statuses_form_an_ordered_union() {
    repeated_statuses("xliff");
}

fn omitted_status(format: &str) {
    let (dir, project) = project_fixture();
    // The exact pre-filter cmd_export path, using this same database and serializers.
    let db = Database::open(&project).unwrap();
    let entries = db.get_entries(&EntryFilter::default()).unwrap();
    let source_lang = db
        .resolve_export_source_lang("es", &AppConfig::default().default_source_lang)
        .unwrap();
    let baseline = match format {
        "po" => export_po(&entries, &source_lang, "es"),
        "xliff" => export_xliff(&entries, &source_lang, "es"),
        _ => unreachable!(),
    };
    drop(db);
    let output = dir.path().join(format!("all.{format}"));
    export_command(&project, format, &output)
        .assert()
        .success()
        .stdout(predicates::str::contains("Exported 3 entries to"));
    assert_catalog(&output, format, &[PENDING, APPROVED, STALE]);
    assert_eq!(std::fs::read(&output).unwrap(), baseline.as_bytes());
}

#[test]
fn po_omitted_status_matches_pre_change_bytes() {
    omitted_status("po");
}

#[test]
fn xliff_omitted_status_matches_pre_change_bytes() {
    omitted_status("xliff");
}

#[test]
fn invalid_status_is_rejected_before_writing() {
    let (dir, project) = project_fixture();
    for format in ["po", "xliff"] {
        for invalid in ["bogus", "Pending", "pending,approved", " pending", ""] {
            let output = dir.path().join(format!("invalid.{format}"));
            let mut command = export_command(&project, format, &output);
            command.args(["--status", "pending", "--status", invalid]);
            command
                .assert()
                .failure()
                .stderr(predicates::str::contains("invalid export status"))
                .stderr(predicates::str::contains(
                    "pending, translated, reviewed, approved, error",
                ));
            assert!(!output.exists());
        }
        let output = dir.path().join(format!("existing.{format}"));
        std::fs::write(&output, b"existing catalog").unwrap();
        export_command(&project, format, &output)
            .args(["--overwrite", "--status", "bogus"])
            .assert()
            .failure()
            .stderr(predicates::str::contains("bogus"));
        assert_eq!(std::fs::read(&output).unwrap(), b"existing catalog");
    }
}

#[test]
fn zero_matches_refuse_to_write_or_replace_a_catalog() {
    let (dir, project) = project_fixture();
    for format in ["po", "xliff"] {
        for state in ["translated", "reviewed", "error"] {
            let output = dir.path().join(format!("empty.{format}"));
            export_command(&project, format, &output)
                .args(["--status", state])
                .assert()
                .failure()
                .stderr(predicates::str::contains("no entries to export"));
            assert!(!output.exists());
        }
        let output = dir.path().join(format!("existing.{format}"));
        std::fs::write(&output, b"existing catalog").unwrap();
        export_command(&project, format, &output)
            .args(["--overwrite", "--status", "reviewed"])
            .assert()
            .failure()
            .stderr(predicates::str::contains("no entries to export"));
        assert_eq!(std::fs::read(&output).unwrap(), b"existing catalog");
    }
}

#[test]
fn empty_project_uses_the_same_refusal() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("empty.db");
    drop(Database::open(&project).unwrap());
    for format in ["po", "xliff"] {
        let output = dir.path().join(format!("empty.{format}"));
        export_command(&project, format, &output)
            .assert()
            .failure()
            .stderr(predicates::str::contains("no entries to export"));
        assert!(!output.exists());
    }
}

#[test]
fn destination_checks_precede_status_validation() {
    let (dir, project) = project_fixture();
    let before = std::fs::read(&project).unwrap();
    for format in ["po", "xliff"] {
        export_command(&project, format, &project)
            .args(["--overwrite", "--status", "bogus"])
            .assert()
            .failure()
            .stderr(predicates::str::contains(
                "the export destination is the project database itself",
            ));
        let output = dir.path().join(format!("existing.{format}"));
        std::fs::write(&output, b"existing catalog").unwrap();
        export_command(&project, format, &output)
            .args(["--status", "bogus"])
            .assert()
            .failure()
            .stderr(predicates::str::contains(
                "already exists; pass --overwrite",
            ));
        assert_eq!(std::fs::read(&output).unwrap(), b"existing catalog");
    }
    assert_eq!(std::fs::read(&project).unwrap(), before);
}
