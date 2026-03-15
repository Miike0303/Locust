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
            max_concurrent: 3,
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
        let mut total_cost = 0.0f64;
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
            self.glossary.build_hint(&lang_pair).unwrap_or(None)
        } else {
            None
        };

        // 5. Process remaining in chunks
        for chunk in remaining.chunks(opts.batch_size) {
            // 5a. Check cancellation
            if cancel.is_cancelled() {
                let _ = tx.send(ProgressEvent::Paused).await;
                return Ok(());
            }

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

            // 5c. Build TranslationRequests
            let requests: Vec<TranslationRequest> = chunk
                .iter()
                .map(|entry| {
                    let context = match (&entry.context, &opts.game_context) {
                        (Some(ec), Some(gc)) => Some(format!("{} | {}", gc, ec)),
                        (Some(ec), None) => Some(ec.clone()),
                        (None, Some(gc)) => Some(gc.clone()),
                        (None, None) => None,
                    };
                    TranslationRequest {
                        entry_id: entry.id.clone(),
                        source: entry.source.clone(),
                        source_lang: opts.source_lang.clone(),
                        target_lang: opts.target_lang.clone(),
                        context,
                        glossary_hint: glossary_hint.clone(),
                    }
                })
                .collect();

            // 5d. Call provider
            match self.provider.translate(&requests).await {
                Ok(results) => {
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

                        if opts.use_memory {
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

        // 6. Send Completed
        let duration = start.elapsed().as_secs_f64();
        let _ = tx
            .send(ProgressEvent::Completed {
                total_translated: completed,
                total_cost,
                duration_secs: duration,
            })
            .await;

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
        assert!(hints[0].as_ref().unwrap().contains("HP = Health Points"));
    }
}
