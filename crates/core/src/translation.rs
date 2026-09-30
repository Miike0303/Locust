use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::database::{Database, TranslationSaveGuard};
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
    /// Approximate input budget: source/context bytes divided by 3, plus overhead.
    #[serde(default = "default_max_batch_tokens")]
    pub max_batch_tokens: Option<usize>,
    /// Explicit opt-in: mechanical shortening can destroy meaning.
    #[serde(default)]
    pub allow_lossy_binary_fit: bool,
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
            max_batch_tokens: default_max_batch_tokens(),
            allow_lossy_binary_fit: false,
        }
    }
}

fn translation_fits_entry(entry: &StringEntry, translation: &str) -> bool {
    if translation.trim().is_empty()
        || !PlaceholderProcessor::validate(&entry.source, translation).is_empty()
    {
        return false;
    }
    if crate::textasset_group::is_grouped_entry(entry) {
        return false;
    }
    match crate::validation::binary_slot_budget(entry) {
        Ok(Some((slot, budget))) => {
            return crate::validation::encoded_byte_len(&slot, translation)
                .is_some_and(|size| size <= budget);
        }
        Err(_) => return false,
        Ok(None) => {}
    }
    true
}

fn default_max_batch_tokens() -> Option<usize> {
    Some(6000)
}

/// Only occurrence locators/state are excluded. Raw source also pins the
/// placeholder map: two different controls can sanitize to the same request.
/// Languages, game context, glossary and fit policy are constant within a run.
#[derive(Hash, PartialEq, Eq)]
struct RepeatedSourceKey {
    source: String,
    context: Option<String>,
    tags: Vec<String>,
    char_limit: Option<usize>,
    physical_source: String,
    slot_budget: Option<(String, usize)>,
    // Shared TextAsset acceptance/retry hints depend on its other cells. Keep
    // these entries independent rather than assume interchangeable output.
    shared_cell: Option<String>,
}

struct RepeatedSources {
    members: HashMap<String, Vec<String>>,
}

impl RepeatedSources {
    fn collect(entries: Vec<StringEntry>) -> (Vec<StringEntry>, Self) {
        let mut representatives = Vec::new();
        let mut by_key: HashMap<RepeatedSourceKey, String> = HashMap::new();
        let mut members: HashMap<String, Vec<String>> = HashMap::new();
        for entry in entries {
            let key = RepeatedSourceKey {
                source: entry.source.clone(),
                context: entry.context.clone(),
                tags: entry.tags.clone(),
                char_limit: entry.char_limit,
                physical_source: entry
                    .injection_source()
                    .expect("injection source metadata prevalidated")
                    .to_owned(),
                slot_budget: crate::validation::binary_slot_budget(&entry)
                    .expect("binary slot metadata prevalidated"),
                shared_cell: crate::textasset_group::is_grouped_entry(&entry)
                    .then(|| entry.id.clone()),
            };
            if let Some(id) = by_key.get(&key) {
                members.get_mut(id).unwrap().push(entry.id);
            } else {
                by_key.insert(key, entry.id.clone());
                members.insert(entry.id.clone(), vec![entry.id.clone()]);
                representatives.push(entry);
            }
        }
        (representatives, Self { members })
    }

    fn expand(
        &self,
        results: Vec<TranslationResult>,
        placeholders: &mut HashMap<String, Vec<Placeholder>>,
        budgets: &mut HashMap<String, (String, usize)>,
    ) -> Vec<TranslationResult> {
        let mut expanded = Vec::new();
        for result in results {
            for id in &self.members[&result.entry_id] {
                let mut member = result.clone();
                if id != &result.entry_id {
                    member.entry_id.clone_from(id);
                    // Provider usage belongs to the representative, never to
                    // the number of rows that reuse its answer.
                    member.tokens_used = None;
                    member.input_tokens = None;
                    member.output_tokens = None;
                    member.cost_usd = None;
                    if let Some(phs) = placeholders.get(&result.entry_id).cloned() {
                        placeholders.insert(id.clone(), phs);
                    }
                    if let Some(budget) = budgets.get(&result.entry_id).cloned() {
                        budgets.insert(id.clone(), budget);
                    }
                }
                expanded.push(member);
            }
        }
        expanded
    }
}

fn translation_batches<'a>(
    entries: &'a [StringEntry],
    opts: &TranslationOptions,
) -> Vec<&'a [StringEntry]> {
    let mut batches = Vec::new();
    let mut start = 0;
    let mut tokens = 0usize;
    for (index, entry) in entries.iter().enumerate() {
        let estimate = entry
            .source
            .len()
            .saturating_add(entry.context.as_ref().map_or(0, String::len))
            .saturating_add(opts.game_context.as_ref().map_or(0, String::len))
            .div_ceil(3)
            .saturating_add(128);
        if index > start
            && (index - start >= opts.batch_size
                || opts
                    .max_batch_tokens
                    .is_some_and(|limit| tokens.saturating_add(estimate) > limit))
        {
            batches.push(&entries[start..index]);
            start = index;
            tokens = 0;
        }
        tokens = tokens.saturating_add(estimate);
    }
    if start < entries.len() {
        batches.push(&entries[start..]);
    }
    batches
}

fn group_retry_batches<'a>(
    requests: &'a [TranslationRequest],
    opts: &TranslationOptions,
) -> std::result::Result<Vec<&'a [TranslationRequest]>, &'static str> {
    let mut batches = Vec::new();
    let mut start = 0;
    let mut tokens = 0usize;
    for (index, request) in requests.iter().enumerate() {
        let estimate = request
            .source
            .len()
            .saturating_add(request.context.as_ref().map_or(0, String::len))
            .saturating_add(request.glossary_hint.as_ref().map_or(0, String::len))
            .div_ceil(3)
            .saturating_add(128);
        if opts.max_batch_tokens.is_some_and(|limit| estimate > limit) {
            return Err(
                "shared TextAsset retry exceeds the request token budget; entries remain pending",
            );
        }
        if index > start
            && (index - start >= opts.batch_size
                || opts
                    .max_batch_tokens
                    .is_some_and(|limit| tokens.saturating_add(estimate) > limit))
        {
            batches.push(&requests[start..index]);
            start = index;
            tokens = 0;
        }
        tokens = tokens.saturating_add(estimate);
    }
    if start < requests.len() {
        batches.push(&requests[start..]);
    }
    Ok(batches)
}

/// Restore placeholder tokens in a provider result (best-effort on failure).
fn restore_placeholders_in_result(
    result: &mut TranslationResult,
    placeholders_by_id: &std::collections::HashMap<String, Vec<Placeholder>>,
) {
    let Some(phs) = placeholders_by_id.get(&result.entry_id) else {
        return;
    };
    if phs.is_empty() {
        return;
    }
    match PlaceholderProcessor::restore(&result.translation, phs) {
        Ok(restored) => result.translation = restored,
        Err(e) => {
            tracing::warn!(
                "Failed to restore placeholders for {}: {}. Falling back to original with any missing tokens replaced.",
                result.entry_id, e
            );
            let mut t = result.translation.clone();
            for ph in phs {
                t = t.replace(&ph.token, &ph.original);
            }
            result.translation = t;
        }
    }
}

/// Tight UI slots (short source labels) need a harsher first-pass budget hint.
/// Threshold is in **encoded bytes** of the source string.
const TIGHT_BINARY_SLOT_BYTES: usize = 12;

/// Extra provider attempts after an oversize first answer (not counting the
/// original batch call). Two retries help very tight UI labels (e.g. 7–9 byte
/// slots) when the first shortening still misses by 1 byte.
const MAX_BINARY_SLOT_LENGTH_RETRIES: usize = 2;

struct GroupAccumulator {
    selected: HashMap<String, Vec<String>>,
    group_by_entry: HashMap<String, String>,
    changed_groups: HashSet<String>,
    results: HashMap<String, TranslationResult>,
    requests: HashMap<String, TranslationRequest>,
    placeholders: HashMap<String, Vec<Placeholder>>,
    failed: HashSet<String>,
    finalized: HashSet<String>,
}

impl GroupAccumulator {
    fn from_entries(entries: &[StringEntry]) -> Self {
        let mut selected: HashMap<String, Vec<String>> = HashMap::new();
        let mut group_by_entry = HashMap::new();
        for entry in entries {
            if let Ok(meta) = crate::textasset_group::parse_group_meta(entry) {
                group_by_entry.insert(entry.id.clone(), meta.id.clone());
                selected.entry(meta.id).or_default().push(entry.id.clone());
            }
        }
        Self {
            selected,
            group_by_entry,
            changed_groups: HashSet::new(),
            results: HashMap::new(),
            requests: HashMap::new(),
            placeholders: HashMap::new(),
            failed: HashSet::new(),
            finalized: HashSet::new(),
        }
    }

    fn is_grouped(&self, id: &str) -> bool {
        self.group_by_entry.contains_key(id)
    }

    fn mark_changed(&mut self, id: &str) {
        if let Some(group_id) = self.group_by_entry.get(id) {
            self.changed_groups.insert(group_id.clone());
        }
    }

    fn ready_group_ids(&mut self) -> Vec<String> {
        self.changed_groups
            .drain()
            .filter(|gid| {
                !self.finalized.contains(gid)
                    && self.selected[gid]
                        .iter()
                        .all(|id| self.results.contains_key(id) || self.failed.contains(id))
            })
            .collect()
    }

    fn abandon_missing(&mut self) {
        for (group_id, ids) in &self.selected {
            if self.finalized.contains(group_id) {
                continue;
            }
            self.changed_groups.insert(group_id.clone());
            for id in ids {
                if !self.results.contains_key(id) {
                    self.failed.insert(id.clone());
                }
            }
        }
    }
}

fn collect_group_owned_patches(
    meta: &crate::textasset_group::GroupMeta,
    selected_ids: &[String],
    results: &HashMap<String, TranslationResult>,
    entries_by_id: &HashMap<String, StringEntry>,
    db_groups: &HashMap<String, Vec<StringEntry>>,
) -> Vec<(StringEntry, String)> {
    let mut owned = Vec::new();
    let mut seen = HashSet::new();
    for id in selected_ids {
        if let (Some(entry), Some(result)) = (entries_by_id.get(id), results.get(id)) {
            owned.push((entry.clone(), result.translation.clone()));
            seen.insert(id.clone());
        }
    }
    for sibling in db_groups.get(&meta.id).into_iter().flatten() {
        let Ok(sibling_meta) = crate::textasset_group::parse_group_meta(sibling) else {
            continue;
        };
        if sibling_meta != *meta || seen.contains(&sibling.id) {
            continue;
        }
        let Some(translation) = sibling.translation.as_deref() else {
            continue;
        };
        if translation.is_empty() || translation == sibling.source {
            continue;
        }
        seen.insert(sibling.id.clone());
        owned.push((sibling.clone(), translation.to_string()));
    }
    owned
}

