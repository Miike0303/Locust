use async_trait::async_trait;
use locust_core::{
    database::Database,
    error::Result,
    glossary::Glossary,
    models::{StringEntry, StringStatus, TranslationRequest, TranslationResult},
    translation::{TranslationManager, TranslationOptions, TranslationProvider},
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

enum ReplyMode {
    WrongRetryId,
    BrokenRetryPlaceholder,
    Empty,
    Valid,
}
struct Provider {
    mode: ReplyMode,
    calls: AtomicUsize,
}
#[async_trait]
impl TranslationProvider for Provider {
    fn id(&self) -> &str {
        "answer-integrity"
    }
    fn name(&self) -> &str {
        "Answer integrity fixture"
    }
    fn is_free(&self) -> bool {
        true
    }
    fn requires_api_key(&self) -> bool {
        false
    }
    async fn health_check(&self) -> Result<()> {
        Ok(())
    }
    async fn estimate_cost(&self, _: usize, _: &str) -> Option<f64> {
        Some(0.01)
    }
    async fn translate(&self, requests: &[TranslationRequest]) -> Result<Vec<TranslationResult>> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(requests
            .iter()
            .map(|r| TranslationResult {
                entry_id: if matches!(self.mode, ReplyMode::WrongRetryId) && call > 0 {
                    "foreign-entry".into()
                } else {
                    r.entry_id.clone()
                },
                translation: match self.mode {
                    ReplyMode::Empty => "  ".into(),
                    ReplyMode::Valid => r.source.replace("Hello", "Hola"),
                    _ if call == 0 => format!("Long first answer {}", r.source),
                    _ => "OK".into(),
                },
                detected_source_lang: None,
                provider: self.id().into(),
                tokens_used: Some(10),
                input_tokens: Some(6),
                output_tokens: Some(4),
                cost_usd: Some(0.01),
            })
            .collect())
    }
}

struct RetryCostProvider {
    calls: AtomicUsize,
    retry_cost: Option<f64>,
}

#[async_trait]
impl TranslationProvider for RetryCostProvider {
    fn id(&self) -> &str {
        "retry-cost"
    }

    fn name(&self) -> &str {
        "Retry cost fixture"
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
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(requests
            .iter()
            .map(|request| TranslationResult {
                entry_id: request.entry_id.clone(),
                translation: if call == 0 { "Far too long" } else { "OK" }.into(),
                detected_source_lang: None,
                provider: self.id().into(),
                tokens_used: Some(10),
                input_tokens: Some(6),
                output_tokens: Some(4),
                cost_usd: if call == 0 {
                    Some(0.1)
                } else {
                    self.retry_cost
                },
            })
            .collect())
    }
}

async fn run(
    db: Arc<Database>,
    entry: StringEntry,
    provider: Arc<Provider>,
    glossary: Arc<Glossary>,
) {
    let (tx, mut rx) = mpsc::channel(100);
    let consume = tokio::spawn(async move { while rx.recv().await.is_some() {} });
    TranslationManager::new(provider, db, glossary)
        .translate_entries(
            vec![entry],
            TranslationOptions::default(),
            tx,
            "integrity".into(),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    consume.await.unwrap();
}

#[tokio::test]
async fn invalid_length_retry_never_replaces_the_previous_valid_answer() {
    for (mode, source) in [
        (ReplyMode::WrongRetryId, "Hi"),
        (ReplyMode::BrokenRetryPlaceholder, "Hi [name]"),
    ] {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let glossary = Arc::new(Glossary::new(db.clone()));
        let mut entry = StringEntry::new("line", source, PathBuf::from("game.assets"));
        entry
            .metadata
            .insert("binary_slot".into(), serde_json::json!("utf8"));
        db.save_entries(std::slice::from_ref(&entry)).unwrap();
        let provider = Arc::new(Provider {
            mode,
            calls: AtomicUsize::new(0),
        });
        run(db.clone(), entry.clone(), provider.clone(), glossary).await;
        let saved = db.get_entry("line").unwrap().unwrap();
        assert_eq!(
            saved.translation,
            Some(format!("Long first answer {source}"))
        );
        assert_eq!(provider.calls.load(Ordering::SeqCst), 3);
        assert!(db
            .lookup_memory(&entry.source_hash(), "ja-en")
            .unwrap()
            .is_none());
        let ledger = db.get_translation_runs().unwrap();
        assert_eq!(ledger[0].tokens_used, 30);
        assert_eq!((ledger[0].input_tokens, ledger[0].output_tokens), (18, 12));
        assert!((ledger[0].cost_usd - 0.03).abs() < 1e-9);
    }
}

#[tokio::test]
async fn empty_answer_remains_pending_and_does_not_enter_memory() {
    let db = Arc::new(Database::open_in_memory().unwrap());
    let glossary = Arc::new(Glossary::new(db.clone()));
    let entry = StringEntry::new("line", "Hello", PathBuf::from("game.json"));
    db.save_entries(std::slice::from_ref(&entry)).unwrap();
    run(
        db.clone(),
        entry.clone(),
        Arc::new(Provider {
            mode: ReplyMode::Empty,
            calls: AtomicUsize::new(0),
        }),
        glossary,
    )
    .await;
    let saved = db.get_entry("line").unwrap().unwrap();
    assert_eq!(saved.status, StringStatus::Pending);
    assert!(saved.translation.is_none());
    assert!(db
        .lookup_memory(&entry.source_hash(), "ja-en")
        .unwrap()
        .is_none());
    let ledger = db.get_translation_runs().unwrap();
    assert_eq!(ledger[0].strings_translated, 0);
    assert_eq!(ledger[0].tokens_used, 10);
}

#[tokio::test]
async fn invalid_exact_glossary_and_empty_memory_do_not_bypass_validation() {
    for glossary_translation in ["", "Hola"] {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let glossary = Arc::new(Glossary::new(db.clone()));
        let entry = StringEntry::new("line", "Hello [name]", PathBuf::from("game.json"));
        db.save_entries(std::slice::from_ref(&entry)).unwrap();
        glossary
            .add(&entry.source, glossary_translation, "ja-en", None)
            .unwrap();
        db.save_memory(&entry.source_hash(), &entry.source, " ", "ja-en")
            .await
            .unwrap();
        let provider = Arc::new(Provider {
            mode: ReplyMode::Valid,
            calls: AtomicUsize::new(0),
        });
        run(db.clone(), entry, provider.clone(), glossary).await;
        assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            db.get_entry("line")
                .unwrap()
                .unwrap()
                .translation
                .as_deref(),
            Some("Hola [name]")
        );
    }
}

