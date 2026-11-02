use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use locust_core::database::Database;
use locust_core::error::{LocustError, Result};
use locust_core::glossary::Glossary;
use locust_core::models::{
    ProgressEvent, StringEntry, StringStatus, TranslationRequest, TranslationResult,
};
use locust_core::translation::{
    run_fallback_chain, RetryConfig, TranslationManager, TranslationOptions, TranslationProvider,
};
use tokio::sync::{mpsc, Notify};
use tokio_util::sync::CancellationToken;

// An independent connection and SQL triggers observe actual catalog writes,
// including an accidental write that happens to leave the final value unchanged.
struct Catalog {
    db: Arc<Database>,
    audit: rusqlite::Connection,
    _dir: tempfile::TempDir,
}

impl Catalog {
    fn new(entries: &[StringEntry]) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("scope.db");
        let db = Arc::new(Database::open(&path).unwrap());
        db.save_entries(entries).unwrap();
        let audit = rusqlite::Connection::open(path).unwrap();
        audit
            .execute_batch(
                "CREATE TABLE writes (id TEXT NOT NULL);
                 CREATE TRIGGER audit_update AFTER UPDATE ON strings
                 BEGIN INSERT INTO writes VALUES (NEW.id); END;
                 CREATE TRIGGER audit_insert AFTER INSERT ON strings
                 BEGIN INSERT INTO writes VALUES (NEW.id); END;
                 CREATE TRIGGER audit_delete AFTER DELETE ON strings
                 BEGIN INSERT INTO writes VALUES (OLD.id); END;",
            )
            .unwrap();
        Self {
            db,
            audit,
            _dir: dir,
        }
    }

    fn writes(&self) -> BTreeMap<String, usize> {
        self.audit
            .prepare("SELECT id, COUNT(*) FROM writes GROUP BY id ORDER BY id")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    }
}

fn row(id: &str, source: &str, file: &str) -> StringEntry {
    StringEntry::new(id, source, file.into())
}

fn options(ids: &[&str]) -> TranslationOptions {
    TranslationOptions {
        source_lang: "en".into(),
        target_lang: "es".into(),
        file_paths: Some(vec!["selected.json".into()]),
        entry_ids: Some(ids.iter().map(|id| (*id).into()).collect()),
        // An excluded file's priority must never admit its rows.
        file_priority: vec!["excluded.json".into(), "selected.json".into()],
        use_memory: false,
        use_glossary: false,
        ..Default::default()
    }
}

#[derive(Default)]
struct Gate {
    entered: Notify,
    release: Notify,
}

struct Step {
    successes: Vec<&'static str>,
    gate: Option<Arc<Gate>>,
    stall: bool,
}

impl Step {
    fn new(successes: &[&'static str]) -> Self {
        Self {
            successes: successes.to_vec(),
            gate: None,
            stall: false,
        }
    }
}

struct ScriptedPartial {
    id: &'static str,
    steps: Mutex<VecDeque<Step>>,
    calls: Mutex<Vec<Vec<String>>>,
    active: AtomicUsize,
    dropped: AtomicUsize,
    changed: Notify,
}

struct RequestLease<'a>(&'a ScriptedPartial);

impl Drop for RequestLease<'_> {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::SeqCst);
        self.0.dropped.fetch_add(1, Ordering::SeqCst);
        self.0.changed.notify_one();
    }
}

impl ScriptedPartial {
    fn new(id: &'static str, steps: Vec<Step>) -> Arc<Self> {
        Arc::new(Self {
            id,
            steps: Mutex::new(steps.into()),
            calls: Mutex::new(Vec::new()),
            active: AtomicUsize::new(0),
            dropped: AtomicUsize::new(0),
            changed: Notify::new(),
        })
    }

    fn calls(&self) -> Vec<Vec<String>> {
        self.calls.lock().unwrap().clone()
    }
}

