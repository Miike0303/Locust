use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::database::Database;
use crate::error::{LocustError, Result};
use crate::glossary::Glossary;
use crate::models::{
    ProgressEvent, StringEntry, StringStatus, TranslationRequest, TranslationResult,
};
use crate::placeholder::{Placeholder, PlaceholderProcessor};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LangPair {
    pub source: String,
    pub target: String,
}

#[async_trait]
pub trait TranslationProvider: Send + Sync {
    fn id(&self) -> &str;
    fn name(&self) -> &str;
    fn is_free(&self) -> bool;
    fn requires_api_key(&self) -> bool;
    fn supported_languages(&self) -> Vec<LangPair> {
        vec![]
    }
    async fn translate(&self, requests: &[TranslationRequest]) -> Result<Vec<TranslationResult>>;
    async fn estimate_cost(&self, char_count: usize, target_lang: &str) -> Option<f64>;
    async fn health_check(&self) -> Result<()>;
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TranslationOptions {
    pub source_lang: String,
    pub target_lang: String,
    pub batch_size: usize,
    pub max_concurrent: usize,
    pub cost_limit_usd: Option<f64>,
    pub game_context: Option<String>,
    pub use_glossary: bool,
    pub use_memory: bool,
    pub skip_approved: bool,
}

impl Default for TranslationOptions {
    fn default() -> Self {
        Self {
            source_lang: "ja".to_string(),
            target_lang: "en".to_string(),
            batch_size: 40,
            // Sequential by default: local GPU models degrade under parallel
            // requests; API providers can opt in via --concurrency.
            max_concurrent: 1,
            cost_limit_usd: None,
            game_context: None,
            use_glossary: true,
            use_memory: true,
            skip_approved: true,
        }
    }
}

pub struct TranslationManager {
    provider: Arc<dyn TranslationProvider>,
    db: Arc<Database>,
    glossary: Arc<Glossary>,
}

impl TranslationManager {
    pub fn new(
        provider: Arc<dyn TranslationProvider>,
        db: Arc<Database>,
        glossary: Arc<Glossary>,
    ) -> Self {
        Self {
            provider,
            db,
            glossary,
        }
    }

