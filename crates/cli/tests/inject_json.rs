//! The CLI JSON seam serializes core reports, not a new success-only summary.
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;
use std::time::Duration;

use assert_cmd::Command;
use locust_core::database::{Database, EntryFilter};
use locust_core::extraction::DirectInjectReport;
use serde_json::{json, Value};

struct Fixture {
    root: tempfile::TempDir,
    game: PathBuf,
    project: PathBuf,
    asset: PathBuf,
    original: Vec<u8>,
}

impl Fixture {
    fn new(engine: &str, translate: bool) -> Self {
        let root = tempfile::tempdir().unwrap();
        let game = root.path().join("Game");
        let (relative, contents) = match engine {
            "renpy" => (
                "game/script.rpy",
                "label start:\n    e \"Hello there!\"\n    e \"See you soon.\"\n",
            ),
            "rpgmaker-mv" => (
                "data/System.json",
                r#"{"gameTitle":"Hello there!","terms":{"basic":["See you soon."]}}"#,
            ),
            _ => unreachable!(),
        };
        let asset = game.join(relative);
        fs::create_dir_all(asset.parent().unwrap()).unwrap();
        fs::write(&asset, contents).unwrap();
        let profile = root.path().join("profile");
        fs::create_dir(&profile).unwrap();
        fs::write(profile.join("config.json"), "{}").unwrap();
        let project = root.path().join("project.locust.db");
        let fixture = Self {
            root,
            game,
            project,
            asset,
            original: contents.as_bytes().to_vec(),
        };
        fixture
            .cli()
            .arg("extract")
            .arg(&fixture.game)
            .args(["--format", engine, "-o"])
            .arg(&fixture.project)
            .assert()
            .success();
        if translate {
            fixture
                .cli()
                .arg("translate")
                .arg(&fixture.project)
                .args(["-p", "mock", "-s", "en", "-t", "es", "--no-memory"])
                .assert()
                .success();
        }
        let db = Database::open(&fixture.project).unwrap();
        let entries = db.get_entries(&EntryFilter::default()).unwrap();
        assert_eq!(entries.len(), 2);
        if translate {
            assert!(entries
                .iter()
                .all(|row| row.provider_used.as_deref() == Some("mock")));
        }
        fixture
    }

    fn cli(&self) -> Command {
        let mut command = Command::cargo_bin("locust").unwrap();
        command
            .current_dir(self.root.path())
            .env("LOCUST_DATA_DIR", self.root.path().join("profile"))
            .env(
                "LOCUST_CONFIG",
                self.root.path().join("profile/config.json"),
            )
            .env("LOCUST_BACKUP_ROOT", self.root.path().join("backups"))
            .timeout(Duration::from_secs(30));
        command
    }

    fn inject(&self) -> Command {
        let mut command = self.cli();
        command
            .arg("inject")
            .arg(&self.game)
            .arg("-P")
            .arg(&self.project);
        command
    }

    fn json(&self, args: &[&str]) -> Value {
        let output = self.inject().args(args).arg("--json").output().unwrap();
        parse_report(&output, true)
    }

    fn assert_recording(&self, root: &Path, paths: &[PathBuf]) {
        let db = Database::open(&self.project).unwrap();
        let record = db.get_injection(Some("es")).unwrap().unwrap();
        assert_eq!(
            record.root.canonicalize().unwrap(),
            root.canonicalize().unwrap()
        );
        assert_eq!(record.files.len(), paths.len());
        for path in paths {
            assert!(path.is_file());
        }
        let backup = record.pristine_backup.unwrap();
        assert!(self.root.path().join("backups").join(backup.id).is_dir());
    }
}