#[async_trait]
impl TranslationProvider for ScriptedPartial {
    fn id(&self) -> &str {
        self.id
    }
    fn name(&self) -> &str {
        self.id
    }
    fn is_free(&self) -> bool {
        false
    }
    fn requires_api_key(&self) -> bool {
        false
    }
    async fn translate(&self, _: &[TranslationRequest]) -> Result<Vec<TranslationResult>> {
        panic!("partial provider must never use all-or-nothing delivery")
    }
    async fn translate_partial(
        &self,
        requests: &[TranslationRequest],
        receipts: &mut Vec<TranslationResult>,
    ) -> Option<Vec<(String, LocustError)>> {
        self.active.fetch_add(1, Ordering::SeqCst);
        let _lease = RequestLease(self);
        self.calls
            .lock()
            .unwrap()
            .push(requests.iter().map(|r| r.entry_id.clone()).collect());
        let step = self
            .steps
            .lock()
            .unwrap()
            .pop_front()
            .expect("extra request");
        for request in requests {
            if step.successes.contains(&request.entry_id.as_str()) {
                receipts.push(TranslationResult {
                    entry_id: request.entry_id.clone(),
                    translation: format!("Traducido {}", request.entry_id),
                    detected_source_lang: None,
                    provider: self.id.into(),
                    tokens_used: Some(7),
                    input_tokens: Some(3),
                    output_tokens: Some(4),
                    cost_usd: Some(0.01),
                });
            }
        }
        self.changed.notify_one();
        if let Some(gate) = step.gate {
            gate.entered.notify_one();
            gate.release.notified().await;
        }
        if step.stall {
            std::future::pending::<()>().await;
        }
        Some(
            requests
                .iter()
                .filter(|r| !step.successes.contains(&r.entry_id.as_str()))
                .map(|r| {
                    (
                        r.entry_id.clone(),
                        LocustError::ProviderError("503 retry".into()),
                    )
                })
                .collect(),
        )
    }
    async fn estimate_cost(&self, _: usize, _: &str) -> Option<f64> {
        Some(0.0)
    }
    async fn health_check(&self) -> Result<()> {
        Ok(())
    }
}

async fn events(mut rx: mpsc::Receiver<ProgressEvent>) -> Vec<ProgressEvent> {
    let mut events = Vec::new();
    while let Some(event) = rx.recv().await {
        events.push(event);
    }
    events
}

fn successes(events: &[ProgressEvent]) -> BTreeMap<String, usize> {
    let mut ids = BTreeMap::new();
    for event in events {
        if let ProgressEvent::StringTranslated { entry_id, .. } = event {
            *ids.entry(entry_id.clone()).or_default() += 1;
        }
    }
    ids
}

fn expected_writes(ids: &[&str]) -> BTreeMap<String, usize> {
    ids.iter().map(|id| ((*id).into(), 1)).collect()
}