    pub async fn translate_entries(
        &self,
        entries: Vec<StringEntry>,
        opts: TranslationOptions,
        tx: mpsc::Sender<ProgressEvent>,
        job_id: String,
        cancel: CancellationToken,
    ) -> Result<()> {
        let start = Instant::now();

        // 1. Filter translatable entries
        let mut translatable: Vec<StringEntry> = entries
            .into_iter()
            .filter(|e| {
                e.is_translatable() && !(opts.skip_approved && e.status == StringStatus::Approved)
            })
            .collect();

        let total = translatable.len();

        // 2. Send Started
        let _ = tx
            .send(ProgressEvent::Started {
                total,
                job_id: job_id.clone(),
            })
            .await;

        let mut completed = 0usize;
        let mut oversize_count = 0usize;
        let mut total_cost = 0.0f64;
        let mut total_tokens = 0u64;
        let mut total_input_tokens = 0u64;
        let mut total_output_tokens = 0u64;
        let started_at = chrono::Utc::now().to_rfc3339();
        let lang_pair = format!("{}-{}", opts.source_lang, opts.target_lang);

        // 3. Check translation memory for each entry
        let mut remaining = Vec::new();
        if opts.use_memory {
            for entry in translatable.drain(..) {
                let hash = entry.source_hash();
                if let Ok(Some(cached)) = self.db.lookup_memory(&hash, &lang_pair) {
                    self.db
                        .save_translation(&entry.id, &cached, "memory")
                        .await?;
                    let _ = tx
                        .send(ProgressEvent::StringTranslated {
                            entry_id: entry.id.clone(),
                            translation: cached,
                        })
                        .await;
                    completed += 1;
                } else {
                    remaining.push(entry);
                }
            }
        } else {
            remaining = translatable;
        }

        // 4. Build glossary hint
        let glossary_hint = if opts.use_glossary {
            self.glossary.build_hint(&opts.source_lang, &opts.target_lang)
        } else {
            None
        };

        // 5. Process remaining in chunks — up to `max_concurrent` provider calls
        // in flight at once. Result handling (DB writes, progress, cost) stays on
        // this task. Cost-limited runs stay sequential so the pre-dispatch
        // estimate cannot be overtaken by batches already in flight.
        let concurrency = if opts.cost_limit_usd.is_some() {
            1
        } else {
            opts.max_concurrent.max(1)
        };

        /// Per-entry binary inject budget: (slot encoding name, max encoded bytes).
        type SlotBudget = (String, usize);
        type BatchOutcome = (
            Vec<TranslationRequest>,
            std::collections::HashMap<String, Vec<Placeholder>>,
            std::collections::HashMap<String, SlotBudget>,
            Result<Vec<TranslationResult>>,
        );
        let mut in_flight: tokio::task::JoinSet<BatchOutcome> = tokio::task::JoinSet::new();
        let mut chunk_iter = remaining.chunks(opts.batch_size);
        let mut cancelled = false;

        loop {
            // 5a. Fill the in-flight window
            while in_flight.len() < concurrency && !cancelled {
                if cancel.is_cancelled() {
                    cancelled = true;
                    break;
                }

                let Some(chunk) = chunk_iter.next() else { break };

                // 5b. Check cost limit
                if let Some(limit) = opts.cost_limit_usd {
                    let char_count: usize = chunk.iter().map(|e| e.source.len()).sum();
                    if let Some(estimated) = self
                        .provider
                        .estimate_cost(char_count, &opts.target_lang)
                        .await
                    {
                        if total_cost + estimated > limit {
                            return Err(LocustError::CostLimitExceeded {
                                estimated: total_cost + estimated,
                                limit,
                            });
                        }
                    }
                }

                // 5c. Build TranslationRequests — sanitize placeholders so the translator
                // doesn't translate variable names like [player_name] or Ren'Py tags {i}{/i}
                let mut placeholders_by_id: std::collections::HashMap<String, Vec<Placeholder>> =
                    std::collections::HashMap::new();
                let mut budgets_by_id: std::collections::HashMap<String, SlotBudget> =
                    std::collections::HashMap::new();
                let requests: Vec<TranslationRequest> = chunk
                    .iter()
                    .map(|entry| {
                        let mut context = match (&entry.context, &opts.game_context) {
                            (Some(ec), Some(gc)) => Some(format!("{} | {}", gc, ec)),
                            (Some(ec), None) => Some(ec.clone()),
                            (None, Some(gc)) => Some(gc.clone()),
                            (None, None) => None,
                        };
                        // Binary-slot engines (Unity/Unreal/Wolf): hint the model to stay
                        // within the inject byte budget for this string.
                        if let Some(slot) = entry
                            .metadata
                            .get("binary_slot")
                            .and_then(|v| v.as_str())
                        {
                            if let Some(budget) =
                                crate::validation::encoded_byte_len(slot, &entry.source)
                            {
                                budgets_by_id
                                    .insert(entry.id.clone(), (slot.to_string(), budget));
                                let hint = format!(
                                    "LENGTH LIMIT: the translation MUST fit in {budget} bytes when encoded as {slot}; abbreviate if needed."
                                );
                                context = Some(match context {
                                    Some(c) => format!("{c} | {hint}"),
                                    None => hint,
                                });
                            }
                        }
                        let (sanitized, phs) = PlaceholderProcessor::extract(&entry.source);
                        placeholders_by_id.insert(entry.id.clone(), phs);
                        TranslationRequest {
                            entry_id: entry.id.clone(),
                            source: sanitized,
                            source_lang: opts.source_lang.clone(),
                            target_lang: opts.target_lang.clone(),
                            context,
                            glossary_hint: glossary_hint.clone(),
                        }
                    })
                    .collect();

                // 5d. Dispatch provider call to the in-flight window
                let provider = self.provider.clone();
                in_flight.spawn(async move {
                    let result = provider.translate(&requests).await;
                    (requests, placeholders_by_id, budgets_by_id, result)
                });
            }

            // 5e. Wait for the next batch to finish; done when nothing is in flight
            let Some(joined) = in_flight.join_next().await else {
                break;
            };
            let (requests, placeholders_by_id, budgets_by_id, batch_result) = match joined {
                Ok(outcome) => outcome,
                Err(e) => {
                    tracing::error!("Translation batch task panicked: {}", e);
                    continue;
                }
            };

            match batch_result {
                Ok(mut results) => {
                    // Restore placeholders in translations before saving
                    for result in &mut results {
                        if let Some(phs) = placeholders_by_id.get(&result.entry_id) {
                            if !phs.is_empty() {
                                match PlaceholderProcessor::restore(&result.translation, phs) {
                                    Ok(restored) => result.translation = restored,
                                    Err(e) => {
                                        tracing::warn!(
                                            "Failed to restore placeholders for {}: {}. Falling back to original with any missing tokens replaced.",
                                            result.entry_id, e
                                        );
                                        // Best-effort restore: replace each token even if some missing
                                        let mut t = result.translation.clone();
                                        for ph in phs {
                                            t = t.replace(&ph.token, &ph.original);
                                        }
                                        result.translation = t;
                                    }
                                }
                            }
                        }
                        // Flag oversize after restore; still save — validate/inject preflight
                        // are the enforcement points for binary slots.
                        if let Some((slot, budget)) = budgets_by_id.get(&result.entry_id) {
                            if let Some(actual) =
                                crate::validation::encoded_byte_len(slot, &result.translation)
                            {
                                if actual > *budget {
                                    tracing::warn!(
                                        entry_id = %result.entry_id,
                                        actual,
                                        budget,
                                        slot = %slot,
                                        "translation exceeds binary slot budget"
                                    );
                                    oversize_count += 1;
                                }
                            }
                        }
                    }
                    // 5e. Process results
                    for result in &results {
                        let _ = self
                            .db
                            .save_translation(
                                &result.entry_id,
                                &result.translation,
                                &result.provider,
                            )
                            .await;

                        // Don't cache mock translations in memory
                        if opts.use_memory && result.provider != "mock" {
                            if let Some(req) =
                                requests.iter().find(|r| r.entry_id == result.entry_id)
                            {
                                use sha2::{Digest, Sha256};
                                let hash =
                                    hex::encode(Sha256::digest(req.source.as_bytes()));
                                let _ = self
                                    .db
                                    .save_memory(
                                        &hash,
                                        &req.source,
                                        &result.translation,
                                        &lang_pair,
                                    )
                                    .await;
                            }
                        }

                        let _ = tx
                            .send(ProgressEvent::StringTranslated {
                                entry_id: result.entry_id.clone(),
                                translation: result.translation.clone(),
                            })
                            .await;

                        if let Some(cost) = result.cost_usd {
                            total_cost += cost;
                        }
                        if let Some(tokens) = result.tokens_used {
                            total_tokens += tokens as u64;
                        }
                        if let Some(t) = result.input_tokens {
                            total_input_tokens += t as u64;
                        }
                        if let Some(t) = result.output_tokens {
                            total_output_tokens += t as u64;
                        }
                        completed += 1;
                    }
                }
                Err(e) => {
                    let _ = tx
                        .send(ProgressEvent::Failed {
                            entry_id: None,
                            error: e.to_string(),
                        })
                        .await;
                    tracing::error!("Batch translation failed: {}", e);
                    continue;
                }
            }

            // 5f. Send BatchCompleted
            let _ = tx
                .send(ProgressEvent::BatchCompleted {
                    completed,
                    total,
                    cost_so_far: total_cost,
                    language: None,
                })
                .await;
        }

        if cancelled {
            let _ = tx.send(ProgressEvent::Paused).await;
            return Ok(());
        }

        // ProgressEvent / return type live outside this file; surface oversize via log only.
        if oversize_count > 0 {
            tracing::warn!(
                "{oversize_count} translations exceed their binary slot; run locust validate"
            );
        }

        // 6. Send Completed and record the run in the project ledger
        let duration = start.elapsed().as_secs_f64();
        let _ = tx
            .send(ProgressEvent::Completed {
                total_translated: completed,
                total_cost,
                duration_secs: duration,
            })
            .await;

        if completed > 0 {
            let run = crate::database::TranslationRun {
                started_at,
                duration_secs: duration,
                provider: self.provider.id().to_string(),
                source_lang: opts.source_lang.clone(),
                target_lang: opts.target_lang.clone(),
                strings_translated: completed,
                tokens_used: total_tokens,
                input_tokens: total_input_tokens,
                output_tokens: total_output_tokens,
                cost_usd: total_cost,
            };
            if let Err(e) = self.db.record_translation_run(&run).await {
                tracing::warn!("failed to record translation run: {}", e);
            }
        }

        Ok(())
    }
}

pub struct ProviderRegistry {
    providers: Vec<Arc<dyn TranslationProvider>>,
}

impl ProviderRegistry {
    pub fn new() -> Self {
        Self {
            providers: Vec::new(),
        }
    }

