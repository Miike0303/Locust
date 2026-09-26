use async_trait::async_trait;
use locust_core::{
    database::{Database, TranslationRun},
    error::{LocustError, Result},
    glossary::Glossary,
    models::{ProgressEvent, StringEntry, TranslationRequest, TranslationResult},
    translation::{RetryConfig, TranslationManager, TranslationOptions, TranslationProvider},
};
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[test]
fn legacy_progress_is_not_claimed_free() {
    let event: ProgressEvent = serde_json::from_str(
        r#"{"type":"completed","total_translated":1,"total_cost":0.0,"duration_secs":1.0}"#,
    )
    .unwrap();
    assert!(matches!(
        event,
        ProgressEvent::Completed {
            cost_is_complete: false,
            ..
        }
    ));
}

#[tokio::test]
async fn ledger_roundtrips_unknown_partial_and_known_zero() {
    let temp = tempfile::tempdir().unwrap();
    let db = Database::open(&temp.path().join("project.db")).unwrap();
    for (amount, complete) in [(0.0, false), (0.25, false), (0.0, true)] {
        db.record_translation_run(&TranslationRun {
            cost_usd: amount,
            cost_is_complete: complete,
            ..Default::default()
        })
        .await
        .unwrap();
    }
    let runs = db.get_translation_runs().unwrap();
    assert_eq!(runs.len(), 3);
    assert_eq!((runs[0].cost_usd, runs[0].cost_is_complete), (0.0, false));
    assert_eq!((runs[1].cost_usd, runs[1].cost_is_complete), (0.25, false));
    assert_eq!((runs[2].cost_usd, runs[2].cost_is_complete), (0.0, true));
}

#[test]
fn old_database_preserves_subtotal_but_marks_completeness_unknown() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("old.db");
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch(
        "CREATE TABLE translation_runs (
        id INTEGER PRIMARY KEY, started_at TEXT NOT NULL, duration_secs REAL NOT NULL,
        provider TEXT NOT NULL, source_lang TEXT NOT NULL, target_lang TEXT NOT NULL,
        strings_translated INTEGER NOT NULL, tokens_used INTEGER NOT NULL DEFAULT 0,
        cost_usd REAL NOT NULL DEFAULT 0);
        INSERT INTO translation_runs VALUES (1,'old',1,'grok-sub','en','es',1,10,0.0);",
    )
    .unwrap();
    drop(conn);
    for _ in 0..2 {
        let db = Database::open(&path).unwrap();
        let runs = db.get_translation_runs().unwrap();
        assert_eq!(runs[0].cost_usd, 0.0);
        assert!(!runs[0].cost_is_complete);
    }
}

struct CostProvider {
    calls: AtomicUsize,
    response_cost: Option<f64>,
    invalid_ids: bool,
    fail_first: bool,
    free: bool,
}

#[async_trait]
impl TranslationProvider for CostProvider {
    fn id(&self) -> &str {
        "cost-completeness-test"
    }

    fn name(&self) -> &str {
        "Cost completeness test"
    }

    fn is_free(&self) -> bool {
        self.free
    }

    fn requires_api_key(&self) -> bool {
        false
    }

    async fn translate(&self, requests: &[TranslationRequest]) -> Result<Vec<TranslationResult>> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        if self.fail_first && call == 0 {
            return Err(LocustError::ProviderError("network timeout".into()));
        }
        Ok(requests
            .iter()
            .map(|request| TranslationResult {
                entry_id: if self.invalid_ids {
                    "unexpected".into()
                } else {
                    request.entry_id.clone()
                },
                translation: "Hola".into(),
                detected_source_lang: None,
                provider: self.id().into(),
                tokens_used: Some(10),
                input_tokens: Some(6),
                output_tokens: Some(4),
                cost_usd: self.response_cost,
            })
            .collect())
    }

    async fn estimate_cost(&self, _: usize, _: &str) -> Option<f64> {
        Some(0.1)
    }

    async fn health_check(&self) -> Result<()> {
        Ok(())
    }
}

async fn translate_with_cost_provider(
    provider: Arc<CostProvider>,
    entries: Vec<StringEntry>,
    options: TranslationOptions,
) -> (Arc<Database>, Result<()>) {
    let db = Arc::new(Database::open_in_memory().unwrap());
    db.save_entries(&entries).unwrap();
    let glossary = Arc::new(Glossary::new(db.clone()));
    let (tx, mut rx) = mpsc::channel(32);
    let drain = tokio::spawn(async move { while rx.recv().await.is_some() {} });
    let result = TranslationManager::new(provider, db.clone(), glossary)
        .with_retry_config(RetryConfig {
            max_attempts: 2,
            initial_delay_ms: 1,
            max_delay_ms: 1,
            ..Default::default()
        })
        .translate_entries(
            entries,
            options,
            tx,
            "cost-check".into(),
            CancellationToken::new(),
        )
        .await;
    drain.await.unwrap();
    (db, result)
}

