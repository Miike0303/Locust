use std::{path::PathBuf, sync::Arc};

use httpmock::{prelude::*, Mock};
use locust_core::{
    database::Database,
    error::{LocustError, Result},
    glossary::Glossary,
    models::{ProgressEvent, StringEntry, StringStatus},
    translation::{RetryConfig, TranslationManager, TranslationOptions},
};
use locust_providers::deepl::DeepLProvider;
use tokio::sync::mpsc;

fn mock_translations(server: &MockServer) -> [Mock<'_>; 2] {
    [("Hello", "Hola"), ("Goodbye", "Adiós")].map(|(source, translation)| {
        server.mock(|when, then| {
            when.method(POST)
                .path("/v2/translate")
                .header("Authorization", "DeepL-Auth-Key test-key")
                .body_contains(format!(r#""text":["{source}"]"#));
            then.status(200).json_body(serde_json::json!({
                "translations": [{"text": translation, "detected_source_language": "EN"}]
            }));
        })
    })
}

async fn run_deepl_job(
    server: &MockServer,
    free_tier: bool,
    cost_limit_usd: Option<f64>,
    batch_size: usize,
) -> (Arc<Database>, Result<()>, Vec<ProgressEvent>) {
    let db = Arc::new(Database::open_in_memory().unwrap());
    let entries = vec![
        StringEntry::new("one", "Hello", PathBuf::from("game.json")),
        StringEntry::new("two", "Goodbye", PathBuf::from("game.json")),
    ];
    db.save_entries(&entries).unwrap();
    let provider = Arc::new(DeepLProvider::with_base_url(
        "test-key".into(),
        free_tier,
        server.base_url(),
    ));
    let manager =
        TranslationManager::new(provider, db.clone(), Arc::new(Glossary::new(db.clone())))
            .with_retry_config(RetryConfig {
                max_attempts: 1,
                ..Default::default()
            });
    let (tx, mut rx) = mpsc::channel(128);
    let outcome = manager
        .translate_entries(
            entries,
            TranslationOptions {
                source_lang: "en".into(),
                target_lang: "es".into(),
                batch_size,
                max_concurrent: 1,
                cost_limit_usd,
                use_memory: false,
                use_glossary: false,
                ..Default::default()
            },
            tx,
            "deepl-budget".into(),
            Default::default(),
        )
        .await;
    let mut events = Vec::new();
    while let Some(event) = rx.recv().await {
        events.push(event);
    }
    (db, outcome, events)
}

fn assert_saved_translations(db: &Database, expected: [Option<&str>; 2]) {
    for (id, translation) in ["one", "two"].into_iter().zip(expected) {
        let entry = db.get_entry(id).unwrap().unwrap();
        assert_eq!(entry.translation.as_deref(), translation, "{id}");
        assert_eq!(
            entry.status,
            if translation.is_some() {
                StringStatus::Translated
            } else {
                StringStatus::Pending
            },
            "{id}"
        );
    }
}

#[tokio::test]
async fn deepl_free_tier_zero_budget_dispatches_and_records_complete_zero_cost() {
    let server = MockServer::start();
    let mocks = mock_translations(&server);
    let (db, outcome, events) = run_deepl_job(&server, true, Some(0.0), 1).await;
    outcome.unwrap();
    for mock in mocks {
        mock.assert_hits(1);
    }
    assert_saved_translations(&db, [Some("Hola"), Some("Adiós")]);
    let runs = db.get_translation_runs().unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].strings_translated, 2);
    assert_eq!(runs[0].cost_usd, 0.0);
    assert!(runs[0].cost_is_complete);
    assert!(events.iter().any(|event| matches!(
        event,
        ProgressEvent::Completed {
            total_translated: 2,
            total_cost: 0.0,
            cost_is_complete: true,
            ..
        }
    )));
}

