use locust_core::database::{Database, EntryFilter};
use locust_core::models::{StringEntry, StringStatus};
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

#[test]
fn glossary_add_pipeline_defaults_preserve_exact_hit_and_run_evidence() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config.json");
    std::fs::write(
        &config,
        serde_json::to_vec(&serde_json::json!({
            "providers": { "pipeline-scripted": {
                "api_key": "fixture-key",
                "base_url": "http://127.0.0.1:1",
                "model": "fixture-model"
            }}
        }))
        .unwrap(),
    )
    .unwrap();

    for (source_lang, target_lang, term, source, target) in [
        ("ja", "en", "錬金術師", "錬金術師", "Alchemist"),
        ("en", "es", "Alchemist", "ALCHEMIST", "Alquimista"),
    ] {
        let pair = format!("{source_lang}-{target_lang}");
        let project = dir.path().join(format!("{pair}.db"));
        let db = Database::open(&project).unwrap();
        db.save_entries(&[StringEntry::new(
            "script.rpy#1",
            source,
            "script.rpy".into(),
        )])
        .unwrap();
        drop(db);

        // Match the pipeline's equals-form arguments and omitted new flags.
        locust()
            .args(["glossary", "add"])
            .arg(&project)
            .args([
                format!("--term={term}"),
                format!("--translation={target}"),
                "--lang-pair".into(),
                pair.clone(),
            ])
            .assert()
            .success();
        locust()
            .arg("--config")
            .arg(&config)
            .arg("translate")
            .arg(&project)
            .args([
                "-p",
                "pipeline-scripted",
                "-s",
                source_lang,
                "-t",
                target_lang,
                "--no-memory",
            ])
            .assert()
            .success();

        let db = Database::open(&project).unwrap();
        let glossary = db.get_glossary(&pair).unwrap();
        assert_eq!(glossary.len(), 1);
        assert!(!glossary[0].case_sensitive);
        assert_eq!(glossary[0].context, None);
        let rows = db.get_entries(&EntryFilter::default()).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].translation.as_deref(), Some(target));
        assert_eq!(rows[0].status, StringStatus::Translated);
        assert_eq!(rows[0].provider_used.as_deref(), Some("glossary"));
        let runs = db.get_translation_runs().unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].provider, "pipeline-scripted");
        assert_eq!(runs[0].source_lang, source_lang);
        assert_eq!(runs[0].target_lang, target_lang);
        assert_eq!(runs[0].strings_translated, 1);
    }
}

#[test]
fn glossary_partial_pipeline_pass_keeps_pending_rows_and_run_evidence() {
    let dir = tempfile::tempdir().unwrap();
    for allow_partial in [false, true] {
        let project = dir.path().join(format!("partial-{allow_partial}.db"));
        let db = Database::open(&project).unwrap();
        db.save_entries(&[
            StringEntry::new("exact", "Alchemist", "script.rpy".into()),
            // The mock reverses this into a protected control, which is rejected.
            StringEntry::new("pending", r"n\", "script.rpy".into()),
        ])
        .unwrap();
        drop(db);
        locust()
            .args(["glossary", "add"])
            .arg(&project)
            .args([
                "--term=Alchemist",
                "--translation=Alquimista",
                "--lang-pair",
                "en-es",
            ])
            .assert()
            .success();

        let mut command = locust();
        command.arg("translate").arg(&project).args([
            "-p",
            "mock",
            "-s",
            "en",
            "-t",
            "es",
            "--no-memory",
        ]);
        if allow_partial {
            command.arg("--allow-partial");
        }
        let result = command
            .assert()
            .stdout(predicate::str::contains("1 strings still pending"));
        if allow_partial {
            result.success();
        } else {
            result
                .failure()
                .stderr(predicate::str::contains("Translation incomplete:"));
        }

        let db = Database::open(&project).unwrap();
        let rows = db.get_entries(&EntryFilter::default()).unwrap();
        let exact = rows.iter().find(|row| row.id == "exact").unwrap();
        assert_eq!(exact.translation.as_deref(), Some("Alquimista"));
        assert_eq!(exact.provider_used.as_deref(), Some("glossary"));
        let pending = rows.iter().find(|row| row.id == "pending").unwrap();
        assert_eq!(pending.status, StringStatus::Pending);
        assert_eq!(pending.translation, None);
        assert_eq!(pending.provider_used, None);
        let runs = db.get_translation_runs().unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].provider, "mock");
        assert_eq!(runs[0].source_lang, "en");
        assert_eq!(runs[0].target_lang, "es");
        assert_eq!(runs[0].strings_translated, 1);

        // Fatal worker errors must propagate even when partial passes are allowed.
        locust()
            .arg("translate")
            .arg(&project)
            .args(["-p", "mock", "--allow-partial", "--batch-size", "0"])
            .assert()
            .failure()
            .stderr(predicate::str::contains("must be greater than zero"));
        assert_eq!(db.get_translation_runs().unwrap().len(), 1);
    }
}
