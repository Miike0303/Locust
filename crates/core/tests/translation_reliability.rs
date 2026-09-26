use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use std::time::Duration;

use async_trait::async_trait;
use locust_core::{
    database::{Database, EntryFilter},
    error::Result,
    glossary::Glossary,
    models::{StringEntry, StringStatus, TranslationRequest, TranslationResult},
    translation::{
        with_retry, RetryConfig, TranslationManager, TranslationOptions, TranslationProvider,
    },
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

struct Provider {
    calls: AtomicUsize,
    drop_placeholder: bool,
}

#[async_trait]
impl TranslationProvider for Provider {
    fn id(&self) -> &str {
        "reliability-test"
    }
    fn name(&self) -> &str {
        "Test"
    }
    fn is_free(&self) -> bool {
        false
    }
    fn requires_api_key(&self) -> bool {
        false
    }
    async fn health_check(&self) -> Result<()> {
        Ok(())
    }
    async fn estimate_cost(&self, _: usize, _: &str) -> Option<f64> {
        None
    }
    async fn translate(&self, requests: &[TranslationRequest]) -> Result<Vec<TranslationResult>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(requests
            .iter()
            .map(|r| TranslationResult {
                entry_id: r.entry_id.clone(),
                translation: if self.drop_placeholder {
                    "Hola".into()
                } else {
                    r.source.replace("Hello", "Hola")
                },
                detected_source_lang: None,
                provider: self.id().into(),
                tokens_used: Some(10),
                input_tokens: Some(6),
                output_tokens: Some(4),
                cost_usd: None,
            })
            .collect())
    }
}

fn entry(id: &str, source: &str) -> StringEntry {
    StringEntry::new(id, source, PathBuf::from("dialogue.json"))
}

async fn run(
    db: Arc<Database>,
    provider: Arc<dyn TranslationProvider>,
    entries: Vec<StringEntry>,
    opts: TranslationOptions,
) -> Result<()> {
    let glossary = Arc::new(Glossary::new(db.clone()));
    let (tx, mut rx) = mpsc::channel(100);
    let consumer = tokio::spawn(async move { while rx.recv().await.is_some() {} });
    let result = TranslationManager::new(provider, db, glossary)
        .translate_entries(
            entries,
            opts,
            tx,
            "regression".into(),
            CancellationToken::new(),
        )
        .await;
    consumer.await.unwrap();
    result
}

#[tokio::test(start_paused = true)]
async fn deadline_interrupts_a_provider_that_never_returns() {
    let retry = RetryConfig {
        overall_deadline: Some(Duration::from_secs(2)),
        ..Default::default()
    };
    let result: Result<()> = with_retry(&retry, std::future::pending).await;
    assert!(result.unwrap_err().to_string().contains("timeout"));
}

#[tokio::test]
async fn cancellation_interrupts_a_provider_that_never_returns() {
    let cancel = CancellationToken::new();
    let retry = RetryConfig {
        cancel: Some(cancel.clone()),
        overall_deadline: None,
        ..Default::default()
    };
    let started = Arc::new(tokio::sync::Notify::new());
    let signal = started.clone();
    let task = tokio::spawn(async move {
        with_retry(&retry, || {
            signal.notify_one();
            std::future::pending::<Result<()>>()
        })
        .await
    });
    started.notified().await;
    cancel.cancel();
    let result = tokio::time::timeout(Duration::from_secs(1), task)
        .await
        .unwrap()
        .unwrap();
    assert!(result.unwrap_err().to_string().contains("cancelled"));
}

#[tokio::test]
async fn memory_reuses_original_source_with_placeholders_without_another_api_call() {
    let db = Arc::new(Database::open_in_memory().unwrap());
    let provider = Arc::new(Provider {
        calls: AtomicUsize::new(0),
        drop_placeholder: false,
    });
    let first = entry("first", "Hello [name]");
    db.save_entries(std::slice::from_ref(&first)).unwrap();
    let opts = TranslationOptions {
        source_lang: "en".into(),
        target_lang: "es".into(),
        ..Default::default()
    };
    run(db.clone(), provider.clone(), vec![first], opts.clone())
        .await
        .unwrap();
    let second = entry("second", "Hello [name]");
    db.save_entries(std::slice::from_ref(&second)).unwrap();
    run(db.clone(), provider.clone(), vec![second], opts)
        .await
        .unwrap();
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    let rows = db.get_entries(&EntryFilter::default()).unwrap();
    let row = rows.iter().find(|r| r.id == "second").unwrap();
    assert_eq!(row.translation.as_deref(), Some("Hola [name]"));
    assert_eq!(row.provider_used.as_deref(), Some("memory"));
}

