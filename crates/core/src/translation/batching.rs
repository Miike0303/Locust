use crate::models::{StringEntry, TranslationRequest};

use super::provider::TranslationOptions;

pub(super) fn default_max_batch_tokens() -> Option<usize> {
    Some(6000)
}

pub(super) fn translation_batches<'a>(
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

pub(super) fn group_retry_batches<'a>(
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

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
}
