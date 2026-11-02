use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use locust_core::{
    database::Database,
    error::{LocustError, Result},
    glossary::Glossary,
    model_policy::ModelPolicy,
    models::{StringEntry, StringStatus, TranslationRequest, TranslationResult},
    translation::{TranslationManager, TranslationOptions, TranslationProvider},
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

fn request_size(request: &TranslationRequest) -> usize {
    request.source.len()
        + request.context.as_ref().map_or(0, String::len)
        + request.glossary_hint.as_ref().map_or(0, String::len)
}

#[derive(Default)]
struct CountingProvider {
    estimates: Mutex<Vec<usize>>,
    calls: Mutex<Vec<Vec<TranslationRequest>>>,
    policy: Option<ModelPolicy>,
    break_first_controls: bool,
}

impl CountingProvider {
    fn estimates(&self) -> Vec<usize> {
        self.estimates.lock().unwrap().clone()
    }

    fn calls(&self) -> Vec<Vec<TranslationRequest>> {
        self.calls.lock().unwrap().clone()
    }

    fn assert_estimates_match_calls(&self) {
        let sizes: Vec<usize> = self
            .calls()
            .iter()
            .map(|requests| requests.iter().map(request_size).sum())
            .collect();
        assert_eq!(self.estimates(), sizes);
    }
}

#[async_trait]
impl TranslationProvider for CountingProvider {
    fn id(&self) -> &str {
        "w8-dispatch-budget"
    }

    fn name(&self) -> &str {
        self.id()
    }

    fn is_free(&self) -> bool {
        false
    }

    fn requires_api_key(&self) -> bool {
        false
    }

    fn model_policy(&self) -> Option<&ModelPolicy> {
        self.policy.as_ref()
    }

    async fn translate(&self, requests: &[TranslationRequest]) -> Result<Vec<TranslationResult>> {
        let break_controls = {
            let mut calls = self.calls.lock().unwrap();
            let first = calls.is_empty();
            calls.push(requests.to_vec());
            first && self.break_first_controls
        };
        Ok(requests
            .iter()
            .map(|request| TranslationResult {
                entry_id: request.entry_id.clone(),
                translation: if break_controls {
                    "Hola".into()
                } else {
                    request.source.replace("Hello", "Hola")
                },
                detected_source_lang: None,
                provider: self.id().into(),
                tokens_used: None,
                input_tokens: None,
                output_tokens: None,
                cost_usd: Some(request_size(request) as f64),
            })
            .collect())
    }

    async fn estimate_cost(&self, char_count: usize, _: &str) -> Option<f64> {
        self.estimates.lock().unwrap().push(char_count);
        Some(char_count as f64)
    }

    async fn health_check(&self) -> Result<()> {
        Ok(())
    }
}

struct Harness {
    db: Arc<Database>,
    glossary: Arc<Glossary>,
    provider: Arc<CountingProvider>,
}

impl Harness {
    fn new(provider: CountingProvider) -> Self {
        let db = Arc::new(Database::open_in_memory().unwrap());
        Self {
            glossary: Arc::new(Glossary::new(db.clone())),
            db,
            provider: Arc::new(provider),
        }
    }

    async fn run(&self, entries: Vec<StringEntry>, options: TranslationOptions) -> Result<()> {
        self.db.save_entries(&entries).unwrap();
        let (tx, mut rx) = mpsc::channel(32);
        let drain = tokio::spawn(async move { while rx.recv().await.is_some() {} });
        let result = TranslationManager::new(
            self.provider.clone(),
            self.db.clone(),
            self.glossary.clone(),
        )
        .translate_entries(
            entries,
            options,
            tx,
            "w8-dispatch-budget".into(),
            CancellationToken::new(),
        )
        .await;
        drain.await.unwrap();
        result
    }
}

fn entry(id: &str, source: &str) -> StringEntry {
    StringEntry::new(id, source, "game.json".into())
}

fn options(limit: f64) -> TranslationOptions {
    TranslationOptions {
        source_lang: "en".into(),
        target_lang: "es".into(),
        cost_limit_usd: Some(limit),
        use_memory: false,
        use_glossary: false,
        ..Default::default()
    }
}

fn assert_cost_limit(result: Result<()>, expected: f64, expected_limit: f64) {
    match result {
        Err(LocustError::CostLimitExceeded { estimated, limit }) => {
            assert_eq!(estimated, expected);
            assert_eq!(limit, expected_limit);
        }
        other => panic!("expected cost-limit refusal, got {other:?}"),
    }
}

#[tokio::test]
async fn game_context_refuses_before_any_translate_call() {
    let h = Harness::new(CountingProvider::default());
    let result = h
        .run(
            vec![entry("a", "a")],
            TranslationOptions {
                game_context: Some("x".repeat(1000)),
                ..options(5.0)
            },
        )
        .await;
    assert!(h.provider.calls().is_empty());
    assert_cost_limit(result, 1001.0, 5.0);
    assert_eq!(h.provider.estimates(), [1001]);
    assert_eq!(
        h.db.get_entry("a").unwrap().unwrap().status,
        StringStatus::Pending
    );
}

#[tokio::test]
async fn complete_request_at_budget_still_dispatches() {
    let h = Harness::new(CountingProvider::default());
    let mut row = entry("a", "a");
    row.context = Some("speaker".into());
    h.run(
        vec![row],
        TranslationOptions {
            game_context: Some("world".into()),
            ..options(16.0)
        },
    )
    .await
    .unwrap();
    let calls = h.provider.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0][0].context.as_deref(), Some("world | speaker"));
    assert_eq!(h.provider.estimates(), [16]);
    h.provider.assert_estimates_match_calls();
    assert_eq!(h.db.get_stats().unwrap().translated, 1);
}

