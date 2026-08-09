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
    if budget <= TIGHT_BINARY_SLOT_BYTES {
        format!(
            "LENGTH LIMIT: HARD MAX {budget} bytes ({slot}). Source: «{src_display}». \
             Prefer one short word or heavy abbreviation; spaces and accents count as bytes."
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

        // 3b. Exact glossary hits (full-string): short-circuit provider — key for
        // short UI binary slots where the user already fixed a fitting form.
        if opts.use_glossary && !remaining.is_empty() {
            let mut still = Vec::with_capacity(remaining.len());
            for entry in remaining.drain(..) {
                let Some(term) = self.glossary.lookup_exact(
                    &entry.source,
                    &opts.source_lang,
                    &opts.target_lang,
                ) else {
                    still.push(entry);
                    continue;
                };
                // Binary-slot: only apply when the glossary form fits the budget.
                if let Some(slot) = entry
                    .metadata
                    .get("binary_slot")
                    .and_then(|v| v.as_str())
                {
                    if let Some(budget) =
                        crate::validation::encoded_byte_len(slot, &entry.source)
                    {
                        if crate::validation::encoded_byte_len(slot, &term)
                            .map(|n| n > budget)
                            .unwrap_or(true)
                        {
                            still.push(entry);
                            continue;
                        }
                    }
                }
                self.db
                    .save_translation(&entry.id, &term, "glossary")
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
                                let hint =
                                    binary_slot_length_hint(slot, budget, &entry.source);
                                context = Some(match context {
                                    Some(c) => format!("{c} | {hint}"),
                                    None => hint,
                                });
                            }
                        }
                        let (sanitized, phs) = PlaceholderProcessor::extract(&entry.source);
                        placeholders_by_id.insert(entry.id.clone(), phs);
                        // Per-entry glossary: only terms present in this source
                        // (keeps short UI slots free of bulk noise).
                        let glossary_hint = if opts.use_glossary {
                            self.glossary.build_hint_for_text(
                                &opts.source_lang,
                                &opts.target_lang,
                                &entry.source,
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
                            let correction = binary_slot_retry_correction(
                                slot,
                                *budget,
                                prev_len,
                                &prev_text,
                            );
                            let mut retry_req = orig_req.clone();
                            retry_req.context = Some(match &orig_req.context {
                                Some(c) => format!("{c} | {correction}"),
                                None => correction,
                            });

                            match self
                                .provider
                                .translate(std::slice::from_ref(&retry_req))
                                .await
                            {
                                Ok(mut retry_batch) => {
                                    let mut retry_result = match retry_batch
                                        .iter()
                                        .position(|r| r.entry_id == result.entry_id)
                                    {
                                        Some(i) => retry_batch.swap_remove(i),
                                        None => match retry_batch.pop() {
                                            Some(r) => r,
                                            None => {
                                                tracing::warn!(
                                                    entry_id = %result.entry_id,
                                                    attempt,
                                                    "length retry returned no result; keeping best attempt"
                                                );
                                                break;
                                            }
                                        },
                                    };
                                    restore_placeholders_in_result(
                                        &mut retry_result,
                                        &placeholders_by_id,
                                    );
                                    let new_len = crate::validation::encoded_byte_len(
                                        slot,
                                        &retry_result.translation,
                                    )
                                    .unwrap_or(usize::MAX);

                                    if let Some(c) = retry_result.cost_usd {
                                        result.cost_usd =
                                            Some(result.cost_usd.unwrap_or(0.0) + c);
                                    }
                                    if let Some(t) = retry_result.tokens_used {
                                        result.tokens_used =
                                            Some(result.tokens_used.unwrap_or(0) + t);
                                    }

                                    if new_len <= *budget {
                                        result.translation = retry_result.translation;
                                        best_len = new_len;
                                        fitted = true;
                                        retried_ok += 1;
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

                        if !fitted {
                            oversize_after_retry += 1;
                            tracing::warn!(
                                entry_id = %result.entry_id,
                                best_len,
                                budget,
                                slot = %slot,
                                "translation still exceeds binary slot after length-aware retries"
                            );
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
            if emit_lifecycle {
                let _ = tx.send(ProgressEvent::Paused).await;
            }
            return Ok(());
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

        // 6. Send Completed and record the run in the project ledger
        let duration = start.elapsed().as_secs_f64();
        if emit_lifecycle {
            let _ = tx
                .send(ProgressEvent::Completed {
                    total_translated: completed,
                    total_cost,
                    duration_secs: duration,
                })
                .await;
        }

        if completed > 0 {
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
                cost_usd: total_cost,
            };
            if let Err(e) = self.db.record_translation_run(&run).await {
                tracing::warn!("failed to record translation run: {}", e);
            }
        }

        Ok(())
    }
}

// ─── Provider fallback chain (shared by CLI + HTTP server) ─────────────────

/// Pending (and otherwise translatable) entries, fresh from the DB.
/// Restricting to pending keeps fallbacks from overwriting earlier providers.
pub fn load_pending_entries(db: &Database) -> Result<Vec<StringEntry>> {
    Ok(db
        .get_entries(&crate::database::EntryFilter::default())?
        .into_iter()
        .filter(|e| e.status == StringStatus::Pending)
        .collect())
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
                duration_secs: start.elapsed().as_secs_f64(),
            })
            .await;
        return Ok(());
    }

    let mut cumulative_completed = 0usize;

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
        if let Err(e) = manager
            .translate_entries_inner(
                pending,
                opts.clone(),
                tx.clone(),
                job_id.clone(),
                cancel.clone(),
                false,
            )
            .await
        {
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
    let total_translated = initial_total.saturating_sub(remaining).max(cumulative_completed);

    let _ = tx
        .send(ProgressEvent::Completed {
            total_translated,
            total_cost: 0.0, // per-pass cost is on BatchCompleted; chain total not aggregated
            duration_secs: start.elapsed().as_secs_f64(),
        })
        .await;

    Ok(())
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

    #[tokio::test]
    async fn test_glossary_exact_short_circuits_provider() {
        let (db, glossary) = setup();
        glossary
            .add("Options", "Opcns", "en-es", None)
            .unwrap();
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
        glossary
            .add("Options", "Opciones", "en-es", None)
            .unwrap();
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
            !slotted.contains("encoded as utf8"),
            "tight path should not use the longer-line phrasing: {slotted}"
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
        assert!(
            loose.contains("40 bytes when encoded as utf8"),
            "{loose}"
        );
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
    async fn test_binary_slot_retry_still_oversize_keeps_shorter() {
        let (db, glossary) = setup();
        let entry = binary_utf8_entry("slot-still-over", "Hi"); // budget 2
        db.save_entries(std::slice::from_ref(&entry)).unwrap();

        let provider = Arc::new(ScriptedLengthProvider::new(&[
            "XXXXXXXXXXXXXXXX", // 16
            "YYYYYYYY",         // 8 shorter but still over
            "ZZZZ",             // 4 still over budget 2; shortest kept
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
            Some("ZZZZ"),
            "must store the shortest of the oversize attempts"
        );
        let budget = crate::validation::encoded_byte_len("utf8", "Hi").unwrap();
        let actual =
            crate::validation::encoded_byte_len("utf8", saved.translation.as_ref().unwrap())
                .unwrap();
        assert!(actual > budget);
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
                        translation: format!("[{}] {}", self.id, r.source),
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

        let mut map: std::collections::HashMap<String, Arc<dyn TranslationProvider>> = std::collections::HashMap::new();
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
                ProgressEvent::Completed { total_translated, .. } => {
                    assert_eq!(total_translated, 3);
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
        let mut map: std::collections::HashMap<String, Arc<dyn TranslationProvider>> = std::collections::HashMap::new();
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
}
