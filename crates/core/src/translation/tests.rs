use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::database::Database;
use crate::error::{LocustError, Result};
use crate::glossary::Glossary;
use crate::models::{
    ProgressEvent, StringEntry, StringStatus, TranslationRequest, TranslationResult,
};
use crate::placeholder::{PlaceholderProcessor, PlaceholderProfile};

use super::manager::RepeatedSources;
use super::protection::MAX_BINARY_SLOT_LENGTH_RETRIES;
use super::*;

#[tokio::test(start_paused = true)]
async fn antigravity_allows_slow_cli_calls_but_remains_cancellable() {
    let db = Arc::new(Database::open_in_memory().unwrap());
    let provider = Arc::new(LimitProvider::new("antigravity", "Antigravity", 1));
    let mut manager = TranslationManager::new(provider, db.clone(), Arc::new(Glossary::new(db)));
    assert_eq!(default_requests_per_minute("antigravity"), 60);
    let start = Instant::now();
    with_retry(&manager.retry, || async {
        tokio::time::sleep(Duration::from_secs(75)).await;
        Ok(())
    })
    .await
    .unwrap();
    assert_eq!(start.elapsed(), Duration::from_secs(75));

    let cancel = CancellationToken::new();
    manager.retry.cancel = Some(cancel.clone());
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(1)).await;
        cancel.cancel();
    });
    let start = Instant::now();
    let error = with_retry(&manager.retry, || async {
        tokio::time::sleep(Duration::from_secs(300)).await;
        Ok(())
    })
    .await
    .unwrap_err();
    assert!(error.to_string().contains("cancelled"));
    assert_eq!(start.elapsed(), Duration::from_secs(1));
}

pub(super) struct MockProvider {
    pub(super) call_count: AtomicUsize,
    entry_ids: Mutex<Vec<String>>,
}