#[tokio::test]
async fn sanitized_source_can_fit_when_raw_source_exceeds_budget() {
    let h = Harness::new(CountingProvider::default());
    let source = format!("Hello [{}]", "player_name".repeat(100));
    let limit = (source.len() - 1) as f64;
    h.run(vec![entry("a", &source)], options(limit))
        .await
        .unwrap();
    let calls = h.provider.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0][0].source, "Hello {PL_0}");
    assert!(request_size(&calls[0][0]) < source.len());
    h.provider.assert_estimates_match_calls();
    assert_eq!(
        h.db.get_entry("a").unwrap().unwrap().translation,
        Some(source.replace("Hello", "Hola"))
    );
}

#[tokio::test]
async fn sanitized_source_and_control_hints_refuse_before_dispatch() {
    let h = Harness::new(CountingProvider::default());
    let source = "Hello [x]";
    let limit = source.len() as f64;
    let result = h.run(vec![entry("a", source)], options(limit)).await;
    assert!(h.provider.calls().is_empty());
    let estimates = h.provider.estimates();
    assert_eq!(estimates.len(), 1);
    assert!(estimates[0] > source.len());
    assert_cost_limit(result, estimates[0] as f64, limit);
}

#[tokio::test]
async fn glossary_hints_refuse_before_dispatch() {
    let h = Harness::new(CountingProvider::default());
    h.glossary.add("Gem", "Joya", "en-es", None).unwrap();
    // The term is part of a sentence, so this cannot take the exact-hit shortcut.
    let source = "Take Gem";
    let limit = source.len() as f64;
    let result = h
        .run(
            vec![entry("a", source)],
            TranslationOptions {
                use_glossary: true,
                ..options(limit)
            },
        )
        .await;
    assert!(h.provider.calls().is_empty());
    let estimates = h.provider.estimates();
    assert_eq!(estimates.len(), 1);
    let hint = "Glossary (use these translations consistently):\n  Gem → Joya\n";
    assert!(estimates[0] >= "Take {G_0_0}".len() + hint.len());
    assert_cost_limit(result, estimates[0] as f64, limit);
}

