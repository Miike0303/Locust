use std::path::PathBuf;
use std::process::{Command, Output};

use locust_core::database::{Database, TranslationRun};
use locust_core::export;
use locust_core::models::{StringEntry, StringStatus};
use serde_json::{json, Value};

struct Fixture {
    directory: tempfile::TempDir,
    project: PathBuf,
    catalog: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let project = directory.path().join("project.db");
        let catalog = directory.path().join("catalog");
        Database::open(&project)
            .unwrap()
            .save_entries(&[StringEntry::new("id", "Source", "story.rpy".into())])
            .unwrap();
        Self {
            directory,
            project,
            catalog,
        }
    }

    fn record_pair(&self, source: &str, target: &str) {
        let db = Database::open(&self.project).unwrap();
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(db.record_translation_run(&TranslationRun {
                source_lang: source.into(),
                target_lang: target.into(),
                ..Default::default()
            }))
            .unwrap();
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_locust"));
        command
            .env("LOCUST_DATA_DIR", self.directory.path().join("profile"))
            .env("LOCUST_BACKUP_ROOT", self.directory.path().join("backups"));
        command
    }

    fn run(&self, format: &str, target: &str, flags: &[&str]) -> Output {
        self.command()
            .arg("import")
            .arg(&self.project)
            .args(["-f", format, "--source-lang", "en", "-l", target, "-i"])
            .arg(&self.catalog)
            .args(flags)
            .output()
            .unwrap()
    }

    fn snapshot(&self) -> Value {
        let db = Database::open(&self.project).unwrap();
        json!({"rows": db.get_entries(&Default::default()).unwrap(),
            "stats": db.get_stats().unwrap(), "history": db.get_string_history("id", 50).unwrap(),
            "issues": db.get_validation_issues(None).unwrap(), "memory": db.memory_count().unwrap()})
    }

    fn write(&self, format: &str, source: &str, target: &str) -> String {
        let mut row = StringEntry::new("id", "Source", "story.rpy".into());
        row.translation = Some("Imported".into());
        let content = match format {
            "po" => export::export_po(&[row], source, target),
            "xliff" => export::export_xliff(&[row], source, target),
            "tmx" => export::export_tmx(&[row], source, target).unwrap(),
            _ => unreachable!(),
        };
        std::fs::write(&self.catalog, &content).unwrap();
        content
    }
}

fn assert_cli_export_round_trip_without_confirmation(format: &str) {
    let source = Fixture::new();
    source.record_pair("ja", "en");
    let mut original = StringEntry::new("id", "日本語 & <原文>", "story.rpy".into());
    original.translation = Some("English & <target>".into());
    original.status = StringStatus::Reviewed;
    Database::open(&source.project)
        .unwrap()
        .save_entries(&[original.clone()])
        .unwrap();
    let source_before = source.snapshot();
    let mut command = source.command();
    command
        .arg("export")
        .arg(&source.project)
        .args(["-f", format, "-l", "en", "-o"])
        .arg(&source.catalog);
    if format == "tmx" {
        command.args(["--source-lang", "ja"]);
    }
    let output = command.output().unwrap();
    assert!(output.status.success(), "{format}: {output:?}");
    assert_eq!(source.snapshot(), source_before);
    let content = std::fs::read_to_string(&source.catalog).unwrap();
    match format {
        "po" => assert!(content.contains("\"Language: en\\n\"")),
        "xliff" => {
            assert!(content.contains("source-language=\"ja\" target-language=\"en\""));
        }
        "tmx" => {
            assert!(content.contains("srclang=\"ja\""));
            assert!(content.contains("xml:lang=\"en\""));
        }
        _ => unreachable!(),
    }

    let destination = Fixture::new();
    let mut empty = original.clone();
    empty.translation = None;
    empty.status = StringStatus::Pending;
    Database::open(&destination.project)
        .unwrap()
        .save_entries(&[empty])
        .unwrap();
    std::fs::write(&destination.catalog, content).unwrap();
    let before = destination.snapshot();
    for dry_run in [true, false] {
        // Match the JA->EN pipeline: XLIFF carries the source declaration;
        // only TMX requires --source-lang. No undeclared-language flag is sent.
        let mut command = destination.command();
        command
            .arg("import")
            .arg(&destination.project)
            .args(["-f", format, "-l", "en", "-i"])
            .arg(&destination.catalog)
            .arg("--keep-existing");
        if format == "tmx" {
            command.args(["--source-lang", "ja"]);
        }
        if dry_run {
            command.arg("--dry-run");
        }
        let output = command.output().unwrap();
        assert!(output.status.success(), "{format}: {output:?}");
        assert!(output.stderr.is_empty(), "{format}: {output:?}");
        assert!(
            String::from_utf8_lossy(&output.stdout).contains(if dry_run {
                "Would fill 1"
            } else {
                "Imported 1"
            })
        );
        if dry_run {
            assert_eq!(destination.snapshot(), before);
        }
    }
    let saved = Database::open(&destination.project)
        .unwrap()
        .get_entry(&original.id)
        .unwrap()
        .unwrap();
    assert_eq!(saved.id, original.id);
    assert_eq!(saved.source, original.source);
    assert_eq!(saved.translation, original.translation);
    assert_eq!(saved.status, StringStatus::Translated);
    assert_eq!(saved.provider_used.as_deref(), Some("import"));
}