async fn refuse_group(
    tx: &mpsc::Sender<ProgressEvent>,
    selected_ids: &[String],
    oversize_after_retry: &mut usize,
    error: &str,
) {
    for id in selected_ids {
        *oversize_after_retry += 1;
        let _ = tx
            .send(ProgressEvent::BatchFailed {
                entry_id: Some(id.clone()),
                error: error.to_string(),
            })
            .await;
    }
}

/// First-pass context hint for binary-slot inject budgets.
/// When `source` is set and the budget is tight, quote the source so the model
/// can edit length against the actual label (helps short UI slots).
fn binary_slot_length_hint(slot: &str, budget: usize, source: &str) -> String {
    let src_display = if source.chars().count() > 48 {
        let t: String = source.chars().take(48).collect();
        format!("{t}…")
    } else {
        source.to_string()
    };
    let accent_note = if slot == "utf8" && source.is_ascii() && budget <= TIGHT_BINARY_SLOT_BYTES {
        " Prefer ASCII when needed: accented letters cost 2 UTF-8 bytes each (ó/ñ/ü)."
    } else {
        ""
    };
    if budget <= TIGHT_BINARY_SLOT_BYTES {
        format!(
            "LENGTH LIMIT: HARD MAX {budget} bytes ({slot}). Source: «{src_display}». \
             Prefer one short word or heavy abbreviation; spaces and accents count as bytes.\
             {accent_note}"
        )
    } else {
        format!(
            "LENGTH LIMIT: the translation MUST fit in {budget} bytes when encoded as {slot}; \
             abbreviate if needed."
        )
    }
}

/// Length-aware retry correction: include the failed text and exact excess so the
/// model can edit instead of retranslating from scratch.
fn binary_slot_retry_correction(
    slot: &str,
    budget: usize,
    first_len: usize,
    previous: &str,
) -> String {
    let excess = first_len.saturating_sub(budget);
    // Cap quoted previous text so a runaway provider answer does not blow context.
    let prev_display = if previous.chars().count() > 80 {
        let truncated: String = previous.chars().take(80).collect();
        format!("{truncated}…")
    } else {
        previous.to_string()
    };
    format!(
        "PREVIOUS ATTEMPT WAS {first_len} BYTES — HARD LIMIT {budget} BYTES ({slot}). \
         Previous text: «{prev_display}». Remove at least {excess} byte(s). \
         Shorten aggressively: drop articles/vowels/spaces, use abbreviations; \
         return ONLY the shortened translation."
    )
}

#[derive(Clone, Debug)]
pub struct RetryConfig {
    pub max_attempts: u32,
    pub initial_delay_ms: u64,
    pub max_delay_ms: u64,
    pub backoff_multiplier: f64,
    pub overall_deadline: Option<Duration>,
    pub cancel: Option<CancellationToken>,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            initial_delay_ms: 1000,
            max_delay_ms: 30000,
            backoff_multiplier: 2.0,
            overall_deadline: Some(Duration::from_secs(60)),
            cancel: None,
        }
    }
}

pub async fn with_retry<F, Fut, T>(config: &RetryConfig, operation: F) -> Result<T>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Result<T>>,
{
    let deadline = config
        .overall_deadline
        .map(|d| tokio::time::Instant::now() + d);
    let mut last_err = None;

    for attempt in 0..config.max_attempts {
        if let Some(cancel) = &config.cancel {
            if cancel.is_cancelled() {
                return Err(LocustError::ProviderError("cancelled".into()));
            }
        }
        if let Some(dl) = deadline {
            if tokio::time::Instant::now() >= dl {
                return Err(last_err.unwrap_or_else(|| {
                    LocustError::ProviderError("retry deadline exceeded".into())
                }));
            }
        }

        // Bound the in-flight request too, not just the delay between attempts.
        // Dropping the future cancels the local HTTP wait when the job is stopped.
        let outcome = tokio::select! {
            biased;
            _ = async {
                match &config.cancel {
                    Some(cancel) => cancel.cancelled().await,
                    None => std::future::pending().await,
                }
            } => return Err(LocustError::ProviderError("cancelled".into())),
            _ = async {
                match deadline {
                    Some(dl) => tokio::time::sleep_until(dl).await,
                    None => std::future::pending().await,
                }
            } => return Err(LocustError::ProviderError("provider request timeout: retry deadline exceeded".into())),
            outcome = operation() => outcome,
        };
        match outcome {
            Ok(v) => return Ok(v),
            Err(e) => {
                if is_retryable(&e) && attempt < config.max_attempts - 1 {
                    let mut delay_ms = jitter_delay(compute_delay(config, attempt));
                    if let Some(dl) = deadline {
                        let remaining = dl
                            .saturating_duration_since(tokio::time::Instant::now())
                            .as_millis() as u64;
                        if remaining == 0 {
                            return Err(e);
                        }
                        delay_ms = delay_ms.min(remaining);
                    }
                    tracing::warn!(
                        "Retryable error on attempt {}/{}: {}. Retrying in {}ms",
                        attempt + 1,
                        config.max_attempts,
                        e,
                        delay_ms
                    );
                    let sleep = tokio::time::sleep(Duration::from_millis(delay_ms));
                    if let Some(cancel) = &config.cancel {
                        tokio::select! {
                            biased;
                            _ = cancel.cancelled() => {
                                return Err(LocustError::ProviderError("cancelled".into()));
                            }
                            _ = sleep => {}
                        }
                    } else {
                        sleep.await;
                    }
                    last_err = Some(e);
                } else {
                    return Err(e);
                }
            }
        }
    }

    Err(last_err
        .unwrap_or_else(|| LocustError::ProviderError("retry exhausted with no error".to_string())))
}

fn compute_delay(config: &RetryConfig, attempt: u32) -> u64 {
    let delay = config.initial_delay_ms as f64 * config.backoff_multiplier.powi(attempt as i32);
    (delay as u64).min(config.max_delay_ms)
}

fn jitter_delay(base_ms: u64) -> u64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let factor = 0.5 + (nanos % 1000) as f64 / 1000.0;
    ((base_ms as f64) * factor) as u64
}

pub fn is_retryable(e: &LocustError) -> bool {
    match e {
        LocustError::ProviderError(msg) => {
            let lower = msg.to_ascii_lowercase();
            if lower.contains("cancelled")
                || lower.contains("401")
                || lower.contains("403")
                || lower.contains("unauthorized")
                || lower.contains("forbidden")
            {
                return false;
            }
            lower.contains("429")
                || lower.contains("rate limit")
                || lower.contains("timeout")
                || lower.contains("timed out")
                || lower.contains("500")
                || lower.contains("502")
                || lower.contains("503")
                || lower.contains("504")
                || lower.contains("connection")
                || lower.contains("network")
                || lower.contains("dns")
        }
        LocustError::IoError(_) => true,
        _ => false,
    }
}

pub struct RateLimiter {
    requests_per_minute: u32,
    last_requests: Mutex<VecDeque<tokio::time::Instant>>,
}

impl RateLimiter {
    pub fn new(requests_per_minute: u32) -> Self {
        Self {
            requests_per_minute,
            last_requests: Mutex::new(VecDeque::new()),
        }
    }

    pub fn unlimited() -> Self {
        Self::new(u32::MAX)
    }

    pub fn requests_per_minute(&self) -> u32 {
        self.requests_per_minute
    }

    pub fn is_unlimited(&self) -> bool {
        self.requests_per_minute == u32::MAX
    }

    pub async fn acquire(&self) {
        loop {
            let should_wait = {
                let mut requests = self.last_requests.lock().unwrap();
                let now = tokio::time::Instant::now();
                let window = Duration::from_secs(60);

                while let Some(&front) = requests.front() {
                    if now.duration_since(front) > window {
                        requests.pop_front();
                    } else {
                        break;
                    }
                }

                if (requests.len() as u32) < self.requests_per_minute {
                    requests.push_back(now);
                    None
                } else {
                    requests.front().map(|&oldest| {
                        let elapsed = now.duration_since(oldest);
                        if elapsed < window {
                            window - elapsed + Duration::from_millis(10)
                        } else {
                            Duration::from_millis(10)
                        }
                    })
                }
            };

            match should_wait {
                None => return,
                Some(dur) => tokio::time::sleep(dur).await,
            }
        }
    }
}

/// Built-in requests/minute when `ProviderConfig.extra` has no override.
/// Local engines stay unlimited; the unofficial Google web endpoint is tight.
pub fn default_requests_per_minute(provider_id: &str) -> u32 {
    match provider_id {
        "ollama" | "lmstudio" | "argos" | "mock" => u32::MAX,
        // Free translate.googleapis.com endpoint — conservative to avoid blocks.
        "google" => 8,
        "deepl" => 120,
        "openai" | "claude" | "grok" | "grok-sub" | "deepseek" | "gemini" => 90,
        _ => 60,
    }
}

fn rpm_from_provider_extra(extra: &HashMap<String, String>) -> Option<u32> {
    extra
        .get("requests_per_minute")
        .and_then(|s| s.parse().ok())
        .filter(|&n| n > 0)
}

/// Resolve rpm from existing `ProviderConfig.extra["requests_per_minute"]`,
/// else the built-in table. No new config field.
pub fn requests_per_minute_for(provider_id: &str, extra: Option<&HashMap<String, String>>) -> u32 {
    extra
        .and_then(rpm_from_provider_extra)
        .unwrap_or_else(|| default_requests_per_minute(provider_id))
}

/// Per-provider-id limiter so one chatty engine does not stall the others.
pub fn rate_limiter_for(provider_id: &str) -> Arc<RateLimiter> {
    static LIMITERS: OnceLock<Mutex<HashMap<String, Arc<RateLimiter>>>> = OnceLock::new();
    let map = LIMITERS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = map.lock().unwrap();
    guard
        .entry(provider_id.to_string())
        .or_insert_with(|| {
            let extra = crate::config::AppConfig::load(&crate::config::AppConfig::default_path())
                .ok()
                .and_then(|cfg| cfg.get_provider_config(provider_id).cloned())
                .map(|pc| pc.extra);
            let rpm = requests_per_minute_for(provider_id, extra.as_ref());
            Arc::new(RateLimiter::new(rpm))
        })
        .clone()
}