    pub fn register(&mut self, provider: Arc<dyn TranslationProvider>) {
        self.providers.push(provider);
    }

    pub fn get(&self, id: &str) -> Option<Arc<dyn TranslationProvider>> {
        self.providers.iter().find(|p| p.id() == id).cloned()
    }

    pub fn list(&self) -> Vec<ProviderInfo> {
        self.providers
            .iter()
            .map(|p| ProviderInfo {
                id: p.id().to_string(),
                name: p.name().to_string(),
                is_free: p.is_free(),
                requires_api_key: p.requires_api_key(),
            })
            .collect()
    }
}

impl Default for ProviderRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ProviderInfo {
    pub id: String,
    pub name: String,
    pub is_free: bool,
    pub requires_api_key: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::Database;
    use crate::glossary::Glossary;
    use crate::models::StringStatus;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct MockProvider {
        call_count: AtomicUsize,
    }

    impl MockProvider {
        fn new() -> Self {
            Self {
                call_count: AtomicUsize::new(0),
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
        async fn translate(
            &self,
            requests: &[TranslationRequest],
        ) -> Result<Vec<TranslationResult>> {
            self.call_count.fetch_add(requests.len(), Ordering::SeqCst);
            Ok(requests
                .iter()
                .map(|r| TranslationResult {
                    entry_id: r.entry_id.clone(),
                    translation: format!("[{}] {}", r.target_lang, r.source),
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
        async fn translate(
            &self,
            requests: &[TranslationRequest],
        ) -> Result<Vec<TranslationResult>> {
            let call = self.call_count.fetch_add(1, Ordering::SeqCst);
            if call == 0 {
                return Err(LocustError::ProviderError("simulated failure".to_string()));
            }
            Ok(requests
                .iter()
                .map(|r| TranslationResult {
                    entry_id: r.entry_id.clone(),
                    translation: format!("[translated] {}", r.source),
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

    fn setup() -> (Arc<Database>, Arc<Glossary>) {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let glossary = Arc::new(Glossary::new(db.clone()));
        (db, glossary)
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

        assert!(matches!(
            result,
            Err(LocustError::CostLimitExceeded { .. })
        ));
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
            .any(|e| matches!(e, ProgressEvent::Failed { .. })));
        assert!(events
            .iter()
            .any(|e| matches!(e, ProgressEvent::StringTranslated { .. })));
    }

    #[test]
    fn test_provider_registry_register_and_get() {
        let mut reg = ProviderRegistry::new();
        let provider = Arc::new(MockProvider::new());
        reg.register(provider);
        assert!(reg.get("mock").is_some());
        assert_eq!(reg.get("mock").unwrap().id(), "mock");
        assert!(reg.get("nonexistent").is_none());
        assert_eq!(reg.list().len(), 1);
    }

    #[tokio::test]
    async fn test_glossary_hint_injected() {
        let (db, glossary) = setup();

        db.save_glossary_entry(&crate::database::GlossaryEntry {
            term: "HP".to_string(),
            translation: "Health Points".to_string(),
            lang_pair: "ja-en".to_string(),
            context: None,
            case_sensitive: false,
        })
        .unwrap();

        let mut entries = make_entries(1);
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
        async fn translate(
            &self,
            requests: &[TranslationRequest],
        ) -> Result<Vec<TranslationResult>> {
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

        let mut with_slot =
            StringEntry::new("with_slot", "Hello", PathBuf::from("resources.assets"));
        with_slot.metadata.insert(
            "binary_slot".to_string(),
            serde_json::Value::String("utf8".to_string()),
        );
        let without = StringEntry::new("no_slot", "World", PathBuf::from("script.txt"));
        db.save_entries(&[with_slot.clone(), without.clone()])
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
                vec![with_slot, without],
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
        assert!(
            slotted.contains("5 bytes"),
            "utf8 budget for \"Hello\" is 5: {slotted}"
        );
        assert!(
            slotted.contains("encoded as utf8"),
            "slot name must appear: {slotted}"
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
            .translate_entries(vec![entry], opts, tx, "job-u16".into(), CancellationToken::new())
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
            ctx.contains("6 bytes"),
            "utf16le budget must be code units * 2, not utf8 len: {ctx}"
        );
        assert!(
            !ctx.contains("9 bytes"),
            "must not use utf8 length for utf16le slot: {ctx}"
        );
        assert!(ctx.contains("encoded as utf16le"), "{ctx}");
    }

    #[tokio::test]
    async fn test_binary_slot_oversize_translation_still_saved() {
        let (db, glossary) = setup();
        let source = "Hi";
        let mut entry = StringEntry::new("oversized", source, PathBuf::from("x.assets"));
        entry.metadata.insert(
            "binary_slot".to_string(),
            serde_json::Value::String("utf8".to_string()),
        );
        db.save_entries(std::slice::from_ref(&entry)).unwrap();

        struct OversizeProvider;
        #[async_trait]
        impl TranslationProvider for OversizeProvider {
            fn id(&self) -> &str {
                "oversize"
            }
            fn name(&self) -> &str {
                "Oversize"
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
                Ok(requests
                    .iter()
                    .map(|r| TranslationResult {
                        entry_id: r.entry_id.clone(),
                        // Far longer than any short source utf8 budget.
                        translation: "XXXXXXXXXXXXXXXXXXXXXXXX".to_string(),
                        detected_source_lang: None,
                        provider: "oversize".to_string(),
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

        let manager = TranslationManager::new(Arc::new(OversizeProvider), db.clone(), glossary);
        let (tx, mut rx) = mpsc::channel(100);
        let opts = TranslationOptions {
            use_memory: false,
            use_glossary: false,
            ..Default::default()
        };
        manager
            .translate_entries(vec![entry], opts, tx, "job-over".into(), CancellationToken::new())
            .await
            .unwrap();
        rx.close();
        while rx.recv().await.is_some() {}

        let saved = db
            .get_entries(&crate::database::EntryFilter::default())
            .unwrap()
            .into_iter()
            .find(|e| e.id == "oversized")
            .expect("entry must exist");
        assert_eq!(
            saved.translation.as_deref(),
            Some("XXXXXXXXXXXXXXXXXXXXXXXX"),
            "oversize translations must still be saved (validate/inject preflight enforce later)"
        );
        let budget = crate::validation::encoded_byte_len("utf8", source).unwrap();
        let actual =
            crate::validation::encoded_byte_len("utf8", saved.translation.as_ref().unwrap())
                .unwrap();
        assert!(actual > budget, "fixture must actually be oversize");
    }
}
