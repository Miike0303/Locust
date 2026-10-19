use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::models::{StringEntry, StringStatus};

pub const TRANSLATION_CONFLICT_MESSAGE: &str = "translation changed since it was loaded";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranslationSaveOutcome {
    Updated,
    Missing,
    Conflict,
}

/// With `serde(default)`, absent is None, JSON null is Some(None), and text is
/// Some(Some(text)). Plain nested Options would collapse absent and null.
pub fn deserialize_expected_translation<'de, D>(
    deserializer: D,
) -> std::result::Result<Option<Option<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<String>::deserialize(deserializer).map(Some)
}

#[derive(Debug, Deserialize)]
pub struct TranslationBatchItem {
    pub id: String,
    pub translation: String,
    #[serde(default, deserialize_with = "deserialize_expected_translation")]
    pub expected_translation: Option<Option<String>>,
}

#[derive(Debug, Default, Serialize)]
pub struct TranslationBatchReport {
    pub requested: usize,
    pub applied: usize,
    /// Includes conflicts and unknown ids, preserving the existing count.
    pub skipped: usize,
    pub conflicts: Vec<String>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct ImportApplyReport {
    pub imported: usize,
    /// Identical translations, also included in `imported`.
    #[serde(default)]
    pub unchanged: usize,
    #[serde(default)]
    pub kept_existing: usize,
    pub stale_sources: usize,
    pub unknown_ids: usize,
}

/// A read-only import plan. Counts include confirmations and unchanged rows as imported.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct ImportPreview {
    pub report: ImportApplyReport,
    pub replacements: Vec<ImportChange>,
    pub confirmations: Vec<ImportChange>,
    pub fill_ids: Vec<String>,
    pub kept_ids: Vec<String>,
    pub unchanged_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportChange {
    pub id: String,
    pub previous: String,
    pub previous_status: String,
    pub new_text: String,
}

/// Semantic row state used to compute an automatic translation. Physical pivot
/// source metadata is deliberately excluded: `source` is the request language.
#[derive(Clone, Debug)]
pub(crate) struct TranslationSaveGuard {
    pub(super) source: String,
    pub(super) translation: Option<String>,
    pub(super) status: String,
    pub(super) provider: Option<String>,
}

impl From<&StringEntry> for TranslationSaveGuard {
    fn from(entry: &StringEntry) -> Self {
        Self {
            source: entry.source.clone(),
            translation: entry.translation.clone(),
            status: entry.status.to_string(),
            provider: entry.provider_used.clone(),
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct EntryFilter {
    pub status: Option<StringStatus>,
    pub file_path: Option<String>,
    pub tag: Option<String>,
    pub search: Option<String>,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
}

/// One completed translation run — the unit of the per-project
/// tokens/time/cost ledger.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TranslationRun {
    /// Auto-increment primary key (0 when constructing a run to insert).
    #[serde(default)]
    pub id: i64,
    pub started_at: String,
    pub duration_secs: f64,
    /// Provider id, or a chain summary when multiple were used (e.g. `"a→b"`).
    pub provider: String,
    pub source_lang: String,
    pub target_lang: String,
    pub strings_translated: usize,
    pub tokens_used: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// Sum of observed costs; not necessarily the full charge.
    pub cost_usd: f64,
    #[serde(default)]
    pub cost_is_complete: bool,
}

/// Result of [`super::Database::merge_entries`] — counts for the open-project UI.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MergeStats {
    pub added: usize,
    pub updated: usize,
    pub stale_source_reset: usize,
    pub removed: usize,
    pub preserved_translations: usize,
    /// Rows removed or reset to pending that currently have a non-empty translation.
    #[serde(default)]
    pub lost_translations: usize,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ProjectStats {
    pub total: usize,
    pub pending: usize,
    pub translated: usize,
    pub reviewed: usize,
    pub approved: usize,
    pub error: usize,
    pub total_cost_usd: f64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileStats {
    pub file_path: String,
    pub total: usize,
    pub pending: usize,
    pub translated: usize,
    pub reviewed: usize,
    pub approved: usize,
    pub error: usize,
}

/// Distinct file paths and tags across the whole project (not one page).
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct StringFacets {
    pub file_paths: Vec<String>,
    pub tags: Vec<String>,
}

/// Result of [`super::Database::pivot_to`]: a new project whose SOURCE is this
/// project's translations.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PivotResult {
    pub database_path: String,
    pub entries: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MemoryEntry {
    pub source_hash: String,
    pub lang_pair: String,
    pub source: String,
    pub translation: String,
    pub uses: i64,
    pub last_used: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GlossaryEntry {
    pub term: String,
    pub translation: String,
    pub lang_pair: String,
    pub context: Option<String>,
    pub case_sensitive: bool,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GlossaryMergeReport {
    pub added: usize,
    pub overwritten: usize,
    pub kept_existing: usize,
    pub unchanged: usize,
}

/// One file in an injection recording: the game-root-relative path (always
/// forward slashes, never `..`), the SHA-256 of the bytes injection wrote,
/// and their size. `locust patch` re-verifies both before packing, so a
/// packed file is provably the file injection reported writing.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordedFile {
    pub rel: String,
    pub hash: String,
    pub size: u64,
}

/// Exact pre-injection copy associated with a committed recording.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordedBackup {
    pub id: String,
    pub source_path: PathBuf,
    /// Absolute store location, independent of the caller's current profile.
    /// Older recordings resolve their ID in the caller's configured store.
    #[serde(default)]
    pub storage_root: Option<PathBuf>,
}

/// Everything one `locust inject` run recorded for one language key: the
/// absolutized root of the tree it wrote into and the files it wrote there.
/// `lang: None` is the reserved language-unspecified key (`--direct` without
/// `-l`), rendered as "(unspecified)" and matched only by `patch` without `-l`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InjectionRecording {
    pub lang: Option<String>,
    pub root: PathBuf,
    pub files: Vec<RecordedFile>,
    pub recorded_at: String,
    #[serde(default)]
    pub pristine_backup: Option<RecordedBackup>,
}