fn batch_entries() -> Vec<StringEntry> {
    ["a", "b", "c", "d", "e"]
        .into_iter()
        .map(|id| entry(id, id))
        .collect()
}

#[tokio::test]
async fn every_split_chunk_counts_each_complete_request() {
    let h = Harness::new(CountingProvider::default());
    h.run(
        batch_entries(),
        TranslationOptions {
            batch_size: 2,
            game_context: Some("x".repeat(10)),
            ..options(55.0)
        },
    )
    .await
    .unwrap();
    let calls = h.provider.calls();
    assert_eq!(calls.iter().map(Vec::len).collect::<Vec<_>>(), [2, 2, 1]);
    assert_eq!(h.provider.estimates(), [22, 22, 11]);
    h.provider.assert_estimates_match_calls();
    assert_eq!(h.db.get_stats().unwrap().translated, 5);
}

#[tokio::test]
async fn split_chunk_refuses_before_call_when_remaining_budget_is_too_small() {
    let h = Harness::new(CountingProvider::default());
    let result = h
        .run(
            batch_entries(),
            TranslationOptions {
                batch_size: 2,
                max_concurrent: 4,
                game_context: Some("x".repeat(10)),
                ..options(32.0)
            },
        )
        .await;
    assert_eq!(h.provider.calls().len(), 1);
    assert_cost_limit(result, 44.0, 32.0);
    assert_eq!(h.provider.estimates(), [22, 22]);
    let stats = h.db.get_stats().unwrap();
    assert_eq!((stats.translated, stats.pending), (2, 3));
}

#[tokio::test]
async fn model_context_splits_estimate_only_the_complete_dispatch_slice() {
    let h = Harness::new(CountingProvider {
        policy: Some(ModelPolicy {
            context_window: Some(600),
            ..Default::default()
        }),
        ..Default::default()
    });
    h.glossary.add("Gem", "Joya", "en-es", None).unwrap();
    let entries = ["a", "b", "c"]
        .into_iter()
        .map(|id| entry(id, &format!("Take Gem {id}")))
        .collect();
    h.run(
        entries,
        TranslationOptions {
            use_glossary: true,
            ..options(10_000.0)
        },
    )
    .await
    .unwrap();
    let calls = h.provider.calls();
    assert_eq!(calls.iter().map(Vec::len).collect::<Vec<_>>(), [2, 1]);
    for request in calls.iter().flatten() {
        assert_eq!(
            request.source,
            format!("Take {{G_0_0}} {}", request.entry_id)
        );
        assert!(request
            .glossary_hint
            .as_ref()
            .unwrap()
            .contains("Gem → Joya"));
    }
    h.provider.assert_estimates_match_calls();
    assert_eq!(h.db.get_stats().unwrap().translated, 3);
}

#[tokio::test]
async fn initial_dispatch_and_control_repair_use_the_same_request_accounting() {
    let h = Harness::new(CountingProvider {
        break_first_controls: true,
        ..Default::default()
    });
    h.glossary.add("Gem", "Joya", "en-es", None).unwrap();
    h.run(
        vec![entry("a", "Hello Gem [x]")],
        TranslationOptions {
            game_context: Some("world".into()),
            use_glossary: true,
            ..options(10_000.0)
        },
    )
    .await
    .unwrap();
    let calls = h.provider.calls();
    assert_eq!(calls.len(), 2);
    assert!(calls[1][0]
        .context
        .as_ref()
        .unwrap()
        .contains("PROTECTED-CONTROL CORRECTION"));
    for request in calls.iter().flatten() {
        assert!(request.context.as_ref().unwrap().contains("world"));
        assert!(request
            .glossary_hint
            .as_ref()
            .unwrap()
            .contains("Gem → Joya"));
    }
    h.provider.assert_estimates_match_calls();
    assert_eq!(h.db.get_stats().unwrap().translated, 1);
}