struct ProviderCall {
    results: Vec<TranslationResult>,
    attempts: usize,
}

async fn call_provider(
    provider: Arc<dyn TranslationProvider>,
    requests: &[TranslationRequest],
    retry: &RetryConfig,
    cancel: &CancellationToken,
) -> Result<ProviderCall> {
    let limiter = rate_limiter_for(provider.id());
    call_provider_with_limiter(provider, requests, retry, cancel, limiter).await
}

async fn call_provider_with_limiter(
    provider: Arc<dyn TranslationProvider>,
    requests: &[TranslationRequest],
    retry: &RetryConfig,
    cancel: &CancellationToken,
    limiter: Arc<RateLimiter>,
) -> Result<ProviderCall> {
    if cancel.is_cancelled() {
        return Err(LocustError::ProviderError("cancelled".into()));
    }
    // Queueing for the first allowance is not HTTP latency. Slow provider
    // quotas may legitimately require more than the request deadline.
    tokio::select! {
        biased;
        _ = cancel.cancelled() => return Err(LocustError::ProviderError("cancelled".into())),
        _ = limiter.acquire() => {}
    }
    let mut cfg = retry.clone();
    cfg.cancel = Some(cancel.clone());
    let attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let provider = provider.clone();
    let requests = requests.to_vec();
    let attempts_for_call = attempts.clone();
    let results = with_retry(&cfg, move || {
        let provider = provider.clone();
        let requests = requests.clone();
        let limiter = limiter.clone();
        let is_retry = attempts_for_call.fetch_add(1, std::sync::atomic::Ordering::Relaxed) > 0;
        async move {
            // Every HTTP attempt consumes rate allowance, including retries.
            if is_retry {
                limiter.acquire().await;
            }
            tracing::debug!(
                provider = provider.id(),
                strings = requests.len(),
                "dispatching translation requests"
            );
            provider.translate(&requests).await
        }
    })
    .await?;
    Ok(ProviderCall {
        results,
        attempts: attempts.load(std::sync::atomic::Ordering::Relaxed),
    })
}

pub struct TranslationManager {
    provider: Arc<dyn TranslationProvider>,
    db: Arc<Database>,
    glossary: Arc<Glossary>,
    retry: RetryConfig,
}

impl TranslationManager {
    pub fn new(
        provider: Arc<dyn TranslationProvider>,
        db: Arc<Database>,
        glossary: Arc<Glossary>,
    ) -> Self {
        let mut retry = RetryConfig::default();
        // Reasoning models can legitimately take longer than one minute for
        // game-dialogue batches. Cancellation remains immediate and bounded.
        if matches!(provider.id(), "grok" | "grok-sub") {
            retry.overall_deadline = Some(Duration::from_secs(180));
        }
        Self {
            provider,
            db,
            glossary,
            retry,
        }
    }

    pub fn with_retry_config(mut self, retry: RetryConfig) -> Self {
        self.retry = retry;
        self
    }

