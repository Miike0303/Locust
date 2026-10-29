use std::collections::{HashMap, HashSet};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::database::{Database, TranslationSaveGuard};
use crate::error::{LocustError, Result};
use crate::glossary::{Glossary, PreparedGlossary, VisibleGlossary};
use crate::models::{
    ProgressEvent, StringEntry, StringStatus, TranslationRequest, TranslationResult,
};
use crate::placeholder::{
    PlaceholderKind, PlaceholderProcessor, PlaceholderProfile, SpeakerArgumentTrace,
};

use crate::speakers::{resolve_speaker, SpeakerNotes, NOTES_METADATA_KEY};

use super::batching::{group_retry_batches, translation_batches};
use super::protection::{
    binary_slot_length_hint, binary_slot_retry_correction, collapse_runaway_letters,
    has_mv_mz_provenance, protected_control_inventory, protected_control_repair_request,
    restore_and_collapse_result, restore_placeholders_in_result, translation_fits_entry,
    MtControlRepair, RecordProtection, MAX_BINARY_SLOT_LENGTH_RETRIES,
    MAX_PROTECTED_CONTROL_REPAIRS,
};
use super::provider::{TranslationOptions, TranslationProvider};
use super::retry::{call_provider, ProviderCall, RetryConfig};

/// One immutable snapshot per job, also shared by all fallback passes.
struct CharacterNotes(SpeakerNotes);

/// Immutable glossary policy shared by every provider pass in the job. Keeping
/// preparation lazy preserves validation-before-glossary work for malformed rows.
struct JobGlossary {
    prepared: PreparedGlossary,
    visible: VisibleGlossary,
    is_empty: bool,
}

impl JobGlossary {
    fn load(glossary: &Glossary, opts: &TranslationOptions, has_entries: bool) -> Self {
        let pair = format!("{}-{}", opts.source_lang, opts.target_lang);
        let entries = if opts.use_glossary && has_entries {
            glossary.get_all(&pair).unwrap_or_default()
        } else {
            Vec::new()
        };
        Self {
            prepared: PreparedGlossary::compile(&entries),
            visible: VisibleGlossary::compile(&entries, &pair),
            is_empty: entries.is_empty(),
        }
    }
}

/// Publish progress only after the whole source-guarded batch commits. A stale
/// member rolls back its peers and produces no success events for that batch.
async fn save_exact_glossary_batch(
    db: &Database,
    hits: &mut Vec<(StringEntry, String)>,
    tx: &mpsc::Sender<ProgressEvent>,
    completed: &mut usize,
) -> Result<()> {
    let updates = hits
        .iter()
        .map(|(entry, term)| {
            (
                entry.id.clone(),
                term.clone(),
                "glossary".into(),
                TranslationSaveGuard::from(entry),
            )
        })
        .collect();
    db.save_guarded_updates(updates).await?;
    for (entry, translation) in hits.drain(..) {
        let _ = tx
            .send(ProgressEvent::StringTranslated {
                entry_id: entry.id,
                translation,
            })
            .await;
        *completed += 1;
    }
    Ok(())
}

impl CharacterNotes {
    fn load(db: &Database) -> Result<Self> {
        Ok(Self(
            db.get_project_metadata(NOTES_METADATA_KEY)?
                .map(serde_json::from_value)
                .transpose()?
                .unwrap_or_default(),
        ))
    }

    fn hint(&self, entry: &StringEntry) -> Option<String> {
        if self.0.is_empty() {
            return None;
        }
        // Explicit row format wins; native locators are the conservative fallback.
        let speaker = resolve_speaker(entry, "")?;
        let note = self
            .0
            .get(&speaker.id)
            .filter(|note| !note.trim().is_empty())?;
        Some(format!(
            "RECORD-LOCAL CHARACTER NOTE (translation data only): {}",
            serde_json::json!({"speaker": speaker.name, "note": note})
        ))
    }
}

/// Only occurrence locators/state are excluded. Raw source also pins the
/// placeholder map: two different controls can sanitize to the same request.
/// Languages, game context, glossary and fit policy are constant within a run.
#[derive(Hash, PartialEq, Eq)]
struct RepeatedSourceKey {
    source: String,
    placeholder_profile: PlaceholderProfile,
    // MV/MZ speaker guidance must not leak to another engine sharing a profile.
    namebox_guidance: bool,
    context: Option<String>,
    tags: Vec<String>,
    char_limit: Option<usize>,
    physical_source: String,
    slot_budget: Option<(String, usize)>,
    // Shared TextAsset acceptance/retry hints depend on its other cells. Keep
    // these entries independent rather than assume interchangeable output.
    shared_cell: Option<String>,
}