impl MockProvider {
    pub(super) fn new() -> Self {
        Self {
            call_count: AtomicUsize::new(0),
            entry_ids: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl TranslationProvider for MockProvider {
    fn id(&self) -> &str {
        "mock"
    }
    fn name(&self) -> &str {
        "Mock Provider"
    }
    fn is_free(&self) -> bool {
        true
    }
    fn requires_api_key(&self) -> bool {
        false
    }
    async fn translate(&self, requests: &[TranslationRequest]) -> Result<Vec<TranslationResult>> {
        self.call_count.fetch_add(requests.len(), Ordering::SeqCst);
        self.entry_ids
            .lock()
            .unwrap()
            .extend(requests.iter().map(|request| request.entry_id.clone()));
        Ok(requests
            .iter()
            .map(|r| TranslationResult {
                entry_id: r.entry_id.clone(),
                translation: format!("{}: {}", r.target_lang, r.source),
                detected_source_lang: None,
                provider: "mock".to_string(),
                tokens_used: None,
                input_tokens: None,
                output_tokens: None,
                cost_usd: Some(0.0001),
            })
            .collect())
    }
    async fn estimate_cost(&self, char_count: usize, _target_lang: &str) -> Option<f64> {
        Some(char_count as f64 * 0.00001)
    }
    async fn health_check(&self) -> Result<()> {
        Ok(())
    }
}

struct FailOnceMockProvider {
    call_count: AtomicUsize,
}

impl FailOnceMockProvider {
    fn new() -> Self {
        Self {
            call_count: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl TranslationProvider for FailOnceMockProvider {
    fn id(&self) -> &str {
        "fail-once"
    }
    fn name(&self) -> &str {
        "Fail Once"
    }
    fn is_free(&self) -> bool {
        true
    }
    fn requires_api_key(&self) -> bool {
        false
    }
    async fn translate(&self, requests: &[TranslationRequest]) -> Result<Vec<TranslationResult>> {
        let call = self.call_count.fetch_add(1, Ordering::SeqCst);
        if call == 0 {
            return Err(LocustError::ProviderError("simulated failure".to_string()));
        }
        Ok(requests
            .iter()
            .map(|r| TranslationResult {
                entry_id: r.entry_id.clone(),
                translation: format!("translated: {}", r.source),
                detected_source_lang: None,
                provider: "fail-once".to_string(),
                tokens_used: None,
                input_tokens: None,
                output_tokens: None,
                cost_usd: Some(0.0001),
            })
            .collect())
    }
    async fn estimate_cost(&self, _char_count: usize, _target_lang: &str) -> Option<f64> {
        None
    }
    async fn health_check(&self) -> Result<()> {
        Ok(())
    }
}

fn make_entries(count: usize) -> Vec<StringEntry> {
    (0..count)
        .map(|i| {
            StringEntry::new(
                format!("e{}", i),
                format!("Source {}", i),
                PathBuf::from("test.json"),
            )
        })
        .collect()
}

pub(super) fn setup() -> (Arc<Database>, Arc<Glossary>) {
    let db = Arc::new(Database::open_in_memory().unwrap());
    let glossary = Arc::new(Glossary::new(db.clone()));
    (db, glossary)
}

#[derive(Clone, Copy)]
enum RepeatedReply {
    Success,
    TransientOnce,
    Error,
    Partial,
    DuplicateIds,
    Empty,
    LengthRetry,
    Cancel,
}

struct RepeatedProvider {
    reply: RepeatedReply,
    calls: Mutex<Vec<Vec<TranslationRequest>>>,
    cancel: CancellationToken,
}

impl RepeatedProvider {
    fn new(reply: RepeatedReply) -> Self {
        Self {
            reply,
            calls: Mutex::new(Vec::new()),
            cancel: CancellationToken::new(),
        }
    }

    fn sent(&self) -> usize {
        self.calls.lock().unwrap().iter().map(Vec::len).sum()
    }
}

#[async_trait]
impl TranslationProvider for RepeatedProvider {
    fn id(&self) -> &str {
        "repeat-test"
    }
    fn name(&self) -> &str {
        "Counting provider"
    }
    fn is_free(&self) -> bool {
        false
    }
    fn requires_api_key(&self) -> bool {
        false
    }
    async fn translate(&self, requests: &[TranslationRequest]) -> Result<Vec<TranslationResult>> {
        let call = {
            let mut calls = self.calls.lock().unwrap();
            let call = calls.len();
            calls.push(requests.to_vec());
            call
        };
        match self.reply {
            RepeatedReply::TransientOnce if call == 0 => {
                return Err(LocustError::ProviderError("503 unavailable".into()));
            }
            RepeatedReply::Error => {
                return Err(LocustError::ProviderError("401 unauthorized".into()));
            }
            RepeatedReply::Cancel => self.cancel.cancel(),
            _ => {}
        }
        let mut results: Vec<_> = requests
            .iter()
            .map(|r| TranslationResult {
                entry_id: r.entry_id.clone(),
                translation: match self.reply {
                    RepeatedReply::Empty => String::new(),
                    RepeatedReply::LengthRetry if call == 0 => "Much too long".into(),
                    RepeatedReply::LengthRetry => "OK".into(),
                    _ => format!("en: {}", r.source),
                },
                detected_source_lang: None,
                provider: self.id().into(),
                tokens_used: Some(7),
                input_tokens: Some(4),
                output_tokens: Some(3),
                cost_usd: Some(0.002),
            })
            .collect();
        if matches!(self.reply, RepeatedReply::Partial) {
            results.pop();
        }
        if matches!(self.reply, RepeatedReply::DuplicateIds) && results.len() > 1 {
            results[1].entry_id = results[0].entry_id.clone();
        }
        Ok(results)
    }
    async fn estimate_cost(&self, _chars: usize, _lang: &str) -> Option<f64> {
        Some(0.002)
    }
    async fn health_check(&self) -> Result<()> {
        Ok(())
    }
}

fn repeated_entries(count: usize, source: &str) -> Vec<StringEntry> {
    (0..count)
        .map(|i| StringEntry::new(format!("r-{i}"), source, PathBuf::from("test.json")))
        .collect()
}

async fn run_repeated_job(
    entries: Vec<StringEntry>,
    provider: Arc<RepeatedProvider>,
    mut opts: TranslationOptions,
) -> (Arc<Database>, Vec<ProgressEvent>) {
    let (db, glossary) = setup();
    db.save_entries(&entries).unwrap();
    opts.use_memory = false;
    opts.use_glossary = false;
    let manager = TranslationManager::new(provider.clone(), db.clone(), glossary)
        .with_retry_config(RetryConfig {
            initial_delay_ms: 1,
            max_delay_ms: 1,
            ..Default::default()
        });
    let (tx, mut rx) = mpsc::channel(100);
    manager
        .translate_entries(
            entries,
            opts,
            tx,
            "repeat-test".into(),
            provider.cancel.clone(),
        )
        .await
        .unwrap();
    let mut events = Vec::new();
    while let Some(event) = rx.recv().await {
        events.push(event);
    }
    (db, events)
}

#[tokio::test]
async fn format_profile_controls_masking_and_repeated_source_grouping() {
    let provider = Arc::new(RepeatedProvider::new(RepeatedReply::Success));
    let entries = ["rpgmaker-mv", "renpy"]
        .into_iter()
        .map(|format| {
            StringEntry::new(format, r"[Noble] Hello \V[1]", PathBuf::from("test.json"))
                .with_format_id(format)
        })
        .collect();
    let (db, _) = run_repeated_job(entries, provider.clone(), Default::default()).await;
    assert_eq!(provider.sent(), 2);
    let calls = provider.calls.lock().unwrap();
    let requests: HashMap<_, _> = calls
        .iter()
        .flatten()
        .map(|r| (r.entry_id.as_str(), r.source.as_str()))
        .collect();
    assert_eq!(requests["rpgmaker-mv"], "[Noble] Hello {PL_0}");
    assert_eq!(requests["renpy"], "{PL_0} Hello {PL_1}");
    for format in ["rpgmaker-mv", "renpy"] {
        let entry = db.get_entry(format).unwrap().unwrap();
        assert_eq!(
            entry.translation.as_deref(),
            Some(r"en: [Noble] Hello \V[1]")
        );
        assert!(entry.require_preserved_translation_controls().is_ok());
    }
}

#[tokio::test]
async fn repeated_sources_stats_count_provider_usage_once_and_all_rows() {
    let provider = Arc::new(RepeatedProvider::new(RepeatedReply::Success));
    let mut entries = repeated_entries(10, "Source");
    for (i, entry) in entries.iter_mut().enumerate() {
        entry.source = format!("Source {}", i % 3);
    }
    let (db, events) = run_repeated_job(entries, provider.clone(), Default::default()).await;
    assert_eq!(provider.sent(), 3);
    assert_eq!(provider.calls.lock().unwrap().len(), 1);
    let run = db.get_translation_runs().unwrap().remove(0);
    assert_eq!(run.strings_translated, 10);
    assert_eq!(
        (run.tokens_used, run.input_tokens, run.output_tokens),
        (21, 12, 9)
    );
    assert!((run.cost_usd - 0.006).abs() < 1e-10);
    assert!(run.cost_is_complete);
    assert!(events.iter().any(|e| matches!(
        e,
        ProgressEvent::BatchCompleted {
            completed: 10,
            total: 10,
            ..
        }
    )));
    assert!(events.iter().any(|e| matches!(
        e,
        ProgressEvent::Completed {
            total_translated: 10,
            ..
        }
    )));
}

#[tokio::test]
async fn repeated_sources_keep_context_tags_limits_encoding_and_physical_source_separate() {
    let provider = Arc::new(RepeatedProvider::new(RepeatedReply::Success));
    let mut entries = repeated_entries(9, "ABC");
    entries[2].context = Some("speaker".into());
    entries[3].tags = vec!["name".into()];
    entries[4].char_limit = Some(20);
    for (i, encoding, physical) in [
        (5, "utf8", "long physical"),
        (6, "utf8", "longer physical"),
        (7, "utf16le", "long physical"),
    ] {
        entries[i]
            .metadata
            .insert("binary_slot".into(), serde_json::json!(encoding));
        entries[i].metadata.insert(
            crate::models::INJECTION_SOURCE_METADATA_KEY.into(),
            serde_json::json!(physical),
        );
    }
    entries[8].metadata.insert(
        crate::models::INJECTION_SOURCE_METADATA_KEY.into(),
        serde_json::json!("baseline"),
    );
    let (db, _) = run_repeated_job(entries, provider.clone(), Default::default()).await;
    assert_eq!(
        provider.sent(),
        8,
        "only the first two entries are interchangeable"
    );
    assert_eq!(db.get_translation_runs().unwrap()[0].strings_translated, 9);
}

#[tokio::test]
async fn repeated_sources_restore_controls_and_do_not_merge_sanitized_collisions() {
    let provider = Arc::new(RepeatedProvider::new(RepeatedReply::Success));
    let mut entries = repeated_entries(5, r"Hello {alice} \V[1]");
    entries[4].source = r"Hello {bob} \V[1]".into();
    let originals = entries.clone();
    let (db, _) = run_repeated_job(entries, provider.clone(), Default::default()).await;
    assert_eq!(provider.sent(), 2);
    for original in originals {
        let saved = db.get_entry(&original.id).unwrap().unwrap();
        assert_eq!(saved.translation, Some(format!("en: {}", original.source)));
        assert_eq!(saved.status, StringStatus::Translated);
        assert_eq!(saved.provider_used.as_deref(), Some("repeat-test"));
    }
}

#[tokio::test]
async fn repeated_sources_transport_retry_retries_one_representative() {
    let provider = Arc::new(RepeatedProvider::new(RepeatedReply::TransientOnce));
    let (db, _) = run_repeated_job(
        repeated_entries(6, "Source"),
        provider.clone(),
        Default::default(),
    )
    .await;
    assert_eq!(provider.sent(), 2);
    assert_eq!(provider.calls.lock().unwrap().len(), 2);
    let run = db.get_translation_runs().unwrap().remove(0);
    assert_eq!(run.strings_translated, 6);
    assert_eq!(run.tokens_used, 7);
    assert!(
        !run.cost_is_complete,
        "failed paid attempt has unknown cost"
    );
    for i in 0..6 {
        assert_eq!(
            db.get_entry(&format!("r-{i}")).unwrap().unwrap().status,
            StringStatus::Translated
        );
    }
}

#[tokio::test]
async fn repeated_sources_errors_partial_duplicate_and_empty_results_save_no_members() {
    for (reply, sent) in [
        (RepeatedReply::Error, 1),
        (RepeatedReply::Partial, 2),
        (RepeatedReply::DuplicateIds, 2),
        (RepeatedReply::Empty, 1),
    ] {
        let provider = Arc::new(RepeatedProvider::new(reply));
        let mut entries = repeated_entries(6, "Source");
        if sent == 2 {
            entries[3].source = "Other".into();
        }
        let (db, events) = run_repeated_job(entries, provider.clone(), Default::default()).await;
        assert_eq!(provider.sent(), sent);
        for i in 0..6 {
            let entry = db.get_entry(&format!("r-{i}")).unwrap().unwrap();
            assert_eq!(entry.status, StringStatus::Pending);
            assert!(entry.translation.is_none());
        }
        assert!(events
            .iter()
            .any(|e| matches!(e, ProgressEvent::BatchFailed { .. })));
        assert!(!events
            .iter()
            .any(|e| matches!(e, ProgressEvent::StringTranslated { .. })));
        assert!(events.iter().any(|e| matches!(
            e,
            ProgressEvent::Completed {
                total_translated: 0,
                ..
            }
        )));
    }
}

#[tokio::test]
async fn repeated_sources_length_retry_is_shared_and_usage_is_not_multiplied() {
    let provider = Arc::new(RepeatedProvider::new(RepeatedReply::LengthRetry));
    let mut entries = repeated_entries(5, "ABC");
    for entry in &mut entries {
        entry
            .metadata
            .insert("binary_slot".into(), serde_json::json!("utf8"));
    }
    let (db, _) = run_repeated_job(entries, provider.clone(), Default::default()).await;
    assert_eq!(provider.sent(), 2);
    let run = db.get_translation_runs().unwrap().remove(0);
    assert_eq!(run.strings_translated, 5);
    assert_eq!(
        (run.tokens_used, run.input_tokens, run.output_tokens),
        (14, 8, 6)
    );
    assert!((run.cost_usd - 0.004).abs() < 1e-10);
    for i in 0..5 {
        let entry = db.get_entry(&format!("r-{i}")).unwrap().unwrap();
        assert_eq!(entry.status, StringStatus::Translated);
        assert_eq!(entry.translation.as_deref(), Some("OK"));
    }
}

#[tokio::test]
async fn repeated_sources_cost_limit_estimates_only_representatives() {
    let (db, glossary) = setup();
    let entries = repeated_entries(10, "Source");
    db.save_entries(&entries).unwrap();
    let provider = Arc::new(MockProvider::new());
    let manager = TranslationManager::new(provider.clone(), db.clone(), glossary);
    let (tx, _rx) = mpsc::channel(100);
    manager
        .translate_entries(
            entries,
            TranslationOptions {
                use_memory: false,
                cost_limit_usd: Some(0.0002),
                ..Default::default()
            },
            tx,
            "budget".into(),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(provider.call_count.load(Ordering::SeqCst), 1);
    assert_eq!(db.get_translation_runs().unwrap()[0].strings_translated, 10);
}

#[tokio::test]
async fn repeated_sources_cancellation_before_dispatch_and_before_fanout_save_nothing() {
    for before_dispatch in [true, false] {
        let provider = Arc::new(RepeatedProvider::new(RepeatedReply::Cancel));
        if before_dispatch {
            provider.cancel.cancel();
        }
        let (db, events) = run_repeated_job(
            repeated_entries(5, "Source"),
            provider.clone(),
            Default::default(),
        )
        .await;
        assert_eq!(provider.sent(), usize::from(!before_dispatch));
        assert!(events.iter().any(|e| matches!(e, ProgressEvent::Paused)));
        for i in 0..5 {
            assert_eq!(
                db.get_entry(&format!("r-{i}")).unwrap().unwrap().status,
                StringStatus::Pending
            );
        }
    }
}

#[tokio::test]
async fn repeated_sources_translate_ten_entries_with_three_provider_strings() {
    let (db, glossary) = setup();
    let entries: Vec<_> = (0..10)
        .map(|i| {
            StringEntry::new(
                format!("repeat-{i}"),
                format!("Source {}", i % 3),
                PathBuf::from(format!("file-{i}.json")),
            )
        })
        .collect();
    db.save_entries(&entries).unwrap();
    let provider = Arc::new(MockProvider::new());
    let manager = TranslationManager::new(provider.clone(), db.clone(), glossary);
    let (tx, mut rx) = mpsc::channel(100);
    manager
        .translate_entries(
            entries.clone(),
            TranslationOptions {
                batch_size: 1,
                max_concurrent: 3,
                use_memory: false,
                use_glossary: false,
                ..Default::default()
            },
            tx,
            "repeated-sources".into(),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let mut translated = HashSet::new();
    let mut finished = false;
    while let Some(event) = rx.recv().await {
        match event {
            ProgressEvent::Started { total, .. } => assert_eq!(total, 10),
            ProgressEvent::StringTranslated { entry_id, .. } => {
                assert!(translated.insert(entry_id));
            }
            ProgressEvent::BatchCompleted {
                completed, total, ..
            } => {
                assert_eq!(total, 10);
                assert!(completed <= total);
            }
            ProgressEvent::Completed {
                total_translated, ..
            } => {
                assert_eq!(total_translated, 10);
                finished = true;
            }
            _ => {}
        }
    }
    assert!(finished);
    assert_eq!(translated.len(), 10);
    for original in &entries {
        let saved = db.get_entry(&original.id).unwrap().unwrap();
        assert_eq!(saved.status, StringStatus::Translated);
        assert_eq!(saved.provider_used.as_deref(), Some("mock"));
        assert_eq!(saved.translation, Some(format!("en: {}", original.source)));
    }
    let run = db.get_translation_runs().unwrap().remove(0);
    assert_eq!(run.strings_translated, 10);
    let sent = provider.call_count.load(Ordering::SeqCst);
    eprintln!("10 entries, 3 distinct sources: provider strings sent = {sent}");
    assert_eq!(sent, 3);
    assert!((run.cost_usd - 0.0003).abs() < 1e-10);
}

#[tokio::test]
async fn test_translate_entries_all_translated() {
    let (db, glossary) = setup();
    let entries = make_entries(5);
    db.save_entries(&entries).unwrap();
    let provider = Arc::new(MockProvider::new());
    let manager = TranslationManager::new(provider, db.clone(), glossary);
    let (tx, mut rx) = mpsc::channel(100);
    let cancel = CancellationToken::new();

    manager
        .translate_entries(
            entries,
            TranslationOptions::default(),
            tx,
            "job1".into(),
            cancel,
        )
        .await
        .unwrap();

    rx.close();
    while rx.recv().await.is_some() {}

    for i in 0..5 {
        let entry = db.get_entry(&format!("e{}", i)).unwrap().unwrap();
        assert_eq!(entry.status, StringStatus::Translated);
        assert!(entry.translation.is_some());
    }
}

#[tokio::test]
async fn test_translation_run_recorded() {
    let (db, glossary) = setup();
    let entries = make_entries(5);
    db.save_entries(&entries).unwrap();
    let provider = Arc::new(MockProvider::new());
    let manager = TranslationManager::new(provider, db.clone(), glossary);
    let (tx, mut rx) = mpsc::channel(100);

    manager
        .translate_entries(
            entries,
            TranslationOptions::default(),
            tx,
            "job-stats".into(),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    rx.close();
    while rx.recv().await.is_some() {}

    let runs = db.get_translation_runs().unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].strings_translated, 5);
    assert_eq!(runs[0].provider, "mock");
    assert_eq!(runs[0].source_lang, "ja");
    assert_eq!(runs[0].target_lang, "en");
}

#[tokio::test]
async fn test_concurrent_batches_translate_all() {
    let (db, glossary) = setup();
    let entries = make_entries(25);
    db.save_entries(&entries).unwrap();
    let provider = Arc::new(MockProvider::new());
    let manager = TranslationManager::new(provider, db.clone(), glossary);
    let (tx, mut rx) = mpsc::channel(100);
    let cancel = CancellationToken::new();

    let opts = TranslationOptions {
        batch_size: 3,
        max_concurrent: 8,
        ..Default::default()
    };
    manager
        .translate_entries(entries, opts, tx, "job-conc".into(), cancel)
        .await
        .unwrap();

    rx.close();
    while rx.recv().await.is_some() {}

    for i in 0..25 {
        let entry = db.get_entry(&format!("e{}", i)).unwrap().unwrap();
        assert_eq!(entry.status, StringStatus::Translated);
        assert!(entry.translation.is_some());
    }
}

#[tokio::test]
async fn test_translate_uses_memory_cache() {
    let (db, glossary) = setup();
    let entries = make_entries(5);
    db.save_entries(&entries).unwrap();

    // Pre-populate memory for entry 0
    let hash = entries[0].source_hash();
    db.save_memory(&hash, &entries[0].source, "Cached translation", "ja-en")
        .await
        .unwrap();

    let provider = Arc::new(MockProvider::new());
    let provider_ref = provider.clone();
    let manager = TranslationManager::new(provider, db.clone(), glossary);
    let (tx, mut rx) = mpsc::channel(100);
    let cancel = CancellationToken::new();

    manager
        .translate_entries(
            entries,
            TranslationOptions::default(),
            tx,
            "job2".into(),
            cancel,
        )
        .await
        .unwrap();

    rx.close();
    while rx.recv().await.is_some() {}

    assert_eq!(provider_ref.call_count.load(Ordering::SeqCst), 4);
    let e0 = db.get_entry("e0").unwrap().unwrap();
    assert_eq!(e0.translation, Some("Cached translation".to_string()));
}

async fn memory_run_events(
    mut rx: mpsc::Receiver<ProgressEvent>,
) -> (Vec<(String, String)>, usize) {
    let mut translated = Vec::new();
    let mut completed = None;
    while let Some(event) = rx.recv().await {
        match event {
            ProgressEvent::StringTranslated {
                entry_id,
                translation,
            } => {
                translated.push((entry_id, translation));
            }
            ProgressEvent::Completed {
                total_translated, ..
            } => {
                completed = Some(total_translated);
            }
            ProgressEvent::Failed { error, .. } | ProgressEvent::BatchFailed { error, .. } => {
                panic!("unexpected translation failure: {error}");
            }
            _ => {}
        }
    }
    (translated, completed.expect("run must complete"))
}

async fn check_memory_prepass_counts(hits: bool) {
    let (db, glossary) = setup();
    let entries = make_entries(2_000);
    db.save_entries(&entries).unwrap();
    if hits {
        let items: Vec<_> = entries
            .iter()
            .map(|entry| {
                (
                    entry.source_hash(),
                    entry.source.clone(),
                    format!("Cached {}", entry.id),
                )
            })
            .collect();
        db.save_memory_batch(&items, "ja-en").await.unwrap();
    }
    let provider = Arc::new(MockProvider::new());
    let manager = TranslationManager::new(provider.clone(), db.clone(), glossary);
    let (tx, rx) = mpsc::channel(4_100);
    crate::database::MEMORY_LOOKUP_QUERIES.with(|count| count.set(Some(0)));
    crate::database::GUARDED_TRANSLATION_TRANSACTIONS.with(|count| count.set(Some(0)));
    manager
        .translate_entries(
            entries.clone(),
            TranslationOptions {
                batch_size: 2_000,
                max_batch_tokens: None,
                use_glossary: false,
                ..Default::default()
            },
            tx,
            "memory-prepass-counts".into(),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let queries = crate::database::MEMORY_LOOKUP_QUERIES
        .with(|count| count.replace(None))
        .unwrap();
    let transactions = crate::database::GUARDED_TRANSLATION_TRANSACTIONS
        .with(|count| count.replace(None))
        .unwrap();
    assert!(
        (1..=4).contains(&queries),
        "2,000 memory lookups must use at most 4 queries, got {queries}"
    );
    assert_eq!(
        transactions, 1,
        "2,000 hits must share one guarded transaction"
    );
    assert_eq!(
        provider.call_count.load(Ordering::SeqCst),
        if hits { 0 } else { 2_000 }
    );
    let (events, completed) = memory_run_events(rx).await;
    assert_eq!(completed, 2_000);
    assert_eq!(
        db.get_translation_runs().unwrap()[0].strings_translated,
        2_000
    );
    let expected: Vec<_> = entries
        .iter()
        .map(|entry| {
            let translation = if hits {
                format!("Cached {}", entry.id)
            } else {
                format!("en: {}", entry.source)
            };
            let saved = db.get_entry(&entry.id).unwrap().unwrap();
            assert_eq!(saved.translation.as_deref(), Some(translation.as_str()));
            assert_eq!(
                saved.provider_used.as_deref(),
                Some(if hits { "memory" } else { "mock" })
            );
            assert_eq!(saved.status, StringStatus::Translated);
            (entry.id.clone(), translation)
        })
        .collect();
    assert_eq!(events, expected);
}

#[tokio::test]
async fn memory_prepass_2000_misses_use_at_most_four_queries() {
    check_memory_prepass_counts(false).await;
}

#[tokio::test]
async fn memory_prepass_2000_hits_use_one_transaction() {
    check_memory_prepass_counts(true).await;
}

#[tokio::test]
async fn memory_prepass_preserves_mixed_entry_and_event_order() {
    let (db, glossary) = setup();
    let mut entries = make_entries(7);
    for (entry, id) in entries.iter_mut().zip(["z", "b", "y", "a", "x", "c", "w"]) {
        entry.id = id.into();
    }
    entries[2].source = entries[0].source.clone();
    entries[3].source = "Hello [name]".into();
    db.save_entries(&entries).unwrap();
    for (index, translation) in [
        (0, "Cached zero"),
        (3, "Missing placeholder"),
        (4, "Cached four"),
        (6, "   "),
    ] {
        db.save_memory(
            &entries[index].source_hash(),
            &entries[index].source,
            translation,
            "ja-en",
        )
        .await
        .unwrap();
    }
    let provider = Arc::new(MockProvider::new());
    let manager = TranslationManager::new(provider.clone(), db.clone(), glossary);
    let (tx, rx) = mpsc::channel(100);
    manager
        .translate_entries(
            entries,
            TranslationOptions {
                use_glossary: false,
                ..Default::default()
            },
            tx,
            "memory-mixed".into(),
            CancellationToken::new(),
        )
        .await
        .unwrap();

    // Written-out per-entry outcome: duplicate y hits too; invalid a/w stay
    // with misses b/c, in input order, when passed to the provider.
    assert_eq!(*provider.entry_ids.lock().unwrap(), ["b", "a", "c", "w"]);
    let expected = [
        ("z", "Cached zero", "memory"),
        ("y", "Cached zero", "memory"),
        ("x", "Cached four", "memory"),
        ("b", "en: Source 1", "mock"),
        ("a", "en: Hello [name]", "mock"),
        ("c", "en: Source 5", "mock"),
        ("w", "en: Source 6", "mock"),
    ];
    for (id, translation, provider) in expected {
        let saved = db.get_entry(id).unwrap().unwrap();
        assert_eq!(saved.translation.as_deref(), Some(translation));
        assert_eq!(saved.provider_used.as_deref(), Some(provider));
        assert_eq!(saved.status, StringStatus::Translated);
    }
    let (events, completed) = memory_run_events(rx).await;
    assert_eq!(completed, 7);
    assert_eq!(
        events,
        expected
            .into_iter()
            .map(|(id, text, _)| (id.into(), text.into()))
            .collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn memory_prepass_lookup_error_discards_earlier_chunks_and_continues() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("memory-error.db");
    let db = Arc::new(Database::open(&path).unwrap());
    let entries = make_entries(501);
    db.save_entries(&entries).unwrap();
    let items: Vec<_> = entries
        .iter()
        .map(|entry| (entry.source_hash(), entry.source.clone(), "Cached".into()))
        .collect();
    db.save_memory_batch(&items, "ja-en").await.unwrap();
    // SQLite accepts a blob in this TEXT column; decoding the last chunk
    // as String must fail after the first 500 hits have been read.
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute(
            "UPDATE translation_memory SET translation = X'80' WHERE source_hash = ?1",
            rusqlite::params![entries[500].source_hash()],
        )
        .unwrap();
    let hashes: Vec<_> = entries.iter().map(StringEntry::source_hash).collect();
    assert!(db.lookup_memory_batch(&hashes, "ja-en").is_err());
    let glossary = Arc::new(Glossary::new(db.clone()));
    let provider = Arc::new(MockProvider::new());
    let manager = TranslationManager::new(provider.clone(), db.clone(), glossary);
    let (tx, rx) = mpsc::channel(1_100);
    manager
        .translate_entries(
            entries.clone(),
            TranslationOptions {
                use_glossary: false,
                max_batch_tokens: None,
                batch_size: 501,
                ..Default::default()
            },
            tx,
            "memory-lookup-error".into(),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(
        *provider.entry_ids.lock().unwrap(),
        entries
            .iter()
            .map(|entry| entry.id.clone())
            .collect::<Vec<_>>()
    );
    let (events, completed) = memory_run_events(rx).await;
    assert_eq!(completed, 501);
    assert_eq!(
        events,
        entries
            .iter()
            .map(|entry| (entry.id.clone(), format!("en: {}", entry.source)))
            .collect::<Vec<_>>()
    );
    for entry in entries {
        let saved = db.get_entry(&entry.id).unwrap().unwrap();
        assert_eq!(saved.provider_used.as_deref(), Some("mock"));
        assert_eq!(saved.translation, Some(format!("en: {}", entry.source)));
    }
}

#[tokio::test]
async fn memory_prepass_guard_conflict_errors_without_saves_or_events() {
    for missing in [false, true] {
        let (db, glossary) = setup();
        let entries = make_entries(2);
        db.save_entries(&entries).unwrap();
        for entry in &entries {
            db.save_memory(&entry.source_hash(), &entry.source, "Cached", "ja-en")
                .await
                .unwrap();
        }
        if missing {
            db.clear_entries().unwrap();
            db.save_entries(&entries[..1]).unwrap();
        } else {
            db.save_translation("e1", "User edit", "user")
                .await
                .unwrap();
        }
        let old_error = db
            .save_translation_guarded(&entries[1], "Cached", "memory")
            .await
            .unwrap_err();
        let provider = Arc::new(MockProvider::new());
        let manager = TranslationManager::new(provider.clone(), db.clone(), glossary);
        let (tx, mut rx) = mpsc::channel(100);
        let error = manager
            .translate_entries(
                entries,
                TranslationOptions {
                    use_glossary: false,
                    ..Default::default()
                },
                tx,
                "memory-conflict".into(),
                CancellationToken::new(),
            )
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), old_error.to_string());
        assert!(error.to_string().contains("translation_conflict"));
        assert!(db.get_entry("e0").unwrap().unwrap().translation.is_none());
        let edited = db.get_entry("e1").unwrap();
        if missing {
            assert!(edited.is_none());
        } else {
            let edited = edited.unwrap();
            assert_eq!(edited.translation.as_deref(), Some("User edit"));
            assert_eq!(edited.provider_used.as_deref(), Some("user"));
        }
        assert_eq!(provider.call_count.load(Ordering::SeqCst), 0);
        while let Some(event) = rx.recv().await {
            assert!(!matches!(
                event,
                ProgressEvent::StringTranslated { .. } | ProgressEvent::Completed { .. }
            ));
        }
    }
}

#[tokio::test]
async fn memory_prepass_disabled_does_not_query_or_reuse_memory() {
    let (db, glossary) = setup();
    let entries = make_entries(1);
    db.save_entries(&entries).unwrap();
    db.save_memory(
        &entries[0].source_hash(),
        &entries[0].source,
        "Cached",
        "ja-en",
    )
    .await
    .unwrap();
    let provider = Arc::new(MockProvider::new());
    let manager = TranslationManager::new(provider.clone(), db.clone(), glossary);
    let (tx, rx) = mpsc::channel(100);
    crate::database::MEMORY_LOOKUP_QUERIES.with(|count| count.set(Some(0)));
    manager
        .translate_entries(
            entries,
            TranslationOptions {
                use_memory: false,
                use_glossary: false,
                ..Default::default()
            },
            tx,
            "memory-disabled".into(),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(
        crate::database::MEMORY_LOOKUP_QUERIES.with(|count| count.replace(None)),
        Some(0)
    );
    assert_eq!(provider.call_count.load(Ordering::SeqCst), 1);
    let (events, completed) = memory_run_events(rx).await;
    assert_eq!(completed, 1);
    assert_eq!(events, [("e0".into(), "en: Source 0".into())]);
    assert_eq!(
        db.get_entry("e0")
            .unwrap()
            .unwrap()
            .provider_used
            .as_deref(),
        Some("mock")
    );
}

#[tokio::test]
async fn test_cost_limit_aborts() {
    let (db, glossary) = setup();
    let entries = make_entries(5);
    db.save_entries(&entries).unwrap();
    let provider = Arc::new(MockProvider::new());
    let manager = TranslationManager::new(provider, db, glossary);
    let (tx, mut rx) = mpsc::channel(100);
    let cancel = CancellationToken::new();

    let opts = TranslationOptions {
        cost_limit_usd: Some(0.000001),
        use_memory: false,
        ..Default::default()
    };

    let result = manager
        .translate_entries(entries, opts, tx, "job3".into(), cancel)
        .await;

    rx.close();
    while rx.recv().await.is_some() {}

    assert!(matches!(result, Err(LocustError::CostLimitExceeded { .. })));
}

#[tokio::test]
async fn test_cancellation() {
    let (db, glossary) = setup();
    let entries = make_entries(5);
    db.save_entries(&entries).unwrap();
    let provider = Arc::new(MockProvider::new());
    let manager = TranslationManager::new(provider, db, glossary);
    let (tx, mut rx) = mpsc::channel(100);
    let cancel = CancellationToken::new();
    cancel.cancel();

    let opts = TranslationOptions {
        use_memory: false,
        ..Default::default()
    };

    manager
        .translate_entries(entries, opts, tx, "job4".into(), cancel)
        .await
        .unwrap();

    rx.close();
    let mut events = Vec::new();
    while let Some(ev) = rx.recv().await {
        events.push(ev);
    }

    assert!(events.iter().any(|e| matches!(e, ProgressEvent::Paused)));
}

#[tokio::test]
async fn test_progress_sequence() {
    let (db, glossary) = setup();
    let entries = make_entries(3);
    db.save_entries(&entries).unwrap();
    let provider = Arc::new(MockProvider::new());
    let manager = TranslationManager::new(provider, db, glossary);
    let (tx, mut rx) = mpsc::channel(100);
    let cancel = CancellationToken::new();

    let opts = TranslationOptions {
        use_memory: false,
        ..Default::default()
    };

    manager
        .translate_entries(entries, opts, tx, "job5".into(), cancel)
        .await
        .unwrap();

    rx.close();
    let mut events = Vec::new();
    while let Some(ev) = rx.recv().await {
        events.push(ev);
    }

    assert!(matches!(
        events.first(),
        Some(ProgressEvent::Started { .. })
    ));
    assert!(events
        .iter()
        .any(|e| matches!(e, ProgressEvent::BatchCompleted { .. })));
    assert!(matches!(
        events.last(),
        Some(ProgressEvent::Completed { .. })
    ));
}

#[tokio::test]
async fn test_skip_approved() {
    let (db, glossary) = setup();
    let mut entries = make_entries(3);
    entries[0].status = StringStatus::Approved;
    db.save_entries(&entries).unwrap();
    let provider = Arc::new(MockProvider::new());
    let provider_ref = provider.clone();
    let manager = TranslationManager::new(provider, db, glossary);
    let (tx, mut rx) = mpsc::channel(100);
    let cancel = CancellationToken::new();

    let opts = TranslationOptions {
        use_memory: false,
        ..Default::default()
    };

    manager
        .translate_entries(entries, opts, tx, "job6".into(), cancel)
        .await
        .unwrap();

    rx.close();
    while rx.recv().await.is_some() {}

    assert_eq!(provider_ref.call_count.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn test_failed_batch_continues() {
    let (db, glossary) = setup();
    let entries = make_entries(2);
    db.save_entries(&entries).unwrap();
    let provider = Arc::new(FailOnceMockProvider::new());
    let manager = TranslationManager::new(provider, db.clone(), glossary);
    let (tx, mut rx) = mpsc::channel(100);
    let cancel = CancellationToken::new();

    let opts = TranslationOptions {
        batch_size: 1,
        use_memory: false,
        ..Default::default()
    };

    manager
        .translate_entries(entries, opts, tx, "job7".into(), cancel)
        .await
        .unwrap();

    rx.close();
    let mut events = Vec::new();
    while let Some(ev) = rx.recv().await {
        events.push(ev);
    }

    assert!(events
        .iter()
        .any(|e| matches!(e, ProgressEvent::BatchFailed { .. })));
    assert!(events
        .iter()
        .any(|e| matches!(e, ProgressEvent::StringTranslated { .. })));
}

#[tokio::test]
async fn test_glossary_hint_injected_with_one_query() {
    let (db, glossary) = setup();

    db.save_glossary_entry(&crate::database::GlossaryEntry {
        term: "HP".to_string(),
        translation: "Health Points".to_string(),
        lang_pair: "ja-en".to_string(),
        context: None,
        case_sensitive: false,
    })
    .unwrap();

    let mut entries = make_entries(4);
    // Source must contain the glossary term so filtered hints attach.
    entries[0].source = "Current HP is low".to_string();
    entries[0].context = Some("battle screen".to_string());
    db.save_entries(&entries).unwrap();

    struct ContextCapture {
        contexts: std::sync::Mutex<Vec<Option<String>>>,
        glossary_hints: std::sync::Mutex<Vec<Option<String>>>,
    }

    #[async_trait]
    impl TranslationProvider for ContextCapture {
        fn id(&self) -> &str {
            "ctx"
        }
        fn name(&self) -> &str {
            "Context Capture"
        }
        fn is_free(&self) -> bool {
            true
        }
        fn requires_api_key(&self) -> bool {
            false
        }
        async fn translate(
            &self,
            requests: &[TranslationRequest],
        ) -> Result<Vec<TranslationResult>> {
            for r in requests {
                self.contexts.lock().unwrap().push(r.context.clone());
                self.glossary_hints
                    .lock()
                    .unwrap()
                    .push(r.glossary_hint.clone());
            }
            Ok(requests
                .iter()
                .map(|r| TranslationResult {
                    entry_id: r.entry_id.clone(),
                    translation: "translated".to_string(),
                    detected_source_lang: None,
                    provider: "ctx".to_string(),
                    tokens_used: None,
                    input_tokens: None,
                    output_tokens: None,
                    cost_usd: None,
                })
                .collect())
        }
        async fn estimate_cost(&self, _: usize, _: &str) -> Option<f64> {
            None
        }
        async fn health_check(&self) -> Result<()> {
            Ok(())
        }
    }

    let provider = Arc::new(ContextCapture {
        contexts: std::sync::Mutex::new(Vec::new()),
        glossary_hints: std::sync::Mutex::new(Vec::new()),
    });
    let provider_ref = provider.clone();

    let glossary_ref = glossary.clone();
    let manager = TranslationManager::new(provider, db, glossary);
    let (tx, mut rx) = mpsc::channel(100);
    let cancel = CancellationToken::new();

    let opts = TranslationOptions {
        game_context: Some("RPG game".to_string()),
        use_memory: false,
        ..Default::default()
    };

    manager
        .translate_entries(entries, opts, tx, "job8".into(), cancel)
        .await
        .unwrap();

    rx.close();
    while rx.recv().await.is_some() {}

    let contexts = provider_ref.contexts.lock().unwrap();
    assert!(contexts[0].as_ref().unwrap().contains("RPG game"));
    assert!(contexts[0].as_ref().unwrap().contains("battle screen"));

    let hints = provider_ref.glossary_hints.lock().unwrap();
    assert!(hints[0].as_ref().unwrap().contains("HP → Health Points"));
    assert_eq!(glossary_ref.get_all_call_count(), 1);
}

#[tokio::test]
async fn test_glossary_exact_short_circuits_provider() {
    let (db, glossary) = setup();
    glossary.add("Options", "Opcns", "en-es", None).unwrap();
    let entry = StringEntry::new("ui-opt", "Options", PathBuf::from("ui.assets"));
    db.save_entries(std::slice::from_ref(&entry)).unwrap();

    let provider = Arc::new(ScriptedLengthProvider::new(&["SHOULD_NOT_RUN"]));
    let capture = provider.clone();
    let manager = TranslationManager::new(provider, db.clone(), glossary);
    let (tx, mut rx) = mpsc::channel(100);
    let opts = TranslationOptions {
        use_memory: false,
        use_glossary: true,
        source_lang: "en".into(),
        target_lang: "es".into(),
        ..Default::default()
    };
    manager
        .translate_entries(
            vec![entry],
            opts,
            tx,
            "job-gloss-exact".into(),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    rx.close();
    while rx.recv().await.is_some() {}

    assert_eq!(
        capture.calls.load(Ordering::SeqCst),
        0,
        "exact glossary must not call provider"
    );
    let saved = db
        .get_entries(&crate::database::EntryFilter::default())
        .unwrap()
        .into_iter()
        .find(|e| e.id == "ui-opt")
        .unwrap();
    assert_eq!(saved.translation.as_deref(), Some("Opcns"));
    assert_eq!(saved.provider_used.as_deref(), Some("glossary"));
}

#[tokio::test]
async fn test_glossary_exact_oversize_binary_slot_falls_through() {
    let (db, glossary) = setup();
    // Budget for "Options" is 7; glossary form is 9 → must not short-circuit.
    glossary.add("Options", "Opciones", "en-es", None).unwrap();
    let mut entry = StringEntry::new("ui-opt2", "Options", PathBuf::from("ui.assets"));
    entry.metadata.insert(
        "binary_slot".to_string(),
        serde_json::Value::String("utf8".to_string()),
    );
    db.save_entries(std::slice::from_ref(&entry)).unwrap();

    let provider = Arc::new(ScriptedLengthProvider::new(&["Opcns"])); // 5 bytes, fits
    let capture = provider.clone();
    let manager = TranslationManager::new(provider, db.clone(), glossary);
    let (tx, mut rx) = mpsc::channel(100);
    let opts = TranslationOptions {
        use_memory: false,
        use_glossary: true,
        source_lang: "en".into(),
        target_lang: "es".into(),
        ..Default::default()
    };
    manager
        .translate_entries(
            vec![entry],
            opts,
            tx,
            "job-gloss-oversize".into(),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    rx.close();
    while rx.recv().await.is_some() {}

    assert!(
        capture.calls.load(Ordering::SeqCst) >= 1,
        "oversize glossary form must fall through to provider"
    );
    let saved = db
        .get_entries(&crate::database::EntryFilter::default())
        .unwrap()
        .into_iter()
        .find(|e| e.id == "ui-opt2")
        .unwrap();
    assert_eq!(saved.translation.as_deref(), Some("Opcns"));
}

/// Answers every request with a scream stretched to 200 letters.
struct StretchingProvider;

#[async_trait]
impl TranslationProvider for StretchingProvider {
    fn id(&self) -> &str {
        "stretching"
    }
    fn name(&self) -> &str {
        "Stretching"
    }
    fn is_free(&self) -> bool {
        true
    }
    fn requires_api_key(&self) -> bool {
        false
    }
    async fn translate(&self, requests: &[TranslationRequest]) -> Result<Vec<TranslationResult>> {
        Ok(requests
            .iter()
            .map(|r| TranslationResult {
                entry_id: r.entry_id.clone(),
                translation: format!("{{PL_0}}<Old man>¡H{}!", "a".repeat(200)),
                detected_source_lang: None,
                provider: "stretching".to_string(),
                tokens_used: None,
                input_tokens: None,
                output_tokens: None,
                cost_usd: None,
            })
            .collect())
    }
    async fn estimate_cost(&self, _: usize, _: &str) -> Option<f64> {
        None
    }
    async fn health_check(&self) -> Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn stretched_provider_letters_are_saved_capped() {
    let (db, glossary) = setup();
    let entry = StringEntry::new(
        "Map001.json#0#event_1#page_0#cmd_1#msg",
        "\\n<老人>はぁぁ～～～～！",
        PathBuf::from("data/Map001.json"),
    )
    .with_format_id("rpgmaker-mv");
    db.save_entries(std::slice::from_ref(&entry)).unwrap();
    let manager = TranslationManager::new(Arc::new(StretchingProvider), db.clone(), glossary);
    let (tx, mut rx) = mpsc::channel(100);
    let opts = TranslationOptions {
        use_memory: false,
        use_glossary: false,
        ..Default::default()
    };
    manager
        .translate_entries(
            vec![entry],
            opts,
            tx,
            "job-stretch".into(),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    rx.close();
    while rx.recv().await.is_some() {}
    let saved = db
        .get_entries(&crate::database::EntryFilter::default())
        .unwrap()
        .remove(0);
    assert_eq!(saved.translation.as_deref(), Some("\\n<Old man>¡Haaaaa!"));
}

#[tokio::test]
async fn stretched_memory_hits_are_saved_capped() {
    let (db, glossary) = setup();
    let entry = StringEntry::new("m1", "あぁぁぁぁ！", PathBuf::from("data/Map001.json"));
    db.save_entries(std::slice::from_ref(&entry)).unwrap();
    // A memory row written by an older build, before the cap existed.
    db.save_memory(
        &entry.source_hash(),
        &entry.source,
        &format!("A{}h!", "a".repeat(30)),
        "ja-en",
    )
    .await
    .unwrap();
    let manager = TranslationManager::new(Arc::new(StretchingProvider), db.clone(), glossary);
    let (tx, mut rx) = mpsc::channel(100);
    let opts = TranslationOptions {
        source_lang: "ja".into(),
        target_lang: "en".into(),
        use_memory: true,
        use_glossary: false,
        ..Default::default()
    };
    manager
        .translate_entries(
            vec![entry],
            opts,
            tx,
            "job-mem".into(),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    rx.close();
    while rx.recv().await.is_some() {}
    let saved = db
        .get_entries(&crate::database::EntryFilter::default())
        .unwrap()
        .remove(0);
    assert_eq!(saved.provider_used.as_deref(), Some("memory"));
    assert_eq!(saved.translation.as_deref(), Some("Aaaaah!"));
}

/// Captures request context keyed by entry id (for binary-slot budget tests).
struct ContextById {
    by_id: std::sync::Mutex<std::collections::HashMap<String, Option<String>>>,
}

#[async_trait]
impl TranslationProvider for ContextById {
    fn id(&self) -> &str {
        "ctx-by-id"
    }
    fn name(&self) -> &str {
        "Context By Id"
    }
    fn is_free(&self) -> bool {
        true
    }
    fn requires_api_key(&self) -> bool {
        false
    }
    async fn translate(&self, requests: &[TranslationRequest]) -> Result<Vec<TranslationResult>> {
        let mut guard = self.by_id.lock().unwrap();
        for r in requests {
            guard.insert(r.entry_id.clone(), r.context.clone());
        }
        Ok(requests
            .iter()
            .map(|r| TranslationResult {
                entry_id: r.entry_id.clone(),
                translation: "ok".to_string(),
                detected_source_lang: None,
                provider: "ctx-by-id".to_string(),
                tokens_used: None,
                input_tokens: None,
                output_tokens: None,
                cost_usd: None,
            })
            .collect())
    }
    async fn estimate_cost(&self, _: usize, _: &str) -> Option<f64> {
        None
    }
    async fn health_check(&self) -> Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn test_binary_slot_utf8_adds_length_limit_to_context() {
    let (db, glossary) = setup();

    let mut with_slot = StringEntry::new("with_slot", "Hello", PathBuf::from("resources.assets"));
    with_slot.metadata.insert(
        "binary_slot".to_string(),
        serde_json::Value::String("utf8".to_string()),
    );
    let mut pivoted = StringEntry::new("pivot_slot", "English", PathBuf::from("resources.assets"));
    pivoted
        .metadata
        .insert("binary_slot".into(), serde_json::json!("utf8"));
    pivoted.metadata.insert(
        crate::models::INJECTION_SOURCE_METADATA_KEY.into(),
        serde_json::json!("日本語文"),
    );
    pivoted.metadata.insert(
        crate::models::INJECTION_CAPACITY_METADATA_KEY.into(),
        serde_json::json!({"encoding": "utf8", "bytes": 12}),
    );
    let without = StringEntry::new("no_slot", "World", PathBuf::from("script.txt"));
    db.save_entries(&[with_slot.clone(), pivoted.clone(), without.clone()])
        .unwrap();

    let provider = Arc::new(ContextById {
        by_id: std::sync::Mutex::new(std::collections::HashMap::new()),
    });
    let capture = provider.clone();
    let manager = TranslationManager::new(provider, db, glossary);
    let (tx, mut rx) = mpsc::channel(100);
    let cancel = CancellationToken::new();
    let opts = TranslationOptions {
        use_memory: false,
        use_glossary: false,
        ..Default::default()
    };

    manager
        .translate_entries(
            vec![with_slot, pivoted, without],
            opts,
            tx,
            "job-slot-ctx".into(),
            cancel,
        )
        .await
        .unwrap();
    rx.close();
    while rx.recv().await.is_some() {}

    let map = capture.by_id.lock().unwrap();
    let slotted = map
        .get("with_slot")
        .and_then(|c| c.as_ref())
        .expect("slotted entry must have context");
    assert!(
        slotted.contains("LENGTH LIMIT"),
        "binary_slot entry must get a LENGTH LIMIT hint: {slotted}"
    );
    // "Hello" is 5 bytes → tight UI path (HARD MAX), not the longer-line phrasing.
    assert!(
        slotted.contains("HARD MAX 5 bytes") && slotted.contains("utf8"),
        "tight utf8 budget for \"Hello\" is 5: {slotted}"
    );
    assert!(
        slotted.contains("Source: «Hello»"),
        "tight hint must quote source: {slotted}"
    );
    assert!(
        slotted.contains("accented letters cost 2 UTF-8 bytes"),
        "ASCII source tight slot should warn about accents: {slotted}"
    );
    assert!(
        !slotted.contains("encoded as utf8"),
        "tight path should not use the longer-line phrasing: {slotted}"
    );
    let pivot_context = map
        .get("pivot_slot")
        .and_then(|context| context.as_ref())
        .expect("pivoted binary slot context");
    assert!(
        pivot_context.contains("HARD MAX 12 bytes") && pivot_context.contains("Source: «English»"),
        "semantic source must use immutable physical capacity: {pivot_context}"
    );

    let plain = map.get("no_slot").cloned().flatten();
    assert!(
        plain
            .as_ref()
            .map(|c| !c.contains("LENGTH LIMIT"))
            .unwrap_or(true),
        "entry without binary_slot must not get LENGTH LIMIT: {plain:?}"
    );
}

#[tokio::test]
async fn test_binary_slot_utf16le_budget_uses_utf16_bytes() {
    let (db, glossary) = setup();
    // Three CJK chars: UTF-8 is 9 bytes; UTF-16LE is 3 code units * 2 = 6.
    let source = "テスト";
    assert_eq!(source.len(), 9);
    assert_eq!(source.encode_utf16().count() * 2, 6);

    let mut entry = StringEntry::new("u16", source, PathBuf::from("game.pak"));
    entry.metadata.insert(
        "binary_slot".to_string(),
        serde_json::Value::String("utf16le".to_string()),
    );
    db.save_entries(std::slice::from_ref(&entry)).unwrap();

    let provider = Arc::new(ContextById {
        by_id: std::sync::Mutex::new(std::collections::HashMap::new()),
    });
    let capture = provider.clone();
    let manager = TranslationManager::new(provider, db, glossary);
    let (tx, mut rx) = mpsc::channel(100);
    let opts = TranslationOptions {
        use_memory: false,
        use_glossary: false,
        ..Default::default()
    };
    manager
        .translate_entries(
            vec![entry],
            opts,
            tx,
            "job-u16".into(),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    rx.close();
    while rx.recv().await.is_some() {}

    let map = capture.by_id.lock().unwrap();
    let ctx = map
        .get("u16")
        .and_then(|c| c.as_ref())
        .expect("utf16le entry must have LENGTH LIMIT context");
    assert!(
        ctx.contains("HARD MAX 6 bytes") && ctx.contains("utf16le"),
        "utf16le budget must be code units * 2, not utf8 len: {ctx}"
    );
    assert!(
        !ctx.contains("9 bytes"),
        "must not use utf8 length for utf16le slot: {ctx}"
    );
}

/// Scripted provider: `responses[n]` for the n-th `translate()` call (last
/// response repeats if more calls than entries). Counts invocations, not
/// request count.
struct ScriptedLengthProvider {
    calls: AtomicUsize,
    responses: Vec<String>,
    /// Captured contexts per call for assertions.
    contexts: std::sync::Mutex<Vec<Option<String>>>,
}

impl ScriptedLengthProvider {
    fn new(responses: &[&str]) -> Self {
        assert!(!responses.is_empty(), "need at least one scripted response");
        Self {
            calls: AtomicUsize::new(0),
            responses: responses.iter().map(|s| (*s).to_string()).collect(),
            contexts: std::sync::Mutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl TranslationProvider for ScriptedLengthProvider {
    fn id(&self) -> &str {
        "scripted-length"
    }
    fn name(&self) -> &str {
        "Scripted Length"
    }
    fn is_free(&self) -> bool {
        true
    }
    fn requires_api_key(&self) -> bool {
        false
    }
    async fn translate(&self, requests: &[TranslationRequest]) -> Result<Vec<TranslationResult>> {
        let n = self.calls.fetch_add(1, Ordering::SeqCst);
        let idx = n.min(self.responses.len() - 1);
        let text = self.responses[idx].clone();
        {
            let mut ctxs = self.contexts.lock().unwrap();
            for r in requests {
                ctxs.push(r.context.clone());
            }
        }
        Ok(requests
            .iter()
            .map(|r| TranslationResult {
                entry_id: r.entry_id.clone(),
                translation: text.clone(),
                detected_source_lang: None,
                provider: "scripted-length".to_string(),
                tokens_used: None,
                input_tokens: None,
                output_tokens: None,
                cost_usd: None,
            })
            .collect())
    }
    async fn estimate_cost(&self, _: usize, _: &str) -> Option<f64> {
        None
    }
    async fn health_check(&self) -> Result<()> {
        Ok(())
    }
}

fn binary_utf8_entry(id: &str, source: &str) -> StringEntry {
    let mut e = StringEntry::new(id, source, PathBuf::from("x.assets"));
    e.metadata.insert(
        "binary_slot".to_string(),
        serde_json::Value::String("utf8".to_string()),
    );
    e
}

#[tokio::test]
async fn test_binary_slot_retry_ok_stores_fitting_second_attempt() {
    let (db, glossary) = setup();
    // budget = 5 for "Hello"
    let entry = binary_utf8_entry("slot-retry-ok", "Hello");
    db.save_entries(std::slice::from_ref(&entry)).unwrap();

    let provider = Arc::new(ScriptedLengthProvider::new(&[
        "ABCDEFGHIJKLMNOP", // 16 > 5
        "Hola!",            // 5 == 5 fits on first retry
    ]));
    let capture = provider.clone();
    let manager = TranslationManager::new(provider, db.clone(), glossary);
    let (tx, mut rx) = mpsc::channel(100);
    let opts = TranslationOptions {
        use_memory: false,
        use_glossary: false,
        ..Default::default()
    };
    manager
        .translate_entries(
            vec![entry],
            opts,
            tx,
            "job-retry-ok".into(),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    rx.close();
    while rx.recv().await.is_some() {}

    assert_eq!(
        capture.calls.load(Ordering::SeqCst),
        2,
        "fit on first retry must not spend the second retry"
    );
    let saved = db
        .get_entries(&crate::database::EntryFilter::default())
        .unwrap()
        .into_iter()
        .find(|e| e.id == "slot-retry-ok")
        .unwrap();
    assert_eq!(saved.translation.as_deref(), Some("Hola!"));
    let ctxs = capture.contexts.lock().unwrap();
    assert_eq!(ctxs.len(), 2);
    assert!(
        ctxs[0].as_ref().is_some_and(|c| c.contains("LENGTH LIMIT")),
        "first call must include budget hint: {:?}",
        ctxs[0]
    );
    let retry_ctx = ctxs[1].as_ref().expect("retry context");
    assert!(
        retry_ctx.contains("PREVIOUS ATTEMPT WAS") && retry_ctx.contains("HARD LIMIT"),
        "retry must include correction: {retry_ctx}"
    );
    assert!(
        retry_ctx.contains("Previous text: «ABCDEFGHIJKLMNOP»"),
        "retry must quote the failed attempt so the model can edit: {retry_ctx}"
    );
    assert!(
        retry_ctx.contains("Remove at least 11 byte"),
        "retry must state exact excess (16-5=11): {retry_ctx}"
    );
}

#[tokio::test]
async fn test_binary_slot_retry_still_oversize_mechanical_fit() {
    let (db, glossary) = setup();
    let entry = binary_utf8_entry("slot-still-over", "Hi"); // budget 2
    db.save_entries(std::slice::from_ref(&entry)).unwrap();

    let provider = Arc::new(ScriptedLengthProvider::new(&[
        "ABCDEFGHIJKLMNOP", // 16
        "YYYYYYYY",         // 8 shorter but still over
        "ZZZZ",             // 4 still over budget 2 → mechanical truncates to "ZZ"
    ]));
    let capture = provider.clone();
    let manager = TranslationManager::new(provider, db.clone(), glossary);
    let (tx, mut rx) = mpsc::channel(100);
    let opts = TranslationOptions {
        allow_lossy_binary_fit: true,
        use_memory: false,
        use_glossary: false,
        ..Default::default()
    };
    manager
        .translate_entries(
            vec![entry],
            opts,
            tx,
            "job-still-over".into(),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    rx.close();
    while rx.recv().await.is_some() {}

    assert_eq!(
        capture.calls.load(Ordering::SeqCst),
        3,
        "initial + {MAX_BINARY_SLOT_LENGTH_RETRIES} retries when always oversize"
    );
    let saved = db
        .get_entries(&crate::database::EntryFilter::default())
        .unwrap()
        .into_iter()
        .find(|e| e.id == "slot-still-over")
        .unwrap();
    assert_eq!(
        saved.translation.as_deref(),
        Some("ZZ"),
        "mechanical fit must truncate shortest oversize attempt to budget"
    );
    let budget = crate::validation::encoded_byte_len("utf8", "Hi").unwrap();
    let actual =
        crate::validation::encoded_byte_len("utf8", saved.translation.as_ref().unwrap()).unwrap();
    assert!(actual <= budget);
}

#[tokio::test]
async fn test_binary_slot_mechanical_fit_after_accented_oversize() {
    let (db, glossary) = setup();
    // budget 7; provider only returns accented 8-byte form.
    let entry = binary_utf8_entry("slot-accent", "Options"); // 7 bytes
    db.save_entries(std::slice::from_ref(&entry)).unwrap();
    let provider = Arc::new(ScriptedLengthProvider::new(&[
        "Canción!", // 9 utf8
        "Canción",  // 8
        "Canción",  // 8 still over
    ]));
    let manager = TranslationManager::new(provider, db.clone(), glossary);
    let (tx, mut rx) = mpsc::channel(100);
    let opts = TranslationOptions {
        allow_lossy_binary_fit: true,
        use_memory: false,
        use_glossary: false,
        ..Default::default()
    };
    manager
        .translate_entries(
            vec![entry],
            opts,
            tx,
            "job-accent".into(),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    rx.close();
    while rx.recv().await.is_some() {}
    let saved = db
        .get_entries(&crate::database::EntryFilter::default())
        .unwrap()
        .into_iter()
        .find(|e| e.id == "slot-accent")
        .unwrap();
    let tr = saved.translation.as_deref().unwrap();
    assert!(
        crate::validation::encoded_byte_len("utf8", tr).unwrap() <= 7,
        "mechanical accent fold should fit: {tr}"
    );
    assert!(
        tr.starts_with("Canci"),
        "should preserve stem after fold: {tr}"
    );
}

#[tokio::test]
async fn test_duplicate_placeholder_provider_answer_is_not_saved() {
    let (db, glossary) = setup();
    let entry = StringEntry::new("duplicate-token", "Hello %1", PathBuf::from("script.txt"));
    db.save_entries(std::slice::from_ref(&entry)).unwrap();
    let provider = Arc::new(ScriptedLengthProvider::new(&["Hola {PL_0} {PL_0}"]));
    let manager = TranslationManager::new(provider.clone(), db.clone(), glossary);
    let (tx, mut rx) = mpsc::channel(100);
    manager
        .translate_entries(
            vec![entry],
            TranslationOptions {
                use_memory: false,
                use_glossary: false,
                ..Default::default()
            },
            tx,
            "job-duplicate-token".into(),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    rx.close();
    let mut rejected = false;
    while let Some(event) = rx.recv().await {
        if let ProgressEvent::BatchFailed { entry_id, error } = event {
            if entry_id.as_deref() == Some("duplicate-token") {
                assert!(error.contains("protected placeholders"), "{error}");
                rejected = true;
            }
        }
    }
    assert!(
        rejected,
        "duplicate tokens must fail placeholder validation"
    );
    // Initial answer plus two bounded semantic repairs; all remain invalid.
    assert_eq!(provider.calls.load(Ordering::SeqCst), 3);
    let saved = db.get_entry("duplicate-token").unwrap().unwrap();
    assert_eq!(saved.status, StringStatus::Pending);
    assert!(saved.translation.is_none());
}

#[tokio::test]
async fn namebox_record_local_context_maps_sanitized_positions_and_legacy_rows() {
    let mut entries = vec![StringEntry::new(
        "ordinary",
        "Hello",
        PathBuf::from("text.json"),
    )];
    for (i, escape) in [r"\n", r"\nc", r"\nr", r"\N", r"\NC", r"\NR", r"\nC", r"\Nr"]
        .iter()
        .enumerate()
    {
        entries.push(
            StringEntry::new(
                format!("variant-{i}"),
                format!(r"\C[2]é {escape}<Alchemist>Hi {i} \V[1]"),
                PathBuf::from("text.json"),
            )
            .with_format_id(if i % 2 == 0 {
                "rpgmaker-mv"
            } else {
                "rpgmaker-mz"
            })
            .with_context("scene"),
        );
    }
    for (id, path) in [
        ("Map001.json#0#event_1#page_0#cmd_0#msg", "data/Map001.json"),
        ("Map001.jsono#displayName", "data/Map001.jsono"),
        ("plugins.js#plugins#0#Plugin#text", "js/plugins.js"),
    ] {
        entries.push(
            StringEntry::new(id, r"\nr<Adventurer>Hello", PathBuf::from(path)).with_context(path),
        );
    }
    let original_contexts: HashMap<_, _> = entries
        .iter()
        .map(|e| (e.id.clone(), e.context.clone()))
        .collect();
    let provider = Arc::new(RepeatedProvider::new(RepeatedReply::Success));
    let (db, _) = run_repeated_job(
        entries,
        provider.clone(),
        TranslationOptions {
            source_lang: "en".into(),
            target_lang: "es".into(),
            ..Default::default()
        },
    )
    .await;
    let calls = provider.calls.lock().unwrap();
    let requests: Vec<_> = calls.iter().flatten().collect();
    assert_eq!(requests.len(), 12);
    assert_eq!(requests[0].entry_id, "ordinary");
    assert!(requests[0].context.is_none());
    for req in &requests[1..] {
        let note = req
            .context
            .as_deref()
            .expect("confirmed namebox needs context");
        for rule in [
            "RECORD-LOCAL",
            "generic roles, occupations and descriptive titles",
            "target language",
            "established proper names",
            "glossary",
            "every placeholder",
            "control code",
            "angle brackets",
        ] {
            assert!(note.contains(rule), "missing {rule}: {note}");
        }
        if req.entry_id.starts_with("variant-") {
            assert!(req.source.starts_with("{PL_0}é {PL_1}<Alchemist>"));
            assert!(note.starts_with("scene | "));
            assert!(note.contains("byte 9 ({PL_1}<...>)"), "{note}");
            assert!(!note.contains("{PL_0}<...>"), "{note}");
        } else {
            assert_eq!(req.source, "{PL_0}<Adventurer>Hello");
            assert!(note.contains("byte 0 ({PL_0}<...>)"), "{note}");
        }
        let saved = db.get_entry(&req.entry_id).unwrap().unwrap();
        assert!(saved.require_preserved_translation_controls().is_ok());
        assert_eq!(saved.context, original_contexts[&req.entry_id]);
    }
}

#[tokio::test]
async fn namebox_context_excludes_html_unknown_other_formats_and_malformed_arguments() {
    let mut entries = Vec::new();
    for (i, (source, format, path)) in [
        (r"\n<Boy>Hello", "rpgmaker-vxa", "Actors.rvdata2"),
        (r"\n<Boy>Hello", "wolf-rpg", "Map.wolf"),
        (r"\n<Boy>Hello", "renpy", "script.rpy"),
        (r"\n<Boy>Hello", "html", "page.html"),
        (r"\n<Boy>Hello", "unknown", "Map001.json"),
        (r"\n<Boy>Hello", "", "Map001.json"),
        (r"\n<Boy>Hello", "", "Actors.rvdata2"),
        (r"\n<Boy>Hello", "", "Map.wolf"),
        ("<Boy>Hello</Boy>", "rpgmaker-mv", "Map001.json"),
        (r"\\n<Boy>Hello", "rpgmaker-mv", "Map001.json"),
        (r"\fn<Boy>Hello", "rpgmaker-mv", "Map001.json"),
        (r"\nl<Boy>Hello", "rpgmaker-mv", "Map001.json"),
        (r"\n <Boy>Hello", "rpgmaker-mv", "Map001.json"),
        (r"\n<Boy", "rpgmaker-mv", "Map001.json"),
        (r"\n<>Hello", "rpgmaker-mv", "Map001.json"),
        (r"\n< >Hello", "rpgmaker-mv", "Map001.json"),
        (r"\n<<Boy>>Hello", "rpgmaker-mv", "Map001.json"),
        ("\\n<Boy\nAlchemist>Hello", "rpgmaker-mv", "Map001.json"),
        (
            r#"<b title="\n<Boy>">Hello</b>"#,
            "rpgmaker-mv",
            "Map001.json",
        ),
        ("Hello", "rpgmaker-mv", "Map001.json"),
    ]
    .iter()
    .enumerate()
    {
        let mut entry = StringEntry::new(format!("negative-{i}"), *source, PathBuf::from(path))
            .with_context(format!("record-{i}"));
        if !format.is_empty() {
            entry = entry.with_format_id(format);
        }
        if *format == "unknown" {
            entry.id = "Map001.json#0#event_1#page_0#cmd_0#msg".into();
        }
        entries.push(entry);
    }
    let mut malformed_id = StringEntry::new(
        "Map002.json#0#event_1#page_0#cmd_0#msg",
        r"\n<Boy>Hello",
        PathBuf::from("Map002.json"),
    )
    .with_context("malformed metadata");
    malformed_id.metadata.insert(
        crate::models::FORMAT_ID_METADATA_KEY.into(),
        serde_json::json!(42),
    );
    entries.push(malformed_id);
    // Identical RPG Maker profiles are not enough to share the new guidance.
    entries.push(
        StringEntry::new("confirmed", r"\n<Boy>Hello", PathBuf::from("Map001.json"))
            .with_format_id("rpgmaker-mv")
            .with_context("record-0"),
    );
    let provider = Arc::new(RepeatedProvider::new(RepeatedReply::Success));
    let (db, _) = run_repeated_job(entries.clone(), provider.clone(), Default::default()).await;
    let calls = provider.calls.lock().unwrap();
    let requests: Vec<_> = calls.iter().flatten().collect();
    assert_eq!(
        requests.len(),
        entries.len(),
        "guidance must not be deduplicated with VXA"
    );
    for source in [r"Hello \V[1]", r"\n<Missing close"] {
        let (representatives, _) = RepeatedSources::collect(
            ["rpgmaker-mv", "rpgmaker-vxa"]
                .into_iter()
                .map(|format| {
                    StringEntry::new(format, source, PathBuf::from("text.json"))
                        .with_format_id(format)
                })
                .collect(),
        );
        assert_eq!(
            representatives.len(),
            1,
            "unchanged grouping without guidance"
        );
    }
    for req in requests {
        let entry = entries.iter().find(|e| e.id == req.entry_id).unwrap();
        let context = req.context.as_deref().unwrap();
        let original_context = entry.context.as_deref().unwrap();
        assert!(context.starts_with(original_context), "{}", req.entry_id);
        assert_eq!(
            db.get_entry(&req.entry_id).unwrap().unwrap().context,
            entry.context,
            "stored record context must remain unchanged"
        );
        if req.entry_id == "confirmed" {
            assert!(context.contains("RECORD-LOCAL"));
        } else {
            for speaker_note in [
                "RECORD-LOCAL",
                "speaker arguments",
                "generic roles, occupations and descriptive titles",
            ] {
                assert!(
                    !context.contains(speaker_note),
                    "{}: {context}",
                    req.entry_id
                );
            }
            let (_, phs) = PlaceholderProcessor::extract_with_profile(
                &entry.source,
                entry.placeholder_profile(),
            );
            let expected = if phs.is_empty() {
                original_context.to_owned()
            } else {
                let inventory = phs
                    .iter()
                    .map(|ph| ph.token.as_str())
                    .collect::<Vec<_>>()
                    .join(" ");
                format!(
                    "{original_context} | EXPECTED SENTINELS: {inventory}. Preserve each exactly once; output no additional sentinel or control tokens."
                )
            };
            assert_eq!(context, expected, "{}", req.entry_id);
        }
    }
}

#[tokio::test]
async fn namebox_guidance_keeps_names_glossary_controls_and_cached_bypasses() {
    let (db, _) = setup();
    let entries: Vec<_> = [
        ("memory", r"\n<Boy>Hi"),
        ("exact", r"\n<Alchemist>Hi"),
        (
            "provider",
            r"\C[2]é \n<Lydia \V[1]>Hi \nr<Young Boy><b>Hello</b>",
        ),
    ]
    .into_iter()
    .map(|(id, source)| {
        StringEntry::new(id, source, PathBuf::from("Map001.json")).with_format_id("rpgmaker-mv")
    })
    .collect();
    db.save_entries(&entries).unwrap();
    // Existing memory is deliberately not retroactively repaired.
    db.save_memory(
        &entries[0].source_hash(),
        &entries[0].source,
        r"\n<Boy>Hola",
        "en-es",
    )
    .await
    .unwrap();
    for (term, translation) in [
        (&*entries[1].source, r"\n<Maestro>Hola"),
        ("Lydia", "Lidia"),
    ] {
        db.save_glossary_entry(&crate::database::GlossaryEntry {
            term: term.into(),
            translation: translation.into(),
            lang_pair: "en-es".into(),
            context: None,
            case_sensitive: true,
        })
        .unwrap();
    }
    let provider = Arc::new(RepeatedProvider::new(RepeatedReply::Success));
    run_group_job(
        db.clone(),
        provider.clone(),
        entries.clone(),
        TranslationOptions {
            source_lang: "en".into(),
            target_lang: "es".into(),
            ..Default::default()
        },
    )
    .await;
    let calls = provider.calls.lock().unwrap();
    let requests: Vec<_> = calls.iter().flatten().collect();
    assert_eq!(requests.len(), 1);
    let req = requests[0];
    assert_eq!(req.entry_id, "provider");
    assert_eq!(
        req.source,
        "{PL_0}é {PL_1}<{G_0_0} {PL_2}>Hi {PL_3}<Young Boy>{PL_4}Hello{PL_5}"
    );
    assert!(req
        .glossary_hint
        .as_deref()
        .unwrap()
        .contains("Lydia → Lidia"));
    let note = req.context.as_deref().unwrap();
    assert!(note.contains("byte 9 ({PL_1}<...>)"), "{note}");
    assert!(note.contains("byte 34 ({PL_3}<...>)"), "{note}");
    assert!(!note.contains("{PL_4}<...>"));
    assert!(
        note.contains("preserve established proper names and follow this record's glossary"),
        "{note}"
    );
    for (id, expected, attribution) in [
        ("memory", r"\n<Boy>Hola", "memory"),
        ("exact", r"\n<Maestro>Hola", "glossary"),
        (
            "provider",
            r"en: \C[2]é \n<Lidia \V[1]>Hi \nr<Young Boy><b>Hello</b>",
            "repeat-test",
        ),
    ] {
        let saved = db.get_entry(id).unwrap().unwrap();
        assert_eq!(saved.translation.as_deref(), Some(expected));
        assert_eq!(saved.provider_used.as_deref(), Some(attribution));
        assert!(saved.require_preserved_translation_controls().is_ok());
        assert!(saved.context.is_none());
    }
    assert_eq!(
        db.lookup_memory(&entries[0].source_hash(), "en-es")
            .unwrap()
            .as_deref(),
        Some(r"\n<Boy>Hola")
    );
}

#[tokio::test]
async fn namebox_provider_translation_is_saved() {
    for source in [r"\n<ルイ>あの……", r"\n<Louis>Um..."] {
        let (db, glossary) = setup();
        let entry = StringEntry::new("namebox", source, PathBuf::from("Map001.json"));
        db.save_entries(std::slice::from_ref(&entry)).unwrap();
        let provider = Arc::new(ScriptedLengthProvider::new(&["{PL_0}<Luis>Eh..."]));
        let manager = TranslationManager::new(provider, db.clone(), glossary);
        let (tx, mut rx) = mpsc::channel(100);
        manager
            .translate_entries(
                vec![entry],
                TranslationOptions {
                    use_memory: false,
                    use_glossary: false,
                    ..Default::default()
                },
                tx,
                "namebox-job".into(),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        while let Some(event) = rx.recv().await {
            assert!(
                !matches!(
                    event,
                    ProgressEvent::BatchFailed { .. } | ProgressEvent::Failed { .. }
                ),
                "{event:?}"
            );
        }
        let saved = db.get_entry("namebox").unwrap().unwrap();
        assert_eq!(saved.status, StringStatus::Translated);
        assert_eq!(saved.translation.as_deref(), Some(r"\n<Luis>Eh..."));
    }
}

#[tokio::test]
async fn format_profile_accepts_provider_brackets_only_for_rpgmaker() {
    for (format, accepted) in [("rpgmaker-mv", true), ("renpy", false), ("unknown", false)] {
        let (db, _) = setup();
        let entry = StringEntry::new(
            "namebox",
            r"\n<【貴族】リディア視点>Hello",
            PathBuf::from("data.json"),
        )
        .with_format_id(format);
        db.save_entries(std::slice::from_ref(&entry)).unwrap();
        let provider = Arc::new(ScriptedLengthProvider::new(&["{PL_0}<[Noble] Lidia>Hola"]));
        run_group_job(
            db.clone(),
            provider,
            vec![entry],
            TranslationOptions {
                use_memory: false,
                use_glossary: false,
                ..Default::default()
            },
        )
        .await;
        let saved = db.get_entry("namebox").unwrap().unwrap();
        assert_eq!(
            saved.translation.as_deref(),
            accepted.then_some(r"\n<[Noble] Lidia>Hola"),
            "{format}"
        );
        assert_eq!(
            saved.placeholder_profile(),
            PlaceholderProfile::for_format(format)
        );
        assert!(crate::validation::Validator::validate_entry(&saved).is_empty());
    }
}

#[tokio::test]
async fn format_profile_accepts_brackets_in_wolf_length_retry() {
    let (db, _) = setup();
    let mut entry = StringEntry::new(
        "wolf",
        "A sufficiently long source",
        PathBuf::from("Map.wolf"),
    )
    .with_format_id("wolf-rpg");
    entry
        .metadata
        .insert("binary_slot".into(), serde_json::json!("sjis"));
    db.save_entries(std::slice::from_ref(&entry)).unwrap();
    let provider = Arc::new(ScriptedLengthProvider::new(&[
        "A first translation that is much too long for this binary slot",
        "[Noble]",
    ]));
    run_group_job(
        db.clone(),
        provider.clone(),
        vec![entry],
        TranslationOptions {
            use_memory: false,
            use_glossary: false,
            ..Default::default()
        },
    )
    .await;
    assert_eq!(provider.calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        db.get_entry("wolf")
            .unwrap()
            .unwrap()
            .translation
            .as_deref(),
        Some("[Noble]")
    );
}

#[tokio::test]
async fn test_no_binary_slot_provider_called_once() {
    let (db, glossary) = setup();
    let entry = StringEntry::new("plain", "Hello", PathBuf::from("script.txt"));
    db.save_entries(std::slice::from_ref(&entry)).unwrap();

    // First would be "oversize" length for a slot, but no slot → no retry.
    let provider = Arc::new(ScriptedLengthProvider::new(&[
        "ABCDEFGHIJKLMNOP",
        "should-not-be-used",
    ]));
    let capture = provider.clone();
    let manager = TranslationManager::new(provider, db.clone(), glossary);
    let (tx, mut rx) = mpsc::channel(100);
    let opts = TranslationOptions {
        use_memory: false,
        use_glossary: false,
        ..Default::default()
    };
    manager
        .translate_entries(
            vec![entry],
            opts,
            tx,
            "job-no-slot".into(),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    rx.close();
    while rx.recv().await.is_some() {}

    assert_eq!(
        capture.calls.load(Ordering::SeqCst),
        1,
        "entry without binary_slot must not retry"
    );
    let saved = db
        .get_entries(&crate::database::EntryFilter::default())
        .unwrap()
        .into_iter()
        .find(|e| e.id == "plain")
        .unwrap();
    assert_eq!(saved.translation.as_deref(), Some("ABCDEFGHIJKLMNOP"));
}

#[tokio::test]
async fn test_binary_slot_second_retry_fits() {
    let (db, glossary) = setup();
    // budget = 8 for "New Game"
    let entry = binary_utf8_entry("slot-retry2", "New Game");
    db.save_entries(std::slice::from_ref(&entry)).unwrap();

    let provider = Arc::new(ScriptedLengthProvider::new(&[
        "Nuevo Juego", // 11 > 8
        "Nuevo Jgo",   // 9 > 8 first retry still over
        "Jugar",       // 5 <= 8 second retry fits
    ]));
    let capture = provider.clone();
    let manager = TranslationManager::new(provider, db.clone(), glossary);
    let (tx, mut rx) = mpsc::channel(100);
    let opts = TranslationOptions {
        use_memory: false,
        use_glossary: false,
        ..Default::default()
    };
    manager
        .translate_entries(
            vec![entry],
            opts,
            tx,
            "job-retry2".into(),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    rx.close();
    while rx.recv().await.is_some() {}

    assert_eq!(
        capture.calls.load(Ordering::SeqCst),
        3,
        "second retry must run when first retry still oversize"
    );
    let saved = db
        .get_entries(&crate::database::EntryFilter::default())
        .unwrap()
        .into_iter()
        .find(|e| e.id == "slot-retry2")
        .unwrap();
    assert_eq!(saved.translation.as_deref(), Some("Jugar"));
    let ctxs = capture.contexts.lock().unwrap();
    assert_eq!(ctxs.len(), 3);
    assert!(
        ctxs[2]
            .as_ref()
            .is_some_and(|c| c.contains("Previous text: «Nuevo Jgo»")),
        "second retry must quote the prior failed text: {:?}",
        ctxs[2]
    );
}

#[tokio::test]
async fn test_fitting_first_answer_no_retry() {
    let (db, glossary) = setup();
    let entry = binary_utf8_entry("slot-fit", "Hello"); // budget 5
    db.save_entries(std::slice::from_ref(&entry)).unwrap();

    let provider = Arc::new(ScriptedLengthProvider::new(&[
        "Hola!", // 5 bytes, fits
        "UNUSED",
    ]));
    let capture = provider.clone();
    let manager = TranslationManager::new(provider, db.clone(), glossary);
    let (tx, mut rx) = mpsc::channel(100);
    let opts = TranslationOptions {
        use_memory: false,
        use_glossary: false,
        ..Default::default()
    };
    manager
        .translate_entries(
            vec![entry],
            opts,
            tx,
            "job-fit".into(),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    rx.close();
    while rx.recv().await.is_some() {}

    assert_eq!(
        capture.calls.load(Ordering::SeqCst),
        1,
        "fitting first answer must not retry"
    );
    let saved = db
        .get_entries(&crate::database::EntryFilter::default())
        .unwrap()
        .into_iter()
        .find(|e| e.id == "slot-fit")
        .unwrap();
    assert_eq!(saved.translation.as_deref(), Some("Hola!"));
}

// ── Fallback chain ────────────────────────────────────────────────────

/// Translates at most `max` strings total across all translate() calls, then
/// returns empty results for the rest (leaves them pending).
struct LimitProvider {
    id: String,
    name: String,
    max: usize,
    done: AtomicUsize,
    calls: AtomicUsize,
}

impl LimitProvider {
    fn new(id: &str, name: &str, max: usize) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            max,
            done: AtomicUsize::new(0),
            calls: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl TranslationProvider for LimitProvider {
    fn id(&self) -> &str {
        &self.id
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn is_free(&self) -> bool {
        true
    }
    fn requires_api_key(&self) -> bool {
        false
    }
    async fn translate(&self, requests: &[TranslationRequest]) -> Result<Vec<TranslationResult>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let mut out = Vec::new();
        for r in requests {
            let n = self.done.fetch_add(1, Ordering::SeqCst);
            if n < self.max {
                out.push(TranslationResult {
                    entry_id: r.entry_id.clone(),
                    translation: format!("{}: {}", self.id, r.source),
                    detected_source_lang: None,
                    provider: self.id.clone(),
                    tokens_used: None,
                    input_tokens: None,
                    output_tokens: None,
                    cost_usd: Some(0.001),
                });
            }
        }
        Ok(out)
    }
    async fn estimate_cost(&self, _: usize, _: &str) -> Option<f64> {
        None
    }
    async fn health_check(&self) -> Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn test_fallback_chain_primary_partial_fallback_completes() {
    let (db, glossary) = setup();
    let entries = vec![
        StringEntry::new("a", "Alpha", PathBuf::from("f.json")),
        StringEntry::new("b", "Bravo", PathBuf::from("f.json")),
        StringEntry::new("c", "Charlie", PathBuf::from("f.json")),
    ];
    db.save_entries(&entries).unwrap();

    let primary = Arc::new(LimitProvider::new("primary", "Primary", 1));
    let fallback = Arc::new(LimitProvider::new("fallback", "Fallback", 100));
    let p_calls = Arc::clone(&primary);
    let f_calls = Arc::clone(&fallback);

    let mut map: std::collections::HashMap<String, Arc<dyn TranslationProvider>> =
        std::collections::HashMap::new();
    map.insert(
        "primary".into(),
        Arc::clone(&primary) as Arc<dyn TranslationProvider>,
    );
    map.insert(
        "fallback".into(),
        Arc::clone(&fallback) as Arc<dyn TranslationProvider>,
    );
    let map = Arc::new(map);

    let (tx, mut rx) = mpsc::channel(256);
    let chain = vec!["primary".into(), "fallback".into()];
    let opts = TranslationOptions {
        use_memory: false,
        use_glossary: false,
        skip_approved: true,
        ..Default::default()
    };

    let map2 = map.clone();
    let db2 = db.clone();
    let job = tokio::spawn(async move {
        let resolve = |id: &str| map2.get(id).cloned();
        run_fallback_chain(
            &chain,
            &resolve,
            db2,
            glossary,
            opts,
            tx,
            "job-chain".into(),
            CancellationToken::new(),
        )
        .await
    });

    let mut switched = 0usize;
    let mut completed_evt = false;
    while let Some(ev) = rx.recv().await {
        match ev {
            ProgressEvent::ProviderSwitched {
                provider_id,
                remaining_pending,
                ..
            } => {
                assert_eq!(provider_id, "fallback");
                assert!(remaining_pending >= 1);
                switched += 1;
            }
            ProgressEvent::Completed {
                total_translated,
                total_cost,
                cost_is_complete,
                ..
            } => {
                assert_eq!(total_translated, 3);
                // Primary billed one rejected partial result; fallback billed three.
                assert!((total_cost - 0.004).abs() < 1e-9);
                assert!(cost_is_complete);
                completed_evt = true;
            }
            _ => {}
        }
    }
    job.await.unwrap().unwrap();
    assert_eq!(switched, 1, "expected one ProviderSwitched");
    assert!(completed_evt);
    assert!(p_calls.calls.load(Ordering::SeqCst) >= 1);
    assert!(f_calls.calls.load(Ordering::SeqCst) >= 1);

    let left = load_pending_entries(&db).unwrap().len();
    assert_eq!(left, 0, "all strings should be translated");
}

#[tokio::test]
async fn test_fallback_chain_single_provider_no_switch() {
    let (db, glossary) = setup();
    let entries = vec![
        StringEntry::new("x", "One", PathBuf::from("f.json")),
        StringEntry::new("y", "Two", PathBuf::from("f.json")),
    ];
    db.save_entries(&entries).unwrap();

    let only = Arc::new(LimitProvider::new("only", "Only", 100));
    let mut map: std::collections::HashMap<String, Arc<dyn TranslationProvider>> =
        std::collections::HashMap::new();
    map.insert("only".into(), only as Arc<dyn TranslationProvider>);
    let map = Arc::new(map);

    let (tx, mut rx) = mpsc::channel(256);
    let chain = vec!["only".into()];
    let opts = TranslationOptions {
        use_memory: false,
        use_glossary: false,
        ..Default::default()
    };
    let map2 = map.clone();
    let db2 = db.clone();
    let job = tokio::spawn(async move {
        let resolve = |id: &str| map2.get(id).cloned();
        run_fallback_chain(
            &chain,
            &resolve,
            db2,
            glossary,
            opts,
            tx,
            "job-solo".into(),
            CancellationToken::new(),
        )
        .await
    });

    let mut switched = 0usize;
    while let Some(ev) = rx.recv().await {
        if matches!(ev, ProgressEvent::ProviderSwitched { .. }) {
            switched += 1;
        }
    }
    job.await.unwrap().unwrap();
    assert_eq!(switched, 0, "no fallback → no ProviderSwitched");
    assert_eq!(load_pending_entries(&db).unwrap().len(), 0);
}

struct AlwaysErrorProvider {
    calls: AtomicUsize,
}

impl AlwaysErrorProvider {
    fn new() -> Self {
        Self {
            calls: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl TranslationProvider for AlwaysErrorProvider {
    fn id(&self) -> &str {
        "always-error"
    }
    fn name(&self) -> &str {
        "Always Error"
    }
    fn is_free(&self) -> bool {
        true
    }
    fn requires_api_key(&self) -> bool {
        false
    }
    async fn translate(&self, _requests: &[TranslationRequest]) -> Result<Vec<TranslationResult>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Err(LocustError::ProviderError(
            "primary provider refused".into(),
        ))
    }
    async fn estimate_cost(&self, _: usize, _: &str) -> Option<f64> {
        None
    }
    async fn health_check(&self) -> Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn test_tauri_path_fallback_sets_provider_used() {
    // Same helper the Tauri command uses: unique_provider_chain + run_fallback_chain.
    let (db, glossary) = setup();
    db.save_entries(&[StringEntry::new(
        "e1",
        "Hello world",
        PathBuf::from("a.json"),
    )])
    .unwrap();

    let primary = Arc::new(AlwaysErrorProvider::new());
    let fallback = Arc::new(MockProvider::new());
    let mut map: std::collections::HashMap<String, Arc<dyn TranslationProvider>> =
        std::collections::HashMap::new();
    map.insert(
        "always-error".into(),
        primary as Arc<dyn TranslationProvider>,
    );
    map.insert("mock".into(), fallback as Arc<dyn TranslationProvider>);
    let map = Arc::new(map);

    let chain = unique_provider_chain("always-error", Some(&["mock".into()]));
    let (tx, mut rx) = mpsc::channel(256);
    let opts = TranslationOptions {
        use_memory: false,
        use_glossary: false,
        ..Default::default()
    };
    let map2 = map.clone();
    let db2 = db.clone();
    let job = tokio::spawn(async move {
        let resolve = |id: &str| map2.get(id).cloned();
        run_fallback_chain(
            &chain,
            &resolve,
            db2,
            glossary,
            opts,
            tx,
            "job-tauri-fb".into(),
            CancellationToken::new(),
        )
        .await
    });
    while rx.recv().await.is_some() {}
    job.await.unwrap().unwrap();

    let entry = db.get_entry("e1").unwrap().unwrap();
    assert_eq!(entry.status, StringStatus::Translated);
    assert_eq!(entry.provider_used.as_deref(), Some("mock"));
}

struct FlakyTransientProvider {
    attempts: AtomicUsize,
}

impl FlakyTransientProvider {
    fn new() -> Self {
        Self {
            attempts: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl TranslationProvider for FlakyTransientProvider {
    fn id(&self) -> &str {
        "flaky-transient"
    }
    fn name(&self) -> &str {
        "Flaky Transient"
    }
    fn is_free(&self) -> bool {
        true
    }
    fn requires_api_key(&self) -> bool {
        false
    }
    async fn translate(&self, requests: &[TranslationRequest]) -> Result<Vec<TranslationResult>> {
        let n = self.attempts.fetch_add(1, Ordering::SeqCst);
        if n < 2 {
            return Err(LocustError::ProviderError("503 Service Unavailable".into()));
        }
        Ok(requests
            .iter()
            .map(|r| TranslationResult {
                entry_id: r.entry_id.clone(),
                translation: format!("ok:{}", r.source),
                detected_source_lang: None,
                provider: "flaky-transient".into(),
                tokens_used: None,
                input_tokens: None,
                output_tokens: None,
                cost_usd: None,
            })
            .collect())
    }
    async fn estimate_cost(&self, _: usize, _: &str) -> Option<f64> {
        None
    }
    async fn health_check(&self) -> Result<()> {
        Ok(())
    }
}

struct AuthFailProvider {
    attempts: AtomicUsize,
}

impl AuthFailProvider {
    fn new() -> Self {
        Self {
            attempts: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl TranslationProvider for AuthFailProvider {
    fn id(&self) -> &str {
        "auth-fail"
    }
    fn name(&self) -> &str {
        "Auth Fail"
    }
    fn is_free(&self) -> bool {
        true
    }
    fn requires_api_key(&self) -> bool {
        true
    }
    async fn translate(&self, _requests: &[TranslationRequest]) -> Result<Vec<TranslationResult>> {
        self.attempts.fetch_add(1, Ordering::SeqCst);
        Err(LocustError::ProviderError("401 Unauthorized".into()))
    }
    async fn estimate_cost(&self, _: usize, _: &str) -> Option<f64> {
        None
    }
    async fn health_check(&self) -> Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn test_provider_retry_on_transient_then_success() {
    let (db, glossary) = setup();
    let entries = vec![StringEntry::new("e0", "Hello", PathBuf::from("a.json"))];
    db.save_entries(&entries).unwrap();
    let provider = Arc::new(FlakyTransientProvider::new());
    let attempts = Arc::clone(&provider);
    let manager =
        TranslationManager::new(provider, db.clone(), glossary).with_retry_config(RetryConfig {
            max_attempts: 3,
            initial_delay_ms: 1,
            max_delay_ms: 5,
            ..Default::default()
        });
    let (tx, mut rx) = mpsc::channel(32);
    manager
        .translate_entries(
            entries,
            TranslationOptions {
                use_memory: false,
                use_glossary: false,
                ..Default::default()
            },
            tx,
            "job-retry".into(),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    while rx.recv().await.is_some() {}

    assert_eq!(attempts.attempts.load(Ordering::SeqCst), 3);
    let entry = db.get_entry("e0").unwrap().unwrap();
    assert_eq!(entry.status, StringStatus::Translated);
    assert_eq!(entry.provider_used.as_deref(), Some("flaky-transient"));
}

#[tokio::test]
async fn test_provider_auth_error_is_not_retried() {
    let (db, glossary) = setup();
    let entries = vec![StringEntry::new("e0", "Hello", PathBuf::from("a.json"))];
    db.save_entries(&entries).unwrap();
    let provider = Arc::new(AuthFailProvider::new());
    let attempts = Arc::clone(&provider);
    let manager =
        TranslationManager::new(provider, db.clone(), glossary).with_retry_config(RetryConfig {
            max_attempts: 3,
            initial_delay_ms: 1,
            max_delay_ms: 5,
            ..Default::default()
        });
    let (tx, mut rx) = mpsc::channel(32);
    manager
        .translate_entries(
            entries,
            TranslationOptions {
                use_memory: false,
                use_glossary: false,
                ..Default::default()
            },
            tx,
            "job-auth".into(),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let mut saw_failed = false;
    while let Some(ev) = rx.recv().await {
        if matches!(ev, ProgressEvent::BatchFailed { .. }) {
            saw_failed = true;
        }
    }
    assert_eq!(attempts.attempts.load(Ordering::SeqCst), 1);
    assert!(saw_failed, "auth error must be surfaced");
    let entry = db.get_entry("e0").unwrap().unwrap();
    assert_ne!(entry.status, StringStatus::Translated);
}

#[tokio::test]
async fn default_preserves_oversized_translation_for_review_without_lossy_cache() {
    let (db, glossary) = setup();
    let entry = binary_utf8_entry("no-loss", "Hi");
    db.save_entries(std::slice::from_ref(&entry)).unwrap();
    let provider = Arc::new(ScriptedLengthProvider::new(&["Hello", "Hola", "Buenas"]));
    let manager = TranslationManager::new(provider, db.clone(), glossary);
    let (tx, mut rx) = mpsc::channel(100);
    manager
        .translate_entries(
            vec![entry.clone()],
            TranslationOptions::default(),
            tx,
            "no-loss".into(),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let saved = db
        .get_entries(&crate::database::EntryFilter::default())
        .unwrap();
    assert_eq!(saved[0].translation.as_deref(), Some("Hola"));
    assert!(db
        .lookup_memory(&entry.source_hash(), "ja-en")
        .unwrap()
        .is_none());
    let mut warned = false;
    while let Some(event) = rx.recv().await {
        if let ProgressEvent::BatchFailed { error, .. } = event {
            warned |= error.contains("no automatic truncation");
        }
    }
    assert!(warned);
}

struct GroupMapProvider {
    calls: AtomicUsize,
    scripts: Vec<HashMap<String, String>>,
    contexts: std::sync::Mutex<Vec<(String, Option<String>)>>,
    batch_sizes: std::sync::Mutex<Vec<usize>>,
    cancel_on_call: Option<(CancellationToken, usize)>,
    cost_usd: Option<f64>,
    is_free: bool,
}

impl GroupMapProvider {
    fn new(scripts: Vec<HashMap<String, String>>) -> Self {
        Self {
            calls: AtomicUsize::new(0),
            scripts,
            contexts: std::sync::Mutex::new(Vec::new()),
            batch_sizes: std::sync::Mutex::new(Vec::new()),
            cancel_on_call: None,
            cost_usd: None,
            is_free: true,
        }
    }
}

#[async_trait]
impl TranslationProvider for GroupMapProvider {
    fn id(&self) -> &str {
        "group-map"
    }
    fn name(&self) -> &str {
        "Group Map"
    }
    fn is_free(&self) -> bool {
        self.is_free
    }
    fn requires_api_key(&self) -> bool {
        false
    }
    async fn translate(&self, requests: &[TranslationRequest]) -> Result<Vec<TranslationResult>> {
        let n = self.calls.fetch_add(1, Ordering::SeqCst);
        self.batch_sizes.lock().unwrap().push(requests.len());
        if let Some((cancel, call)) = &self.cancel_on_call {
            if n == *call {
                cancel.cancel();
            }
        }
        let map = self
            .scripts
            .get(n.min(self.scripts.len().saturating_sub(1)));
        {
            let mut ctxs = self.contexts.lock().unwrap();
            for r in requests {
                ctxs.push((r.entry_id.clone(), r.context.clone()));
            }
        }
        Ok(requests
            .iter()
            .map(|r| TranslationResult {
                entry_id: r.entry_id.clone(),
                translation: map
                    .and_then(|m| m.get(&r.entry_id).cloned())
                    .unwrap_or_else(|| r.source.clone()),
                detected_source_lang: None,
                provider: self.id().into(),
                tokens_used: Some(4),
                input_tokens: Some(2),
                output_tokens: Some(2),
                cost_usd: self.cost_usd,
            })
            .collect())
    }
    async fn estimate_cost(&self, _: usize, _: &str) -> Option<f64> {
        self.cost_usd
    }
    async fn health_check(&self) -> Result<()> {
        Ok(())
    }
}

fn grouped_loc_entries(original: &str, rows: &[(&str, &str, &str)]) -> Vec<StringEntry> {
    let mut members: Vec<StringEntry> = rows
        .iter()
        .enumerate()
        .map(|(index, (id, key, value))| {
            let mut entry = StringEntry::new(*id, *value, PathBuf::from("sharedassets0.assets"));
            entry.metadata.insert(
                "extraction_method".into(),
                serde_json::json!("textasset_loc_line"),
            );
            entry
                .metadata
                .insert("loc_key".into(), serde_json::json!(key));
            entry
                .metadata
                .insert("line_index".into(), serde_json::json!(index));
            entry
                .metadata
                .insert("binary_slot".into(), serde_json::json!("utf8"));
            entry
        })
        .collect();
    crate::textasset_group::attach_to_entries(
        &mut members,
        crate::textasset_group::GroupKind::LocLine,
        original,
        original.len(),
        "g-test",
    );
    members
}

fn group_script(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect()
}

async fn run_group_job(
    db: Arc<Database>,
    provider: Arc<dyn TranslationProvider>,
    entries: Vec<StringEntry>,
    opts: TranslationOptions,
) {
    let glossary = Arc::new(Glossary::new(db.clone()));
    let (tx, mut rx) = mpsc::channel(100);
    TranslationManager::new(provider, db, glossary)
        .translate_entries(
            entries,
            opts,
            tx,
            "group-job".into(),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    rx.close();
    while rx.recv().await.is_some() {}
}

#[tokio::test]
async fn grouped_textasset_run_materializes_only_selected_siblings() {
    let (db, _) = setup();
    let original = "Menu.A: Hi\nMenu.B: Hello\nMenu.C: Bye\n        ";
    let mut members = grouped_loc_entries(
        original,
        &[
            ("z", "Menu.A", "Hi"),
            ("a", "Menu.B", "Hello"),
            ("m", "Menu.C", "Bye"),
        ],
    );
    for entry in &mut members[1..] {
        entry.status = StringStatus::Translated;
        entry.translation = Some("Ya".into());
    }
    db.save_entries(&make_entries(20_000)).unwrap();
    db.save_entries(&members).unwrap();
    let selected = HashSet::from(["g-test".into()]);
    let expected: Vec<_> = db
        .get_entries(&crate::database::EntryFilter::default())
        .unwrap()
        .into_iter()
        .filter(|entry| {
            entry
                .metadata
                .get(crate::textasset_group::GROUP_ID_KEY)
                .and_then(|v| v.as_str())
                == Some("g-test")
        })
        .collect();
    let actual = db.get_entries_for_textasset_groups(&selected).unwrap();
    assert_eq!(
        serde_json::to_value(&actual).unwrap(),
        serde_json::to_value(&expected).unwrap()
    );
    assert_eq!(
        actual
            .iter()
            .map(|entry| entry.id.as_str())
            .collect::<Vec<_>>(),
        ["a", "m", "z"]
    );
    for entry in &actual {
        assert_eq!(
            crate::textasset_group::original_textasset(entry),
            Some(original)
        );
    }
    crate::database::ENTRY_ROWS_MATERIALIZED.with(|count| count.set(Some(0)));
    run_group_job(
        db.clone(),
        Arc::new(GroupMapProvider::new(vec![group_script(&[("z", "Hola")])])),
        vec![members[0].clone()],
        TranslationOptions {
            use_memory: false,
            use_glossary: false,
            ..Default::default()
        },
    )
    .await;
    assert_eq!(
        crate::database::ENTRY_ROWS_MATERIALIZED.with(|count| count.replace(None)),
        Some(3)
    );
    assert_eq!(
        db.get_entry("z").unwrap().unwrap().translation.as_deref(),
        Some("Hola")
    );
    for id in ["a", "m"] {
        assert_eq!(
            db.get_entry(id).unwrap().unwrap().translation.as_deref(),
            Some("Ya")
        );
    }
}

async fn check_translation_memory_batch(grouped: bool) {
    let (db, _) = setup();
    let mut entries = grouped_loc_entries(
        "Menu.A: Hello\nMenu.B: Hello\nMenu.C: World\nMenu.D: Hello\n                ",
        &[
            ("a", "Menu.A", "Hello"),
            ("b", "Menu.B", "Hello"),
            ("c", "Menu.C", "World"),
            ("d", "Menu.D", "Hello"),
        ],
    );
    if !grouped {
        for entry in &mut entries {
            entry.metadata.clear();
            entry.textasset_original = None;
            // This test scripts different answers for equal sources; make
            // their request contexts distinct so those answers are valid.
            entry.context = Some(entry.id.clone());
        }
    }
    db.save_entries(&entries).unwrap();
    let answers = group_script(&[
        ("a", "First"),
        ("b", "Second"),
        ("c", "Third"),
        ("d", "Last"),
    ]);
    let sequential = Database::open_in_memory().unwrap();
    for entry in &entries {
        sequential
            .save_memory(
                &entry.source_hash(),
                &entry.source,
                &answers[&entry.id],
                "en-es",
            )
            .await
            .unwrap();
    }
    crate::database::MEMORY_TRANSACTIONS.with(|count| count.set(Some(0)));
    run_group_job(
        db.clone(),
        Arc::new(GroupMapProvider::new(vec![answers])),
        entries,
        TranslationOptions {
            source_lang: "en".into(),
            target_lang: "es".into(),
            use_memory: true,
            use_glossary: false,
            batch_size: 10,
            ..Default::default()
        },
    )
    .await;
    assert_eq!(
        crate::database::MEMORY_TRANSACTIONS.with(|count| count.replace(None)),
        Some(1)
    );
    let snapshot = |db: &Database| {
        let (mut rows, _) = db.list_memory(None, Some("en-es"), 100, 0).unwrap();
        rows.sort_by(|a, b| a.source_hash.cmp(&b.source_hash));
        for row in &mut rows {
            row.last_used.clear();
        }
        serde_json::to_value(rows).unwrap()
    };
    assert_eq!(snapshot(&db), snapshot(&sequential));
}

#[tokio::test]
async fn translation_memory_batch_ungrouped_uses_one_transaction() {
    check_translation_memory_batch(false).await;
}

#[tokio::test]
async fn translation_memory_batch_grouped_uses_one_transaction() {
    check_translation_memory_batch(true).await;
}

#[tokio::test]
async fn grouped_textasset_fits_when_one_cell_is_longer() {
    let (db, _glossary) = setup();
    let original = "Menu.A: Hi\nMenu.B: Hello\n";
    let entries = grouped_loc_entries(original, &[("a", "Menu.A", "Hi"), ("b", "Menu.B", "Hello")]);
    db.save_entries(&entries).unwrap();
    let provider = Arc::new(GroupMapProvider::new(vec![group_script(&[
        ("a", "Hola"),
        ("b", "Hey"),
    ])]));
    let capture = provider.clone();
    run_group_job(
        db.clone(),
        provider,
        entries,
        TranslationOptions {
            use_memory: false,
            use_glossary: false,
            ..Default::default()
        },
    )
    .await;
    assert_eq!(capture.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        db.get_entry("a").unwrap().unwrap().translation.as_deref(),
        Some("Hola")
    );
    assert_eq!(
        db.get_entry("b").unwrap().unwrap().translation.as_deref(),
        Some("Hey")
    );
    let ctxs = capture.contexts.lock().unwrap();
    assert!(
        ctxs.iter().any(|(_, ctx)| ctx
            .as_ref()
            .is_some_and(|c| c.contains("SHARED TEXTASSET BUDGET"))),
        "{ctxs:?}"
    );
}

#[tokio::test]
async fn large_shared_textasset_partial_translation_retains_unselected_japanese() {
    let (db, _) = setup();
    let rows: Vec<_> = (0..256)
        .map(|i| {
            (
                format!("row{i}"),
                format!("Menu.K{i}"),
                format!("日本語{}", "あ".repeat(50)),
            )
        })
        .collect();
    let original: String = rows
        .iter()
        .map(|(_, key, value)| format!("{key}: {value}\n"))
        .collect();
    assert!(original.len() > 32_768);
    let refs: Vec<_> = rows
        .iter()
        .map(|(id, key, value)| (id.as_str(), key.as_str(), value.as_str()))
        .collect();
    let entries = grouped_loc_entries(&original, &refs);
    db.save_entries(&entries).unwrap();
    let selected = db
        .get_entries(&crate::database::EntryFilter {
            offset: Some(254),
            limit: Some(2),
            ..Default::default()
        })
        .unwrap();
    let answers: HashMap<_, _> = selected
        .iter()
        .map(|entry| (entry.id.clone(), "Texto español".to_string()))
        .collect();
    let provider = Arc::new(GroupMapProvider::new(vec![answers]));
    run_group_job(
        db.clone(),
        provider,
        selected.clone(),
        TranslationOptions {
            use_memory: false,
            use_glossary: false,
            ..Default::default()
        },
    )
    .await;
    let loaded = db
        .get_entries(&crate::database::EntryFilter::default())
        .unwrap();
    assert_eq!(loaded.iter().filter(|e| e.translation.is_some()).count(), 2);
    assert!(crate::textasset_group::group_validation_issues(&loaded).is_empty());
    for selected in selected {
        let entry = db.get_entry(&selected.id).unwrap().unwrap();
        assert_eq!(entry.translation.as_deref(), Some("Texto español"));
        assert_eq!(
            crate::textasset_group::original_textasset(&entry),
            Some(original.as_str())
        );
    }
}

#[tokio::test]
async fn structural_textasset_capability_allows_growth_without_shortening_retry() {
    let (db, _) = setup();
    let mut entries = grouped_loc_entries("Menu.A: Hi\n", &[("a", "Menu.A", "Hi")]);
    entries[0].metadata.insert(
        "textasset_rewrite".into(),
        serde_json::json!("serialized-v1"),
    );
    entries[0]
        .metadata
        .insert("unity_serialized_version".into(), serde_json::json!(22));
    db.save_entries(&entries).unwrap();
    let translation = "Una traducción española más larga que el original";
    let provider = Arc::new(GroupMapProvider::new(vec![group_script(&[(
        "a",
        translation,
    )])]));
    let capture = provider.clone();
    run_group_job(
        db.clone(),
        provider,
        entries,
        TranslationOptions {
            use_memory: false,
            use_glossary: false,
            ..Default::default()
        },
    )
    .await;
    assert_eq!(capture.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        db.get_entry("a").unwrap().unwrap().translation.as_deref(),
        Some(translation)
    );
}

#[tokio::test]
async fn grouped_textasset_retry_makes_oversize_group_fit() {
    let (db, _glossary) = setup();
    let original = "Menu.A: Hi\nMenu.B: Go\n      ";
    let entries = grouped_loc_entries(original, &[("a", "Menu.A", "Hi"), ("b", "Menu.B", "Go")]);
    db.save_entries(&entries).unwrap();
    let provider = Arc::new(GroupMapProvider::new(vec![
        group_script(&[("a", "Hola!!!!"), ("b", "Vamos")]),
        group_script(&[("a", "Hola"), ("b", "Hey")]),
    ]));
    let capture = provider.clone();
    run_group_job(
        db.clone(),
        provider,
        entries,
        TranslationOptions {
            use_memory: false,
            use_glossary: false,
            ..Default::default()
        },
    )
    .await;
    assert_eq!(capture.calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        db.get_entry("a").unwrap().unwrap().translation.as_deref(),
        Some("Hola")
    );
    assert_eq!(
        db.get_entry("b").unwrap().unwrap().status,
        StringStatus::Translated
    );
    let ctxs = capture.contexts.lock().unwrap();
    assert!(
        ctxs.iter().any(|(_, ctx)| {
            ctx.as_ref()
                .is_some_and(|c| c.contains("PREVIOUS RECONSTRUCTED BLOB WAS"))
        }),
        "{ctxs:?}"
    );
}

#[tokio::test]
async fn grouped_textasset_still_oversize_refuses_without_saving() {
    let (db, _glossary) = setup();
    let original = "Menu.A: Hi\nMenu.B: Go\n";
    let entries = grouped_loc_entries(original, &[("a", "Menu.A", "Hi"), ("b", "Menu.B", "Go")]);
    db.save_entries(&entries).unwrap();
    let provider = Arc::new(GroupMapProvider::new(vec![group_script(&[
        ("a", "XXXXXXXX"),
        ("b", "YYYYYYYY"),
    ])]));
    let capture = provider.clone();
    run_group_job(
        db.clone(),
        provider,
        entries,
        TranslationOptions {
            use_memory: false,
            use_glossary: false,
            ..Default::default()
        },
    )
    .await;
    assert_eq!(
        capture.calls.load(Ordering::SeqCst),
        1 + MAX_BINARY_SLOT_LENGTH_RETRIES
    );
    assert_eq!(
        db.get_entry("a").unwrap().unwrap().status,
        StringStatus::Pending
    );
    assert!(db.get_entry("a").unwrap().unwrap().translation.is_none());
}

#[tokio::test]
async fn grouped_textasset_partial_selection_and_batch_span() {
    let (db, _glossary) = setup();
    let original = "Menu.A: Hi\nMenu.B: Hello\n   ";
    let entries = grouped_loc_entries(original, &[("a", "Menu.A", "Hi"), ("b", "Menu.B", "Hello")]);
    db.save_entries(&entries).unwrap();
    let provider = Arc::new(GroupMapProvider::new(vec![group_script(&[("a", "Hola")])]));
    let capture = provider.clone();
    run_group_job(
        db.clone(),
        provider,
        vec![entries[0].clone()],
        TranslationOptions {
            use_memory: false,
            use_glossary: false,
            batch_size: 1,
            ..Default::default()
        },
    )
    .await;
    assert_eq!(capture.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        db.get_entry("a").unwrap().unwrap().translation.as_deref(),
        Some("Hola")
    );
    assert!(db.get_entry("b").unwrap().unwrap().translation.is_none());

    // Separate extraction: the same physical group cannot legitimately have
    // two independent sets of ids for the same cells in one database.
    let (db, _glossary) = setup();
    let entries = grouped_loc_entries(original, &[("c", "Menu.A", "Hi"), ("d", "Menu.B", "Hello")]);
    db.save_entries(&entries).unwrap();
    let provider = Arc::new(GroupMapProvider::new(vec![
        group_script(&[("c", "Hola")]),
        group_script(&[("d", "Hey")]),
    ]));
    let capture = provider.clone();
    run_group_job(
        db.clone(),
        provider,
        entries,
        TranslationOptions {
            use_memory: false,
            use_glossary: false,
            batch_size: 1,
            ..Default::default()
        },
    )
    .await;
    assert_eq!(capture.calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        db.get_entry("c").unwrap().unwrap().translation.as_deref(),
        Some("Hola")
    );
    assert_eq!(
        db.get_entry("d").unwrap().unwrap().translation.as_deref(),
        Some("Hey")
    );
}

#[tokio::test]
async fn grouped_textasset_retry_respects_single_entry_batches() {
    let (db, _) = setup();
    let original = "Menu.A: Hi\nMenu.B: Go\n      ";
    let entries = grouped_loc_entries(original, &[("a", "Menu.A", "Hi"), ("b", "Menu.B", "Go")]);
    db.save_entries(&entries).unwrap();
    let long = group_script(&[("a", "This is much too long"), ("b", "Also much too long")]);
    let short = group_script(&[("a", "Hola"), ("b", "Hey")]);
    let provider = Arc::new(GroupMapProvider::new(vec![long.clone(), long, short]));
    run_group_job(
        db.clone(),
        provider.clone(),
        entries,
        TranslationOptions {
            batch_size: 1,
            use_memory: false,
            use_glossary: false,
            ..Default::default()
        },
    )
    .await;
    assert_eq!(*provider.batch_sizes.lock().unwrap(), vec![1, 1, 1, 1]);
    assert_eq!(
        db.get_entry("a").unwrap().unwrap().translation.as_deref(),
        Some("Hola")
    );
    assert_eq!(
        db.get_entry("b").unwrap().unwrap().translation.as_deref(),
        Some("Hey")
    );
}

#[tokio::test]
async fn grouped_textasset_cancel_during_retry_does_not_save_staged_results() {
    let (db, glossary) = setup();
    let original = "Menu.A: Hi\nMenu.B: Go\n      ";
    let entries = grouped_loc_entries(original, &[("a", "Menu.A", "Hi"), ("b", "Menu.B", "Go")]);
    db.save_entries(&entries).unwrap();
    let cancel = CancellationToken::new();
    let mut provider = GroupMapProvider::new(vec![
        group_script(&[("a", "This is much too long"), ("b", "Also much too long")]),
        group_script(&[("a", "Hola"), ("b", "Hey")]),
    ]);
    provider.cancel_on_call = Some((cancel.clone(), 1));
    let provider = Arc::new(provider);
    let (tx, mut rx) = mpsc::channel(100);
    TranslationManager::new(provider.clone(), db.clone(), glossary)
        .translate_entries(
            entries,
            TranslationOptions {
                use_memory: false,
                use_glossary: false,
                ..Default::default()
            },
            tx,
            "cancel-group".into(),
            cancel.clone(),
        )
        .await
        .unwrap();
    assert!(cancel.is_cancelled());
    assert_eq!(provider.calls.load(Ordering::SeqCst), 2);
    for id in ["a", "b"] {
        assert!(db.get_entry(id).unwrap().unwrap().translation.is_none());
    }
    rx.close();
    while let Some(event) = rx.recv().await {
        assert!(!matches!(event, ProgressEvent::StringTranslated { .. }));
    }
}

#[tokio::test]
async fn grouped_textasset_failed_group_does_not_block_other_asset() {
    let (db, _) = setup();
    let original = "Menu.A: Hi\nMenu.B: Go\n      ";
    let mut entries =
        grouped_loc_entries(original, &[("a", "Menu.A", "Hi"), ("b", "Menu.B", "Go")]);
    let mut second = grouped_loc_entries(original, &[("c", "Menu.A", "Hi"), ("d", "Menu.B", "Go")]);
    for entry in &mut second {
        entry.file_path = PathBuf::from("sharedassets1.assets");
        entry.metadata.insert(
            crate::textasset_group::GROUP_ID_KEY.into(),
            serde_json::json!("second-asset"),
        );
    }
    entries.extend(second);
    db.save_entries(&entries).unwrap();
    let provider = Arc::new(GroupMapProvider::new(vec![group_script(&[
        ("a", "This will never fit"),
        ("b", "Also too long"),
        ("c", "Hola"),
        ("d", "Hey"),
    ])]));
    run_group_job(
        db.clone(),
        provider,
        entries,
        TranslationOptions {
            use_memory: false,
            use_glossary: false,
            ..Default::default()
        },
    )
    .await;
    assert!(db.get_entry("a").unwrap().unwrap().translation.is_none());
    assert!(db.get_entry("b").unwrap().unwrap().translation.is_none());
    assert_eq!(
        db.get_entry("c").unwrap().unwrap().translation.as_deref(),
        Some("Hola")
    );
    assert_eq!(
        db.get_entry("d").unwrap().unwrap().translation.as_deref(),
        Some("Hey")
    );
}

#[tokio::test]
async fn grouped_malformed_metadata_fails_before_provider() {
    let (db, glossary) = setup();
    let mut entry = StringEntry::new("x", "Hi", PathBuf::from("x.assets"));
    entry.metadata.insert(
        "extraction_method".into(),
        serde_json::json!("textasset_loc_line"),
    );
    db.save_entries(std::slice::from_ref(&entry)).unwrap();
    let provider = Arc::new(GroupMapProvider::new(vec![group_script(&[("x", "Hola")])]));
    let capture = provider.clone();
    let (tx, mut rx) = mpsc::channel(100);
    let err = TranslationManager::new(provider, db, glossary)
        .translate_entries(
            vec![entry],
            TranslationOptions {
                use_memory: false,
                use_glossary: false,
                ..Default::default()
            },
            tx,
            "malformed".into(),
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
    rx.close();
    while rx.recv().await.is_some() {}
    assert!(
        err.to_string().contains("textasset_group")
            || err.to_string().contains("shared TextAsset")
            || err.to_string().contains("missing"),
        "{err}"
    );
    assert_eq!(capture.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn grouped_oversize_retry_cost_is_recorded_when_refused() {
    let (db, _glossary) = setup();
    let original = "Menu.A: Hi\nMenu.B: Go\n";
    let entries = grouped_loc_entries(original, &[("a", "Menu.A", "Hi"), ("b", "Menu.B", "Go")]);
    db.save_entries(&entries).unwrap();
    let mut provider =
        GroupMapProvider::new(vec![group_script(&[("a", "XXXXXXXX"), ("b", "YYYYYYYY")])]);
    provider.cost_usd = Some(0.01);
    provider.is_free = false;
    let provider = Arc::new(provider);
    run_group_job(
        db.clone(),
        provider,
        entries,
        TranslationOptions {
            use_memory: false,
            use_glossary: false,
            ..Default::default()
        },
    )
    .await;
    let runs = db.get_translation_runs().unwrap();
    assert_eq!(runs.len(), 1);
    assert!(runs[0].cost_usd > 0.0, "{:?}", runs[0]);
    assert_eq!(runs[0].strings_translated, 0);
    assert_eq!(
        db.get_entry("a").unwrap().unwrap().status,
        StringStatus::Pending
    );
}