#[tokio::test]
async fn w4_04_partial_scope_twins_shortcuts_and_dedupe_have_zero_excluded_writes() {
    let rows = [
        row("memory", "Remember", "selected.json"),
        row("glossary", "Canonical", "selected.json"),
        row("a", "Repeat", "selected.json"),
        row("a-copy", "Repeat", "selected.json"),
        row("memory-out", "Remember", "selected.json"),
        row("glossary-out", "Canonical", "selected.json"),
        row("a-out", "Repeat", "excluded.json"),
    ];
    let catalog = Catalog::new(&rows);
    let db = &catalog.db;
    db.save_memory(&rows[0].source_hash(), "Remember", "Recuerdo", "en-es")
        .await
        .unwrap();
    let glossary = Arc::new(Glossary::new(db.clone()));
    glossary
        .add("Canonical", "Canónico", "en-es", None)
        .unwrap();
    let provider = ScriptedPartial::new("w4-shortcuts", vec![Step::new(&["a"])]);
    let (tx, rx) = mpsc::channel(100);
    run_fallback_chain(
        &[provider.id.into()],
        &|_| Some(provider.clone()),
        db.clone(),
        glossary,
        TranslationOptions {
            use_memory: true,
            use_glossary: true,
            ..options(&["memory", "glossary", "a", "a-copy", "a-out"])
        },
        tx,
        "w4-shortcuts".into(),
        CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(provider.calls(), [vec!["a"]]);
    let expected = expected_writes(&["memory", "glossary", "a", "a-copy"]);
    assert_eq!(catalog.writes(), expected);
    assert_eq!(successes(&events(rx).await), expected);
    for original in &rows[4..] {
        assert_eq!(
            serde_json::to_value(db.get_entry(&original.id).unwrap().unwrap()).unwrap(),
            serde_json::to_value(original).unwrap()
        );
    }
    for (id, provider) in [
        ("memory", "memory"),
        ("glossary", "glossary"),
        ("a", "w4-shortcuts"),
    ] {
        assert_eq!(
            db.get_entry(id).unwrap().unwrap().provider_used.as_deref(),
            Some(provider)
        );
    }
    let runs = db.get_translation_runs().unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!((runs[0].strings_translated, runs[0].tokens_used), (4, 7));

    // Both explicit empty selectors remain empty even with cached twins present.
    for opts in [
        options(&[]),
        TranslationOptions {
            file_paths: Some(Vec::new()),
            ..options(&["memory-out", "glossary-out", "a-out"])
        },
    ] {
        let (tx, rx) = mpsc::channel(100);
        run_fallback_chain(
            &[provider.id.into()],
            &|_| Some(provider.clone()),
            db.clone(),
            Arc::new(Glossary::new(db.clone())),
            TranslationOptions {
                use_memory: true,
                use_glossary: true,
                ..opts
            },
            tx,
            "w4-empty".into(),
            CancellationToken::new(),
        )
        .await
        .unwrap();
        assert!(matches!(
            events(rx).await.last(),
            Some(ProgressEvent::Completed {
                total_translated: 0,
                remaining: 0,
                ..
            })
        ));
    }
    assert_eq!(provider.calls(), [vec!["a"]]);
    assert_eq!(catalog.writes(), expected);
    assert_eq!(db.get_translation_runs().unwrap().len(), 1);
}

#[tokio::test]
async fn w4_04_two_partial_retries_then_fallback_keep_frozen_ids_and_exact_accounting() {
    let rows = [
        row("a", "First", "selected.json"),
        row("a-copy", "First", "selected.json"),
        row("b", "Second", "selected.json"),
        row("c", "Third", "selected.json"),
        row("a-out", "First", "excluded.json"),
        row("c-out", "Third", "selected.json"),
    ];
    let catalog = Catalog::new(&rows);
    let gate = Arc::new(Gate::default());
    let primary = ScriptedPartial::new(
        "w4-primary",
        vec![
            Step {
                gate: Some(gate.clone()),
                ..Step::new(&["a"])
            },
            Step::new(&["b"]),
            Step::new(&[]),
        ],
    );
    let fallback = ScriptedPartial::new("w4-fallback", vec![Step::new(&["c"])]);
    let (tx, rx) = mpsc::channel(100);
    let job = tokio::spawn({
        let primary = primary.clone();
        let fallback = fallback.clone();
        let db = catalog.db.clone();
        async move {
            run_fallback_chain(
                &[primary.id.into(), fallback.id.into()],
                &|id| {
                    if id == primary.id {
                        Some(primary.clone())
                    } else {
                        Some(fallback.clone())
                    }
                },
                db.clone(),
                Arc::new(Glossary::new(db)),
                options(&["a", "a-copy", "b", "c", "a-out", "late"]),
                tx,
                "w4-chain".into(),
                CancellationToken::new(),
            )
            .await
        }
    });
    tokio::time::timeout(Duration::from_secs(5), gate.entered.notified())
        .await
        .unwrap();
    // This ID was requested but absent at admission. Its later insertion must
    // not join retries or the fallback, even though it shares a failed source.
    catalog
        .db
        .save_entries(&[row("late", "Third", "selected.json")])
        .unwrap();
    gate.release.notify_one();
    tokio::time::timeout(Duration::from_secs(10), job)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(
        primary.calls(),
        [vec!["a", "b", "c"], vec!["b", "c"], vec!["c"]]
    );
    assert_eq!(fallback.calls(), [vec!["c"]]);
    let expected = expected_writes(&["a", "a-copy", "b", "c"]);
    let mut writes_with_insertion = expected.clone();
    writes_with_insertion.insert("late".into(), 1);
    assert_eq!(catalog.writes(), writes_with_insertion);
    assert_eq!(
        catalog.db.get_entry("late").unwrap().unwrap().status,
        StringStatus::Pending
    );
    let events = events(rx).await;
    assert_eq!(successes(&events), expected);
    assert!(matches!(
        events.last(),
        Some(ProgressEvent::Completed {
            total_translated: 4,
            remaining: 0,
            ..
        })
    ));
    assert!(events.iter().any(|e| matches!(
        e,
        ProgressEvent::ProviderSwitched {
            remaining_pending: 1,
            ..
        }
    )));
    let stats = catalog.db.get_stats().unwrap();
    assert_eq!((stats.total, stats.translated, stats.pending), (7, 4, 3));
    let runs = catalog.db.get_translation_runs().unwrap();
    assert_eq!(runs.len(), 2);
    for (id, count, tokens, input, output, cost, complete) in [
        (primary.id, 3, 14, 6, 8, 0.02, false),
        (fallback.id, 1, 7, 3, 4, 0.01, true),
    ] {
        let run = runs.iter().find(|r| r.provider == id).unwrap();
        assert_eq!(
            (
                run.strings_translated,
                run.tokens_used,
                run.input_tokens,
                run.output_tokens
            ),
            (count, tokens, input, output)
        );
        assert_eq!((run.cost_usd, run.cost_is_complete), (cost, complete));
    }
}

async fn interrupted_partial_retry(cancel_job: bool, in_flight: bool) {
    let rows = [
        row("a", "First", "selected.json"),
        row("b", "Second", "selected.json"),
        row("c", "Third", "selected.json"),
        row("out", "First", "excluded.json"),
    ];
    let catalog = Catalog::new(&rows);
    let provider = ScriptedPartial::new(
        "w4-interrupted",
        vec![
            Step::new(&["a"]),
            Step {
                stall: in_flight,
                ..Step::new(&["b"])
            },
        ],
    );
    let cancel = CancellationToken::new();
    let manager = TranslationManager::new(
        provider.clone(),
        catalog.db.clone(),
        Arc::new(Glossary::new(catalog.db.clone())),
    )
    .with_retry_config(RetryConfig {
        initial_delay_ms: 1,
        backoff_multiplier: 60_000.0,
        max_delay_ms: 60_000,
        overall_deadline: Some(Duration::from_secs(1)),
        ..Default::default()
    });
    let (tx, rx) = mpsc::channel(100);
    let token = cancel.clone();
    let job = tokio::spawn(async move {
        manager
            .translate_entries(
                rows.to_vec(),
                options(&["a", "b", "c", "out"]),
                tx,
                "w4-interrupt".into(),
                token,
            )
            .await
    });
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let changed = provider.changed.notified();
            if provider.calls().len() == 2
                && (in_flight || provider.dropped.load(Ordering::SeqCst) == 2)
            {
                break;
            }
            // The stalled second call does not drop its lease until interrupted.
            changed.await;
        }
    })
    .await
    .unwrap();
    if cancel_job {
        cancel.cancel();
    }
    tokio::time::timeout(Duration::from_secs(5), job)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(provider.calls(), [vec!["a", "b", "c"], vec!["b", "c"]]);
    assert_eq!(
        provider.active.load(Ordering::SeqCst),
        0,
        "no subrequest may outlive completion"
    );
    assert_eq!(provider.dropped.load(Ordering::SeqCst), 2);
    let expected = expected_writes(&["a", "b"]);
    assert_eq!(catalog.writes(), expected);
    let events = events(rx).await;
    assert_eq!(successes(&events), expected);
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, ProgressEvent::Paused))
            .count(),
        usize::from(cancel_job)
    );
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, ProgressEvent::Completed { .. }))
            .count(),
        usize::from(!cancel_job)
    );
    let stats = catalog.db.get_stats().unwrap();
    assert_eq!((stats.translated, stats.pending), (2, 2));
    let runs = catalog.db.get_translation_runs().unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(
        (
            runs[0].strings_translated,
            runs[0].tokens_used,
            runs[0].input_tokens,
            runs[0].output_tokens
        ),
        (2, 14, 6, 8)
    );
    assert_eq!((runs[0].cost_usd, runs[0].cost_is_complete), (0.02, false));
}