#[tokio::test]
async fn deepl_paid_tier_zero_budget_blocks_before_dispatch() {
    let server = MockServer::start();
    let mocks = mock_translations(&server);
    let (db, outcome, _) = run_deepl_job(&server, false, Some(0.0), 1).await;
    assert!(matches!(
        outcome,
        Err(LocustError::CostLimitExceeded { limit: 0.0, .. })
    ));
    for mock in mocks {
        mock.assert_hits(0);
    }
    assert_saved_translations(&db, [None, None]);
}

#[tokio::test]
async fn deepl_paid_tier_keeps_character_cost_arithmetic_with_sufficient_budget() {
    let server = MockServer::start();
    let mocks = mock_translations(&server);
    let expected_cost = ("Hello".len() + "Goodbye".len()) as f64 * 0.00002;
    let (db, outcome, _) = run_deepl_job(&server, false, Some(expected_cost), 1).await;
    outcome.unwrap();
    for mock in mocks {
        mock.assert_hits(1);
    }
    assert_saved_translations(&db, [Some("Hola"), Some("Adiós")]);
    let runs = db.get_translation_runs().unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].strings_translated, 2);
    assert!((runs[0].cost_usd - expected_cost).abs() < 1e-12);
    assert!(runs[0].cost_is_complete);
}

#[tokio::test]
async fn deepl_paid_tier_cost_gate_stops_the_next_batch() {
    let server = MockServer::start();
    let [hello, goodbye] = mock_translations(&server);
    let limit = "Hello".len() as f64 * 0.00002;
    let (db, outcome, _) = run_deepl_job(&server, false, Some(limit), 1).await;
    assert!(matches!(
        outcome,
        Err(LocustError::CostLimitExceeded { .. })
    ));
    hello.assert_hits(1);
    goodbye.assert_hits(0);
    assert_saved_translations(&db, [Some("Hola"), None]);
    let runs = db.get_translation_runs().unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].cost_usd, limit);
    assert!(runs[0].cost_is_complete);
}

#[tokio::test]
async fn deepl_count_mismatches_save_no_results() {
    for translations in [vec!["EXTRA", "Hola", "Adiós"], vec!["Hola"], vec![]] {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(POST)
                .path("/v2/translate")
                .body_contains(r#""text":["Hello","Goodbye"]"#);
            then.status(200).json_body(serde_json::json!({
                "translations": translations.iter().map(|text| {
                    serde_json::json!({"text": text})
                }).collect::<Vec<_>>()
            }));
        });
        let (db, outcome, events) = run_deepl_job(&server, false, None, 2).await;
        outcome.unwrap();
        mock.assert_hits(1);
        assert_saved_translations(&db, [None, None]);
        assert!(!events
            .iter()
            .any(|event| matches!(event, ProgressEvent::StringTranslated { .. })));
        assert!(events.iter().any(|event| matches!(
            event,
            ProgressEvent::BatchFailed { error, .. }
                if error.contains("expected 2")
                    && error.contains(&format!("received {}", translations.len()))
        )));
    }
}

#[tokio::test]
async fn deepl_free_tier_zero_budget_still_surfaces_auth_and_quota_errors() {
    for status in [403, 456] {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(POST)
                .path("/v2/translate")
                .body_contains(r#""text":["Hello","Goodbye"]"#);
            then.status(status).body("auth or quota failure: test-key");
        });
        let (db, outcome, events) = run_deepl_job(&server, true, Some(0.0), 2).await;
        outcome.unwrap();
        mock.assert_hits(1);
        assert_saved_translations(&db, [None, None]);
        assert!(events.iter().any(|event| matches!(
            event,
            ProgressEvent::BatchFailed { error, .. }
                if error.contains(&status.to_string()) && !error.contains("test-key")
        )));
        assert!(db.get_translation_runs().unwrap().is_empty());
        assert!(events.iter().any(|event| matches!(
            event,
            ProgressEvent::Completed {
                total_translated: 0,
                total_cost: 0.0,
                cost_is_complete: true,
                ..
            }
        )));
    }
}