    #[allow(clippy::too_many_arguments)]
    async fn finalize_ready_textasset_groups(
        &self,
        acc: &mut GroupAccumulator,
        entries_by_id: &HashMap<String, StringEntry>,
        db_groups: &HashMap<String, Vec<StringEntry>>,
        opts: &TranslationOptions,
        tx: &mpsc::Sender<ProgressEvent>,
        cancel: &CancellationToken,
        lang_pair: &str,
        sources_by_id: &HashMap<String, String>,
        save_guards: &HashMap<String, TranslationSaveGuard>,
        observed_cost: &mut ObservedCost,
        budget_error: &mut Option<LocustError>,
        oversize_after_retry: &mut usize,
        retried_ok: &mut usize,
        completed: &mut usize,
        total_tokens: &mut u64,
        total_input_tokens: &mut u64,
        total_output_tokens: &mut u64,
    ) -> Result<()> {
        'groups: for group_id in acc.ready_group_ids() {
            if cancel.is_cancelled() {
                return Ok(());
            }
            acc.finalized.insert(group_id.clone());
            let selected_ids = acc.selected.get(&group_id).cloned().unwrap_or_default();
            let Some(head) = selected_ids.iter().find_map(|id| entries_by_id.get(id)) else {
                continue;
            };
            let Ok(meta) = crate::textasset_group::parse_group_meta(head) else {
                continue;
            };
            let mut attempts = 0usize;
            'rounds: loop {
                if cancel.is_cancelled() {
                    return Ok(());
                }
                let owned = collect_group_owned_patches(
                    &meta,
                    &selected_ids,
                    &acc.results,
                    entries_by_id,
                    db_groups,
                );
                let mut patches = Vec::new();
                let mut selected_patched = 0usize;
                let selected_with_results = selected_ids
                    .iter()
                    .filter(|id| acc.results.contains_key(*id))
                    .count();
                let mut reconstruct_ok = true;
                for (entry, text) in &owned {
                    match crate::textasset_group::patch_from_entry(entry, text) {
                        Ok(patch) => {
                            if acc.results.contains_key(&entry.id) {
                                selected_patched += 1;
                            }
                            patches.push(patch);
                        }
                        Err(_) => {
                            reconstruct_ok = false;
                            break;
                        }
                    }
                }
                if !reconstruct_ok || selected_patched != selected_with_results {
                    refuse_group(
                        tx,
                        &selected_ids,
                        oversize_after_retry,
                        "translation exceeds the shared TextAsset capacity; entries remain pending (no automatic truncation)",
                    )
                    .await;
                    break;
                }
                match crate::textasset_group::reconstructed_fits(&meta, &patches) {
                    Ok(true) => {
                        let to_save: Vec<TranslationResult> = selected_ids
                            .iter()
                            .filter_map(|id| acc.results.get(id).cloned())
                            .collect();
                        if to_save.is_empty() {
                            break;
                        }
                        if cancel.is_cancelled() {
                            return Ok(());
                        }
                        self.db
                            .save_translation_results_guarded(&to_save, save_guards)
                            .await?;
                        if attempts > 0 {
                            *retried_ok += to_save.len();
                        }
                        let mut memory_items = Vec::new();
                        for result in &to_save {
                            if opts.use_memory && result.provider != "mock" {
                                if let Some(source) = sources_by_id.get(&result.entry_id) {
                                    use sha2::{Digest, Sha256};
                                    let hash = hex::encode(Sha256::digest(source.as_bytes()));
                                    memory_items.push((
                                        hash,
                                        source.clone(),
                                        result.translation.clone(),
                                    ));
                                }
                            }
                        }
                        let _ = self.db.save_memory_batch(&memory_items, lang_pair).await;
                        for result in &to_save {
                            let _ = tx
                                .send(ProgressEvent::StringTranslated {
                                    entry_id: result.entry_id.clone(),
                                    translation: result.translation.clone(),
                                })
                                .await;
                            *completed += 1;
                        }
                        break;
                    }
                    Ok(false) if attempts < MAX_BINARY_SLOT_LENGTH_RETRIES => {
                        let actual = crate::textasset_group::reconstructed_len(&meta, &patches)
                            .unwrap_or(meta.capacity.saturating_add(1));
                        let retry_reqs: Vec<TranslationRequest> = selected_ids
                            .iter()
                            .filter_map(|id| acc.requests.get(id).cloned())
                            .map(|mut req| {
                                let prev = acc
                                    .results
                                    .get(&req.entry_id)
                                    .map(|r| r.translation.as_str())
                                    .unwrap_or("");
                                let correction = crate::textasset_group::retry_correction(
                                    meta.capacity,
                                    actual,
                                    prev,
                                );
                                req.context = Some(match req.context {
                                    Some(c) => format!("{c} | {correction}"),
                                    None => correction,
                                });
                                req
                            })
                            .collect();
                        if retry_reqs.is_empty() {
                            refuse_group(
                                tx,
                                &selected_ids,
                                oversize_after_retry,
                                "translation exceeds the shared TextAsset capacity; entries remain pending (no automatic truncation)",
                            )
                            .await;
                            break;
                        }
                        let retry_batches = match group_retry_batches(&retry_reqs, opts) {
                            Ok(batches) => batches,
                            Err(message) => {
                                refuse_group(tx, &selected_ids, oversize_after_retry, message)
                                    .await;
                                break;
                            }
                        };
                        attempts += 1;
                        for retry_reqs in retry_batches {
                            if cancel.is_cancelled() {
                                return Ok(());
                            }
                            if budget_error.is_some() {
                                refuse_group(
                                tx,
                                &selected_ids,
                                oversize_after_retry,
                                "translation exceeds the shared TextAsset capacity; entries remain pending (no automatic truncation)",
                            )
                            .await;
                                continue 'groups;
                            }
                            if let Some(limit) = opts.cost_limit_usd {
                                if !observed_cost.complete {
                                    *budget_error = Some(LocustError::ProviderError(
                                    "cannot enforce a cost limit: preceding calls have unknown cost"
                                        .into(),
                                ));
                                    refuse_group(
                                    tx,
                                    &selected_ids,
                                    oversize_after_retry,
                                    "translation exceeds the shared TextAsset capacity; entries remain pending (no automatic truncation)",
                                )
                                .await;
                                    continue 'groups;
                                }
                                let chars: usize = retry_reqs
                                    .iter()
                                    .map(|r| {
                                        r.source.len()
                                            + r.context.as_ref().map_or(0, String::len)
                                            + r.glossary_hint.as_ref().map_or(0, String::len)
                                    })
                                    .sum();
                                match self.provider.estimate_cost(chars, &opts.target_lang).await {
                                    Some(estimate) if estimate.is_finite() && estimate >= 0.0 => {
                                        if observed_cost.amount + estimate > limit {
                                            *budget_error = Some(LocustError::CostLimitExceeded {
                                                estimated: observed_cost.amount + estimate,
                                                limit,
                                            });
                                        }
                                    }
                                    None if self.provider.is_free() => {}
                                    _ => {
                                        *budget_error = Some(LocustError::ProviderError(
                                        "cannot enforce a cost limit: this provider has no valid cost estimate".into(),
                                    ));
                                    }
                                }
                                if budget_error.is_some() {
                                    refuse_group(
                                    tx,
                                    &selected_ids,
                                    oversize_after_retry,
                                    "translation exceeds the shared TextAsset capacity; entries remain pending (no automatic truncation)",
                                )
                                .await;
                                    continue 'groups;
                                }
                            }
                            match call_provider(
                                self.provider.clone(),
                                retry_reqs,
                                &self.retry,
                                cancel,
                            )
                            .await
                            {
                                Ok(retry_call) => {
                                    let ProviderCall {
                                        mut results,
                                        attempts: transport_attempts,
                                    } = retry_call;
                                    observed_cost.observe_batch(
                                        results.iter().map(|r| r.cost_usd),
                                        self.provider.is_free(),
                                    );
                                    if transport_attempts > 1 && !self.provider.is_free() {
                                        observed_cost.complete = false;
                                    }
                                    if budget_error.is_none() {
                                        *budget_error = observed_budget_error(
                                            *observed_cost,
                                            opts.cost_limit_usd,
                                        );
                                    }
                                    for usage in &results {
                                        *total_tokens += usage.tokens_used.unwrap_or(0) as u64;
                                        *total_input_tokens +=
                                            usage.input_tokens.unwrap_or(0) as u64;
                                        *total_output_tokens +=
                                            usage.output_tokens.unwrap_or(0) as u64;
                                    }
                                    let expected: HashSet<_> =
                                        retry_reqs.iter().map(|r| r.entry_id.as_str()).collect();
                                    let actual_ids: HashSet<_> =
                                        results.iter().map(|r| r.entry_id.as_str()).collect();
                                    if results.len() != retry_reqs.len()
                                        || actual_ids.len() != results.len()
                                        || actual_ids != expected
                                    {
                                        continue 'rounds;
                                    }
                                    for result in &mut results {
                                        restore_placeholders_in_result(result, &acc.placeholders);
                                        if let Some(source) = sources_by_id.get(&result.entry_id) {
                                            if result.translation.trim().is_empty()
                                                || !PlaceholderProcessor::validate(
                                                    source,
                                                    &result.translation,
                                                )
                                                .is_empty()
                                            {
                                                continue;
                                            }
                                        }
                                        acc.results.insert(result.entry_id.clone(), result.clone());
                                    }
                                }
                                Err(_) => {
                                    observed_cost.complete &= self.provider.is_free();
                                    if budget_error.is_none() {
                                        *budget_error = observed_budget_error(
                                            *observed_cost,
                                            opts.cost_limit_usd,
                                        );
                                    }
                                    refuse_group(
                                    tx,
                                    &selected_ids,
                                    oversize_after_retry,
                                    "translation exceeds the shared TextAsset capacity; entries remain pending (no automatic truncation)",
                                )
                                .await;
                                    continue 'groups;
                                }
                            }
                        }
                    }
                    Ok(false) | Err(_) => {
                        refuse_group(
                            tx,
                            &selected_ids,
                            oversize_after_retry,
                            "translation exceeds the shared TextAsset capacity; entries remain pending (no automatic truncation)",
                        )
                        .await;
                        break;
                    }
                }
            }
        }
        Ok(())
    }

    pub async fn translate_entries(
        &self,
        entries: Vec<StringEntry>,
        opts: TranslationOptions,
        tx: mpsc::Sender<ProgressEvent>,
        job_id: String,
        cancel: CancellationToken,
    ) -> Result<()> {
        self.translate_entries_inner(entries, opts, tx, job_id, cancel, true)
            .await
    }

    /// Like [`translate_entries`] but controls Started/Completed lifecycle events.
    /// Used by multi-provider chains so only the outer job emits terminal events.
    pub async fn translate_entries_inner(
        &self,
        entries: Vec<StringEntry>,
        opts: TranslationOptions,
        tx: mpsc::Sender<ProgressEvent>,
        job_id: String,
        cancel: CancellationToken,
        emit_lifecycle: bool,
    ) -> Result<()> {
        self.translate_entries_accounted(
            entries,
            opts,
            tx,
            job_id,
            cancel,
            emit_lifecycle,
            &mut ObservedCost::default(),
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn translate_entries_accounted(
        &self,
        entries: Vec<StringEntry>,
        opts: TranslationOptions,
        tx: mpsc::Sender<ProgressEvent>,
        job_id: String,
        cancel: CancellationToken,
        emit_lifecycle: bool,
        cumulative_cost: &mut ObservedCost,
    ) -> Result<()> {
        let start = Instant::now();

        if opts.batch_size == 0 || opts.max_batch_tokens == Some(0) {
            return Err(LocustError::ProviderError(
                "batch size and token budget must be greater than zero".into(),
            ));
        }
        if opts
            .cost_limit_usd
            .is_some_and(|n| !n.is_finite() || n < 0.0)
        {
            return Err(LocustError::ProviderError(
                "cost limit must be finite and non-negative".into(),
            ));
        }

        // 1. Filter translatable entries
        let mut translatable: Vec<StringEntry> = entries
            .into_iter()
            .filter(|e| {
                e.is_translatable() && !(opts.skip_approved && e.status == StringStatus::Approved)
            })
            .collect();
        // Fail before memory/glossary/provider work if immutable pivot capacity
        // metadata is malformed; silently deriving a weaker budget is unsafe.
        for entry in &translatable {
            crate::validation::binary_slot_budget(entry)?;
            crate::textasset_group::require_valid_metadata(entry)?;
        }

        let total = translatable.len();
        let sources_by_id: HashMap<_, _> = translatable
            .iter()
            .map(|entry| (entry.id.clone(), entry.source.clone()))
            .collect();
        let save_guards: HashMap<_, _> = translatable
            .iter()
            .map(|entry| (entry.id.clone(), TranslationSaveGuard::from(entry)))
            .collect();

        // 2. Send Started
        if emit_lifecycle {
            let _ = tx
                .send(ProgressEvent::Started {
                    total,
                    job_id: job_id.clone(),
                })
                .await;
        }

        let mut completed = 0usize;
        // Binary-slot entries still over budget after one length-aware retry.
        let mut oversize_after_retry = 0usize;
        // Binary-slot entries that fit only after the length-aware retry.
        let mut retried_ok = 0usize;
        let mut observed_cost = ObservedCost::default();
        let mut total_tokens = 0u64;
        let mut total_input_tokens = 0u64;
        let mut total_output_tokens = 0u64;
        let started_at = chrono::Utc::now().to_rfc3339();
        let lang_pair = format!("{}-{}", opts.source_lang, opts.target_lang);

        // 3. Look up memory in bulk, preserving entry order for saves and events.
        let mut remaining = Vec::new();
        if opts.use_memory {
            let hashes: Vec<_> = translatable.iter().map(StringEntry::source_hash).collect();
            // Lookup failures have always been cache misses, not run failures.
            let memory = self
                .db
                .lookup_memory_batch(&hashes, &lang_pair)
                .unwrap_or_default();
            let mut updates = Vec::new();
            let mut memory_events = Vec::new();
            for (entry, hash) in translatable.drain(..).zip(hashes) {
                if let Some(cached) = memory.get(&hash) {
                    if !translation_fits_entry(&entry, cached) {
                        remaining.push(entry);
                        continue;
                    }
                    let guard = TranslationSaveGuard::from(&entry);
                    updates.push((entry.id.clone(), cached.clone(), "memory".into(), guard));
                    memory_events.push(ProgressEvent::StringTranslated {
                        entry_id: entry.id,
                        translation: cached.clone(),
                    });
                } else {
                    remaining.push(entry);
                }
            }
            self.db.save_guarded_updates(updates).await?;
            for event in memory_events {
                let _ = tx.send(event).await;
                completed += 1;
            }
        } else {
            remaining = translatable;
        }

        let glossary_entries = if opts.use_glossary && !remaining.is_empty() {
            self.glossary.get_all(&lang_pair).unwrap_or_default()
        } else {
            Vec::new()
        };

        // 3b. Exact glossary hits (full-string): short-circuit provider — key for
        // short UI binary slots where the user already fixed a fitting form.
        if opts.use_glossary && !remaining.is_empty() {
            let mut still = Vec::with_capacity(remaining.len());
            for entry in remaining.drain(..) {
                let Some(term) =
                    Glossary::lookup_exact_in_entries(&glossary_entries, &entry.source)
                else {
                    still.push(entry);
                    continue;
                };
                // Exact glossary hits obey the same integrity rules as memory.
                if !translation_fits_entry(&entry, &term) {
                    still.push(entry);
                    continue;
                }
                self.db
                    .save_translation_guarded(&entry, &term, "glossary")
                    .await?;
                let _ = tx
                    .send(ProgressEvent::StringTranslated {
                        entry_id: entry.id.clone(),
                        translation: term,
                    })
                    .await;
                completed += 1;
            }
            remaining = still;
        }

        let entries_by_id: HashMap<String, StringEntry> = remaining
            .iter()
            .cloned()
            .map(|entry| (entry.id.clone(), entry))
            .collect();
        let mut group_acc = GroupAccumulator::from_entries(&remaining);
        let mut db_groups: HashMap<String, Vec<StringEntry>> = HashMap::new();
        if !group_acc.selected.is_empty() {
            for entry in self
                .db
                .get_entries_for_textasset_groups(&group_acc.selected.keys().cloned().collect())?
            {
                if let Some(group_id) = entry
                    .metadata
                    .get(crate::textasset_group::GROUP_ID_KEY)
                    .and_then(|v| v.as_str())
                {
                    if group_acc.selected.contains_key(group_id) {
                        db_groups
                            .entry(group_id.to_owned())
                            .or_default()
                            .push(entry);
                    }
                }
            }
        }

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
            Result<ProviderCall>,
        );
        let mut in_flight: tokio::task::JoinSet<BatchOutcome> = tokio::task::JoinSet::new();
        // Group across the entire pending run, before batching/concurrency.
        // Memory/glossary hits have already taken their normal per-row path.
        let (representatives, repeated) = RepeatedSources::collect(remaining);
        let chunks = translation_batches(&representatives, &opts);
        let mut chunk_iter = chunks.into_iter();
        let mut cancelled = false;
        let mut budget_error = None;

        loop {
            // 5a. Fill the in-flight window
            while in_flight.len() < concurrency && !cancelled {
                if cancel.is_cancelled() {
                    cancelled = true;
                    break;
                }

                let Some(chunk) = chunk_iter.next() else {
                    break;
                };

                // 5b. Check cost limit
                if let Some(limit) = opts.cost_limit_usd {
                    if !observed_cost.complete {
                        budget_error = Some(LocustError::ProviderError(
                            "cannot enforce a cost limit: preceding calls have unknown cost".into(),
                        ));
                        break;
                    }
                    let char_count: usize = chunk.iter().map(|e| e.source.len()).sum();
                    if let Some(estimated) = self
                        .provider
                        .estimate_cost(char_count, &opts.target_lang)
                        .await
                    {
                        if !estimated.is_finite() || estimated < 0.0 {
                            budget_error = Some(LocustError::ProviderError(
                                "cannot enforce a cost limit: this provider has no valid cost estimate".into(),
                            ));
                            break;
                        }
                        if observed_cost.amount + estimated > limit {
                            budget_error = Some(LocustError::CostLimitExceeded {
                                estimated: observed_cost.amount + estimated,
                                limit,
                            });
                            break;
                        }
                    } else if !self.provider.is_free() {
                        budget_error = Some(LocustError::ProviderError(
                            "cannot enforce a cost limit: this provider has no cost estimate"
                                .into(),
                        ));
                        break;
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
                        let mut slot_budget: Option<(String, usize)> = None;
                        if let Some((slot, budget)) = crate::validation::binary_slot_budget(entry)
                            .expect("binary slot metadata prevalidated")
                        {
                            budgets_by_id.insert(entry.id.clone(), (slot.clone(), budget));
                            slot_budget = Some((slot.clone(), budget));
                            let hint = binary_slot_length_hint(&slot, budget, &entry.source);
                            context = Some(match context {
                                Some(c) => format!("{c} | {hint}"),
                                None => hint,
                            });
                        }
                        if let Ok(meta) = crate::textasset_group::parse_group_meta(entry) {
                            let selected = group_acc
                                .selected
                                .get(&meta.id)
                                .map(|ids| ids.len())
                                .unwrap_or(1)
                                .max(1);
                            let hint = crate::textasset_group::first_pass_hint(&meta, selected);
                            context = Some(match context {
                                Some(c) => format!("{c} | {hint}"),
                                None => hint,
                            });
                            // Remaining shared bytes are not a per-cell limit; keep
                            // glossary unbudgeted so a long term can still share the blob.
                            slot_budget = None;
                        }
                        let (sanitized, phs) = PlaceholderProcessor::extract(&entry.source);
                        placeholders_by_id.insert(entry.id.clone(), phs);
                        // Per-entry glossary: terms present in source; for binary
                        // slots also drop translations that cannot fit the budget.
                        let glossary_hint = if opts.use_glossary {
                            Glossary::hint_from_entries(
                                &glossary_entries,
                                &entry.source,
                                slot_budget
                                    .as_ref()
                                    .map(|(encoding, bytes)| (encoding.as_str(), *bytes)),
                            )
                        } else {
                            None
                        };
                        TranslationRequest {
                            entry_id: entry.id.clone(),
                            source: sanitized,
                            source_lang: opts.source_lang.clone(),
                            target_lang: opts.target_lang.clone(),
                            context,
                            glossary_hint,
                        }
                    })
                    .collect();

                // 5d. Dispatch provider call to the in-flight window
                let provider = self.provider.clone();
                let retry = self.retry.clone();
                let cancel_batch = cancel.clone();
                in_flight.spawn(async move {
                    let result = call_provider(provider, &requests, &retry, &cancel_batch).await;
                    (requests, placeholders_by_id, budgets_by_id, result)
                });
            }

            // 5e. Wait for the next batch to finish; done when nothing is in flight
            let Some(joined) = in_flight.join_next().await else {
                break;
            };
            let (requests, mut placeholders_by_id, mut budgets_by_id, batch_result) = match joined {
                Ok(outcome) => outcome,
                Err(e) => {
                    observed_cost.complete &= self.provider.is_free();
                    tracing::error!("Translation batch task panicked: {}", e);
                    if budget_error.is_none() {
                        budget_error = observed_budget_error(observed_cost, opts.cost_limit_usd);
                    }
                    if budget_error.is_some() {
                        break;
                    }
                    continue;
                }
            };

            match batch_result {
                Ok(call) => {
                    let ProviderCall {
                        mut results,
                        attempts,
                    } = call;
                    observed_cost
                        .observe_batch(results.iter().map(|r| r.cost_usd), self.provider.is_free());
                    if attempts > 1 && !self.provider.is_free() {
                        // A successful response describes the final attempt only.
                        // Earlier transport attempts may have been processed remotely.
                        observed_cost.complete = false;
                    }
                    if budget_error.is_none() {
                        budget_error = observed_budget_error(observed_cost, opts.cost_limit_usd);
                    }
                    // Account provider work before validation or fan-out,
                    // including malformed/partial responses that cannot save.
                    for result in &results {
                        total_tokens += result.tokens_used.unwrap_or(0) as u64;
                        total_input_tokens += result.input_tokens.unwrap_or(0) as u64;
                        total_output_tokens += result.output_tokens.unwrap_or(0) as u64;
                    }
                    let expected: std::collections::HashSet<_> =
                        requests.iter().map(|r| r.entry_id.as_str()).collect();
                    let actual: std::collections::HashSet<_> =
                        results.iter().map(|r| r.entry_id.as_str()).collect();
                    if results.len() != requests.len()
                        || actual.len() != results.len()
                        || actual != expected
                    {
                        // Invalid IDs prevent storage, not billing: account for
                        // the provider usage before considering another batch.
                        let _ = tx
                            .send(ProgressEvent::BatchFailed {
                                entry_id: None,
                                error: "provider returned mismatched or duplicate entry IDs".into(),
                            })
                            .await;
                        if budget_error.is_some() {
                            break;
                        }
                        continue;
                    }
                    if cancel.is_cancelled() {
                        cancelled = true;
                        in_flight.abort_all();
                        break;
                    }
                    // Restore placeholders in translations before saving
                    for result in &mut results {
                        restore_placeholders_in_result(result, &placeholders_by_id);
                    }

                    // Length-aware retries: up to MAX_BINARY_SLOT_LENGTH_RETRIES
                    // extra provider attempts per oversize binary-slot entry.
                    for result in &mut results {
                        let Some((slot, budget)) = budgets_by_id.get(&result.entry_id) else {
                            continue;
                        };
                        let Some(mut best_len) =
                            crate::validation::encoded_byte_len(slot, &result.translation)
                        else {
                            continue;
                        };
                        if best_len <= *budget {
                            continue;
                        }

                        let Some(orig_req) =
                            requests.iter().find(|r| r.entry_id == result.entry_id)
                        else {
                            continue;
                        };

                        let mut fitted = false;
                        for attempt in 1..=MAX_BINARY_SLOT_LENGTH_RETRIES {
                            let prev_text = result.translation.clone();
                            let prev_len = best_len;
                            let correction =
                                binary_slot_retry_correction(slot, *budget, prev_len, &prev_text);
                            let mut retry_req = orig_req.clone();
                            retry_req.context = Some(match &orig_req.context {
                                Some(c) => format!("{c} | {correction}"),
                                None => correction,
                            });

                            // Length corrections are paid calls too. Stop before
                            // dispatch when the same run budget cannot cover them.
                            if budget_error.is_some() {
                                break;
                            }
                            if let Some(limit) = opts.cost_limit_usd {
                                if !observed_cost.complete {
                                    budget_error = Some(LocustError::ProviderError(
                                        "cannot enforce a cost limit: preceding calls have unknown cost".into(),
                                    ));
                                    break;
                                }
                                let chars = retry_req
                                    .source
                                    .len()
                                    .saturating_add(
                                        retry_req.context.as_ref().map_or(0, String::len),
                                    )
                                    .saturating_add(
                                        retry_req.glossary_hint.as_ref().map_or(0, String::len),
                                    );
                                match self.provider.estimate_cost(chars, &opts.target_lang).await {
                                    Some(estimate) if estimate.is_finite() && estimate >= 0.0 => {
                                        if observed_cost.amount + estimate > limit {
                                            budget_error = Some(LocustError::CostLimitExceeded {
                                                estimated: observed_cost.amount + estimate,
                                                limit,
                                            });
                                        }
                                    }
                                    None if self.provider.is_free() => {}
                                    _ => {
                                        budget_error = Some(LocustError::ProviderError(
                                            "cannot enforce a cost limit: this provider has no valid cost estimate".into(),
                                        ));
                                    }
                                }
                                if budget_error.is_some() {
                                    break;
                                }
                            }

                            match call_provider(
                                self.provider.clone(),
                                std::slice::from_ref(&retry_req),
                                &self.retry,
                                &cancel,
                            )
                            .await
                            {
                                Ok(retry_call) => {
                                    let ProviderCall {
                                        results: mut retry_batch,
                                        attempts,
                                    } = retry_call;
                                    observed_cost.observe_batch(
                                        retry_batch.iter().map(|r| r.cost_usd),
                                        self.provider.is_free(),
                                    );
                                    if attempts > 1 && !self.provider.is_free() {
                                        observed_cost.complete = false;
                                    }
                                    if budget_error.is_none() {
                                        budget_error = observed_budget_error(
                                            observed_cost,
                                            opts.cost_limit_usd,
                                        );
                                    }
                                    // Account every returned retry result, even when its IDs
                                    // are invalid. The original result retains original usage.
                                    for usage in &retry_batch {
                                        total_tokens += usage.tokens_used.unwrap_or(0) as u64;
                                        total_input_tokens +=
                                            usage.input_tokens.unwrap_or(0) as u64;
                                        total_output_tokens +=
                                            usage.output_tokens.unwrap_or(0) as u64;
                                    }
                                    if retry_batch.len() != 1
                                        || retry_batch[0].entry_id != result.entry_id
                                    {
                                        let _ = tx.send(ProgressEvent::BatchFailed {
                                            entry_id: Some(result.entry_id.clone()),
                                            error: "length retry returned mismatched or duplicate entry IDs; previous text preserved".into(),
                                        }).await;
                                        continue;
                                    }
                                    let mut retry_result = retry_batch.remove(0);
                                    restore_placeholders_in_result(
                                        &mut retry_result,
                                        &placeholders_by_id,
                                    );
                                    if retry_result.translation.trim().is_empty()
                                        || sources_by_id.get(&result.entry_id).is_some_and(
                                            |source| {
                                                !PlaceholderProcessor::validate(
                                                    source,
                                                    &retry_result.translation,
                                                )
                                                .is_empty()
                                            },
                                        )
                                    {
                                        let _ = tx.send(ProgressEvent::BatchFailed {
                                            entry_id: Some(result.entry_id.clone()),
                                            error: "length retry is empty or changed protected placeholders; previous text preserved".into(),
                                        }).await;
                                        continue;
                                    }
                                    let new_len = crate::validation::encoded_byte_len(
                                        slot,
                                        &retry_result.translation,
                                    )
                                    .unwrap_or(usize::MAX);

                                    if new_len <= *budget {
                                        result.translation = retry_result.translation;
                                        best_len = new_len;
                                        fitted = true;
                                        retried_ok += repeated.members[&result.entry_id].len();
                                        tracing::info!(
                                            entry_id = %result.entry_id,
                                            attempt,
                                            prev_len,
                                            new_len,
                                            budget,
                                            slot = %slot,
                                            "binary-slot translation fits after length-aware retry"
                                        );
                                        break;
                                    }

                                    // Still oversize: keep the shorter attempt and continue.
                                    if new_len < best_len {
                                        result.translation = retry_result.translation;
                                        best_len = new_len;
                                    }
                                    tracing::warn!(
                                        entry_id = %result.entry_id,
                                        attempt,
                                        prev_len,
                                        new_len,
                                        best_len,
                                        budget,
                                        slot = %slot,
                                        "length-aware retry still oversize"
                                    );
                                }
                                Err(e) => {
                                    observed_cost.complete &= self.provider.is_free();
                                    if budget_error.is_none() {
                                        budget_error = observed_budget_error(
                                            observed_cost,
                                            opts.cost_limit_usd,
                                        );
                                    }
                                    tracing::warn!(
                                        entry_id = %result.entry_id,
                                        attempt,
                                        error = %e,
                                        prev_len,
                                        budget,
                                        slot = %slot,
                                        "length-aware retry failed; keeping best attempt"
                                    );
                                    break;
                                }
                            }
                        }

                        if !fitted && opts.allow_lossy_binary_fit {
                            // Deterministic last resort: accent-fold / despace / truncate.
                            if let Some(fitted_text) = crate::validation::mechanical_fit_binary_slot(
                                slot,
                                *budget,
                                &result.translation,
                            ) {
                                let new_len =
                                    crate::validation::encoded_byte_len(slot, &fitted_text)
                                        .unwrap_or(usize::MAX);
                                if new_len <= *budget {
                                    tracing::info!(
                                        entry_id = %result.entry_id,
                                        prev_len = best_len,
                                        new_len,
                                        budget,
                                        slot = %slot,
                                        "binary-slot translation fits after mechanical shrink"
                                    );
                                    result.translation = fitted_text;
                                    best_len = new_len;
                                    fitted = true;
                                    retried_ok += repeated.members[&result.entry_id].len();
                                }
                            }
                        }
                        if !fitted {
                            for id in &repeated.members[&result.entry_id] {
                                oversize_after_retry += 1;
                                let _ = tx.send(ProgressEvent::BatchFailed {
                                    entry_id: Some(id.clone()),
                                    error: "translation exceeds the binary slot; full text preserved for review (no automatic truncation)".into(),
                                }).await;
                            }
                            tracing::warn!(
                                entry_id = %result.entry_id,
                                best_len,
                                budget,
                                slot = %slot,
                                "translation still exceeds binary slot after length-aware retries"
                            );
                        }
                    }

                    if cancel.is_cancelled() {
                        cancelled = true;
                        in_flight.abort_all();
                        break;
                    }
                    let results =
                        repeated.expand(results, &mut placeholders_by_id, &mut budgets_by_id);
                    // Raw source and budgets pin identical restoration/length
                    // fitting. Each occurrence still validates its own controls
                    // and uses its own guarded save/progress event.
                    // Reject broken variables, including damage from length fitting.
                    // Leave those entries pending so they can be retried/reviewed.
                    let mut valid = Vec::with_capacity(results.len());
                    for result in results {
                        group_acc.mark_changed(&result.entry_id);
                        if result.translation.trim().is_empty() {
                            let _ = tx
                                .send(ProgressEvent::BatchFailed {
                                    entry_id: Some(result.entry_id.clone()),
                                    error: "translation is empty; entry remains pending".into(),
                                })
                                .await;
                            if group_acc.is_grouped(&result.entry_id) {
                                group_acc.failed.insert(result.entry_id.clone());
                            }
                            continue;
                        }
                        if let Some(source) = sources_by_id.get(&result.entry_id) {
                            if !PlaceholderProcessor::validate(source, &result.translation)
                                .is_empty()
                            {
                                let _ = tx.send(ProgressEvent::BatchFailed {
                                    entry_id: Some(result.entry_id.clone()),
                                    error: "translation changed protected placeholders; entry remains pending".into(),
                                }).await;
                                if group_acc.is_grouped(&result.entry_id) {
                                    group_acc.failed.insert(result.entry_id.clone());
                                }
                                continue;
                            }
                        }
                        if group_acc.is_grouped(&result.entry_id) {
                            if let Some(orig_req) =
                                requests.iter().find(|r| r.entry_id == result.entry_id)
                            {
                                group_acc
                                    .requests
                                    .insert(result.entry_id.clone(), orig_req.clone());
                            }
                            if let Some(phs) = placeholders_by_id.get(&result.entry_id) {
                                group_acc
                                    .placeholders
                                    .insert(result.entry_id.clone(), phs.clone());
                            }
                            group_acc.results.insert(result.entry_id.clone(), result);
                            continue;
                        }
                        valid.push(result);
                    }
                    let results = valid;
                    // One transaction per batch, before publishing any success.
                    if let Err(error) = self
                        .db
                        .save_translation_results_guarded(&results, &save_guards)
                        .await
                    {
                        budget_error = Some(error);
                        if !in_flight.is_empty() {
                            observed_cost.complete &= self.provider.is_free();
                        }
                        in_flight.abort_all();
                        break;
                    }
                    let mut memory_items = Vec::new();
                    for result in &results {
                        // Don't cache mock translations in memory
                        if opts.use_memory
                            && result.provider != "mock"
                            && budgets_by_id
                                .get(&result.entry_id)
                                .is_none_or(|(slot, budget)| {
                                    crate::validation::encoded_byte_len(slot, &result.translation)
                                        .is_some_and(|n| n <= *budget)
                                })
                        {
                            if let Some(source) = sources_by_id.get(&result.entry_id) {
                                use sha2::{Digest, Sha256};
                                let hash = hex::encode(Sha256::digest(source.as_bytes()));
                                memory_items.push((
                                    hash,
                                    source.clone(),
                                    result.translation.clone(),
                                ));
                            }
                        }
                    }
                    let _ = self.db.save_memory_batch(&memory_items, &lang_pair).await;
                    for result in &results {
                        let _ = tx
                            .send(ProgressEvent::StringTranslated {
                                entry_id: result.entry_id.clone(),
                                translation: result.translation.clone(),
                            })
                            .await;

                        completed += 1;
                    }
                    if let Err(error) = self
                        .finalize_ready_textasset_groups(
                            &mut group_acc,
                            &entries_by_id,
                            &db_groups,
                            &opts,
                            &tx,
                            &cancel,
                            &lang_pair,
                            &sources_by_id,
                            &save_guards,
                            &mut observed_cost,
                            &mut budget_error,
                            &mut oversize_after_retry,
                            &mut retried_ok,
                            &mut completed,
                            &mut total_tokens,
                            &mut total_input_tokens,
                            &mut total_output_tokens,
                        )
                        .await
                    {
                        budget_error = Some(error);
                        if !in_flight.is_empty() {
                            observed_cost.complete &= self.provider.is_free();
                        }
                        in_flight.abort_all();
                        break;
                    }
                }
                Err(e) => {
                    let _ = tx
                        .send(ProgressEvent::BatchFailed {
                            entry_id: None,
                            error: e.to_string(),
                        })
                        .await;
                    observed_cost.complete &= self.provider.is_free();
                    if budget_error.is_none() {
                        budget_error = observed_budget_error(observed_cost, opts.cost_limit_usd);
                    }
                    tracing::error!("Batch translation failed: {}", e);
                    if budget_error.is_some() {
                        break;
                    }
                    continue;
                }
            }

            // 5f. Send BatchCompleted
            let _ = tx
                .send(ProgressEvent::BatchCompleted {
                    completed,
                    total,
                    cost_so_far: cumulative_cost.amount + observed_cost.amount,
                    cost_is_complete: cumulative_cost.complete && observed_cost.complete,
                    language: None,
                })
                .await;
            if budget_error.is_some() {
                break;
            }
        }

        group_acc.abandon_missing();
        if budget_error.is_none() {
            if let Err(error) = self
                .finalize_ready_textasset_groups(
                    &mut group_acc,
                    &entries_by_id,
                    &db_groups,
                    &opts,
                    &tx,
                    &cancel,
                    &lang_pair,
                    &sources_by_id,
                    &save_guards,
                    &mut observed_cost,
                    &mut budget_error,
                    &mut oversize_after_retry,
                    &mut retried_ok,
                    &mut completed,
                    &mut total_tokens,
                    &mut total_input_tokens,
                    &mut total_output_tokens,
                )
                .await
            {
                budget_error = Some(error);
            }
        }

        // ProgressEvent / return type live outside this file; surface counters via log.
        if oversize_after_retry > 0 || retried_ok > 0 {
            if oversize_after_retry > 0 {
                tracing::warn!(
                    oversize_after_retry,
                    retried_ok,
                    "{oversize_after_retry} translations still exceed binary slot after length retries \
                     ({retried_ok} fixed on retry); run locust validate"
                );
            } else {
                tracing::info!(
                    retried_ok,
                    "{retried_ok} binary-slot translations fit after length-aware retry"
                );
            }
        }

        // Cancelled in-flight calls can have an unknown server-side charge.
        if cancelled {
            observed_cost.complete &= self.provider.is_free();
        }
        cumulative_cost.add(observed_cost);

        // 6. Send Completed and record the run in the project ledger
        let duration = start.elapsed().as_secs_f64();
        if completed > 0
            || total_tokens > 0
            || total_input_tokens > 0
            || total_output_tokens > 0
            || observed_cost.amount > 0.0
            || !observed_cost.complete
        {
            let run = crate::database::TranslationRun {
                id: 0,
                started_at,
                duration_secs: duration,
                provider: self.provider.id().to_string(),
                source_lang: opts.source_lang.clone(),
                target_lang: opts.target_lang.clone(),
                strings_translated: completed,
                tokens_used: total_tokens,
                input_tokens: total_input_tokens,
                output_tokens: total_output_tokens,
                cost_usd: observed_cost.amount,
                cost_is_complete: observed_cost.complete,
            };
            if let Err(e) = self.db.record_translation_run(&run).await {
                tracing::warn!("failed to record translation run: {}", e);
            }
        }

        // Preserve observed usage on partial/cancelled runs before emitting a
        // terminal event. Cancellation cannot undo charges already incurred.
        if let Some(error) = budget_error {
            return Err(error);
        }
        if emit_lifecycle {
            let event = if cancelled {
                ProgressEvent::Paused
            } else {
                ProgressEvent::Completed {
                    total_translated: completed,
                    total_cost: cumulative_cost.amount,
                    cost_is_complete: cumulative_cost.complete,
                    duration_secs: duration,
                }
            };
            let _ = tx.send(event).await;
        }

        Ok(())
    }
}