#[tokio::test]
async fn w4_04_cancel_during_partial_retry_backoff_saves_receipts_once() {
    interrupted_partial_retry(true, false).await;
}

#[tokio::test]
async fn w4_04_deadline_during_partial_retry_backoff_saves_receipts_once() {
    interrupted_partial_retry(false, false).await;
}

#[tokio::test]
async fn w4_04_cancel_during_partial_retry_drops_inflight_request_and_saves_receipts_once() {
    interrupted_partial_retry(true, true).await;
}

#[tokio::test]
async fn w4_04_deadline_during_partial_retry_drops_inflight_request_and_saves_receipts_once() {
    interrupted_partial_retry(false, true).await;
}

#[tokio::test]
async fn w4_04_user_edit_rejects_stale_partial_batch_without_success_accounting() {
    let rows = [
        row("a", "First", "selected.json"),
        row("b", "Second", "selected.json"),
    ];
    let catalog = Catalog::new(&rows);
    let gate = Arc::new(Gate::default());
    let provider = ScriptedPartial::new(
        "w4-stale",
        vec![Step {
            gate: Some(gate.clone()),
            ..Step::new(&["a", "b"])
        }],
    );
    let db = catalog.db.clone();
    let manager = TranslationManager::new(
        provider.clone(),
        db.clone(),
        Arc::new(Glossary::new(db.clone())),
    );
    let (tx, rx) = mpsc::channel(100);
    let job = tokio::spawn(async move {
        manager
            .translate_entries(
                rows.to_vec(),
                options(&["a", "b"]),
                tx,
                "w4-stale".into(),
                CancellationToken::new(),
            )
            .await
    });
    tokio::time::timeout(Duration::from_secs(5), gate.entered.notified())
        .await
        .unwrap();
    db.save_translation("a", "Human revision", "manual")
        .await
        .unwrap();
    let edited = db.get_entry("a").unwrap().unwrap();
    gate.release.notify_one();
    let error = job.await.unwrap().unwrap_err();
    assert!(
        error.to_string().contains("translation_conflict"),
        "{error}"
    );
    assert_eq!(
        serde_json::to_value(db.get_entry("a").unwrap()).unwrap(),
        serde_json::to_value(Some(edited)).unwrap()
    );
    assert_eq!(
        catalog.writes(),
        expected_writes(&["a"]),
        "only the manual edit committed; stale batch rolled back"
    );
    assert!(successes(&events(rx).await).is_empty());
    assert_eq!(
        db.get_entry("b").unwrap().unwrap().status,
        StringStatus::Pending
    );
    let runs = db.get_translation_runs().unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(
        (
            runs[0].strings_translated,
            runs[0].tokens_used,
            runs[0].cost_usd
        ),
        (0, 14, 0.02)
    );
}