#[tokio::test]
async fn memory_does_not_bypass_a_binary_slot_budget() {
    let db = Arc::new(Database::open_in_memory().unwrap());
    let provider = Arc::new(Provider {
        calls: AtomicUsize::new(0),
        drop_placeholder: false,
    });
    let mut row = entry("button", "Play");
    row.metadata
        .insert("binary_slot".into(), serde_json::json!("utf8"));
    db.save_entries(std::slice::from_ref(&row)).unwrap();
    db.save_memory(&row.source_hash(), &row.source, "Comenzar", "en-es")
        .await
        .unwrap();
    run(
        db.clone(),
        provider.clone(),
        vec![row],
        TranslationOptions {
            source_lang: "en".into(),
            target_lang: "es".into(),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        db.get_entries(&EntryFilter::default()).unwrap()[0]
            .translation
            .as_deref(),
        Some("Play")
    );
}

#[tokio::test]
async fn broken_placeholders_are_neither_saved_nor_cached() {
    let db = Arc::new(Database::open_in_memory().unwrap());
    let provider = Arc::new(Provider {
        calls: AtomicUsize::new(0),
        drop_placeholder: true,
    });
    let row = entry("first", "Hello [name]");
    db.save_entries(std::slice::from_ref(&row)).unwrap();
    run(
        db.clone(),
        provider,
        vec![row.clone()],
        TranslationOptions {
            source_lang: "en".into(),
            target_lang: "es".into(),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let rows = db.get_entries(&EntryFilter::default()).unwrap();
    assert_eq!(rows[0].status, StringStatus::Pending);
    assert!(rows[0].translation.is_none());
    assert!(db
        .lookup_memory(&row.source_hash(), "en-es")
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn unknown_pricing_with_a_budget_never_dispatches_paid_requests() {
    let db = Arc::new(Database::open_in_memory().unwrap());
    let provider = Arc::new(Provider {
        calls: AtomicUsize::new(0),
        drop_placeholder: false,
    });
    let row = entry("first", "Hello");
    db.save_entries(std::slice::from_ref(&row)).unwrap();
    let result = run(
        db,
        provider.clone(),
        vec![row],
        TranslationOptions {
            cost_limit_usd: Some(1.0),
            ..Default::default()
        },
    )
    .await;
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("cannot enforce a cost limit"));
    assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn provider_batch_save_rolls_back_if_any_entry_disappeared() {
    let db = Database::open_in_memory().unwrap();
    let first = entry("first", "Hello");
    db.save_entries(&[first]).unwrap();
    let provider = Provider {
        calls: AtomicUsize::new(0),
        drop_placeholder: false,
    };
    let requests: Vec<_> = ["first", "missing"]
        .into_iter()
        .map(|id| TranslationRequest {
            entry_id: id.into(),
            source: "Hello".into(),
            source_lang: "en".into(),
            target_lang: "es".into(),
            context: None,
            glossary_hint: None,
        })
        .collect();
    let results = provider.translate(&requests).await.unwrap();
    assert!(db.save_translation_results(&results).await.is_err());
    assert!(db.get_entries(&EntryFilter::default()).unwrap()[0]
        .translation
        .is_none());
}

#[tokio::test]
async fn failed_batch_save_preserves_observed_usage_in_the_run_ledger() {
    let db = Arc::new(Database::open_in_memory().unwrap());
    let provider = Arc::new(Provider {
        calls: AtomicUsize::new(0),
        drop_placeholder: false,
    });
    // An entry removed after selection must fail the save, but its completed
    // provider call still consumed tokens and must remain in the usage history.
    let result = run(
        db.clone(),
        provider,
        vec![entry("removed", "Hello")],
        TranslationOptions::default(),
    )
    .await;
    assert!(result.is_err());
    let runs = db.get_translation_runs().unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].tokens_used, 10);
    assert_eq!(runs[0].input_tokens, 6);
    assert_eq!(runs[0].output_tokens, 4);
}

struct CostOnlyProvider {
    calls: AtomicUsize,
    invalid_ids: bool,
}

#[async_trait]
impl TranslationProvider for CostOnlyProvider {
    fn id(&self) -> &str {
        "cost-only-test"
    }
    fn name(&self) -> &str {
        "Test"
    }
    fn is_free(&self) -> bool {
        false
    }
    fn requires_api_key(&self) -> bool {
        false
    }
    async fn health_check(&self) -> Result<()> {
        Ok(())
    }
    async fn estimate_cost(&self, _: usize, _: &str) -> Option<f64> {
        Some(0.1)
    }
    async fn translate(&self, requests: &[TranslationRequest]) -> Result<Vec<TranslationResult>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(requests
            .iter()
            .map(|r| TranslationResult {
                entry_id: if self.invalid_ids {
                    "unexpected".into()
                } else {
                    r.entry_id.clone()
                },
                translation: "Hola".into(),
                detected_source_lang: None,
                provider: self.id().into(),
                tokens_used: None,
                input_tokens: None,
                output_tokens: None,
                cost_usd: Some(0.5),
            })
            .collect())
    }
}

#[tokio::test]
async fn cost_without_tokens_survives_a_failed_save() {
    let db = Arc::new(Database::open_in_memory().unwrap());
    let provider = Arc::new(CostOnlyProvider {
        calls: AtomicUsize::new(0),
        invalid_ids: false,
    });
    assert!(run(
        db.clone(),
        provider,
        vec![entry("removed", "Hello")],
        TranslationOptions::default()
    )
    .await
    .is_err());
    let runs = db.get_translation_runs().unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].tokens_used, 0);
    assert_eq!(runs[0].strings_translated, 0);
    assert_eq!(runs[0].cost_usd, 0.5);
}

#[tokio::test]
async fn invalid_ids_still_consume_budget_and_are_recorded() {
    let db = Arc::new(Database::open_in_memory().unwrap());
    let provider = Arc::new(CostOnlyProvider {
        calls: AtomicUsize::new(0),
        invalid_ids: true,
    });
    let entries = vec![entry("a", "Hello"), entry("b", "Goodbye")];
    db.save_entries(&entries).unwrap();
    let error = run(
        db.clone(),
        provider.clone(),
        entries,
        TranslationOptions {
            batch_size: 1,
            cost_limit_usd: Some(0.5),
            ..Default::default()
        },
    )
    .await
    .unwrap_err();
    assert!(matches!(
        error,
        locust_core::error::LocustError::CostLimitExceeded { .. }
    ));
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    assert!(db
        .get_entries(&EntryFilter::default())
        .unwrap()
        .iter()
        .all(|r| r.translation.is_none()));
    let runs = db.get_translation_runs().unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].cost_usd, 0.5);
}