// ─── Provider fallback chain (shared by CLI + HTTP server) ─────────────────

/// Pending (and otherwise translatable) entries, fresh from the DB.
/// Restricting to pending keeps fallbacks from overwriting earlier providers.
pub fn load_pending_entries(db: &Database) -> Result<Vec<StringEntry>> {
    db.get_entries(&crate::database::EntryFilter {
        status: Some(StringStatus::Pending),
        ..Default::default()
    })
}

/// Primary first, then unique fallbacks (duplicates of the primary skipped).
pub fn unique_provider_chain(primary: &str, fallbacks: Option<&[String]>) -> Vec<String> {
    let mut chain = vec![primary.to_string()];
    if let Some(ids) = fallbacks {
        for id in ids {
            if !chain.iter().any(|c| c == id) {
                chain.push(id.clone());
            }
        }
    }
    chain
}

/// Run primary then fallbacks: each provider gets one full pass over remaining
/// pending entries. A provider is abandoned for the next when its pass finishes
/// with work still pending (or if the provider id is missing). Emits a single
/// [`ProgressEvent::Started`] / [`ProgressEvent::Completed`] for the whole job
/// and [`ProgressEvent::ProviderSwitched`] when advancing.
// ponytail: 8 args, two call sites (CLI + server); a params struct would be pure ceremony.
#[allow(clippy::too_many_arguments)]
pub async fn run_fallback_chain(
    chain: &[String],
    resolve: &(dyn Fn(&str) -> Option<Arc<dyn TranslationProvider>> + Send + Sync),
    db: Arc<Database>,
    glossary: Arc<Glossary>,
    opts: TranslationOptions,
    tx: mpsc::Sender<ProgressEvent>,
    job_id: String,
    cancel: CancellationToken,
) -> Result<()> {
    if opts.batch_size == 0 || opts.max_batch_tokens == Some(0) {
        return Err(LocustError::ProviderError(
            "batch size and token budget must be greater than zero".into(),
        ));
    }
    if opts
        .cost_limit_usd
        .is_some_and(|n| !n.is_finite() || n < 0.0)
    {
        return Err(LocustError::ProviderError(
            "cost limit must be finite and non-negative".into(),
        ));
    }
    let start = Instant::now();
    let initial = load_pending_entries(&db)?;
    let initial_total = initial.len();

    let _ = tx
        .send(ProgressEvent::Started {
            total: initial_total,
            job_id: job_id.clone(),
        })
        .await;

    if initial_total == 0 {
        let _ = tx
            .send(ProgressEvent::Completed {
                total_translated: 0,
                total_cost: 0.0,
                cost_is_complete: true,
                duration_secs: start.elapsed().as_secs_f64(),
            })
            .await;
        return Ok(());
    }

    let mut cumulative_completed = 0usize;
    let mut cumulative_cost = ObservedCost::default();

    for (i, id) in chain.iter().enumerate() {
        if cancel.is_cancelled() {
            let _ = tx.send(ProgressEvent::Paused).await;
            return Ok(());
        }

        let pending = load_pending_entries(&db)?;
        if pending.is_empty() {
            break;
        }
        let before = pending.len();

        let Some(provider) = resolve(id) else {
            tracing::warn!(provider = %id, "provider not found in registry, skipping");
            continue;
        };

        if i > 0 {
            let _ = tx
                .send(ProgressEvent::ProviderSwitched {
                    provider_id: id.clone(),
                    provider_name: provider.name().to_string(),
                    remaining_pending: before,
                })
                .await;
        }

        let manager = TranslationManager::new(provider, db.clone(), glossary.clone());
        // Intermediate pass: no lifecycle events (we own Started/Completed).
        let mut pass_opts = opts.clone();
        if let Some(limit) = pass_opts.cost_limit_usd {
            if !cumulative_cost.complete {
                return Err(LocustError::ProviderError(
                    "cannot enforce a cost limit: preceding calls have unknown cost".into(),
                ));
            }
            pass_opts.cost_limit_usd = Some((limit - cumulative_cost.amount).max(0.0));
        }
        if let Err(e) = manager
            .translate_entries_accounted(
                pending,
                pass_opts,
                tx.clone(),
                job_id.clone(),
                cancel.clone(),
                false,
                &mut cumulative_cost,
            )
            .await
        {
            if matches!(
                &e,
                LocustError::CostLimitExceeded { .. } | LocustError::DatabaseError(_)
            ) || matches!(&e, LocustError::ValidationError { message, .. } if message.starts_with("translation_conflict:"))
                || e.to_string().contains("cannot enforce a cost limit")
            {
                return Err(e);
            }
            let _ = tx
                .send(ProgressEvent::BatchFailed {
                    entry_id: None,
                    error: e.to_string(),
                })
                .await;
            tracing::warn!(provider = %id, error = %e, "provider pass failed; trying next in chain");
        }

        let after = load_pending_entries(&db)?.len();
        cumulative_completed += before.saturating_sub(after);

        if after == 0 {
            break;
        }
        if after >= before {
            tracing::warn!(
                provider = %id,
                remaining = after,
                "provider made no progress on pending count"
            );
        }
    }

    let remaining = load_pending_entries(&db)?.len();
    let total_translated = initial_total
        .saturating_sub(remaining)
        .max(cumulative_completed);

    let _ = tx
        .send(ProgressEvent::Completed {
            total_translated,
            total_cost: cumulative_cost.amount,
            cost_is_complete: cumulative_cost.complete,
            duration_secs: start.elapsed().as_secs_f64(),
        })
        .await;

    Ok(())
}