#[tokio::test]
async fn w4_04_manual_edit_of_partial_failure_is_skipped_in_fallback_total() {
    let rows = [
        row("a", "First", "selected.json"),
        row("b", "Second", "selected.json"),
    ];
    let catalog = Catalog::new(&rows);
    let gate = Arc::new(Gate::default());
    let provider = ScriptedPartial::new(
        "w4-manual-failure",
        vec![
            Step {
                gate: Some(gate.clone()),
                ..Step::new(&["a"])
            },
            Step::new(&[]),
            Step::new(&[]),
        ],
    );
    let fallback = ScriptedPartial::new("w4-unused-fallback", vec![]);
    let db = catalog.db.clone();
    let (tx, rx) = mpsc::channel(100);
    let job = tokio::spawn({
        let db = db.clone();
        let provider = provider.clone();
        let fallback = fallback.clone();
        async move {
            run_fallback_chain(
                &[provider.id.into(), fallback.id.into()],
                &|id| {
                    if id == provider.id {
                        Some(provider.clone())
                    } else {
                        Some(fallback.clone())
                    }
                },
                db.clone(),
                Arc::new(Glossary::new(db)),
                options(&["a", "b"]),
                tx,
                "w4-manual-failure".into(),
                CancellationToken::new(),
            )
            .await
        }
    });
    tokio::time::timeout(Duration::from_secs(5), gate.entered.notified())
        .await
        .unwrap();
    db.save_translation("b", "Human revision", "manual")
        .await
        .unwrap();
    gate.release.notify_one();
    job.await.unwrap().unwrap();
    assert_eq!(provider.calls(), [vec!["a", "b"], vec!["b"], vec!["b"]]);
    assert!(fallback.calls().is_empty());
    assert_eq!(catalog.writes(), expected_writes(&["a", "b"]));
    assert_eq!(
        db.get_entry("b").unwrap().unwrap().translation.as_deref(),
        Some("Human revision")
    );
    let events = events(rx).await;
    assert_eq!(successes(&events), expected_writes(&["a"]));
    let runs = db.get_translation_runs().unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].strings_translated, 1);
    assert!(
        matches!(
            events.last(),
            Some(ProgressEvent::Completed {
                total_translated: 1,
                remaining: 0,
                ..
            })
        ),
        "manual edits must be skipped, not included in job successes: {events:?}"
    );
}