fn parse_report(output: &Output, success: bool) -> Value {
    assert_eq!(
        output.status.success(),
        success,
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    // Parse ALL stdout: prefixes, tables, progress, logs, or a second object fail.
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "{error}: stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

fn assert_counts(report: &Value, files: usize, written: usize, skipped: usize) {
    assert_eq!(report["files_modified"], files);
    assert_eq!(report["strings_written"], written);
    assert_eq!(report["strings_skipped"], skipped);
    assert!(report["warnings"].is_array());
    assert!(report["skip_reasons"].is_object());
}

fn written_paths(report: &Value) -> Vec<PathBuf> {
    serde_json::from_value(report["files_written"].clone()).unwrap()
}

fn assert_paths(report: &Value, expected: &[PathBuf]) {
    let canonical = |paths: &[PathBuf]| -> BTreeSet<PathBuf> {
        paths
            .iter()
            .map(|path| path.canonicalize().unwrap())
            .collect()
    };
    assert_eq!(canonical(&written_paths(report)), canonical(expected));
}

#[test]
fn direct_json_records_then_keeps_the_original_backup_on_unchanged_runs() {
    for engine in ["renpy", "rpgmaker-mv"] {
        let fixture = Fixture::new(engine, true);
        let first = fixture.json(&["--direct", "-l", "es"]);
        // Deserializes as the existing report, with no envelope or renamed fields.
        let typed: DirectInjectReport = serde_json::from_value(first.clone()).unwrap();
        assert_eq!(serde_json::to_value(typed).unwrap(), first);
        assert_eq!(first["mode"], "direct");
        assert_eq!(first["languages_processed"], json!(["es"]));
        assert_eq!(first["languages_failed"], json!([]));
        assert_counts(&first["reports"]["es"], 1, 2, 0);
        assert_eq!(first["strings_written"], 2);
        assert_eq!(
            first["outcomes"],
            json!([["es", {"Recorded": {"files": 1}}]])
        );
        assert_paths(&first, std::slice::from_ref(&fixture.asset));
        assert_eq!(
            first["files_written"],
            first["reports"]["es"]["files_written"]
        );
        fixture.assert_recording(&fixture.game, &written_paths(&first));
        let backup = PathBuf::from(first["backup_path"].as_str().unwrap());
        assert_eq!(first["backup_id"], first["pristine_backup_id"]);
        assert_eq!(
            fs::read(
                backup
                    .join("payload")
                    .join(fixture.asset.strip_prefix(&fixture.game).unwrap())
            )
            .unwrap(),
            fixture.original
        );
        let injected = fs::read(&fixture.asset).unwrap();
        assert_ne!(injected, fixture.original);

        let second = fixture.json(&["--direct", "-l", "es"]);
        // MV replays from pristine bytes: its plugin counts staged strings,
        // while the transaction reports zero physically modified files. Ren'Py
        // retargets its literals and classifies them as unchanged instead.
        let (written, skipped, reasons) = if engine == "renpy" {
            (0, 2, json!({"unchanged": 2}))
        } else {
            (2, 0, json!({}))
        };
        assert_counts(&second["reports"]["es"], 0, written, skipped);
        assert_eq!(second["reports"]["es"]["skip_reasons"], reasons);
        assert_eq!(second["files_written"], json!([]));
        assert_eq!(second["backup_id"], first["backup_id"]);
        assert_eq!(second["backup_path"], first["backup_path"]);
        assert_eq!(second["pristine_backup_id"], first["pristine_backup_id"]);
        assert!(second["outcomes"][0][1]["KeptPrevious"]["recorded_at"].is_string());
        assert!(second["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|warning| warning
                .as_str()
                .unwrap()
                .contains("redundant new backup removed")));
        assert_eq!(fs::read(&fixture.asset).unwrap(), injected);
        assert_eq!(
            fs::read_dir(fixture.root.path().join("backups"))
                .unwrap()
                .count(),
            1
        );
    }
}

#[test]
fn add_json_reports_generated_paths_roots_and_committed_outcomes() {
    for engine in ["renpy", "rpgmaker-mv"] {
        let fixture = Fixture::new(engine, true);
        let report = fixture.json(&["-m", "add", "-l", "es"]);
        let paths = if engine == "renpy" {
            vec![
                fixture.game.join("game/tl/es/script.rpy"),
                fixture.game.join("game/locust_languages.rpy"),
            ]
        } else {
            vec![fixture.game.join("data/Languages/es.json")]
        };
        assert_eq!(report["mode"], "add");
        assert_eq!(report["languages_processed"], json!(["es"]));
        assert_eq!(report["languages_failed"], json!([]));
        assert_counts(&report["reports"]["es"], paths.len(), 2, 0);
        assert_paths(&report["reports"]["es"], &paths);
        assert_eq!(
            report["recording_outcomes"]["es"],
            json!({"Recorded": {"files": paths.len()}})
        );
        let root = PathBuf::from(report["injected_roots"]["es"].as_str().unwrap());
        fixture.assert_recording(&root, &paths);
        assert_eq!(fs::read(&fixture.asset).unwrap(), fixture.original);
        assert!(fixture
            .root
            .path()
            .join("backups")
            .join(report["backup_id"].as_str().unwrap())
            .is_dir());

        let generated: Vec<_> = paths.iter().map(|path| fs::read(path).unwrap()).collect();
        let repeated = fixture.json(&["-m", "add", "-l", "es"]);
        assert_counts(&repeated["reports"]["es"], 0, 2, 0);
        assert_eq!(repeated["reports"]["es"]["files_written"], json!([]));
        assert!(repeated["recording_outcomes"]["es"]["KeptPrevious"]["recorded_at"].is_string());
        assert_eq!(repeated["injected_roots"], report["injected_roots"]);
        // Add keeps its new pre-write backup, unlike Direct's duplicate cleanup.
        assert_ne!(repeated["backup_id"], report["backup_id"]);
        fixture.assert_recording(&root, &paths);
        for (path, bytes) in paths.iter().zip(generated) {
            assert_eq!(fs::read(path).unwrap(), bytes);
        }
        assert_eq!(
            fs::read_dir(fixture.root.path().join("backups"))
                .unwrap()
                .count(),
            2
        );
    }
}

#[test]
fn replace_json_reports_the_copy_and_failure_still_exits_nonzero() {
    let fixture = Fixture::new("rpgmaker-mv", true);
    let out = fixture.root.path().join("out");
    let run = || {
        fixture
            .inject()
            .args(["-m", "replace", "-l", "es", "--json", "-o"])
            .arg(&out)
            .output()
            .unwrap()
    };
    let report = parse_report(&run(), true);
    let copy = out.join("Game-es");
    assert_eq!(report["mode"], "replace");
    assert_counts(&report["reports"]["es"], 1, 2, 0);
    assert_paths(&report["reports"]["es"], &[copy.join("data/System.json")]);
    let root = PathBuf::from(report["injected_roots"]["es"].as_str().unwrap());
    assert_eq!(root.canonicalize().unwrap(), copy.canonicalize().unwrap());
    fixture.assert_recording(&root, &written_paths(&report["reports"]["es"]));
    assert_eq!(
        report["recording_outcomes"]["es"],
        json!({"Recorded": {"files": 1}})
    );
    let before = fs::read(copy.join("data/System.json")).unwrap();
    let output = run(); // The occupied destination must not be overwritten.
    let failed = parse_report(&output, false);
    assert_eq!(failed["languages_processed"], json!([]));
    assert!(failed["reports"].as_object().unwrap().is_empty());
    assert_eq!(failed["languages_failed"][0][0], "es");
    assert!(failed["languages_failed"][0][1]
        .as_str()
        .unwrap()
        .contains("already exists"));
    assert!(String::from_utf8_lossy(&output.stderr).contains("injection failed"));
    assert_eq!(fs::read(copy.join("data/System.json")).unwrap(), before);
    assert_eq!(fs::read(&fixture.asset).unwrap(), fixture.original);
}

#[test]
fn skipped_direct_rows_report_nothing_recorded_without_retaining_a_duplicate() {
    let fixture = Fixture::new("rpgmaker-mv", true);
    let changed = br#"{"gameTitle":"Changed title","terms":{"basic":["Changed term"]}}"#;
    fs::write(&fixture.asset, changed).unwrap();
    let report = fixture.json(&["--direct", "-l", "es"]);
    assert_counts(&report["reports"]["es"], 0, 0, 2);
    assert_eq!(
        report["reports"]["es"]["skip_reasons"],
        json!({"source_changed": 2})
    );
    assert_eq!(report["outcomes"], json!([["es", "NothingRecorded"]]));
    assert_eq!(report["backup_id"], "");
    assert!(report["backup_path"].is_null() && report["pristine_backup_id"].is_null());
    assert_eq!(report["files_written"], json!([]));
    assert!(!report["warnings"].as_array().unwrap().is_empty());
    assert!(Database::open(&fixture.project)
        .unwrap()
        .get_injection(Some("es"))
        .unwrap()
        .is_none());
    assert_eq!(fs::read(&fixture.asset).unwrap(), changed);
}

#[test]
fn add_json_keeps_unclassified_skips_and_sends_preflight_logs_to_stderr() {
    let fixture = Fixture::new("rpgmaker-mv", false);
    {
        let db = Database::open(&fixture.project).unwrap();
        let mut entries = db.get_entries(&EntryFilter::default()).unwrap();
        // Characterize a binary-budget warning on this synthetic row. The Add
        // plugin itself does not enforce binary slots; do not invent that policy.
        entries[0].translation = Some("A deliberately oversized synthetic translation".into());
        entries[0]
            .metadata
            .insert("binary_slot".into(), json!("utf8"));
        db.save_entries(&entries).unwrap();
    }
    let output = fixture
        .inject()
        .args(["-m", "add", "-l", "es", "--json", "--verbose"])
        .output()
        .unwrap();
    let report = parse_report(&output, true);
    assert_counts(&report["reports"]["es"], 1, 1, 1);
    assert_eq!(
        report["reports"]["es"]["skip_reasons"],
        json!({"unclassified": 1})
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Warning: 1 translation(s)"), "{stderr}");
    assert!(
        stderr.contains("translations exceed binary inject slot length"),
        "{stderr}"
    );
}

#[test]
fn human_output_remains_the_default_for_direct_and_add() {
    for args in [vec!["--direct", "-l", "es"], vec!["-m", "add", "-l", "es"]] {
        let fixture = Fixture::new("renpy", true);
        let output = fixture.inject().args(&args).output().unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("Creating backup"), "{stdout}");
        assert!(
            stdout.contains(if args[0] == "--direct" {
                "Strings written"
            } else {
                "Languages processed"
            }),
            "{stdout}"
        );
        assert!(serde_json::from_slice::<Value>(&output.stdout).is_err());
    }
}

#[test]
fn invalid_languages_and_foreign_targets_are_refused_before_game_writes() {
    let fixture = Fixture::new("renpy", true);
    for args in [
        vec!["-m", "add"],
        vec!["--direct", "-l", "es", "fr"],
        vec!["-m", "add", "-l", "es", "fr"],
        vec!["-m", "invalid", "-l", "es"],
    ] {
        let output = fixture.inject().args(args).arg("--json").output().unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(!output.stderr.is_empty());
    }
    let other = fixture.root.path().join("Other");
    fs::create_dir_all(other.join("game")).unwrap();
    fs::write(other.join("game/script.rpy"), &fixture.original).unwrap();
    let output = fixture
        .cli()
        .arg("inject")
        .arg(&other)
        .arg("-P")
        .arg(&fixture.project)
        .args(["--direct", "-l", "es", "--json"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("outside the selected game"));
    assert_eq!(
        fs::read(other.join("game/script.rpy")).unwrap(),
        fixture.original
    );
    assert_eq!(fs::read(&fixture.asset).unwrap(), fixture.original);
    assert_eq!(
        fs::read_dir(fixture.root.path().join("backups"))
            .unwrap()
            .count(),
        0
    );
    assert!(Database::open(&fixture.project)
        .unwrap()
        .list_recorded_langs()
        .unwrap()
        .is_empty());
}