/// Known subtotal plus whether every observed call reported a valid cost.
/// An empty job has known zero cost; missing/invalid provider costs do not.
#[derive(Clone, Copy, Debug)]
struct ObservedCost {
    amount: f64,
    complete: bool,
}

impl Default for ObservedCost {
    fn default() -> Self {
        Self {
            amount: 0.0,
            complete: true,
        }
    }
}

impl ObservedCost {
    fn observe(&mut self, cost: Option<f64>) {
        match cost {
            Some(value) if value.is_finite() && value >= 0.0 => self.amount += value,
            _ => self.complete = false,
        }
    }

    // Providers attach batch-wide usage to one result (normally the first).
    // None on its siblings is not another unpriced request. Sum all reported
    // amounts once; an entirely unpriced paid batch remains unknown.
    fn observe_batch(&mut self, costs: impl IntoIterator<Item = Option<f64>>, free: bool) {
        let mut reported = false;
        for value in costs.into_iter().flatten() {
            reported = true;
            self.observe(Some(value));
        }
        if !reported && !free {
            self.complete = false;
        }
    }

    fn add(&mut self, other: Self) {
        self.amount += other.amount;
        self.complete &= other.complete;
    }
}

fn observed_budget_error(cost: ObservedCost, limit: Option<f64>) -> Option<LocustError> {
    let limit = limit?;
    if cost.amount > limit {
        return Some(LocustError::CostLimitExceeded {
            estimated: cost.amount,
            limit,
        });
    }
    if !cost.complete {
        return Some(LocustError::ProviderError(
            "cannot enforce a cost limit: observed calls have unknown cost".into(),
        ));
    }
    None
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
    use std::time::Duration;
    use tokio::time::Instant;

    #[test]
    fn google_rate_limit_is_finite_and_local_is_unlimited() {
        let google = rate_limiter_for("google");
        assert!(
            google.requests_per_minute() < u32::MAX,
            "google must have a real rpm, got {}",
            google.requests_per_minute()
        );
        assert!(google.requests_per_minute() > 0);
        assert!(!google.is_unlimited());

        for id in ["ollama", "lmstudio", "argos", "mock"] {
            let lim = rate_limiter_for(id);
            assert!(
                lim.is_unlimited(),
                "{id} should be unlimited, got {}",
                lim.requests_per_minute()
            );
        }
    }

    #[test]
    fn extra_requests_per_minute_overrides_table() {
        let mut extra = HashMap::new();
        extra.insert("requests_per_minute".into(), "12".into());
        assert_eq!(requests_per_minute_for("google", Some(&extra)), 12);
        assert_eq!(requests_per_minute_for("ollama", None), u32::MAX);
        extra.insert("requests_per_minute".into(), "0".into());
        assert_eq!(
            requests_per_minute_for("google", Some(&extra)),
            default_requests_per_minute("google")
        );
    }

    #[tokio::test(start_paused = true)]
    async fn local_rate_limiter_never_waits() {
        let limiter = rate_limiter_for("mock");
        let start = Instant::now();
        for _ in 0..40 {
            limiter.acquire().await;
        }
        assert_eq!(
            start.elapsed(),
            Duration::ZERO,
            "local/unlimited limiter must not sleep"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn google_default_rpm_throttles_on_fresh_limiter() {
        let rpm = default_requests_per_minute("google");
        assert!(rpm < u32::MAX);
        let limiter = RateLimiter::new(rpm);
        for _ in 0..rpm {
            limiter.acquire().await;
        }
        let start = Instant::now();
        limiter.acquire().await;
        let waited = start.elapsed();
        assert!(
            waited >= Duration::from_secs(60),
            "google default must wait out the window, waited {waited:?}"
        );
        assert!(
            waited < Duration::from_secs(61),
            "wait must be the window, waited {waited:?}"
        );
    }

    struct MockProvider {
        call_count: AtomicUsize,
        entry_ids: Mutex<Vec<String>>,
    }

    impl MockProvider {
        fn new() -> Self {
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
        async fn translate(
            &self,
            requests: &[TranslationRequest],
        ) -> Result<Vec<TranslationResult>> {
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

    #[tokio::test(start_paused = true)]
    async fn initial_rate_limit_queue_does_not_consume_http_deadline() {
        let provider = Arc::new(MockProvider::new());
        let limiter = Arc::new(RateLimiter::new(1));
        let request = TranslationRequest {
            entry_id: "rate".into(),
            source: "Hello".into(),
            source_lang: "en".into(),
            target_lang: "es".into(),
            context: None,
            glossary_hint: None,
        };
        let cancel = CancellationToken::new();
        for _ in 0..2 {
            call_provider_with_limiter(
                provider.clone(),
                std::slice::from_ref(&request),
                &RetryConfig::default(),
                &cancel,
                limiter.clone(),
            )
            .await
            .unwrap();
        }
        assert_eq!(provider.call_count.load(Ordering::SeqCst), 2);
    }

    fn setup() -> (Arc<Database>, Arc<Glossary>) {
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
        async fn translate(
            &self,
            requests: &[TranslationRequest],
        ) -> Result<Vec<TranslationResult>> {
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
            let (db, events) =
                run_repeated_job(entries, provider.clone(), Default::default()).await;
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
        let mut pivoted =
            StringEntry::new("pivot_slot", "English", PathBuf::from("resources.assets"));
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
            pivot_context.contains("HARD MAX 12 bytes")
                && pivot_context.contains("Source: «English»"),
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

    #[test]
    fn test_binary_slot_length_hint_tight_vs_loose() {
        let tight = binary_slot_length_hint("utf8", 8, "Options");
        assert!(tight.contains("HARD MAX 8 bytes (utf8)"), "{tight}");
        assert!(
            tight.contains("Source: «Options»"),
            "tight hint must quote source: {tight}"
        );
        let loose = binary_slot_length_hint("utf8", 40, "Longer dialogue line here");
        assert!(loose.contains("40 bytes when encoded as utf8"), "{loose}");
        assert!(!loose.contains("HARD MAX"), "{loose}");
        assert!(
            !loose.contains("Source:"),
            "loose budget keeps shorter phrasing: {loose}"
        );
    }

    #[test]
    fn test_binary_slot_retry_correction_quotes_previous() {
        let c = binary_slot_retry_correction("utf8", 7, 8, "Opciones");
        assert!(c.contains("Previous text: «Opciones»"), "{c}");
        assert!(c.contains("Remove at least 1 byte"), "{c}");
        assert!(c.contains("HARD LIMIT 7 BYTES (utf8)"), "{c}");
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
        async fn translate(
            &self,
            requests: &[TranslationRequest],
        ) -> Result<Vec<TranslationResult>> {
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
            "XXXXXXXXXXXXXXXX", // 16 > 5
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
            retry_ctx.contains("Previous text: «XXXXXXXXXXXXXXXX»"),
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
            "XXXXXXXXXXXXXXXX", // 16
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
            crate::validation::encoded_byte_len("utf8", saved.translation.as_ref().unwrap())
                .unwrap();
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
        assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
        let saved = db.get_entry("duplicate-token").unwrap().unwrap();
        assert_eq!(saved.status, StringStatus::Pending);
        assert!(saved.translation.is_none());
    }

    #[tokio::test]
    async fn test_no_binary_slot_provider_called_once() {
        let (db, glossary) = setup();
        let entry = StringEntry::new("plain", "Hello", PathBuf::from("script.txt"));
        db.save_entries(std::slice::from_ref(&entry)).unwrap();

        // First would be "oversize" length for a slot, but no slot → no retry.
        let provider = Arc::new(ScriptedLengthProvider::new(&[
            "XXXXXXXXXXXXXXXX",
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
        assert_eq!(saved.translation.as_deref(), Some("XXXXXXXXXXXXXXXX"));
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
        async fn translate(
            &self,
            requests: &[TranslationRequest],
        ) -> Result<Vec<TranslationResult>> {
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

    #[test]
    fn load_pending_entries_materializes_only_pending_rows() {
        let (db, _) = setup();
        let mut entries: Vec<_> = (0..200)
            .map(|i| {
                let mut entry =
                    StringEntry::new(format!("done-{i}"), "Source", PathBuf::from("f.json"));
                entry.status = StringStatus::Translated;
                entry.translation = Some("Translated".into());
                entry
            })
            .collect();
        entries.push(StringEntry::new(
            "pending",
            "Pending",
            PathBuf::from("f.json"),
        ));
        db.save_entries(&entries).unwrap();

        crate::database::ENTRY_ROWS_MATERIALIZED.with(|count| count.set(Some(0)));
        let pending = load_pending_entries(&db).unwrap();
        let materialized =
            crate::database::ENTRY_ROWS_MATERIALIZED.with(|count| count.replace(None));

        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].id, "pending");
        assert_eq!(materialized, Some(1));
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
        async fn translate(
            &self,
            _requests: &[TranslationRequest],
        ) -> Result<Vec<TranslationResult>> {
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
        async fn translate(
            &self,
            requests: &[TranslationRequest],
        ) -> Result<Vec<TranslationResult>> {
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
        async fn translate(
            &self,
            _requests: &[TranslationRequest],
        ) -> Result<Vec<TranslationResult>> {
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
        let manager = TranslationManager::new(provider, db.clone(), glossary).with_retry_config(
            RetryConfig {
                max_attempts: 3,
                initial_delay_ms: 1,
                max_delay_ms: 5,
                ..Default::default()
            },
        );
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
        let manager = TranslationManager::new(provider, db.clone(), glossary).with_retry_config(
            RetryConfig {
                max_attempts: 3,
                initial_delay_ms: 1,
                max_delay_ms: 5,
                ..Default::default()
            },
        );
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

    #[test]
    fn batch_budget_preserves_order_and_isolates_long_entries() {
        let entries: Vec<_> = [
            "short",
            "a very long entry that must fit alone",
            "tail",
            "end",
        ]
        .iter()
        .enumerate()
        .map(|(i, text)| StringEntry::new(i.to_string(), *text, PathBuf::from("game")))
        .collect();
        let opts = TranslationOptions {
            batch_size: 3,
            max_batch_tokens: Some(270),
            ..Default::default()
        };
        let batches = translation_batches(&entries, &opts);
        assert_eq!(
            batches.iter().map(|b| b.len()).collect::<Vec<_>>(),
            vec![1, 1, 2]
        );
        assert_eq!(
            batches
                .into_iter()
                .flatten()
                .map(|e| &e.id)
                .collect::<Vec<_>>(),
            entries.iter().map(|e| &e.id).collect::<Vec<_>>()
        );
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
        async fn translate(
            &self,
            requests: &[TranslationRequest],
        ) -> Result<Vec<TranslationResult>> {
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
                let mut entry =
                    StringEntry::new(*id, *value, PathBuf::from("sharedassets0.assets"));
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
        let entries =
            grouped_loc_entries(original, &[("a", "Menu.A", "Hi"), ("b", "Menu.B", "Hello")]);
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
        let entries =
            grouped_loc_entries(original, &[("a", "Menu.A", "Hi"), ("b", "Menu.B", "Go")]);
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
        let entries =
            grouped_loc_entries(original, &[("a", "Menu.A", "Hi"), ("b", "Menu.B", "Go")]);
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
        let entries =
            grouped_loc_entries(original, &[("a", "Menu.A", "Hi"), ("b", "Menu.B", "Hello")]);
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
        let entries =
            grouped_loc_entries(original, &[("c", "Menu.A", "Hi"), ("d", "Menu.B", "Hello")]);
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
        let entries =
            grouped_loc_entries(original, &[("a", "Menu.A", "Hi"), ("b", "Menu.B", "Go")]);
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
        let entries =
            grouped_loc_entries(original, &[("a", "Menu.A", "Hi"), ("b", "Menu.B", "Go")]);
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
        let mut second =
            grouped_loc_entries(original, &[("c", "Menu.A", "Hi"), ("d", "Menu.B", "Go")]);
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

    #[test]
    fn grouped_textasset_retry_budget_counts_context_and_glossary() {
        let request = TranslationRequest {
            entry_id: "a".into(),
            source: "Hello".into(),
            source_lang: "en".into(),
            target_lang: "es".into(),
            context: Some("x".repeat(180)),
            glossary_hint: Some("y".repeat(120)),
        };
        let requests = vec![request.clone(), request];
        let opts = TranslationOptions {
            batch_size: 10,
            max_batch_tokens: Some(300),
            ..Default::default()
        };
        let batches = group_retry_batches(&requests, &opts).unwrap();
        assert_eq!(
            batches.iter().map(|b| b.len()).collect::<Vec<_>>(),
            vec![1, 1]
        );
        let opts = TranslationOptions {
            max_batch_tokens: Some(200),
            ..opts
        };
        assert!(group_retry_batches(&requests, &opts).is_err());
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
        let entries =
            grouped_loc_entries(original, &[("a", "Menu.A", "Hi"), ("b", "Menu.B", "Go")]);
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
}

#[cfg(test)]
mod observed_cost_tests {
    use super::ObservedCost;
    #[test]
    fn batch_usage_on_first_result_is_complete_without_double_counting() {
        let mut cost = ObservedCost::default();
        cost.observe_batch([Some(0.25), None, None], false);
        assert_eq!(cost.amount, 0.25);
        assert!(cost.complete);
        cost.observe_batch([None, None], false);
        assert_eq!(cost.amount, 0.25);
        assert!(!cost.complete);
    }
    #[test]
    fn free_unpriced_batch_is_known_zero_but_invalid_price_is_not() {
        let mut cost = ObservedCost::default();
        cost.observe_batch([None, None], true);
        assert_eq!(cost.amount, 0.0);
        assert!(cost.complete);
        cost.observe_batch([Some(0.25), Some(f64::NAN), None], false);
        assert_eq!(cost.amount, 0.25);
        assert!(!cost.complete);
    }
    #[test]
    fn unknown_retry_cannot_become_known_when_another_attempt_reports_cost() {
        let mut cost = ObservedCost::default();
        cost.observe(None);
        cost.observe(Some(0.25));
        assert_eq!(cost.amount, 0.25);
        assert!(!cost.complete);
    }
    #[test]
    fn chain_accumulates_subtotals_and_incompleteness() {
        let mut cost = ObservedCost::default();
        cost.add(ObservedCost {
            amount: 0.1,
            complete: true,
        });
        cost.add(ObservedCost {
            amount: 0.2,
            complete: false,
        });
        assert!((cost.amount - 0.3).abs() < 1e-9);
        assert!(!cost.complete);
    }
    #[test]
    fn invalid_provider_prices_never_poison_serialized_totals() {
        for bad in [f64::NAN, f64::INFINITY, -1.0] {
            let mut cost = ObservedCost::default();
            cost.observe(Some(bad));
            assert_eq!(cost.amount, 0.0);
            assert!(!cost.complete);
        }
    }
}
