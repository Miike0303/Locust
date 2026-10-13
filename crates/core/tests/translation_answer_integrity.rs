use async_trait::async_trait;
use locust_core::{
    database::Database,
    error::Result,
    glossary::Glossary,
    models::{StringEntry, StringStatus, TranslationRequest, TranslationResult},
    translation::{RetryConfig, TranslationManager, TranslationOptions, TranslationProvider},
};
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
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

#[derive(Clone, Copy)]
enum ControlReply {
    Valid,
    Text(&'static str),
    WrongId,
    NoResults,
    Delayed,
}

struct ControlProvider {
    replies: Vec<ControlReply>,
    seen: Mutex<Vec<Vec<TranslationRequest>>>,
    paid: bool,
    costs: Vec<Option<f64>>,
    cancel: Option<CancellationToken>,
    cancel_on_estimate: bool,
    cancel_after_call: usize,
}

impl ControlProvider {
    fn new(replies: Vec<ControlReply>) -> Self {
        Self {
            replies,
            seen: Mutex::new(Vec::new()),
            paid: false,
            costs: vec![Some(0.1)],
            cancel: None,
            cancel_on_estimate: false,
            cancel_after_call: 0,
        }
    }
}

#[async_trait]
impl TranslationProvider for ControlProvider {
    fn id(&self) -> &str {
        "control-repair-fixture"
    }
    fn name(&self) -> &str {
        "Protected control repair fixture"
    }
    fn is_free(&self) -> bool {
        !self.paid
    }
    fn requires_api_key(&self) -> bool {
        false
    }
    async fn health_check(&self) -> Result<()> {
        Ok(())
    }
    async fn estimate_cost(&self, _: usize, _: &str) -> Option<f64> {
        if self.cancel_on_estimate && !self.seen.lock().unwrap().is_empty() {
            self.cancel.as_ref().unwrap().cancel();
        }
        Some(0.1)
    }
    async fn translate(&self, requests: &[TranslationRequest]) -> Result<Vec<TranslationResult>> {
        let call = {
            let mut seen = self.seen.lock().unwrap();
            let call = seen.len();
            seen.push(requests.to_vec());
            call
        };
        if !self.cancel_on_estimate && call >= self.cancel_after_call {
            if let Some(cancel) = &self.cancel {
                cancel.cancel();
            }
        }
        let reply = self.replies[call.min(self.replies.len() - 1)];
        if matches!(reply, ControlReply::Delayed) {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        if matches!(reply, ControlReply::NoResults) {
            return Ok(Vec::new());
        }
        Ok(requests
            .iter()
            .map(|request| TranslationResult {
                entry_id: if matches!(reply, ControlReply::WrongId) {
                    "foreign-entry".into()
                } else {
                    request.entry_id.clone()
                },
                translation: match reply {
                    ControlReply::Text(text) => text.into(),
                    _ => request.source.replace("Hello", "Hola"),
                },
                detected_source_lang: None,
                provider: self.id().into(),
                tokens_used: Some(10),
                input_tokens: Some(6),
                output_tokens: Some(4),
                cost_usd: self.costs[call.min(self.costs.len() - 1)],
            })
            .collect())
    }
}

fn control_entry(id: &str, source: &str) -> StringEntry {
    let mut entry = StringEntry::new(id, source, PathBuf::from("data/Map001.json"));
    entry
        .metadata
        .insert("format_id".into(), serde_json::json!("rpgmaker-mv"));
    entry
}

async fn run_controls(
    db: Arc<Database>,
    entries: Vec<StringEntry>,
    provider: Arc<ControlProvider>,
    opts: TranslationOptions,
    cancel: CancellationToken,
    deadline: Option<Duration>,
) -> Result<()> {
    let (tx, mut rx) = mpsc::channel(100);
    let consume = tokio::spawn(async move { while rx.recv().await.is_some() {} });
    let result = TranslationManager::new(provider, db.clone(), Arc::new(Glossary::new(db)))
        .with_retry_config(RetryConfig {
            max_attempts: 1,
            overall_deadline: deadline,
            ..Default::default()
        })
        .translate_entries(entries, opts, tx, "controls".into(), cancel)
        .await;
    consume.await.unwrap();
    result
}

fn assert_pending_without_memory(db: &Database, entry: &StringEntry) {
    let saved = db.get_entry(&entry.id).unwrap().unwrap();
    assert_eq!(saved.status, StringStatus::Pending);
    assert!(saved.translation.is_none());
    assert!(db
        .lookup_memory(&entry.source_hash(), "ja-en")
        .unwrap()
        .is_none());
}

async fn run_default_controls(
    db: Arc<Database>,
    entries: Vec<StringEntry>,
    provider: Arc<ControlProvider>,
) {
    run_controls(
        db,
        entries,
        provider,
        TranslationOptions::default(),
        CancellationToken::new(),
        None,
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn protected_controls_repair_first_bad_answer_and_account_both_calls() {
    let db = Arc::new(Database::open_in_memory().unwrap());
    let entry = control_entry("line", r"Hello \V[1]");
    db.save_entries(std::slice::from_ref(&entry)).unwrap();
    let provider = Arc::new(ControlProvider::new(vec![
        ControlReply::Text(r"Hola {PL_0} \C[2]"),
        ControlReply::Valid,
    ]));
    run_default_controls(db.clone(), vec![entry.clone()], provider.clone()).await;
    assert_eq!(provider.seen.lock().unwrap().len(), 2);
    assert_eq!(
        db.get_entry("line")
            .unwrap()
            .unwrap()
            .translation
            .as_deref(),
        Some(r"Hola \V[1]")
    );
    assert_eq!(
        db.lookup_memory(&entry.source_hash(), "ja-en")
            .unwrap()
            .as_deref(),
        Some(r"Hola \V[1]")
    );
    let run = &db.get_translation_runs().unwrap()[0];
    assert_eq!(
        (run.tokens_used, run.input_tokens, run.output_tokens),
        (20, 12, 8)
    );
    assert!((run.cost_usd - 0.2).abs() < 1e-9);
}

#[tokio::test]
async fn protected_controls_exhaust_two_repairs_without_saving_invalid_text() {
    for text in [
        "Hola",
        "Hola {PL_0}{PL_0}",
        "Hola {PL_0}{PL_9}",
        "Hola {PL_0}{PL_100}",
        r"Hola {PL_0} \C[2]",
        r"Hola \V[1]", // Literal originals are not a substitute for minted sentinels.
        " ",
    ] {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let entry = control_entry("line", r"Hello \V[1]");
        db.save_entries(std::slice::from_ref(&entry)).unwrap();
        let provider = Arc::new(ControlProvider::new(vec![ControlReply::Text(text)]));
        run_default_controls(db.clone(), vec![entry.clone()], provider.clone()).await;
        assert_eq!(provider.seen.lock().unwrap().len(), 3, "{text}");
        assert_pending_without_memory(&db, &entry);
        let run = &db.get_translation_runs().unwrap()[0];
        assert_eq!(run.tokens_used, 30);
        assert!((run.cost_usd - 0.3).abs() < 1e-9);
    }
}

#[tokio::test]
async fn protected_control_repair_rejects_wrong_ids_and_empty_batches_with_usage_accounted() {
    for (reply, tokens) in [(ControlReply::WrongId, 30), (ControlReply::NoResults, 20)] {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let entry = control_entry("line", r"Hello \V[1]");
        db.save_entries(std::slice::from_ref(&entry)).unwrap();
        let provider = Arc::new(ControlProvider::new(vec![
            ControlReply::Text("Hola"),
            reply,
            ControlReply::Valid,
        ]));
        run_default_controls(db.clone(), vec![entry], provider.clone()).await;
        assert_eq!(provider.seen.lock().unwrap().len(), 3);
        assert_eq!(
            db.get_entry("line")
                .unwrap()
                .unwrap()
                .translation
                .as_deref(),
            Some(r"Hola \V[1]")
        );
        let run = &db.get_translation_runs().unwrap()[0];
        assert_eq!(run.tokens_used, tokens);
        assert!((run.cost_usd - tokens as f64 / 100.0).abs() < 1e-9);
    }
}

#[tokio::test]
async fn protected_control_repair_checks_budget_and_unknown_paid_cost_before_dispatch() {
    for (costs, limit, calls) in [
        (vec![Some(0.1)], 0.15, 1),
        (vec![None], 1.0, 1),
        (vec![Some(0.1), Some(0.5)], 0.5, 2),
        (vec![Some(0.1), None], 1.0, 2),
    ] {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let entry = control_entry("line", r"Hello \V[1]");
        db.save_entries(std::slice::from_ref(&entry)).unwrap();
        let provider = Arc::new(ControlProvider {
            paid: true,
            costs: costs.clone(),
            ..ControlProvider::new(vec![ControlReply::Text("Hola"), ControlReply::WrongId])
        });
        let result = run_controls(
            db.clone(),
            vec![entry.clone()],
            provider.clone(),
            TranslationOptions {
                cost_limit_usd: Some(limit),
                ..Default::default()
            },
            CancellationToken::new(),
            None,
        )
        .await;
        if costs.iter().any(Option::is_none) {
            assert!(
                matches!(result, Err(locust_core::error::LocustError::ProviderError(ref message))
                if message.contains("cannot enforce a cost limit"))
            );
        } else {
            assert!(matches!(
                result,
                Err(locust_core::error::LocustError::CostLimitExceeded { .. })
            ));
        }
        assert_eq!(provider.seen.lock().unwrap().len(), calls);
        assert_pending_without_memory(&db, &entry);
        let run = &db.get_translation_runs().unwrap()[0];
        assert_eq!(run.tokens_used, calls as u64 * 10);
        assert_eq!(run.cost_is_complete, costs.iter().all(Option::is_some));
        let cost: f64 = (0..calls)
            .filter_map(|i| costs[i.min(costs.len() - 1)])
            .sum();
        assert!((run.cost_usd - cost).abs() < 1e-9);
    }
}

#[tokio::test]
async fn protected_control_repair_cancellation_and_deadline_prevent_further_dispatch() {
    for (cancel_on_estimate, cancel_after_call) in [(false, 0), (true, 0), (false, 1)] {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let entry = control_entry("line", r"Hello \V[1]");
        db.save_entries(std::slice::from_ref(&entry)).unwrap();
        let cancel = CancellationToken::new();
        let provider = Arc::new(ControlProvider {
            cancel: Some(cancel.clone()),
            cancel_on_estimate,
            cancel_after_call,
            ..ControlProvider::new(vec![ControlReply::Text("Hola"), ControlReply::Valid])
        });
        run_controls(
            db.clone(),
            vec![entry.clone()],
            provider.clone(),
            TranslationOptions {
                cost_limit_usd: Some(1.0),
                ..Default::default()
            },
            cancel,
            None,
        )
        .await
        .unwrap();
        let calls = cancel_after_call + 1;
        assert_eq!(provider.seen.lock().unwrap().len(), calls);
        assert_pending_without_memory(&db, &entry);
        assert_eq!(
            db.get_translation_runs().unwrap()[0].tokens_used,
            calls as u64 * 10
        );
    }
    let db = Arc::new(Database::open_in_memory().unwrap());
    let entry = control_entry("line", r"Hello \V[1]");
    db.save_entries(std::slice::from_ref(&entry)).unwrap();
    let provider = Arc::new(ControlProvider::new(vec![
        ControlReply::Text("Hola"),
        ControlReply::Delayed,
    ]));
    run_controls(
        db.clone(),
        vec![entry.clone()],
        provider.clone(),
        TranslationOptions::default(),
        CancellationToken::new(),
        Some(Duration::from_millis(20)),
    )
    .await
    .unwrap();
    assert_eq!(provider.seen.lock().unwrap().len(), 2);
    assert_pending_without_memory(&db, &entry);
}

#[tokio::test]
async fn protected_control_inventory_is_record_local_in_later_batches_and_repairs() {
    let db = Arc::new(Database::open_in_memory().unwrap());
    let entries = vec![
        control_entry("plain", "Hello"),
        control_entry("one", r"Hello \V[1]"),
        control_entry("two", r"\C[2]Hello \C[0]"),
    ];
    db.save_entries(&entries).unwrap();
    let provider = Arc::new(ControlProvider::new(vec![
        ControlReply::Valid,
        ControlReply::Text("Hola"),
        ControlReply::Valid,
        ControlReply::Valid,
    ]));
    run_controls(
        db.clone(),
        entries,
        provider.clone(),
        TranslationOptions {
            batch_size: 1,
            max_concurrent: 1,
            ..Default::default()
        },
        CancellationToken::new(),
        None,
    )
    .await
    .unwrap();
    let seen = provider.seen.lock().unwrap();
    assert_eq!(seen.len(), 4);
    assert!(seen[0][0].context.is_none());
    for (index, inventory) in [(1, "{PL_0}"), (2, "{PL_0}"), (3, "{PL_0} {PL_1}")] {
        let context = seen[index][0].context.as_deref().unwrap();
        assert!(
            context.contains(&format!("EXPECTED SENTINELS: {inventory}.")),
            "{context}"
        );
        assert_eq!(context.matches("{PL_0}").count(), 1);
        assert_eq!(context.matches("{PL_1}").count(), usize::from(index == 3));
        assert!(context.contains("each exactly once"));
        assert!(context.contains("no additional sentinel or control tokens"));
    }
    assert_eq!(seen[1][0].source, seen[2][0].source);
    assert_eq!(seen[1][0].entry_id, seen[2][0].entry_id);
    assert_eq!(
        db.get_entry("one").unwrap().unwrap().translation.as_deref(),
        Some(r"Hola \V[1]")
    );
}

#[tokio::test]
async fn valid_exact_glossary_and_memory_bypass_provider_repairs() {
    let db = Arc::new(Database::open_in_memory().unwrap());
    let entries = vec![
        control_entry("memory", r"Hello \V[1]"),
        control_entry("glossary", r"Hello \C[2]"),
    ];
    db.save_entries(&entries).unwrap();
    db.save_memory(
        &entries[0].source_hash(),
        &entries[0].source,
        r"Hola \V[1]",
        "ja-en",
    )
    .await
    .unwrap();
    Glossary::new(db.clone())
        .add(&entries[1].source, r"Hola \C[2]", "ja-en", None)
        .unwrap();
    let provider = Arc::new(ControlProvider::new(vec![ControlReply::Text("invalid")]));
    run_default_controls(db.clone(), entries, provider.clone()).await;
    assert!(provider.seen.lock().unwrap().is_empty());
    for id in ["memory", "glossary"] {
        assert_eq!(
            db.get_entry(id).unwrap().unwrap().status,
            StringStatus::Translated
        );
    }
}

#[tokio::test]
async fn sentinel_like_text_inside_a_protected_original_is_not_an_extra_raw_token() {
    let db = Arc::new(Database::open_in_memory().unwrap());
    let entry = StringEntry::new(
        "line",
        r#"<b data-token="{PL_100}">Hello</b>"#,
        PathBuf::from("page.html"),
    )
    .with_format_id("html");
    db.save_entries(std::slice::from_ref(&entry)).unwrap();
    let provider = Arc::new(ControlProvider::new(vec![ControlReply::Valid]));
    run_default_controls(db.clone(), vec![entry], provider.clone()).await;
    assert_eq!(provider.seen.lock().unwrap().len(), 1);
    assert_eq!(
        db.get_entry("line")
            .unwrap()
            .unwrap()
            .translation
            .as_deref(),
        Some(r#"<b data-token="{PL_100}">Hola</b>"#)
    );
}

#[tokio::test]
async fn protected_control_repairs_preserve_repeated_source_fanout_and_initial_id_contract() {
    for reply in [
        ControlReply::Valid,
        ControlReply::Text(r"Hola \V[1]"),
        ControlReply::WrongId,
    ] {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let entries = vec![
            control_entry("one", r"Hello \V[1]"),
            control_entry("two", r"Hello \V[1]"),
        ];
        db.save_entries(&entries).unwrap();
        let provider = Arc::new(ControlProvider::new(vec![
            if matches!(reply, ControlReply::WrongId) {
                reply
            } else {
                ControlReply::Text("Hola")
            },
            reply,
        ]));
        run_default_controls(db.clone(), entries.clone(), provider.clone()).await;
        let calls = match reply {
            ControlReply::Valid => 2,
            ControlReply::WrongId => 1,
            _ => 3,
        };
        let seen = provider.seen.lock().unwrap();
        assert_eq!(seen.len(), calls);
        assert!(seen.iter().all(|requests| requests.len() == 1));
        for entry in &entries {
            if matches!(reply, ControlReply::Valid) {
                assert_eq!(
                    db.get_entry(&entry.id)
                        .unwrap()
                        .unwrap()
                        .translation
                        .as_deref(),
                    Some(r"Hola \V[1]")
                );
            } else {
                assert_pending_without_memory(&db, entry);
            }
        }
        assert_eq!(
            db.get_translation_runs().unwrap()[0].tokens_used,
            calls as u64 * 10
        );
    }
}

#[tokio::test]
async fn protected_control_repairs_precede_length_fitting_and_never_fit_invalid_controls() {
    for valid in [true, false] {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let mut entry = control_entry("line", r"Hi \V[1]");
        entry
            .metadata
            .insert("binary_slot".into(), serde_json::json!("utf8"));
        db.save_entries(std::slice::from_ref(&entry)).unwrap();
        let provider = Arc::new(ControlProvider::new(if valid {
            vec![
                ControlReply::Text("missing"),
                ControlReply::Text("Long label {PL_0}"),
                ControlReply::Text("OK {PL_0}"),
            ]
        } else {
            vec![ControlReply::Text(r"Long label \V[1]")]
        }));
        run_controls(
            db.clone(),
            vec![entry.clone()],
            provider.clone(),
            TranslationOptions {
                allow_lossy_binary_fit: true,
                ..Default::default()
            },
            CancellationToken::new(),
            None,
        )
        .await
        .unwrap();
        assert_eq!(provider.seen.lock().unwrap().len(), 3);
        if valid {
            assert_eq!(
                db.get_entry("line")
                    .unwrap()
                    .unwrap()
                    .translation
                    .as_deref(),
                Some(r"OK \V[1]")
            );
        } else {
            assert_pending_without_memory(&db, &entry);
        }
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