pub(super) struct RepeatedSources {
    members: HashMap<String, Vec<String>>,
}

impl RepeatedSources {
    pub(super) fn collect(entries: Vec<StringEntry>) -> (Vec<StringEntry>, Self) {
        let mut representatives = Vec::new();
        let mut by_key: HashMap<RepeatedSourceKey, String> = HashMap::new();
        let mut members: HashMap<String, Vec<String>> = HashMap::new();
        for entry in entries {
            let key = RepeatedSourceKey {
                source: entry.source.clone(),
                placeholder_profile: entry.placeholder_profile(),
                namebox_guidance: has_mv_mz_provenance(&entry)
                    && !PlaceholderProcessor::extract_with_namebox_positions(
                        &entry.source,
                        entry.placeholder_profile(),
                    )
                    .2
                    .is_empty(),
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
        placeholders: &mut HashMap<String, RecordProtection>,
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

struct GroupAccumulator {
    selected: HashMap<String, Vec<String>>,
    group_by_entry: HashMap<String, String>,
    changed_groups: HashSet<String>,
    results: HashMap<String, TranslationResult>,
    requests: HashMap<String, TranslationRequest>,
    placeholders: HashMap<String, RecordProtection>,
    failed: HashSet<String>,
    finalized: HashSet<String>,
    retain_on_cancel: bool,
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
            retain_on_cancel: false,
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

pub struct TranslationManager {
    provider: Arc<dyn TranslationProvider>,
    db: Arc<Database>,
    glossary: Arc<Glossary>,
    pub(super) retry: RetryConfig,
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
        // Antigravity bounds each subprocess itself (300s by default) and may
        // split malformed batches. Keep cancellation, without a shorter outer cap.
        if provider.id() == "antigravity" {
            retry.overall_deadline = None;
        }
        Self {
            provider,
            db,
            glossary,
            retry,
        }
    }

    /// Validate complete logical requests with the same estimator/splitter used
    /// for grouped repairs. Legacy oversized singles remain legal without opt-in.
    fn context_request_error(
        &self,
        requests: &[TranslationRequest],
        opts: &TranslationOptions,
    ) -> Result<()> {
        if self
            .provider
            .model_policy()
            .and_then(|p| p.context_window)
            .is_some()
        {
            group_retry_batches(requests, opts).map_err(|message| {
                LocustError::ProviderError(format!("model context input allowance: {message}"))
            })?;
        }
        Ok(())
    }

    /// Additional logical calls share the run's prepaid budget, including all
    /// source/context/glossary bytes and costs from previously rejected answers.
    async fn additional_request_budget_error(
        &self,
        request: &TranslationRequest,
        opts: &TranslationOptions,
        observed_cost: ObservedCost,
    ) -> Option<LocustError> {
        if let Err(error) = self.context_request_error(std::slice::from_ref(request), opts) {
            return Some(error);
        }
        let limit = opts.cost_limit_usd?;
        if !observed_cost.complete {
            return Some(LocustError::ProviderError(
                "cannot enforce a cost limit: preceding calls have unknown cost".into(),
            ));
        }
        let chars = request
            .source
            .len()
            .saturating_add(request.context.as_ref().map_or(0, String::len))
            .saturating_add(request.glossary_hint.as_ref().map_or(0, String::len));
        match self.provider.estimate_cost(chars, &opts.target_lang).await {
            Some(estimate) if estimate.is_finite() && estimate >= 0.0 => (observed_cost.amount
                + estimate
                > limit)
                .then_some(LocustError::CostLimitExceeded {
                    estimated: observed_cost.amount + estimate,
                    limit,
                }),
            None if self.provider.is_free() => None,
            _ => Some(LocustError::ProviderError(
                "cannot enforce a cost limit: this provider has no valid cost estimate".into(),
            )),
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
            if cancel.is_cancelled() && !acc.retain_on_cancel {
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
            let mut partial_repair_failed = false;
            'rounds: loop {
                if cancel.is_cancelled() && !acc.retain_on_cancel {
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
                        if cancel.is_cancelled() && !acc.retain_on_cancel {
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
                    Ok(false)
                        if attempts < MAX_BINARY_SLOT_LENGTH_RETRIES
                            && self.provider.supports_instructions()
                            && !partial_repair_failed
                            && !cancel.is_cancelled() =>
                    {
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
                        if let Err(error) = self.context_request_error(&retry_reqs, opts) {
                            refuse_group(
                                tx,
                                &selected_ids,
                                oversize_after_retry,
                                &error.to_string(),
                            )
                            .await;
                            // Stop this pass through its receipt-drain path, not
                            // the caller's abort path for finalization failures.
                            *budget_error = Some(error);
                            return Ok(());
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
                                if acc.retain_on_cancel {
                                    continue 'rounds;
                                }
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
                                        rejected_results,
                                        failed_ids,
                                        partial,
                                        ..
                                    } = retry_call;
                                    acc.retain_on_cancel |= partial;
                                    observed_cost.observe_batch(
                                        results.iter().chain(&rejected_results).map(|r| r.cost_usd),
                                        self.provider.is_free(),
                                    );
                                    if (transport_attempts > 1 || !failed_ids.is_empty())
                                        && !self.provider.is_free()
                                    {
                                        observed_cost.complete = false;
                                    }
                                    if budget_error.is_none() {
                                        *budget_error = observed_budget_error(
                                            *observed_cost,
                                            opts.cost_limit_usd,
                                        );
                                    }
                                    for usage in results.iter().chain(&rejected_results) {
                                        *total_tokens += usage.tokens_used.unwrap_or(0) as u64;
                                        *total_input_tokens +=
                                            usage.input_tokens.unwrap_or(0) as u64;
                                        *total_output_tokens +=
                                            usage.output_tokens.unwrap_or(0) as u64;
                                    }
                                    let expected: HashSet<_> =
                                        retry_reqs.iter().map(|r| r.entry_id.as_str()).collect();
                                    let actual_ids: HashSet<_> = results
                                        .iter()
                                        .map(|r| r.entry_id.as_str())
                                        .chain(failed_ids.iter().map(String::as_str))
                                        .collect();
                                    if results.len() + failed_ids.len() != retry_reqs.len()
                                        || actual_ids.len() != results.len() + failed_ids.len()
                                        || actual_ids != expected
                                    {
                                        continue 'rounds;
                                    }
                                    // Retain successful shortening receipts. A terminal
                                    // partial failure must not start another whole-group
                                    // dispatch solely because its vector was incomplete.
                                    partial_repair_failed |= !failed_ids.is_empty();
                                    for result in &mut results {
                                        if !restore_placeholders_in_result(
                                            result,
                                            &acc.placeholders,
                                        ) {
                                            continue;
                                        }
                                        if let Some(entry) = entries_by_id.get(&result.entry_id) {
                                            result.translation = collapse_runaway_letters(
                                                &entry.source,
                                                &result.translation,
                                            );
                                            if result.translation.trim().is_empty()
                                                || !PlaceholderProcessor::validate_with_profile(
                                                    &entry.source,
                                                    &result.translation,
                                                    entry.placeholder_profile(),
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
        mut entries: Vec<StringEntry>,
        mut opts: TranslationOptions,
        tx: mpsc::Sender<ProgressEvent>,
        job_id: String,
        cancel: CancellationToken,
        emit_lifecycle: bool,
    ) -> Result<()> {
        // The caller's row snapshot is also a scope. Never let explicit options
        // expand it; all shortcut, dedup, repair and group work starts here.
        opts.select_entries(&mut entries);
        opts.entry_ids = Some(entries.iter().map(|entry| entry.id.clone()).collect());
        opts.file_paths = None;
        let notes = CharacterNotes::load(&self.db)?;
        let glossary = OnceLock::new();
        self.translate_entries_accounted(
            entries,
            opts,
            tx,
            job_id,
            cancel,
            emit_lifecycle,
            &mut ObservedCost::default(),
            &notes,
            &glossary,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn translate_entries_accounted(
        &self,
        entries: Vec<StringEntry>,
        mut opts: TranslationOptions,
        tx: mpsc::Sender<ProgressEvent>,
        job_id: String,
        cancel: CancellationToken,
        emit_lifecycle: bool,
        cumulative_cost: &mut ObservedCost,
        notes: &CharacterNotes,
        job_glossary: &OnceLock<JobGlossary>,
    ) -> Result<()> {
        let start = Instant::now();

        if cancel.is_cancelled() {
            if emit_lifecycle {
                let _ = tx.send(ProgressEvent::Paused).await;
            }
            return Ok(());
        }

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

        // Resolve separately for each provider pass; explicit null disables only
        // the legacy cap. Never mutate the options reused by the fallback chain.
        let model_allowance = self
            .provider
            .model_policy()
            .and_then(|policy| policy.approximate_input_allowance());
        if let Some(allowance) = model_allowance {
            if allowance == 0 {
                return Err(LocustError::ProviderError(
                    "model context input allowance has no room for a request".into(),
                ));
            }
            opts.max_batch_tokens = Some(opts.max_batch_tokens.unwrap_or(allowance).min(allowance));
        }

        // 1. Filter translatable entries
        let translatable: Vec<StringEntry> = entries
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
        let character_hints: HashMap<_, _> = translatable
            .iter()
            .filter_map(|entry| notes.hint(entry).map(|hint| (entry.id.clone(), hint)))
            .collect();
        // Ollama's legacy prompt uses the first record's context. Actual note
        // membership, never user-controlled context text, selects single-record
        // dispatch; retries share these options. No-note runs keep their batches.
        if self.provider.id() == "ollama" && !character_hints.is_empty() {
            opts.batch_size = 1;
        }
        // Source-only TM cannot represent character-dependent output. Excluding
        // noted rows here also prevents both ordinary and shared-cell TM writes.
        let sources_by_id: HashMap<_, _> = translatable
            .iter()
            .filter(|entry| !character_hints.contains_key(&entry.id))
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

        // Compile only explicit entries for this pair, once per job. Capacity
        // eligibility comes from the original row, never a cleared hint budget.
        let glossary = job_glossary
            .get_or_init(|| JobGlossary::load(&self.glossary, &opts, !translatable.is_empty()));
        let matcher = &glossary.visible;
        let mut glossary_plans = HashMap::new();
        let mut eligible = Vec::with_capacity(translatable.len());
        'entries: for entry in translatable {
            if !glossary.is_empty
                && !crate::textasset_group::is_grouped_entry(&entry)
                && crate::validation::binary_slot_budget(&entry)?.is_none()
            {
                match matcher.mask(&entry.source, entry.placeholder_profile()) {
                    Ok((source, phs)) => {
                        if phs
                            .iter()
                            .any(|ph| matches!(ph.pattern_type, PlaceholderKind::GlossaryTerm))
                        {
                            let mut nameboxes =
                                PlaceholderProcessor::extract_with_namebox_positions(
                                    &entry.source,
                                    entry.placeholder_profile(),
                                )
                                .2;
                            for (offset, token) in &mut nameboxes {
                                let Some(position) = source.find(token.as_str()) else {
                                    tracing::warn!(
                                        entry_id = %entry.id,
                                        "Control token missing after glossary masking; entry remains pending"
                                    );
                                    let _ = tx.send(ProgressEvent::BatchFailed {
                                        entry_id: Some(entry.id.clone()),
                                        error: "glossary masking lost a control token; entry remains pending".into(),
                                    }).await;
                                    continue 'entries;
                                };
                                *offset = position;
                            }
                            let recognized = if has_mv_mz_provenance(&entry) {
                                nameboxes.as_slice()
                            } else {
                                &[]
                            };
                            let speakers = SpeakerArgumentTrace::from_sanitized(
                                &source,
                                &phs,
                                recognized,
                                has_mv_mz_provenance(&entry),
                            );
                            if !PlaceholderProcessor::canonical_source_is_safe(
                                &source, &phs, &speakers,
                            ) {
                                let _ = tx.send(ProgressEvent::BatchFailed {
                                    entry_id: Some(entry.id.clone()),
                                    error: "canonical glossary insertion changes source speaker syntax or reserved identities; entry remains pending".into(),
                                }).await;
                                continue;
                            }
                            glossary_plans.insert(entry.id.clone(), (source, phs, speakers));
                        }
                    }
                    Err(()) => {
                        let _ = tx.send(ProgressEvent::BatchFailed {
                            entry_id: Some(entry.id.clone()),
                            error: "conflicting glossary targets or unsafe canonical controls; entry remains pending".into(),
                        }).await;
                        continue;
                    }
                }
            }
            eligible.push(entry);
        }
        let mut translatable = eligible;

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
                if character_hints.contains_key(&entry.id) || glossary_plans.contains_key(&entry.id)
                {
                    // Source-only memory predates/does not encode canonical policy.
                    remaining.push(entry);
                    continue;
                }
                if let Some(cached) = memory.get(&hash) {
                    // Memory written before the cap existed may hold stretched text.
                    let cached = &collapse_runaway_letters(&entry.source, cached);
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

        // 3b. Exact glossary hits (full-string): short-circuit provider — key for
        // short UI binary slots where the user already fixed a fitting form.
        if opts.use_glossary && !remaining.is_empty() {
            let mut still = Vec::with_capacity(remaining.len());
            let mut hits = Vec::new();
            for entry in remaining.drain(..) {
                if glossary_plans.get(&entry.id).is_some_and(|(_, phs, _)| {
                    phs.iter()
                        .any(|ph| !matches!(ph.pattern_type, PlaceholderKind::GlossaryTerm))
                }) {
                    // A whole-record exact shortcut must not bypass the visible
                    // term identities and speaker anchors around its controls.
                    still.push(entry);
                    continue;
                }
                let Some(term) = glossary.prepared.lookup_exact(&entry.source) else {
                    still.push(entry);
                    continue;
                };
                // Exact glossary hits obey the same integrity rules as memory.
                if !translation_fits_entry(&entry, &term) {
                    still.push(entry);
                    continue;
                }
                hits.push((entry, term));
                if hits.len() == opts.batch_size {
                    save_exact_glossary_batch(&self.db, &mut hits, &tx, &mut completed).await?;
                }
            }
            save_exact_glossary_batch(&self.db, &mut hits, &tx, &mut completed).await?;
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
            std::collections::HashMap<String, RecordProtection>,
            std::collections::HashMap<String, SlotBudget>,
            Result<ProviderCall>,
        );
        let mut in_flight: tokio::task::JoinSet<BatchOutcome> = tokio::task::JoinSet::new();
        // Group across the entire pending run, before batching/concurrency.
        // Memory/glossary hits have already committed through their guarded batches.
        // Only request-working copies change: save guards, cache inputs and
        // entries_by_id above retain the original row context. This feeds notes
        // through the existing dedup key AND token-budget batching estimate.
        for entry in &mut remaining {
            if let Some(hint) = character_hints.get(&entry.id) {
                entry.context = Some(match &entry.context {
                    Some(context) => format!("{context} | {hint}"),
                    None => hint.clone(),
                });
            }
        }
        let (representatives, repeated) = RepeatedSources::collect(remaining);
        let chunks = translation_batches(&representatives, &opts);
        let mut chunk_iter: std::collections::VecDeque<_> = chunks.into();
        let mut cancelled = false;
        let mut budget_error = None;
        let mut context_error = None;

        loop {
            // 5a. Fill the in-flight window
            while in_flight.len() < concurrency && !cancelled && context_error.is_none() {
                if cancel.is_cancelled() {
                    cancelled = true;
                    break;
                }

                let Some(mut chunk) = chunk_iter.pop_front() else {
                    break;
                };

                // 5c. Build TranslationRequests — sanitize placeholders so the translator
                // doesn't translate variable names like [player_name] or Ren'Py tags {i}{/i}
                let mut placeholders_by_id: std::collections::HashMap<String, RecordProtection> =
                    std::collections::HashMap::new();
                let mut budgets_by_id: std::collections::HashMap<String, SlotBudget> =
                    std::collections::HashMap::new();
                let mut requests: Vec<TranslationRequest> = chunk
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
                        let (mut sanitized, mut phs, mut nameboxes) =
                            PlaceholderProcessor::extract_with_namebox_positions(
                                &entry.source,
                                entry.placeholder_profile(),
                            );
                        let speakers = if let Some((source, composite, speakers)) = glossary_plans.get(&entry.id) {
                            sanitized.clone_from(source);
                            phs.clone_from(composite);
                            // Stable control identity remaps UTF-8 positions after
                            // every glossary span edit, including multiple boxes.
                            for (offset, token) in &mut nameboxes {
                                *offset = sanitized.find(token.as_str()).expect("control token preserved");
                            }
                            speakers.clone()
                        } else {
                            SpeakerArgumentTrace::default()
                        };
                        if has_mv_mz_provenance(entry) && !nameboxes.is_empty() {
                            let positions = nameboxes
                                .iter()
                                .map(|(offset, token)| format!("byte {offset} ({token}<...>)"))
                                .collect::<Vec<_>>()
                                .join(", ");
                            let hint = format!(
                                "RECORD-LOCAL RPG Maker MV/MZ speaker arguments at sanitized UTF-8 {positions}: only the angle-bracket arguments immediately following these placeholders are speaker text, not HTML. Translate generic roles, occupations and descriptive titles into the target language; preserve established proper names and follow this record's glossary. Preserve every placeholder token and control code exactly, and keep the angle brackets. This note does not apply to other angle-bracket text."
                            );
                            context = Some(match context {
                                Some(c) => format!("{c} | {hint}"),
                                None => hint,
                            });
                        }
                        if let Some(hint) = speakers.hint() {
                            context = Some(match context {
                                Some(c) => format!("{c} | {hint}"),
                                None => hint,
                            });
                        }
                        if !phs.is_empty() {
                            let inventory = protected_control_inventory(&phs);
                            context = Some(match context {
                                Some(c) => format!("{c} | {inventory}"),
                                None => inventory,
                            });
                        }
                        placeholders_by_id.insert(entry.id.clone(), RecordProtection {
                            placeholders: phs, speakers,
                        });
                        // Per-entry glossary: terms present in source; for binary
                        // slots also drop translations that cannot fit the budget.
                        let glossary_hint = if opts.use_glossary {
                            glossary.prepared.hint(
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

                // Complete hints can outgrow source-only batches. Requeue entry
                // slices through the existing builder, preserving order and the
                // in-flight window rather than spawning extra unbounded calls.
                if model_allowance.is_some() {
                    let batches = match group_retry_batches(&requests, &opts) {
                        Ok(batches) => batches,
                        Err(message) => {
                            context_error = Some(LocustError::ProviderError(format!(
                                "model context input allowance: {message}"
                            )));
                            break;
                        }
                    };
                    let first_len = batches[0].len();
                    let mut offset = chunk.len();
                    for batch in batches.iter().skip(1).rev() {
                        let start = offset - batch.len();
                        chunk_iter.push_front(&chunk[start..offset]);
                        offset = start;
                    }
                    chunk = &chunk[..first_len];
                    requests.truncate(first_len);
                    placeholders_by_id.retain(|id, _| requests.iter().any(|r| &r.entry_id == id));
                    budgets_by_id.retain(|id, _| requests.iter().any(|r| &r.entry_id == id));
                }

                // 5b. Check the cost of the actual dispatch slice, not unsent splits.
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
                        failed_ids,
                        partial,
                        rejected_results,
                        error,
                    } = call;
                    observed_cost.observe_batch(
                        results.iter().chain(&rejected_results).map(|r| r.cost_usd),
                        self.provider.is_free(),
                    );
                    if (attempts > 1 || !failed_ids.is_empty()) && !self.provider.is_free() {
                        // Failed paid attempts may have unreported remote charges.
                        observed_cost.complete = false;
                    }
                    if budget_error.is_none() {
                        budget_error = observed_budget_error(observed_cost, opts.cost_limit_usd);
                    }
                    // Account provider work before validation or fan-out,
                    // including malformed/partial responses that cannot save.
                    for result in results.iter().chain(&rejected_results) {
                        total_tokens += result.tokens_used.unwrap_or(0) as u64;
                        total_input_tokens += result.input_tokens.unwrap_or(0) as u64;
                        total_output_tokens += result.output_tokens.unwrap_or(0) as u64;
                    }
                    if let Some(error) = error {
                        let _ = tx
                            .send(ProgressEvent::BatchFailed {
                                entry_id: None,
                                error,
                            })
                            .await;
                    }
                    let expected: std::collections::HashSet<_> =
                        requests.iter().map(|r| r.entry_id.as_str()).collect();
                    let actual: std::collections::HashSet<_> = results
                        .iter()
                        .map(|r| r.entry_id.as_str())
                        .chain(failed_ids.iter().map(String::as_str))
                        .collect();
                    if results.len() + failed_ids.len() != requests.len()
                        || actual.len() != results.len() + failed_ids.len()
                        || actual != expected
                        || (!partial && !failed_ids.is_empty())
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
                        if !partial {
                            continue;
                        }
                    }
                    for id in &failed_ids {
                        let _ = tx
                            .send(ProgressEvent::BatchFailed {
                                entry_id: Some(id.clone()),
                                error: "provider failed to translate entry; entry remains pending"
                                    .into(),
                            })
                            .await;
                    }
                    group_acc.retain_on_cancel |= partial;
                    // Restore/collapse first, then repair controls before any length fitting.
                    let mut invalid_controls = HashSet::new();
                    for result in &mut results {
                        let entry = &entries_by_id[&result.entry_id];
                        let restored =
                            restore_and_collapse_result(result, &placeholders_by_id, &entry.source);
                        if !restored
                            || !PlaceholderProcessor::validate_with_profile(
                                &entry.source,
                                &result.translation,
                                entry.placeholder_profile(),
                            )
                            .is_empty()
                        {
                            invalid_controls.insert(result.entry_id.clone());
                        }
                    }
                    for result in &mut results {
                        if !invalid_controls.contains(&result.entry_id) {
                            continue;
                        }
                        let original = requests
                            .iter()
                            .find(|r| r.entry_id == result.entry_id)
                            .expect("batch IDs validated");
                        let phs = &placeholders_by_id[&result.entry_id].placeholders;
                        let instructions = self.provider.supports_instructions();
                        let mt_repair = (!instructions)
                            .then(|| MtControlRepair::new(original, phs))
                            .flatten();
                        let retry_req = if instructions {
                            protected_control_repair_request(original, phs)
                        } else if let Some(repair) = &mt_repair {
                            repair.request.clone()
                        } else {
                            // No identities to re-encode: do not resend identical
                            // source to a provider that cannot see corrections.
                            continue;
                        };
                        let entry = &entries_by_id[&result.entry_id];
                        let repair_attempts = if instructions {
                            MAX_PROTECTED_CONTROL_REPAIRS
                        } else {
                            1
                        };
                        for _ in 0..repair_attempts {
                            if cancel.is_cancelled() || budget_error.is_some() {
                                break;
                            }
                            budget_error = self
                                .additional_request_budget_error(&retry_req, &opts, observed_cost)
                                .await;
                            if cancel.is_cancelled() || budget_error.is_some() {
                                break;
                            }
                            match call_provider(
                                self.provider.clone(),
                                std::slice::from_ref(&retry_req),
                                &self.retry,
                                &cancel,
                            )
                            .await
                            {
                                Ok(call) => {
                                    // Billing precedes ID/content acceptance, even for a
                                    // malformed retry or a response cancelled before saving.
                                    observed_cost.observe_batch(
                                        call.reported_usage().map(|r| r.cost_usd),
                                        self.provider.is_free(),
                                    );
                                    if (call.attempts > 1 || !call.failed_ids.is_empty())
                                        && !self.provider.is_free()
                                    {
                                        observed_cost.complete = false;
                                    }
                                    budget_error =
                                        observed_budget_error(observed_cost, opts.cost_limit_usd);
                                    for usage in call.reported_usage() {
                                        total_tokens += usage.tokens_used.unwrap_or(0) as u64;
                                        total_input_tokens +=
                                            usage.input_tokens.unwrap_or(0) as u64;
                                        total_output_tokens +=
                                            usage.output_tokens.unwrap_or(0) as u64;
                                    }
                                    if cancel.is_cancelled() {
                                        break;
                                    }
                                    if call.results.len() != 1
                                        || call.results[0].entry_id != result.entry_id
                                    {
                                        let _ = tx.send(ProgressEvent::BatchFailed {
                                            entry_id: Some(result.entry_id.clone()),
                                            error: "protected-control repair returned mismatched or duplicate entry IDs; entry remains pending".into(),
                                        }).await;
                                        continue;
                                    }
                                    let mut repaired = call.results.into_iter().next().unwrap();
                                    if let Some(repair) = &mt_repair {
                                        let Some(decoded) = repair.decode(&repaired.translation)
                                        else {
                                            continue;
                                        };
                                        repaired.translation = decoded;
                                    }
                                    if !restore_and_collapse_result(
                                        &mut repaired,
                                        &placeholders_by_id,
                                        &entry.source,
                                    ) {
                                        continue;
                                    }
                                    if repaired.translation.trim().is_empty()
                                        || !PlaceholderProcessor::validate_with_profile(
                                            &entry.source,
                                            &repaired.translation,
                                            entry.placeholder_profile(),
                                        )
                                        .is_empty()
                                    {
                                        continue;
                                    }
                                    result.translation = repaired.translation;
                                    invalid_controls.remove(&result.entry_id);
                                    break;
                                }
                                Err(error) => {
                                    observed_cost.complete &= self.provider.is_free();
                                    budget_error =
                                        observed_budget_error(observed_cost, opts.cost_limit_usd);
                                    tracing::warn!(entry_id = %result.entry_id, %error, "protected-control repair failed; entry remains pending");
                                    // Do not dispatch a new logical call after transport
                                    // failure/deadline exhaustion. Cancellation wins below.
                                    break;
                                }
                            }
                        }
                    }
                    // Repeated-source fan-out cannot bypass failed raw sentinel integrity.
                    invalid_controls = invalid_controls
                        .into_iter()
                        .flat_map(|id| repeated.members[&id].clone())
                        .collect();

                    // Length-aware retries: up to MAX_BINARY_SLOT_LENGTH_RETRIES
                    // extra provider attempts per oversize binary-slot entry.
                    for result in &mut results {
                        if invalid_controls.contains(&result.entry_id) {
                            continue;
                        }
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
                        // Length-only corrections are also invisible to MT.
                        // Keep the existing review and injection capacity policy.
                        if !self.provider.supports_instructions() {
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
                            // A rejected correction cannot authorize saving a lossy
                            // first answer. Fan-out must preserve these rows too.
                            if let Err(error) =
                                self.context_request_error(std::slice::from_ref(&retry_req), &opts)
                            {
                                invalid_controls.extend(repeated.members[&result.entry_id].clone());
                                budget_error = Some(error);
                                break;
                            }
                            budget_error = self
                                .additional_request_budget_error(&retry_req, &opts, observed_cost)
                                .await;
                            if cancel.is_cancelled() || budget_error.is_some() {
                                break;
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
                                        rejected_results,
                                        failed_ids,
                                        ..
                                    } = retry_call;
                                    observed_cost.observe_batch(
                                        retry_batch
                                            .iter()
                                            .chain(&rejected_results)
                                            .map(|r| r.cost_usd),
                                        self.provider.is_free(),
                                    );
                                    if (attempts > 1 || !failed_ids.is_empty())
                                        && !self.provider.is_free()
                                    {
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
                                    for usage in retry_batch.iter().chain(&rejected_results) {
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
                                    if !restore_placeholders_in_result(
                                        &mut retry_result,
                                        &placeholders_by_id,
                                    ) || retry_result.translation.trim().is_empty()
                                        || entries_by_id.get(&result.entry_id).is_some_and(
                                            |entry| {
                                                !PlaceholderProcessor::validate_with_profile(
                                                    &entry.source,
                                                    &retry_result.translation,
                                                    entry.placeholder_profile(),
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
                        if !partial {
                            continue;
                        }
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
                        if let Some(entry) = entries_by_id.get(&result.entry_id) {
                            if invalid_controls.contains(&result.entry_id)
                                || !PlaceholderProcessor::validate_with_profile(
                                    &entry.source,
                                    &result.translation,
                                    entry.placeholder_profile(),
                                )
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

        // Model refusal can happen during a repair while other paid calls are
        // in flight. Drain their receipts before returning; do not abort/drop
        // them and silently omit usage. Their unaccepted rows stay unchanged.
        if model_allowance.is_some() {
            while let Some(joined) = in_flight.join_next().await {
                match joined {
                    Ok((_, _, _, Ok(call))) => {
                        observed_cost.observe_batch(
                            call.reported_usage().map(|r| r.cost_usd),
                            self.provider.is_free(),
                        );
                        if (call.attempts > 1 || !call.failed_ids.is_empty())
                            && !self.provider.is_free()
                        {
                            observed_cost.complete = false;
                        }
                        for result in call.reported_usage() {
                            total_tokens += result.tokens_used.unwrap_or(0) as u64;
                            total_input_tokens += result.input_tokens.unwrap_or(0) as u64;
                            total_output_tokens += result.output_tokens.unwrap_or(0) as u64;
                        }
                    }
                    _ => observed_cost.complete &= self.provider.is_free(),
                }
            }
            budget_error =
                observed_budget_error(observed_cost, opts.cost_limit_usd).or(budget_error);
        }
        budget_error = budget_error.or(context_error);
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
                    remaining: opts.pending_entries(&self.db)?.len(),
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
    let opts = opts.capture_scope(&db)?;
    let initial = opts.pending_entries(&db)?;
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
                remaining: 0,
                total_cost: 0.0,
                cost_is_complete: true,
                duration_secs: start.elapsed().as_secs_f64(),
            })
            .await;
        return Ok(());
    }

    let notes = CharacterNotes::load(&db)?;
    let job_glossary = OnceLock::new();
    let mut cumulative_completed = 0usize;
    let mut cumulative_cost = ObservedCost::default();

    for (i, id) in chain.iter().enumerate() {
        if cancel.is_cancelled() {
            let _ = tx.send(ProgressEvent::Paused).await;
            return Ok(());
        }

        let pending = opts.pending_entries(&db)?;
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
                &notes,
                &job_glossary,
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

        if cancel.is_cancelled() {
            let _ = tx.send(ProgressEvent::Paused).await;
            return Ok(());
        }
        let after = opts.pending_entries(&db)?.len();
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

    let remaining = opts.pending_entries(&db)?.len();
    let total_translated = initial_total
        .saturating_sub(remaining)
        .max(cumulative_completed);

    let _ = tx
        .send(ProgressEvent::Completed {
            total_translated,
            remaining,
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

#[cfg(test)]
#[path = "w3_glossary_tests.rs"]
mod w3_glossary_tests;

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::super::tests::setup;
    use super::{load_pending_entries, StringEntry, StringStatus};

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
}
