use async_trait::async_trait;
use locust_core::{
    database::Database,
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

struct RejectingProvider {
    calls: AtomicUsize,
    usage: bool,
    price: Option<f64>,
    retry: bool,
    accept_last: bool,
    unknown_first: bool,
}

#[async_trait]
impl TranslationProvider for RejectingProvider {
    fn id(&self) -> &str {
        "w8-rejected-usage"
    }
    fn name(&self) -> &str {
        "Rejected usage fixture"
    }
    fn is_free(&self) -> bool {
        false
    }
    fn requires_api_key(&self) -> bool {
        false
    }
    async fn translate(&self, requests: &[TranslationRequest]) -> Result<Vec<TranslationResult>> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        let receipt = TranslationResult {
            entry_id: requests[0].entry_id.clone(),
            translation: "Hola".into(),
            detected_source_lang: None,
            provider: self.id().into(),
            tokens_used: Some(120),
            input_tokens: Some(100),
            output_tokens: Some(20),
            cost_usd: self.price,
        };
        if self.accept_last && call == 2 {
            return Ok(requests
                .iter()
                .enumerate()
                .map(|(i, request)| {
                    let mut result = receipt.clone();
                    result.entry_id = request.entry_id.clone();
                    if i > 0 {
                        result.tokens_used = None;
                        result.input_tokens = None;
                        result.output_tokens = None;
                        result.cost_usd = None;
                    }
                    result
                })
                .collect());
        }
        let error = LocustError::ProviderError(if self.retry {
            "translation count mismatch: timeout".into()
        } else {
            "translation count mismatch: sent 2 strings, got 1 back".into()
        });
        Err(if self.usage && !(self.unknown_first && call == 0) {
            error.with_usage(receipt)
        } else {
            error
        })
    }
    async fn estimate_cost(&self, _: usize, _: &str) -> Option<f64> {
        Some(0.0)
    }
    async fn health_check(&self) -> Result<()> {
        Ok(())
    }
}

async fn run(usage: bool, price: Option<f64>, retry: bool, accept_last: bool, unknown_first: bool) {
    let provider = Arc::new(RejectingProvider {
        calls: AtomicUsize::new(0),
        usage,
        price,
        retry,
        accept_last,
        unknown_first,
    });
    let db = Arc::new(Database::open_in_memory().unwrap());
    let entries = vec![
        StringEntry::new("one", "Hello", PathBuf::from("fixture.json")),
        StringEntry::new("two", "Goodbye", PathBuf::from("fixture.json")),
    ];
    db.save_entries(&entries).unwrap();
    let (tx, mut rx) = mpsc::channel(32);
    let drain = tokio::spawn(async move {
        let mut events = Vec::new();
        while let Some(event) = rx.recv().await {
            events.push(event);
        }
        events
    });
    TranslationManager::new(
        provider.clone(),
        db.clone(),
        Arc::new(Glossary::new(db.clone())),
    )
    .with_retry_config(RetryConfig {
        max_attempts: 3,
        initial_delay_ms: 1,
        max_delay_ms: 1,
        ..Default::default()
    })
    .translate_entries(
        entries,
        TranslationOptions {
            source_lang: "en".into(),
            target_lang: "es".into(),
            use_memory: false,
            use_glossary: false,
            ..Default::default()
        },
        tx,
        "rejected".into(),
        CancellationToken::new(),
    )
    .await
    .unwrap();
    let events = drain.await.unwrap();
    let calls = if retry { 3 } else { 1 };
    let billed_calls = if usage {
        calls - usize::from(unknown_first)
    } else {
        0
    };
    assert_eq!(provider.calls.load(Ordering::SeqCst), calls);
    for id in ["one", "two"] {
        let entry = db.get_entry(id).unwrap().unwrap();
        assert_eq!(entry.translation.as_deref(), accept_last.then_some("Hola"));
    }
    let runs = db.get_translation_runs().unwrap();
    assert_eq!(runs.len(), 1);
    let run = &runs[0];
    assert_eq!(run.strings_translated, if accept_last { 2 } else { 0 });
    assert_eq!(
        (run.input_tokens, run.output_tokens, run.tokens_used),
        (
            100 * billed_calls as u64,
            20 * billed_calls as u64,
            120 * billed_calls as u64
        )
    );
    let expected_cost = price.unwrap_or(0.0) * billed_calls as f64;
    assert!((run.cost_usd - expected_cost).abs() < 1e-12);
    let complete = usage && price.is_some() && !unknown_first;
    assert_eq!(run.cost_is_complete, complete);
    assert!(events.iter().any(
        |event| matches!(event, ProgressEvent::Completed { total_cost, cost_is_complete, .. }
        if (*total_cost - expected_cost).abs() < 1e-12 && *cost_is_complete == complete)
    ));
    if !accept_last {
        assert!(!events
            .iter()
            .any(|event| matches!(event, ProgressEvent::StringTranslated { .. })));
    }
}

#[tokio::test]
async fn rejected_answer_records_usage_and_known_cost_once() {
    run(true, Some(0.25), false, false, false).await;
}
#[tokio::test]
async fn rejected_answer_with_unpriced_usage_keeps_cost_unknown() {
    run(true, None, false, false, false).await;
}
#[tokio::test]
async fn rejected_answer_without_usage_keeps_cost_unknown() {
    run(false, None, false, false, false).await;
}
#[tokio::test]
async fn retried_rejections_count_each_actual_call_once() {
    run(true, Some(0.25), true, false, false).await;
}
#[tokio::test]
async fn rejected_attempts_then_success_count_each_actual_call_once() {
    run(true, Some(0.25), true, true, false).await;
}
#[tokio::test]
async fn unreported_retry_stays_unknown_after_priced_rejections() {
    run(true, Some(0.25), true, false, true).await;
}
