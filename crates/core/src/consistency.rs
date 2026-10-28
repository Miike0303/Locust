//! Advisory project-wide checks; translations are never modified.

use std::collections::HashMap;

use crate::models::{StringEntry, StringStatus, ValidationIssue, ValidationKind};

#[derive(Hash, PartialEq, Eq)]
struct GroupKey<'a> {
    source: &'a str,
    context: [Option<&'a str>; 6],
}

fn nonblank(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

fn group_key(entry: &StringEntry) -> GroupKey<'_> {
    let metadata_text = |key| nonblank(entry.metadata.get(key).and_then(|value| value.as_str()));
    GroupKey {
        source: entry.source.trim(),
        context: [
            nonblank(entry.context.as_deref()),
            metadata_text("context"),
            metadata_text("speaker"),
            metadata_text("speaker_name"),
            metadata_text("character"),
            nonblank(
                entry
                    .metadata
                    .get("godot_dialogic")
                    .and_then(|value| value.get("speaker"))
                    .and_then(|value| value.as_str()),
            ),
        ],
    }
}

#[derive(Default)]
struct Group<'a> {
    variants: HashMap<&'a str, usize>,
    common: &'a str,
    common_count: usize,
}

fn current_translation(entry: &StringEntry) -> Option<&str> {
    if !matches!(
        entry.status,
        StringStatus::Translated | StringStatus::Reviewed | StringStatus::Approved
    ) || entry.require_current_translation().is_err()
    {
        return None;
    }
    nonblank(entry.translation.as_deref())
}

/// Group exact sources after outer whitespace trimming, across all files.
/// Each group also keys on the trimmed, nonblank `StringEntry::context` and
/// string metadata `context`, `speaker`, `speaker_name`, `character`, and
/// `godot_dialogic.speaker`. Missing/blank values share the unassigned context;
/// distinct values stay separate, including conflicting explicit speaker keys.
/// Each named field is an independent separator, even for speaker aliases.
/// File paths, tags and arbitrary extraction/locator metadata are not separators.
/// Only current Translated/Reviewed/Approved rows with nonblank translations
/// participate. Translation comparisons trim only outer whitespace, preserving
/// case and internal spacing. Counts use O(n) hashing, with no pairwise checks or
/// sorting. Issues follow input order; common-translation ties use lexical order.
pub fn translation_consistency_issues(entries: &[StringEntry]) -> Vec<ValidationIssue> {
    let mut groups: HashMap<GroupKey<'_>, Group<'_>> = HashMap::new();
    for entry in entries {
        let Some(translation) = current_translation(entry) else {
            continue;
        };
        let group = groups.entry(group_key(entry)).or_default();
        let count = group.variants.entry(translation).or_default();
        *count += 1;
        if *count > group.common_count
            || (*count == group.common_count && translation < group.common)
        {
            group.common = translation;
            group.common_count = *count;
        }
    }

    let mut issues = Vec::new();
    for entry in entries {
        if current_translation(entry).is_none() {
            continue;
        }
        let group = &groups[&group_key(entry)];
        let variants = group.variants.len();
        if variants >= 2 {
            issues.push(ValidationIssue {
                entry_id: entry.id.clone(),
                kind: ValidationKind::InconsistentTranslation { variants },
                message: format!(
                    "same source has {variants} translation variants; most common translation: {:?}",
                    group.common
                ),
                source: None,
            });
        }
    }
    issues
}