#[test]
fn cli_po_export_round_trips_without_undeclared_language_confirmation() {
    assert_cli_export_round_trip_without_confirmation("po");
}

#[test]
fn cli_xliff_export_round_trips_without_undeclared_language_confirmation() {
    assert_cli_export_round_trip_without_confirmation("xliff");
}

#[test]
fn cli_tmx_export_round_trips_without_undeclared_language_confirmation() {
    assert_cli_export_round_trip_without_confirmation("tmx");
}

#[test]
fn cli_refuses_wrong_targets_and_reversed_pairs_even_with_confirmation() {
    for format in ["po", "xliff", "tmx"] {
        let fixture = Fixture::new();
        fixture.record_pair("en", "fr");
        fixture.write(format, "en", "es");
        let before = fixture.snapshot();
        for flags in [
            vec![],
            vec!["--dry-run"],
            vec!["--allow-undeclared-language"],
        ] {
            for target in ["es", "fr"] {
                let output = fixture.run(format, target, &flags);
                assert!(!output.status.success(), "{format}: {output:?}");
                assert!(String::from_utf8_lossy(&output.stderr).contains("does not match project"));
                assert_eq!(fixture.snapshot(), before);
            }
        }
    }
    for format in ["xliff", "tmx"] {
        let fixture = Fixture::new();
        fixture.record_pair("en", "es");
        fixture.write(format, "es", "en");
        let before = fixture.snapshot();
        let output = fixture.run(format, "es", &["--allow-undeclared-language"]);
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("source language 'es'"));
        assert_eq!(fixture.snapshot(), before);
    }
}

#[test]
fn cli_undeclared_languages_preview_without_writes_and_apply_only_with_confirmation() {
    for format in ["po", "xliff", "tmx"] {
        let fixture = Fixture::new();
        fixture.record_pair("en", "es");
        let content = fixture
            .write(format, "en", "es")
            .replace("\"Language: es\\n\"", "")
            .replace("source-language=\"en\" target-language=\"es\"", "")
            .replace("srclang=\"en\"", "");
        std::fs::write(&fixture.catalog, content).unwrap();
        let before = fixture.snapshot();
        let output = fixture.run(format, "es", &["--dry-run"]);
        assert!(output.status.success(), "{format}: {output:?}");
        assert!(String::from_utf8_lossy(&output.stderr).contains("language not declared"));
        assert!(String::from_utf8_lossy(&output.stdout).contains("Would fill 1"));
        assert_eq!(fixture.snapshot(), before);
        let output = fixture.run(format, "es", &[]);
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("--allow-undeclared-language"));
        assert_eq!(fixture.snapshot(), before);
        let output = fixture.run(format, "es", &["--allow-undeclared-language"]);
        assert!(output.status.success(), "{format}: {output:?}");
        assert!(String::from_utf8_lossy(&output.stdout).contains("Imported 1"));
    }
}

#[test]
fn cli_regional_language_policy_matches_http_and_native() {
    for format in ["po", "xliff", "tmx"] {
        for target in ["es", "ES-mx", "es-ES", "es_ES"] {
            let fixture = Fixture::new();
            fixture.record_pair("en", "es-MX");
            let content = fixture
                .write(
                    format,
                    "en",
                    if target == "es_ES" { "es-ES" } else { target },
                )
                .replace("es-ES", target);
            std::fs::write(&fixture.catalog, content).unwrap();
            let output = fixture.run(format, "es", &[]);
            assert!(output.status.success(), "{format}/{target}: {output:?}");
            assert_eq!(
                String::from_utf8_lossy(&output.stderr).contains("Warning: region"),
                ["es-ES", "es_ES"].contains(&target)
            );
            assert!(String::from_utf8_lossy(&output.stdout).contains("Imported 1"));
        }
    }
}

#[test]
fn cli_pivot_import_uses_semantic_run_after_reopen() {
    let mut fixture = Fixture::new();
    let db = Database::open(&fixture.project).unwrap();
    let mut row = StringEntry::new("id", "日本語", "story.rpy".into());
    row.translation = Some("Source".into());
    row.status = StringStatus::Translated;
    db.save_entries(&[row]).unwrap();
    db.set_project_metadata("source_lang", &json!("ja"))
        .unwrap();
    let pivot = fixture.directory.path().join("pivot.db");
    db.pivot_to(&pivot).unwrap();
    fixture.project = pivot;
    fixture.record_pair("en", "es");
    fixture.write("tmx", "en", "es");
    let output = fixture.run("tmx", "es", &[]);
    assert!(output.status.success(), "{output:?}");
    let pivot = Database::open(&fixture.project).unwrap();
    let row = pivot.get_entry("id").unwrap().unwrap();
    assert_eq!(row.translation.as_deref(), Some("Imported"));
    assert_eq!(row.injection_source().unwrap(), "日本語");
}