fn entry(id: &str) -> StringEntry {
    StringEntry::new(id, "Hello", PathBuf::from("game.json"))
}

#[tokio::test]
async fn final_batch_checks_actual_cost_and_unknown_cost_after_saving() {
    for (response_cost, expect_exceeded) in [(Some(0.7), true), (None, false)] {
        let provider = Arc::new(CostProvider {
            calls: AtomicUsize::new(0),
            response_cost,
            invalid_ids: false,
            fail_first: false,
            free: false,
        });
        let (db, result) = translate_with_cost_provider(
            provider.clone(),
            vec![entry("one")],
            TranslationOptions {
                cost_limit_usd: Some(0.5),
                use_memory: false,
                use_glossary: false,
                ..Default::default()
            },
        )
        .await;

        if expect_exceeded {
            assert!(matches!(result, Err(LocustError::CostLimitExceeded { .. })));
        } else {
            assert!(
                matches!(result, Err(LocustError::ProviderError(ref message))
                if message.contains("cannot enforce a cost limit"))
            );
        }
        assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            db.get_entry("one").unwrap().unwrap().translation.as_deref(),
            Some("Hola")
        );
        let runs = db.get_translation_runs().unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].cost_is_complete, response_cost.is_some());
        assert!((runs[0].cost_usd - response_cost.unwrap_or(0.0)).abs() < 1e-9);
    }
}

#[tokio::test]
async fn invalid_ids_check_actual_cost_before_any_next_dispatch() {
    let provider = Arc::new(CostProvider {
        calls: AtomicUsize::new(0),
        response_cost: Some(0.7),
        invalid_ids: true,
        fail_first: false,
        free: false,
    });
    let (db, result) = translate_with_cost_provider(
        provider.clone(),
        vec![entry("one"), entry("two")],
        TranslationOptions {
            batch_size: 1,
            cost_limit_usd: Some(0.5),
            use_memory: false,
            use_glossary: false,
            ..Default::default()
        },
    )
    .await;
    assert!(matches!(result, Err(LocustError::CostLimitExceeded { .. })));
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    assert!(db.get_entry("one").unwrap().unwrap().translation.is_none());
    assert_eq!(db.get_translation_runs().unwrap()[0].cost_usd, 0.7);
}

#[tokio::test]
async fn paid_transport_retry_marks_cost_incomplete_and_stops_budgeted_run() {
    let provider = Arc::new(CostProvider {
        calls: AtomicUsize::new(0),
        response_cost: Some(0.1),
        invalid_ids: false,
        fail_first: true,
        free: false,
    });
    let (db, result) = translate_with_cost_provider(
        provider.clone(),
        vec![entry("one"), entry("two")],
        TranslationOptions {
            batch_size: 1,
            cost_limit_usd: Some(1.0),
            use_memory: false,
            use_glossary: false,
            ..Default::default()
        },
    )
    .await;
    assert!(
        matches!(result, Err(LocustError::ProviderError(ref message))
        if message.contains("cannot enforce a cost limit"))
    );
    assert_eq!(provider.calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        db.get_entry("one").unwrap().unwrap().translation.as_deref(),
        Some("Hola")
    );
    assert!(db.get_entry("two").unwrap().unwrap().translation.is_none());
    let run = &db.get_translation_runs().unwrap()[0];
    assert_eq!(run.cost_usd, 0.1);
    assert!(!run.cost_is_complete);
}

#[tokio::test]
async fn free_transport_retry_remains_known_zero() {
    let provider = Arc::new(CostProvider {
        calls: AtomicUsize::new(0),
        response_cost: None,
        invalid_ids: false,
        fail_first: true,
        free: true,
    });
    let (db, result) = translate_with_cost_provider(
        provider.clone(),
        vec![entry("one")],
        TranslationOptions {
            cost_limit_usd: Some(1.0),
            use_memory: false,
            use_glossary: false,
            ..Default::default()
        },
    )
    .await;
    result.unwrap();
    assert_eq!(provider.calls.load(Ordering::SeqCst), 2);
    let run = &db.get_translation_runs().unwrap()[0];
    assert_eq!(run.cost_usd, 0.0);
    assert!(run.cost_is_complete);
}