#[tokio::test]
async fn binary_length_retry_respects_the_remaining_run_budget() {
    let db = Arc::new(Database::open_in_memory().unwrap());
    let glossary = Arc::new(Glossary::new(db.clone()));
    let mut entry = StringEntry::new("line", "Hi", PathBuf::from("game.assets"));
    entry
        .metadata
        .insert("binary_slot".into(), serde_json::json!("utf8"));
    db.save_entries(std::slice::from_ref(&entry)).unwrap();
    let provider = Arc::new(Provider {
        mode: ReplyMode::WrongRetryId,
        calls: AtomicUsize::new(0),
    });
    let (tx, mut rx) = mpsc::channel(100);
    let consume = tokio::spawn(async move { while rx.recv().await.is_some() {} });
    let result = TranslationManager::new(provider.clone(), db.clone(), glossary)
        .translate_entries(
            vec![entry],
            TranslationOptions {
                cost_limit_usd: Some(0.01),
                ..Default::default()
            },
            tx,
            "budget".into(),
            CancellationToken::new(),
        )
        .await;
    consume.await.unwrap();
    assert!(matches!(
        result,
        Err(locust_core::error::LocustError::CostLimitExceeded { .. })
    ));
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        db.get_entry("line")
            .unwrap()
            .unwrap()
            .translation
            .as_deref(),
        Some("Long first answer Hi")
    );
    let ledger = db.get_translation_runs().unwrap();
    assert_eq!(ledger[0].tokens_used, 10);
    assert!((ledger[0].cost_usd - 0.01).abs() < 1e-9);
}

#[tokio::test]
async fn final_length_retry_checks_actual_and_unknown_cost_after_saving() {
    for (retry_cost, expect_exceeded) in [(Some(0.5), true), (None, false)] {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let glossary = Arc::new(Glossary::new(db.clone()));
        let mut entry = StringEntry::new("line", "Hi", PathBuf::from("game.assets"));
        entry
            .metadata
            .insert("binary_slot".into(), serde_json::json!("utf8"));
        db.save_entries(std::slice::from_ref(&entry)).unwrap();
        let provider = Arc::new(RetryCostProvider {
            calls: AtomicUsize::new(0),
            retry_cost,
        });
        let (tx, mut rx) = mpsc::channel(100);
        let consume = tokio::spawn(async move { while rx.recv().await.is_some() {} });
        let result = TranslationManager::new(provider.clone(), db.clone(), glossary)
            .translate_entries(
                vec![entry],
                TranslationOptions {
                    cost_limit_usd: Some(0.5),
                    use_memory: false,
                    use_glossary: false,
                    ..Default::default()
                },
                tx,
                "retry-cost".into(),
                CancellationToken::new(),
            )
            .await;
        consume.await.unwrap();

        if expect_exceeded {
            assert!(matches!(
                result,
                Err(locust_core::error::LocustError::CostLimitExceeded { .. })
            ));
        } else {
            assert!(matches!(
                result,
                Err(locust_core::error::LocustError::ProviderError(ref message))
                    if message.contains("cannot enforce a cost limit")
            ));
        }
        assert_eq!(provider.calls.load(Ordering::SeqCst), 2);
        assert_eq!(
            db.get_entry("line")
                .unwrap()
                .unwrap()
                .translation
                .as_deref(),
            Some("OK")
        );
        let run = &db.get_translation_runs().unwrap()[0];
        assert_eq!(run.tokens_used, 20);
        assert_eq!(run.cost_is_complete, retry_cost.is_some());
        assert!((run.cost_usd - (0.1 + retry_cost.unwrap_or(0.0))).abs() < 1e-9);
    }
}

#[tokio::test]
async fn fallback_rejects_invalid_options_instead_of_completing_zero() {
    use locust_core::translation::run_fallback_chain;
    let invalid = [
        TranslationOptions {
            batch_size: 0,
            ..Default::default()
        },
        TranslationOptions {
            max_batch_tokens: Some(0),
            ..Default::default()
        },
        TranslationOptions {
            cost_limit_usd: Some(-1.0),
            ..Default::default()
        },
        TranslationOptions {
            cost_limit_usd: Some(f64::NAN),
            ..Default::default()
        },
    ];
    for opts in invalid {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let glossary = Arc::new(Glossary::new(db.clone()));
        let (tx, mut rx) = mpsc::channel(10);
        let resolve = |_: &str| -> Option<Arc<dyn TranslationProvider>> { None };
        let result = run_fallback_chain(
            &["unused".into()],
            &resolve,
            db,
            glossary,
            opts,
            tx,
            "invalid".into(),
            CancellationToken::new(),
        )
        .await;
        assert!(result.is_err());
        assert!(rx.recv().await.is_none());
    }
}
