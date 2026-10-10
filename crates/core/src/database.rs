use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use chrono::{DateTime, Utc};
use rusqlite::{params, types::Value, Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[cfg(test)]
thread_local! {
    pub(crate) static ENTRY_ROWS_MATERIALIZED: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
    static TRANSLATION_RUN_ROWS_MATERIALIZED: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
    pub(crate) static MEMORY_TRANSACTIONS: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
    pub(crate) static MEMORY_LOOKUP_QUERIES: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
    static STRINGS_QUERIES: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
    static IMPORT_READ_QUERIES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    pub(crate) static GUARDED_TRANSLATION_TRANSACTIONS: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
}

use crate::error::{LocustError, Result};
use crate::models::{
    StringEntry, StringStatus, TranslationResult, ValidationIssue, ValidationKind,
    INJECTION_CAPACITY_METADATA_KEY, INJECTION_SOURCE_METADATA_KEY, STALE_TRANSLATION_METADATA_KEY,
};

pub struct Database {
    conn: Arc<Mutex<Connection>>,
    path: Mutex<PathBuf>,
}

#[cfg(test)]
mod registration_recording_tests {
    use super::*;

    #[test]
    fn registration_extension_keeps_outputs_and_first_originals() {
        let root = tempfile::tempdir().unwrap();
        let db = Database::open_in_memory().unwrap();
        let pack = root.path().join("lang_es.json");
        let menu = root.path().join("plugins.js");
        std::fs::write(&pack, b"Spanish pack").unwrap();
        std::fs::write(&menu, b"Spanish menu").unwrap();
        db.record_injection(Some("es"), root.path(), std::slice::from_ref(&pack))
            .unwrap();
        let first = db.get_injection(Some("es")).unwrap().unwrap();
        let backup = RecordedBackup {
            id: "first".into(),
            source_path: root.path().to_owned(),
            storage_root: None,
        };
        db.extend_injection_with_registration_backup(&first, std::slice::from_ref(&menu), &backup)
            .unwrap();
        let registered = db.get_injection(Some("es")).unwrap().unwrap();
        assert_eq!(registered.files.len(), 2);
        assert_eq!(registered.files[0], first.files[0]);
        let file = registered
            .files
            .iter()
            .find(|f| f.rel == "plugins.js")
            .unwrap();
        assert_eq!((file.hash.clone(), file.size), sha256_file(&menu).unwrap());
        std::fs::write(&menu, b"Updated Spanish menu").unwrap();
        let later = RecordedBackup {
            id: "later".into(),
            ..backup.clone()
        };
        db.extend_injection_with_registration_backup(
            &registered,
            std::slice::from_ref(&menu),
            &later,
        )
        .unwrap();
        assert_eq!(
            db.registration_backups(Some("es")).unwrap(),
            vec![("plugins.js".into(), backup.clone())]
        );
        let updated = db.get_injection(Some("es")).unwrap().unwrap();
        std::fs::write(&pack, b"Revised Spanish pack").unwrap();
        db.extend_injection_recording(&updated, std::slice::from_ref(&pack), None)
            .unwrap();
        assert_eq!(
            db.registration_backups(Some("es")).unwrap(),
            vec![("plugins.js".into(), backup)]
        );
        assert_eq!(
            db.get_injection(Some("es")).unwrap().unwrap().files.len(),
            2
        );
    }

    #[test]
    fn registration_extension_rejects_stale_generation_and_unrelated_drift() {
        let root = tempfile::tempdir().unwrap();
        let db = Database::open_in_memory().unwrap();
        let pack = root.path().join("lang_es.json");
        let menu = root.path().join("plugins.js");
        std::fs::write(&pack, b"Spanish pack").unwrap();
        std::fs::write(&menu, b"Spanish menu").unwrap();
        db.record_injection(Some("es"), root.path(), std::slice::from_ref(&pack))
            .unwrap();
        let before = db.get_injection(Some("es")).unwrap().unwrap();
        let backup = RecordedBackup {
            id: "first".into(),
            source_path: root.path().to_owned(),
            storage_root: None,
        };
        std::fs::write(&pack, b"external edit").unwrap();
        assert!(db
            .extend_injection_with_registration_backup(
                &before,
                std::slice::from_ref(&menu),
                &backup
            )
            .is_err());
        assert_eq!(db.get_injection(Some("es")).unwrap().unwrap(), before);
        std::fs::write(&pack, b"Spanish pack").unwrap();
        db.record_injection(Some("es"), root.path(), std::slice::from_ref(&menu))
            .unwrap();
        let replacement = db.get_injection(Some("es")).unwrap();
        assert!(db
            .extend_injection_with_registration_backup(&before, &[menu], &backup)
            .unwrap_err()
            .to_string()
            .contains("recording changed"));
        assert_eq!(db.get_injection(Some("es")).unwrap(), replacement);
        assert!(db.registration_backups(Some("es")).unwrap().is_empty());
    }
}

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

fn lock_connection(conn: &Mutex<Connection>) -> MutexGuard<'_, Connection> {
    conn.lock().unwrap_or_else(PoisonError::into_inner)
}

fn sqlite_sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    path.with_file_name(name)
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

enum ImportDecision {
    Unknown,
    StaleSource,
    Unchanged,
    Kept,
    Fill,
    Replace {
        previous: String,
        previous_status: String,
    },
    /// An identical translation on a pending row still needs its status confirmed.
    Confirm {
        previous: String,
        previous_status: String,
    },
}

impl ImportDecision {
    fn record(&self, report: &mut ImportApplyReport) {
        match self {
            Self::Unknown => report.unknown_ids += 1,
            Self::StaleSource => report.stale_sources += 1,
            Self::Unchanged => {
                report.imported += 1;
                report.unchanged += 1;
            }
            Self::Kept => report.kept_existing += 1,
            Self::Fill | Self::Replace { .. } | Self::Confirm { .. } => report.imported += 1,
        }
    }
}

fn validate_import_ids(updates: &[crate::export::ImportedTranslation]) -> Result<()> {
    let mut seen = HashSet::new();
    for entry in updates {
        if !seen.insert(&entry.id) {
            return Err(LocustError::ValidationError {
                entry_id: entry.id.clone(),
                message: "duplicate import id; no translations were saved".into(),
            });
        }
    }
    Ok(())
}

type ImportRow = (String, Option<String>, String);

/// Use the caller's connection so a save reads and writes in the same transaction.
fn read_import_rows(
    conn: &Connection,
    updates: &[crate::export::ImportedTranslation],
) -> Result<HashMap<String, ImportRow>> {
    let mut current = HashMap::with_capacity(updates.len());
    for chunk in updates.chunks(500) {
        let placeholders = vec!["?"; chunk.len()].join(",");
        let mut statement = conn.prepare_cached(&format!(
            "SELECT id, source, translation, status FROM strings WHERE id IN ({placeholders})"
        ))?;
        #[cfg(test)]
        IMPORT_READ_QUERIES.with(|count| count.set(count.get() + 1));
        let rows = statement.query_map(
            rusqlite::params_from_iter(chunk.iter().map(|entry| &entry.id)),
            |row| Ok((row.get(0)?, (row.get(1)?, row.get(2)?, row.get(3)?))),
        )?;
        for row in rows {
            let (id, state) = row?;
            current.insert(id, state);
        }
    }
    Ok(current)
}

fn classify_import(
    current: Option<&ImportRow>,
    entry: &crate::export::ImportedTranslation,
    keep_existing: bool,
) -> ImportDecision {
    let Some((source, translation, status)) = current else {
        return ImportDecision::Unknown;
    };
    if source != &entry.source {
        return ImportDecision::StaleSource;
    }
    if translation.as_deref() == Some(entry.translation.as_str()) {
        return if status != "pending" {
            ImportDecision::Unchanged
        } else {
            ImportDecision::Confirm {
                previous: entry.translation.clone(),
                previous_status: status.clone(),
            }
        };
    }
    match translation.as_ref().filter(|text| !text.trim().is_empty()) {
        Some(_) if keep_existing => ImportDecision::Kept,
        Some(previous) => ImportDecision::Replace {
            previous: previous.clone(),
            previous_status: status.clone(),
        },
        None => ImportDecision::Fill,
    }
}

/// Semantic row state used to compute an automatic translation. Physical pivot
/// source metadata is deliberately excluded: `source` is the request language.
#[derive(Clone, Debug)]
pub(crate) struct TranslationSaveGuard {
    source: String,
    translation: Option<String>,
    status: String,
    provider: Option<String>,
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

fn translation_save_conflict(id: &str) -> LocustError {
    LocustError::ValidationError {
        entry_id: id.to_owned(),
        message: "translation_conflict: source or translation changed while this result was being computed (or the entry was removed); no results from this batch were saved; refresh and retry".into(),
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

/// Result of [`Database::merge_entries`] — counts for the open-project UI.
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

fn has_saved_translation(translation: &Option<String>) -> bool {
    translation.as_ref().is_some_and(|text| !text.is_empty())
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

/// Result of [`Database::pivot_to`]: a new project whose SOURCE is this
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

/// Lowercase hex SHA-256 of `bytes` — the hash stored in injection recordings.
/// SHA-256 over BLAKE3 because `sha2` is already in the dependency tree; the
/// column is TEXT, so nothing else depends on the choice.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

/// 1 MiB — same chunk as patch apply/verify streaming. Pack and injection
/// recording MUST use this path for multi-GB game files; `fs::read` peak RAM
/// equals file size (2× with `--pristine`).
const FILE_HASH_CHUNK: usize = 1024 * 1024;

/// SHA-256 a file without loading it entirely into RAM.
/// Returns `(lowercase_hex, byte_len)`.
pub fn sha256_file(path: &Path) -> Result<(String, u64)> {
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; FILE_HASH_CHUNK];
    let mut total = 0u64;
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        total += n as u64;
        hasher.update(&buf[..n]);
    }
    Ok((hex::encode(hasher.finalize()), total))
}

#[cfg(test)]
thread_local! {
    pub(crate) static SHA256_PATH_CALLS: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
}

/// Digest of a file on disk — same hex as [`sha256_hex`] of its bytes.
pub fn sha256_path(path: &Path) -> Result<String> {
    #[cfg(test)]
    SHA256_PATH_CALLS.with(|calls| {
        if let Some(n) = calls.get() {
            calls.set(Some(n + 1));
        }
    });
    Ok(sha256_file(path)?.0)
}

/// Stream `path` into `out` in fixed-size chunks (no full-file buffer).
pub fn copy_path_chunked(path: &Path, out: &mut dyn std::io::Write) -> Result<u64> {
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut buf = vec![0u8; FILE_HASH_CHUNK];
    let mut total = 0u64;
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        out.write_all(&buf[..n])?;
        total += n as u64;
    }
    Ok(total)
}

/// Case-fold a path fragment for comparison — ONLY where the filesystem
/// itself folds case (NTFS, APFS). On ext4 two case spellings are two
/// different files, and folding would invent a match between them.
pub fn fold_path_case(s: &str) -> String {
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    {
        s.to_lowercase()
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        s.to_string()
    }
}

/// Decompose a path into comparable (folded key, raw component) pairs.
///
/// Both sides of every recording comparison go through this, because the two
/// spellings of one path routinely diverge: relative vs absolute, drive or
/// directory case, or a `\\?\` verbatim prefix left behind by `canonicalize`.
/// A literal `strip_prefix` silently fails on all of those.
///
/// `canonicalize` is preferred (it resolves symlinks and on-disk casing); a
/// path that does not exist on disk cannot be canonicalized, so it falls back
/// to lexical absolutization, which still repairs relative-vs-absolute
/// divergence.
///
/// ponytail: duplicated in spirit with `normalize_path_for_compare` in the
/// Ren'Py plugin; unify once core grows a path-identity module. Ceiling: the
/// two implementations could drift on a platform-specific edge.
fn resolved_parts(p: &Path) -> Vec<(String, std::ffi::OsString)> {
    use std::path::{Component, Prefix};
    let resolved = p
        .canonicalize()
        .or_else(|_| std::path::absolute(p))
        .unwrap_or_else(|_| p.to_path_buf());
    let mut out = Vec::new();
    for c in resolved.components() {
        let key = match c {
            // `\\?\C:\` (verbatim, from canonicalize) and `C:\` (plain) name
            // the same drive and must compare equal.
            Component::Prefix(pr) => match pr.kind() {
                Prefix::VerbatimDisk(d) | Prefix::Disk(d) => {
                    format!("{}:", (d as char))
                }
                Prefix::VerbatimUNC(server, share) | Prefix::UNC(server, share) => format!(
                    r"\\{}\{}",
                    server.to_string_lossy(),
                    share.to_string_lossy()
                ),
                _ => pr.as_os_str().to_string_lossy().into_owned(),
            },
            // Both sides are absolute after resolution, so the root marker
            // carries no information.
            Component::RootDir | Component::CurDir => continue,
            Component::ParentDir => "..".to_string(),
            Component::Normal(s) => s.to_string_lossy().into_owned(),
        };
        out.push((fold_path_case(&key), c.as_os_str().to_os_string()));
    }
    out
}

/// True when two spellings name the same on-disk location. This is the
/// identity check `locust patch` runs between its game-path argument and a
/// recording's root: packing from a different tree would ship that tree's
/// files, not the ones injection wrote.
pub fn paths_identical(a: &Path, b: &Path) -> bool {
    let ka: Vec<String> = resolved_parts(a).into_iter().map(|(k, _)| k).collect();
    let kb: Vec<String> = resolved_parts(b).into_iter().map(|(k, _)| k).collect();
    !ka.is_empty() && ka == kb
}

/// Forward-slash path of `file` relative to `root`, or `None` when `file` is
/// not strictly under `root`. This is the containment check behind injection
/// recordings: a file that does not resolve under its language's root must
/// hard-fail at record time, never be recorded against the wrong tree.
pub fn rel_under_root(file: &Path, root: &Path) -> Option<String> {
    let f = resolved_parts(file);
    let r = resolved_parts(root);
    if f.len() <= r.len() || !f.iter().zip(&r).all(|(a, b)| a.0 == b.0) {
        return None;
    }
    Some(
        f[r.len()..]
            .iter()
            .map(|(_, raw)| raw.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/"),
    )
}

/// Resolved, case-folded identity key for one physical file — two spellings
/// of the same file compare equal, two different files never do.
pub(crate) fn path_identity_key(p: &Path) -> String {
    resolved_parts(p)
        .iter()
        .map(|(k, _)| k.as_str())
        .collect::<Vec<_>>()
        .join("/")
}

/// Shared schema + migration used by [`Database::open`] and [`Database::reopen`].
fn init_schema(conn: &Connection) -> Result<()> {
    // The injected_files table was rebuilt (lang/root/rel/hash/size) when
    // `locust patch` moved to packing exclusively from recordings. Any
    // table missing part of that column set — the legacy file_path
    // schema, or an intermediate one — cannot serve the new contract and
    // would fail at runtime with a raw SQL error the first time a
    // recording is read or written, so it is dropped whole. Recordings
    // are reproducible caches: the next `locust inject` rebuilds one,
    // and `locust patch` on a missing recording names that exact
    // command.
    let legacy: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master
         WHERE type = 'table' AND name = 'injected_files'
           AND (SELECT COUNT(*) FROM pragma_table_info('injected_files')
                WHERE name IN ('lang', 'root', 'rel', 'hash', 'size',
                               'recorded_at')) < 6",
        [],
        |row| row.get(0),
    )?;
    if legacy > 0 {
        conn.execute("DROP TABLE injected_files", [])?;
    }
    conn.execute_batch(
        "
        PRAGMA journal_mode = WAL;
        PRAGMA synchronous = NORMAL;
        PRAGMA foreign_keys = ON;

        CREATE TABLE IF NOT EXISTS project_metadata (
            key TEXT PRIMARY KEY NOT NULL,
            value TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS textasset_originals (
            sha256 TEXT PRIMARY KEY NOT NULL,
            byte_len INTEGER NOT NULL CHECK(byte_len > 0 AND byte_len <= 1048576),
            payload TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS strings (
            id TEXT PRIMARY KEY,
            source TEXT NOT NULL,
            translation TEXT,
            status TEXT NOT NULL DEFAULT 'pending',
            file_path TEXT NOT NULL,
            context TEXT,
            tags TEXT NOT NULL DEFAULT '[]',
            metadata TEXT NOT NULL DEFAULT '{}',
            char_limit INTEGER,
            provider_used TEXT,
            created_at TEXT NOT NULL,
            translated_at TEXT,
            reviewed_at TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_strings_status ON strings(status);
        CREATE INDEX IF NOT EXISTS idx_strings_file ON strings(file_path);

        CREATE TABLE IF NOT EXISTS glossary (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            term TEXT NOT NULL,
            translation TEXT NOT NULL,
            lang_pair TEXT NOT NULL,
            context TEXT,
            case_sensitive INTEGER NOT NULL DEFAULT 0,
            UNIQUE(term, lang_pair)
        );

        CREATE TABLE IF NOT EXISTS translation_memory (
            source_hash TEXT NOT NULL,
            lang_pair TEXT NOT NULL,
            source TEXT NOT NULL,
            translation TEXT NOT NULL,
            uses INTEGER NOT NULL DEFAULT 1,
            last_used TEXT NOT NULL,
            PRIMARY KEY (source_hash, lang_pair)
        );

        CREATE TABLE IF NOT EXISTS validation_issues (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            entry_id TEXT NOT NULL,
            kind TEXT NOT NULL,
            message TEXT NOT NULL,
            resolved INTEGER NOT NULL DEFAULT 0
        );

        CREATE TABLE IF NOT EXISTS translation_runs (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            started_at TEXT NOT NULL,
            duration_secs REAL NOT NULL,
            provider TEXT NOT NULL,
            source_lang TEXT NOT NULL,
            target_lang TEXT NOT NULL,
            strings_translated INTEGER NOT NULL,
            tokens_used INTEGER NOT NULL DEFAULT 0,
            input_tokens INTEGER NOT NULL DEFAULT 0,
            output_tokens INTEGER NOT NULL DEFAULT 0,
            cost_usd REAL NOT NULL DEFAULT 0
        );

        CREATE TABLE IF NOT EXISTS injected_files (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            lang TEXT,
            root TEXT NOT NULL,
            rel TEXT NOT NULL,
            hash TEXT NOT NULL,
            size INTEGER NOT NULL,
            recorded_at TEXT NOT NULL
        );
        ",
    )?;
    // Preserve older recordings; absent provenance must never be guessed.
    let has_pristine_backup = {
        let mut stmt = conn.prepare("PRAGMA table_info(injected_files)")?;
        let names = stmt.query_map([], |row| row.get::<_, String>(1))?;
        names
            .collect::<std::result::Result<Vec<_>, _>>()?
            .iter()
            .any(|name| name == "pristine_backup")
    };
    if !has_pristine_backup {
        conn.execute(
            "ALTER TABLE injected_files ADD COLUMN pristine_backup TEXT",
            [],
        )?;
    }
    let has_registration_backup = conn
        .prepare("PRAGMA table_info(injected_files)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<std::result::Result<Vec<_>, _>>()?
        .iter()
        .any(|name| name == "registration_backup");
    if !has_registration_backup {
        conn.execute(
            "ALTER TABLE injected_files ADD COLUMN registration_backup TEXT",
            [],
        )?;
    }
    // Migrate older DBs that predate the input/output token columns.
    // ADD COLUMN errors if the column already exists — ignore that.
    let _ = conn.execute(
        "ALTER TABLE translation_runs ADD COLUMN input_tokens INTEGER NOT NULL DEFAULT 0",
        [],
    );
    let _ = conn.execute(
        "ALTER TABLE translation_runs ADD COLUMN output_tokens INTEGER NOT NULL DEFAULT 0",
        [],
    );
    // Missing historical metadata cannot prove a zero or complete bill.
    let has_cost_completeness = {
        let mut stmt = conn.prepare("PRAGMA table_info(translation_runs)")?;
        let names = stmt.query_map([], |row| row.get::<_, String>(1))?;
        names
            .collect::<std::result::Result<Vec<_>, _>>()?
            .iter()
            .any(|name| name == "cost_is_complete")
    };
    if !has_cost_completeness {
        conn.execute(
            "ALTER TABLE translation_runs ADD COLUMN cost_is_complete INTEGER NOT NULL DEFAULT 0",
            [],
        )?;
    }
    Ok(())
}

impl Database {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let conn = Connection::open(path)?;
        init_schema(&conn)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            path: Mutex::new(path.to_path_buf()),
        })
    }

    /// Open an existing database without creating directories or migrating it.
    fn open_for_snapshot(path: &Path) -> Result<Self> {
        let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            path: Mutex::new(path.to_path_buf()),
        })
    }

    /// Consistent saved-project reads without schema/configuration writes.
    /// Uses normal read-only SQLite (including committed WAL), not immutable
    /// mode. SQLite may create WAL/SHM sidecars alongside the saved database.
    pub(crate) fn read_project_snapshot(path: &Path) -> Result<Self> {
        let db = Self::open_for_snapshot(path)?;
        lock_connection(&db.conn).execute_batch("BEGIN DEFERRED TRANSACTION")?;
        Ok(db)
    }

    /// Copy `src` with [`Database::snapshot_to`] without modifying that file.
    /// A read-only snapshot can leave an empty `-wal`/`-shm` behind; those are
    /// removed when this call created them. Sidecars that were already there stay.
    pub fn snapshot_existing(src: &Path, dest: &Path) -> Result<()> {
        let mut prior = Vec::new();
        for suffix in ["-wal", "-shm", "-journal"] {
            let path = sqlite_sidecar(src, suffix);
            let existed = path.try_exists()?;
            prior.push((path, existed));
        }
        let db = Self::open_for_snapshot(src)?;
        let result = db.snapshot_to(dest);
        drop(db);
        for (path, existed) in prior {
            if !existed {
                let _ = std::fs::remove_file(path);
            }
        }
        result
    }

    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        init_schema(&conn)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            path: Mutex::new(PathBuf::from(":memory:")),
        })
    }

    /// On-disk path of the live connection (`:memory:` for an in-memory DB).
    pub fn path(&self) -> PathBuf {
        self.path.lock().unwrap().clone()
    }

    /// Write a consistent standalone checkpoint, including committed WAL data.
    pub fn snapshot_to(&self, dest: &Path) -> Result<()> {
        crate::export::check_export_destination(dest, &self.path())?;
        if dest.try_exists()? {
            return Err(LocustError::Other(anyhow::anyhow!(
                "checkpoint destination already exists: {}",
                dest.display()
            )));
        }
        let dest = dest.to_str().ok_or_else(|| {
            LocustError::Other(anyhow::anyhow!("checkpoint destination is not valid UTF-8"))
        })?;
        lock_connection(&self.conn).execute("VACUUM INTO ?1", params![dest])?;
        Ok(())
    }

    /// Swap the live connection to `path` in place and run the same schema
    /// init as [`Database::open`]. Shared `Arc<Database>` handlers keep working.
    pub fn reopen(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let new_conn = Connection::open(path)?;
        init_schema(&new_conn)?;
        *lock_connection(&self.conn) = new_conn;
        *self.path.lock().unwrap() = path.to_path_buf();
        Ok(())
    }

    pub fn get_project_metadata(&self, key: &str) -> Result<Option<serde_json::Value>> {
        let conn = lock_connection(&self.conn);
        let mut stmt = conn.prepare("SELECT value FROM project_metadata WHERE key = ?1")?;
        let mut rows = stmt.query(params![key])?;
        match rows.next()? {
            Some(row) => Ok(Some(serde_json::from_str(&row.get::<_, String>(0)?)?)),
            None => Ok(None),
        }
    }

    pub fn set_project_metadata(&self, key: &str, value: &serde_json::Value) -> Result<()> {
        lock_connection(&self.conn).execute(
            "INSERT OR REPLACE INTO project_metadata(key, value) VALUES (?1, ?2)",
            params![key, serde_json::to_string(value)?],
        )?;
        Ok(())
    }

    pub fn save_entries(&self, entries: &[StringEntry]) -> Result<usize> {
        let conn = lock_connection(&self.conn);
        // Single transaction: per-row implicit transactions fsync each insert,
        // which takes minutes for a full game extraction.
        // prepare_cached: parse/plan the INSERT once per connection, not per row
        // (33k Ochiru / 22k Injuu extracts).
        let tx = conn.unchecked_transaction()?;
        let mut insert = tx.prepare_cached(
            "INSERT OR REPLACE INTO strings
                 (id, source, translation, status, file_path, context, tags, metadata,
                  char_limit, provider_used, created_at, translated_at, reviewed_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        )?;
        let mut count = 0usize;
        let mut originals = OriginalCache::new();
        for entry in entries {
            let tags_json = serde_json::to_string(&entry.tags)?;
            let metadata_json =
                serde_json::to_string(&persist_original_metadata(&tx, entry, &mut originals)?)?;
            let status_str = entry.status.to_string();
            let file_path_str = entry.file_path.to_string_lossy().to_string();
            let created_at_str = entry.created_at.to_rfc3339();
            let translated_at_str = entry.translated_at.map(|d| d.to_rfc3339());
            let reviewed_at_str = entry.reviewed_at.map(|d| d.to_rfc3339());
            let char_limit = entry.char_limit.map(|l| l as i64);

            insert.execute(params![
                entry.id,
                entry.source,
                entry.translation,
                status_str,
                file_path_str,
                entry.context,
                tags_json,
                metadata_json,
                char_limit,
                entry.provider_used,
                created_at_str,
                translated_at_str,
                reviewed_at_str,
            ])?;
            count += 1;
        }
        drop(insert);
        tx.commit()?;
        Ok(count)
    }

    pub fn get_entries(&self, filter: &EntryFilter) -> Result<Vec<StringEntry>> {
        let conn = lock_connection(&self.conn);
        let (where_clause, mut param_values) = entry_where_clause(filter);
        let mut sql = format!("SELECT {ENTRY_COLUMNS} FROM strings {where_clause}");
        append_entry_pagination(&mut sql, &mut param_values, filter);
        let params_refs: Vec<&dyn rusqlite::types::ToSql> =
            param_values.iter().map(|p| p as _).collect();
        query_entries(&conn, &sql, &params_refs, &mut OriginalCache::new())
    }

    /// Return a page and its unpaginated total from one strings query. An empty
    /// page has no window total, so count it under the same connection lock.
    pub fn get_entries_page(&self, filter: &EntryFilter) -> Result<(Vec<StringEntry>, usize)> {
        let conn = lock_connection(&self.conn);
        let (where_clause, param_values) = entry_where_clause(filter);
        let mut sql = format!(
            "SELECT {ENTRY_COLUMNS}, COUNT(*) OVER () AS total FROM strings {where_clause}"
        );
        let mut page_params = param_values.clone();
        append_entry_pagination(&mut sql, &mut page_params, filter);
        let mut entries = Vec::new();
        let mut total = None;
        let mut originals = OriginalCache::new();
        {
            let mut stmt = conn.prepare(&sql)?;
            #[cfg(test)]
            record_strings_query();
            let mut rows = stmt.query(rusqlite::params_from_iter(&page_params))?;
            while let Some(row) = rows.next()? {
                total = Some(row.get::<_, usize>("total")?);
                entries.push(entry_from_row(row, &conn, &mut originals)?);
            }
        }
        let total = match total {
            Some(total) => total,
            None => count_filtered_entries(&conn, &where_clause, &param_values)?,
        };
        Ok((entries, total))
    }

    /// All siblings of the selected TextAsset groups, in the same id order as
    /// get_entries, without materializing unrelated strings.
    pub fn get_entries_for_textasset_groups(
        &self,
        group_ids: &HashSet<String>,
    ) -> Result<Vec<StringEntry>> {
        if group_ids.is_empty() {
            return Ok(Vec::new());
        }
        let conn = lock_connection(&self.conn);
        let mut ids = Vec::new();
        // Filtering with json_extract alone would hide malformed metadata on
        // non-members (and differs from serde for duplicate JSON keys). Scan
        // only id/metadata, retaining neither unrelated rows nor their blobs.
        let mut stmt = conn.prepare("SELECT id, metadata FROM strings ORDER BY id")?;
        let mut rows = stmt.query([])?;
        let mut validated_originals = HashMap::new();
        while let Some(row) = rows.next()? {
            let id: String = row.get(0)?;
            let metadata = parse_entry_metadata(&id, &row.get::<_, String>(1)?)?;
            if metadata
                .get(crate::textasset_group::GROUP_ID_KEY)
                .and_then(|value| value.as_str())
                .is_some_and(|group_id| group_ids.contains(group_id))
            {
                ids.push(id);
            } else {
                validate_unselected_original(&conn, &metadata, &mut validated_originals)?;
            }
        }
        let mut entries = Vec::new();
        let mut originals = OriginalCache::new();
        for chunk in ids.chunks(500) {
            let placeholders = vec!["?"; chunk.len()].join(",");
            let sql = format!("SELECT id, source, translation, status, file_path, context, tags, metadata, char_limit, provider_used, created_at, translated_at, reviewed_at FROM strings WHERE id IN ({placeholders}) ORDER BY id");
            let params_refs: Vec<&dyn rusqlite::types::ToSql> = chunk
                .iter()
                .map(|id| id as &dyn rusqlite::types::ToSql)
                .collect();
            entries.extend(query_entries(&conn, &sql, &params_refs, &mut originals)?);
        }
        Ok(entries)
    }

    pub fn get_entry(&self, id: &str) -> Result<Option<StringEntry>> {
        let conn = lock_connection(&self.conn);
        let mut stmt = conn.prepare(
            "SELECT id, source, translation, status, file_path, context, tags, metadata, char_limit, provider_used, created_at, translated_at, reviewed_at FROM strings WHERE id = ?1",
        )?;
        let mut rows = stmt.query_map(params![id], |row| {
            Ok(RawEntry {
                id: row.get(0)?,
                source: row.get(1)?,
                translation: row.get(2)?,
                status: row.get(3)?,
                file_path: row.get(4)?,
                context: row.get(5)?,
                tags: row.get(6)?,
                metadata: row.get(7)?,
                char_limit: row.get(8)?,
                provider_used: row.get(9)?,
                created_at: row.get(10)?,
                translated_at: row.get(11)?,
                reviewed_at: row.get(12)?,
            })
        })?;

        match rows.next() {
            Some(row) => Ok(Some(raw_to_entry(row?, &conn, &mut OriginalCache::new())?)),
            None => Ok(None),
        }
    }

    /// Distinct `file_path` and tag values for the whole project.
    /// Tags are a JSON array column; flattened via `json_each`, not by
    /// loading every `StringEntry`.
    pub fn get_string_facets(&self) -> Result<StringFacets> {
        let conn = lock_connection(&self.conn);

        let mut file_stmt =
            conn.prepare("SELECT DISTINCT file_path FROM strings ORDER BY file_path")?;
        let file_paths = file_stmt
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<String>>>()?;

        let mut tag_stmt = conn.prepare(
            "SELECT DISTINCT je.value
             FROM strings
             JOIN json_each(strings.tags) AS je
             WHERE json_valid(strings.tags)
               AND json_type(strings.tags) = 'array'
               AND typeof(je.value) = 'text'
               AND length(je.value) > 0
             ORDER BY je.value",
        )?;
        let tags = tag_stmt
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<String>>>()?;

        Ok(StringFacets { file_paths, tags })
    }

    /// Rows with a non-empty trimmed `translation` — the only inputs pivot needs.
    /// Avoids loading every pending row on large projects (tens of thousands).
    fn entries_with_nonempty_translation(&self) -> Result<Vec<StringEntry>> {
        let conn = lock_connection(&self.conn);
        let mut stmt = conn.prepare(
            "SELECT id, source, translation, status, file_path, context, tags, metadata, char_limit, provider_used, created_at, translated_at, reviewed_at
             FROM strings
             WHERE translation IS NOT NULL AND length(trim(translation)) > 0
             ORDER BY id",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(RawEntry {
                id: row.get(0)?,
                source: row.get(1)?,
                translation: row.get(2)?,
                status: row.get(3)?,
                file_path: row.get(4)?,
                context: row.get(5)?,
                tags: row.get(6)?,
                metadata: row.get(7)?,
                char_limit: row.get(8)?,
                provider_used: row.get(9)?,
                created_at: row.get(10)?,
                translated_at: row.get(11)?,
                reviewed_at: row.get(12)?,
            })
        })?;
        let mut entries = Vec::new();
        let mut originals = OriginalCache::new();
        for row in rows {
            entries.push(raw_to_entry(row?, &conn, &mut originals)?);
        }
        Ok(entries)
    }

    /// Write a new project DB at `output` whose SOURCE text is this project's
    /// non-empty translations. Does not modify `self`. Refuses to overwrite.
    pub fn pivot_to(&self, output: &Path) -> Result<PivotResult> {
        if output.exists() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                format!("refusing to overwrite existing file: {}", output.display()),
            )
            .into());
        }

        let entries = self.entries_with_nonempty_translation()?;
        let mut pivoted: Vec<StringEntry> = Vec::with_capacity(entries.len());
        for e in entries {
            e.require_current_translation()
                .map_err(|message| LocustError::Other(anyhow::anyhow!(message)))?;
            e.require_preserved_translation_controls()
                .map_err(|message| LocustError::Other(anyhow::anyhow!(message)))?;
            let injection_source = e
                .injection_source()
                .map_err(|message| LocustError::Other(anyhow::anyhow!(message)))?
                .to_string();
            let grouped = crate::textasset_group::is_grouped_entry(&e);
            let capacity = crate::validation::binary_slot_budget(&e)?;
            let Some(translation) = e.translation.filter(|t| !t.trim().is_empty()) else {
                continue;
            };
            let mut ne = StringEntry::new(e.id, translation, e.file_path);
            ne.context = e.context;
            ne.tags = e.tags;
            ne.char_limit = e.char_limit;
            ne.metadata = e.metadata;
            ne.textasset_original = e.textasset_original;
            ne.metadata.insert(
                INJECTION_SOURCE_METADATA_KEY.to_string(),
                serde_json::Value::String(injection_source),
            );
            if !grouped && crate::textasset_group::structural_textasset_capacity(&ne).is_none() {
                if let Some((encoding, bytes)) = capacity {
                    ne.metadata.insert(
                        INJECTION_CAPACITY_METADATA_KEY.to_string(),
                        serde_json::json!({"encoding": encoding, "bytes": bytes}),
                    );
                }
            }
            pivoted.push(ne);
        }

        if pivoted.is_empty() {
            return Err(LocustError::Other(anyhow::anyhow!(
                "no translated entries in {} to pivot from",
                self.path().display()
            )));
        }

        let out_db = Database::open(output)?;
        let count = out_db.save_entries(&pivoted)?;
        {
            let conn = lock_connection(&self.conn);
            let mut stmt = conn.prepare("SELECT key, value FROM project_metadata")?;
            let rows = stmt.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            for row in rows {
                let (key, value) = row?;
                out_db.set_project_metadata(&key, &serde_json::from_str(&value)?)?;
            }
        }
        Ok(PivotResult {
            database_path: output.to_string_lossy().into_owned(),
            entries: count,
        })
    }

    /// Count packable translations without materializing entries or originals.
    pub fn count_translated_entries(&self) -> Result<usize> {
        let conn = lock_connection(&self.conn);
        let mut stmt = conn.prepare(
            "SELECT translation FROM strings WHERE status IN ('translated', 'reviewed', 'approved')",
        )?;
        let rows = stmt.query_map([], |row| row.get::<_, Option<String>>(0))?;
        let mut count = 0;
        for row in rows {
            // SQLite trim() only strips spaces; retain Rust's Unicode rule.
            if row?.is_some_and(|translation| !translation.trim().is_empty()) {
                count += 1;
            }
        }
        Ok(count)
    }

    pub fn count_entries(&self, filter: &EntryFilter) -> Result<usize> {
        let conn = lock_connection(&self.conn);
        let (where_clause, param_values) = entry_where_clause(filter);
        count_filtered_entries(&conn, &where_clause, &param_values)
    }

    /// Update an existing string's translation. Returns `true` if a row was
    /// updated, `false` if `entry_id` is unknown (import must not count misses
    /// as successes).
    pub async fn save_translation(
        &self,
        entry_id: &str,
        translation: &str,
        provider: &str,
    ) -> Result<bool> {
        Ok(self
            .save_translation_if_unchanged(entry_id, translation, provider, None)
            .await?
            == TranslationSaveOutcome::Updated)
    }

    /// Atomically compare the original translation and write. None preserves
    /// legacy unconditional saves; Some(None) requires SQL NULL (not "").
    pub async fn save_translation_if_unchanged(
        &self,
        entry_id: &str,
        translation: &str,
        provider: &str,
        expected_translation: Option<Option<String>>,
    ) -> Result<TranslationSaveOutcome> {
        let conn = self.conn.clone();
        let entry_id = entry_id.to_string();
        let translation = translation.to_string();
        let provider = provider.to_string();
        tokio::task::spawn_blocking(move || {
            let conn = lock_connection(&conn);
            let now = Utc::now().to_rfc3339();
            let tx = conn.unchecked_transaction()?;
            let n = tx.execute(
                "UPDATE strings SET translation = ?1, status = 'translated', provider_used = ?2, translated_at = ?3, metadata = CASE WHEN ?5 THEN json_remove(metadata, '$.locust_stale_translation') ELSE metadata END WHERE id = ?4 AND (?6 = 0 OR translation IS ?7)",
                params![translation, provider, now, entry_id, !translation.trim().is_empty(), expected_translation.is_some(), expected_translation.flatten()],
            )?;
            let outcome = if n > 0 {
                TranslationSaveOutcome::Updated
            } else if tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM strings WHERE id = ?1)",
                [&entry_id],
                |row| row.get::<_, bool>(0),
            )? {
                TranslationSaveOutcome::Conflict
            } else {
                TranslationSaveOutcome::Missing
            };
            tx.commit()?;
            Ok(outcome)
        })
        .await
        .unwrap()
    }

    /// Apply many translation updates in one transaction (search-replace, bulk
    /// import). Each item is `(entry_id, translation)`. Returns how many rows
    /// were actually updated (unknown ids are skipped, not errors).
    pub async fn save_translations_batch(
        &self,
        updates: Vec<(String, String)>,
        provider: &str,
    ) -> Result<usize> {
        let updates = updates
            .into_iter()
            .map(|(id, translation)| TranslationBatchItem {
                id,
                translation,
                expected_translation: None,
            })
            .collect();
        Ok(self
            .save_translations_batch_if_unchanged(updates, provider)
            .await?
            .applied)
    }

    /// Compare and write each item in one transaction. Conflicts are left
    /// untouched and included in `skipped`; unknown ids are only skipped.
    /// Absent expectations retain unconditional legacy behavior.
    pub async fn save_translations_batch_if_unchanged(
        &self,
        updates: Vec<TranslationBatchItem>,
        provider: &str,
    ) -> Result<TranslationBatchReport> {
        if updates.is_empty() {
            return Ok(TranslationBatchReport::default());
        }
        let conn = self.conn.clone();
        let provider = provider.to_string();
        tokio::task::spawn_blocking(move || {
            let conn = lock_connection(&conn);
            let now = Utc::now().to_rfc3339();
            let tx = conn.unchecked_transaction()?;
            let mut report = TranslationBatchReport {
                requested: updates.len(),
                ..Default::default()
            };
            {
                let mut stmt = tx.prepare_cached(
                    "UPDATE strings SET translation = ?1, status = 'translated', provider_used = ?2, translated_at = ?3, metadata = CASE WHEN ?5 THEN json_remove(metadata, '$.locust_stale_translation') ELSE metadata END WHERE id = ?4 AND (?6 = 0 OR translation IS ?7)",
                )?;
                let mut exists = tx.prepare_cached(
                    "SELECT EXISTS(SELECT 1 FROM strings WHERE id = ?1)",
                )?;
                for update in &updates {
                    let guarded = update.expected_translation.is_some();
                    let expected = update.expected_translation.as_ref().and_then(|v| v.as_deref());
                    let n = stmt.execute(params![update.translation, provider, now, update.id, !update.translation.trim().is_empty(), guarded, expected])?;
                    if n > 0 {
                        report.applied += 1;
                    } else if guarded && exists.query_row([&update.id], |row| row.get::<_, bool>(0))? {
                        report.conflicts.push(update.id.clone());
                    }
                }
            }
            tx.commit()?;
            report.skipped = report.requested - report.applied;
            Ok(report)
        })
        .await
        .unwrap()
    }

    /// Apply imports only to the exact semantic source they were translated
    /// from. Check and write inside one SQLite transaction, including pivots.
    /// Stale/unknown rows remain untouched, including their review metadata.
    pub async fn save_imported_translations_batch(
        &self,
        updates: Vec<crate::export::ImportedTranslation>,
    ) -> Result<ImportApplyReport> {
        self.save_imported_translations_batch_with(updates, false)
            .await
    }

    /// Identical translations on non-pending rows count as imported without changes.
    /// Pending rows are written to confirm their translation, even if identical.
    /// With `keep_existing`, differing nonblank translations are also preserved.
    pub async fn save_imported_translations_batch_with(
        &self,
        updates: Vec<crate::export::ImportedTranslation>,
        keep_existing: bool,
    ) -> Result<ImportApplyReport> {
        validate_import_ids(&updates)?;
        if updates.is_empty() {
            return Ok(ImportApplyReport::default());
        }
        let conn = self.conn.clone();
        let report = tokio::task::spawn_blocking(move || -> Result<_> {
            #[cfg(test)]
            IMPORT_READ_QUERIES.with(|count| count.set(0));
            let conn = lock_connection(&conn);
            let tx = conn.unchecked_transaction()?;
            let now = Utc::now().to_rfc3339();
            let mut report = ImportApplyReport::default();
            {
                let mut write = tx.prepare_cached(
                    "UPDATE strings SET translation = ?1, status = 'translated', provider_used = 'import', translated_at = ?2, metadata = CASE WHEN ?5 THEN json_remove(metadata, '$.locust_stale_translation') ELSE metadata END WHERE id = ?3 AND source = ?4",
                )?;
                let current = read_import_rows(&tx, &updates)?;
                for entry in updates {
                    let decision = classify_import(current.get(&entry.id), &entry, keep_existing);
                    if matches!(decision, ImportDecision::Fill | ImportDecision::Replace { .. } | ImportDecision::Confirm { .. })
                        && write.execute(params![entry.translation, now, entry.id, entry.source, !entry.translation.trim().is_empty()])? == 0 {
                        continue;
                    }
                    decision.record(&mut report);
                }
            }
            tx.commit()?;
            #[cfg(test)]
            let report = (report, IMPORT_READ_QUERIES.with(|count| count.get()));
            Ok(report)
        }).await.map_err(|e| LocustError::Other(anyhow::anyhow!("import save task failed: {e}")))??;
        // Return the blocking worker's measured reads to the calling test thread.
        #[cfg(test)]
        let report = {
            IMPORT_READ_QUERIES.with(|count| count.set(count.get() + report.1));
            report.0
        };
        Ok(report)
    }

    /// Preview an import using only reads. A later save rechecks the current rows.
    pub fn preview_imported_translations(
        &self,
        updates: &[crate::export::ImportedTranslation],
        keep_existing: bool,
    ) -> Result<ImportPreview> {
        validate_import_ids(updates)?;
        let mut preview = ImportPreview::default();
        if updates.is_empty() {
            return Ok(preview);
        }
        let conn = lock_connection(&self.conn);
        let current = read_import_rows(&conn, updates)?;
        for entry in updates {
            let decision = classify_import(current.get(&entry.id), entry, keep_existing);
            decision.record(&mut preview.report);
            let changes = match decision {
                ImportDecision::Replace {
                    previous,
                    previous_status,
                } => Some((&mut preview.replacements, previous, previous_status)),
                ImportDecision::Confirm {
                    previous,
                    previous_status,
                } => Some((&mut preview.confirmations, previous, previous_status)),
                ImportDecision::Fill => {
                    preview.fill_ids.push(entry.id.clone());
                    None
                }
                ImportDecision::Kept => {
                    preview.kept_ids.push(entry.id.clone());
                    None
                }
                ImportDecision::Unchanged => {
                    preview.unchanged_ids.push(entry.id.clone());
                    None
                }
                ImportDecision::Unknown | ImportDecision::StaleSource => None,
            };
            if let Some((changes, previous, previous_status)) = changes {
                changes.push(ImportChange {
                    id: entry.id.clone(),
                    previous,
                    previous_status,
                    new_text: entry.translation.clone(),
                });
            }
        }
        Ok(preview)
    }

    /// Commit a provider batch atomically. Unlike an import, a missing entry is
    /// an error: the caller must never announce translations that were not saved.
    pub async fn save_translation_results(&self, results: &[TranslationResult]) -> Result<()> {
        if results.is_empty() {
            return Ok(());
        }
        let conn = self.conn.clone();
        let results = results.to_vec();
        tokio::task::spawn_blocking(move || {
            let conn = lock_connection(&conn);
            let tx = conn.unchecked_transaction()?;
            let now = Utc::now().to_rfc3339();
            {
                let mut stmt = tx.prepare_cached(
                    "UPDATE strings SET translation = ?1, status = 'translated', provider_used = ?2, translated_at = ?3, metadata = CASE WHEN ?5 THEN json_remove(metadata, '$.locust_stale_translation') ELSE metadata END WHERE id = ?4",
                )?;
                for result in results {
                    if stmt.execute(params![result.translation, result.provider, now, result.entry_id, !result.translation.trim().is_empty()])? != 1 {
                        return Err(LocustError::DatabaseError(rusqlite::Error::QueryReturnedNoRows));
                    }
                }
            }
            tx.commit()?;
            Ok(())
        }).await.map_err(|e| LocustError::ProviderError(format!("translation save task failed: {e}")))?
    }

    /// Automatic provider/cache saves compare their request snapshot in the
    /// same write transaction. One missing or changed row rolls back the entire
    /// batch, including stale-marker removal. Explicit editing APIs stay unconditional.
    pub(crate) async fn save_translation_results_guarded(
        &self,
        results: &[TranslationResult],
        expected: &HashMap<String, TranslationSaveGuard>,
    ) -> Result<()> {
        let mut updates = Vec::with_capacity(results.len());
        let mut ids = std::collections::HashSet::new();
        for result in results {
            if !ids.insert(&result.entry_id) {
                return Err(translation_save_conflict(&result.entry_id));
            }
            let guard = expected
                .get(&result.entry_id)
                .ok_or_else(|| translation_save_conflict(&result.entry_id))?;
            updates.push((
                result.entry_id.clone(),
                result.translation.clone(),
                result.provider.clone(),
                guard.clone(),
            ));
        }
        self.save_guarded_updates(updates).await
    }

    pub(crate) async fn save_translation_guarded(
        &self,
        entry: &StringEntry,
        translation: &str,
        provider: &str,
    ) -> Result<()> {
        self.save_guarded_updates(vec![(
            entry.id.clone(),
            translation.into(),
            provider.into(),
            TranslationSaveGuard::from(entry),
        )])
        .await
    }

    pub(crate) async fn save_guarded_updates(
        &self,
        updates: Vec<(String, String, String, TranslationSaveGuard)>,
    ) -> Result<()> {
        if updates.is_empty() {
            return Ok(());
        }
        let conn = self.conn.clone();
        let result = tokio::task::spawn_blocking(move || {
            let conn = lock_connection(&conn);
            let tx = conn.unchecked_transaction()?;
            let now = Utc::now().to_rfc3339();
            {
                let mut statement = tx.prepare_cached(
                    "UPDATE strings SET translation = ?1, status = 'translated', provider_used = ?2, translated_at = ?3, metadata = CASE WHEN ?5 THEN json_remove(metadata, '$.locust_stale_translation') ELSE metadata END WHERE id = ?4 AND source = ?6 AND translation IS ?7 AND status = ?8 AND provider_used IS ?9"
                )?;
                for (id, translation, provider, guard) in updates {
                    let changed = statement.execute(params![translation, provider, now, id, !translation.trim().is_empty(), guard.source, guard.translation, guard.status, guard.provider])?;
                    if changed != 1 { return Err(translation_save_conflict(&id)); }
                }
            }
            tx.commit()?;
            Ok(())
        }).await.map_err(|e| LocustError::ProviderError(format!("translation save task failed: {e}")))?;
        #[cfg(test)]
        if result.is_ok() {
            GUARDED_TRANSLATION_TRANSACTIONS.with(|count| {
                if let Some(n) = count.get() {
                    count.set(Some(n + 1));
                }
            });
        }
        result
    }

    pub async fn update_entry_status(&self, entry_id: &str, status: StringStatus) -> Result<()> {
        let conn = self.conn.clone();
        let entry_id = entry_id.to_string();
        let status_str = status.to_string();
        tokio::task::spawn_blocking(move || {
            let conn = lock_connection(&conn);
            conn.execute(
                "UPDATE strings SET status = ?1, metadata = CASE WHEN ?1 IN ('reviewed', 'approved') THEN json_remove(metadata, '$.locust_stale_translation') ELSE metadata END, reviewed_at = CASE WHEN ?1 IN ('reviewed', 'approved') THEN strftime('%Y-%m-%dT%H:%M:%fZ', 'now') ELSE reviewed_at END WHERE id = ?2",
                params![status_str, entry_id],
            )?;
            Ok(())
        })
        .await
        .unwrap()
    }

    pub fn lookup_memory(&self, source_hash: &str, lang_pair: &str) -> Result<Option<String>> {
        let conn = lock_connection(&self.conn);
        #[cfg(test)]
        MEMORY_LOOKUP_QUERIES.with(|count| {
            if let Some(n) = count.get() {
                count.set(Some(n + 1));
            }
        });
        let result = conn.query_row(
            "SELECT translation FROM translation_memory WHERE source_hash = ?1 AND lang_pair = ?2",
            params![source_hash, lang_pair],
            |row| row.get(0),
        );
        match result {
            Ok(t) => Ok(Some(t)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Look up a run's hashes with at most 500 hashes plus one language parameter
    /// per query. Reuse the cached statement for each repeated chunk size.
    pub fn lookup_memory_batch(
        &self,
        source_hashes: &[String],
        lang_pair: &str,
    ) -> Result<HashMap<String, String>> {
        let mut translations = HashMap::new();
        if source_hashes.is_empty() {
            return Ok(translations);
        }
        let conn = lock_connection(&self.conn);
        for chunk in source_hashes.chunks(500) {
            let placeholders = vec!["?"; chunk.len()].join(",");
            let mut statement = conn.prepare_cached(&format!(
                "SELECT source_hash, translation FROM translation_memory \
                 WHERE lang_pair = ? AND source_hash IN ({placeholders})"
            ))?;
            #[cfg(test)]
            MEMORY_LOOKUP_QUERIES.with(|count| {
                if let Some(n) = count.get() {
                    count.set(Some(n + 1));
                }
            });
            let rows = statement.query_map(
                rusqlite::params_from_iter(
                    std::iter::once(lang_pair).chain(chunk.iter().map(String::as_str)),
                ),
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )?;
            for row in rows {
                let (hash, translation) = row?;
                translations.insert(hash, translation);
            }
        }
        Ok(translations)
    }

    pub async fn save_memory(
        &self,
        hash: &str,
        source: &str,
        translation: &str,
        lang_pair: &str,
    ) -> Result<()> {
        self.save_memory_batch(
            &[(hash.to_owned(), source.to_owned(), translation.to_owned())],
            lang_pair,
        )
        .await
    }

    pub async fn save_memory_batch(
        &self,
        items: &[(String, String, String)],
        lang_pair: &str,
    ) -> Result<()> {
        if items.is_empty() {
            return Ok(());
        }
        let conn = self.conn.clone();
        let items = items.to_vec();
        let lang_pair = lang_pair.to_string();
        let result = tokio::task::spawn_blocking(move || {
            let conn = lock_connection(&conn);
            let tx = conn.unchecked_transaction()?;
            let mut upsert = tx.prepare_cached(
                "INSERT INTO translation_memory (source_hash, lang_pair, source, translation, uses, last_used)
                 VALUES (?1, ?2, ?3, ?4, 1, ?5)
                 ON CONFLICT(source_hash, lang_pair) DO UPDATE SET
                     translation = excluded.translation,
                     uses = uses + 1,
                     last_used = excluded.last_used",
            )?;
            for (hash, source, translation) in items {
                let now = Utc::now().to_rfc3339();
                upsert.execute(params![hash, lang_pair, source, translation, now])?;
            }
            drop(upsert);
            tx.commit()?;
            Ok(())
        })
        .await
        .unwrap();
        #[cfg(test)]
        if result.is_ok() {
            MEMORY_TRANSACTIONS.with(|count| {
                if let Some(n) = count.get() {
                    count.set(Some(n + 1));
                }
            });
        }
        result
    }

    pub async fn record_translation_run(&self, run: &TranslationRun) -> Result<()> {
        let conn = self.conn.clone();
        let run = run.clone();
        tokio::task::spawn_blocking(move || {
            let conn = lock_connection(&conn);
            conn.execute(
                "INSERT INTO translation_runs
                 (started_at, duration_secs, provider, source_lang, target_lang,
                  strings_translated, tokens_used, input_tokens, output_tokens, cost_usd, cost_is_complete)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params![
                    run.started_at,
                    run.duration_secs,
                    run.provider,
                    run.source_lang,
                    run.target_lang,
                    run.strings_translated as i64,
                    run.tokens_used as i64,
                    run.input_tokens as i64,
                    run.output_tokens as i64,
                    run.cost_usd,
                    run.cost_is_complete,
                ],
            )?;
            Ok(())
        })
        .await
        .unwrap()
    }

    /// Record the files an injection run actually wrote for the `lang` key,
    /// under the root it targeted. `locust patch` packs EXCLUSIVELY from this
    /// recording: entries only name where text was READ, and for archive-based
    /// engines the files injection writes can never become entries.
    ///
    /// - `root` is absolutized here, so the recording survives any later cwd.
    /// - Each file is stored as a forward-slash rel under `root` plus the
    ///   SHA-256 and size of its bytes (read back once, right now).
    /// - CONTAINMENT: any file that does not resolve under `root` — or whose
    ///   rel keeps a `..` component — is a hard error and NOTHING is recorded
    ///   for this key. Recording a cross-tree write is how patches silently
    ///   shipped the wrong tree's files.
    /// - One physical file listed under two spellings is deduplicated; two
    ///   DIFFERENT files whose rels collide under case folding are an error,
    ///   because they cannot both extract from the patch zip on NTFS/APFS.
    /// - A new recording REPLACES the previous one for the SAME key only. An
    ///   EMPTY list is a no-op: a run that wrote nothing must not clobber the
    ///   last good recording, whose files are still on disk.
    pub fn record_injection(
        &self,
        lang: Option<&str>,
        root: &Path,
        written: &[PathBuf],
    ) -> Result<()> {
        self.record_injection_with_backup(lang, root, written, None)
    }

    /// Persist output hashes and their exact backup in one SQLite transaction.
    /// A no-op preserves both; a new legacy recording clears old provenance.
    pub fn record_injection_with_backup(
        &self,
        lang: Option<&str>,
        root: &Path,
        written: &[PathBuf],
        pristine_backup: Option<&RecordedBackup>,
    ) -> Result<()> {
        self.record_injection_generation(lang, root, written, pristine_backup, None)
    }

    /// Extend a checked generation atomically. Newly recorded registration files
    /// retain their pre-registration backup; repeated registration retains the
    /// first original. Ordinary injection recording clears these associations.
    pub fn extend_injection_with_registration_backup(
        &self,
        expected: &InjectionRecording,
        written: &[PathBuf],
        backup: &RecordedBackup,
    ) -> Result<()> {
        self.extend_injection_recording(expected, written, Some(backup))
    }

    /// Merge another Add output into its checked generation without losing
    /// registration originals. The caller holds the game lock throughout.
    pub(crate) fn extend_injection_recording(
        &self,
        expected: &InjectionRecording,
        written: &[PathBuf],
        registration_backup: Option<&RecordedBackup>,
    ) -> Result<()> {
        if written.is_empty() {
            return Ok(());
        }
        let changed: HashSet<_> = written
            .iter()
            .filter_map(|path| rel_under_root(path, &expected.root))
            .map(|rel| fold_path_case(&rel))
            .collect();
        for file in &expected.files {
            if !changed.contains(&fold_path_case(&file.rel)) {
                crate::extraction::verify_recorded_injection_member(&expected.root, file)?;
            }
        }
        let mut files: Vec<_> = expected
            .files
            .iter()
            .map(|f| expected.root.join(&f.rel))
            .collect();
        files.extend_from_slice(written);
        self.record_injection_generation(
            expected.lang.as_deref(),
            &expected.root,
            &files,
            expected.pristine_backup.as_ref(),
            Some((expected, registration_backup)),
        )
    }

    fn record_injection_generation(
        &self,
        lang: Option<&str>,
        root: &Path,
        written: &[PathBuf],
        pristine_backup: Option<&RecordedBackup>,
        extension: Option<(&InjectionRecording, Option<&RecordedBackup>)>,
    ) -> Result<()> {
        if written.is_empty() {
            return Ok(());
        }
        let pristine_backup = pristine_backup.map(serde_json::to_string).transpose()?;
        let root_abs = std::path::absolute(root)?;
        let mut seen: HashMap<String, (String, PathBuf)> = HashMap::new();
        let mut rows: Vec<(String, String, u64)> = Vec::new();
        for p in written {
            let rel = rel_under_root(p, &root_abs).ok_or_else(|| {
                LocustError::InjectionError(format!(
                    "cannot record injection output: \"{}\" is not under the \
                     injection root \"{}\"",
                    p.display(),
                    root_abs.display()
                ))
            })?;
            if Path::new(&rel)
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
            {
                return Err(LocustError::InjectionError(format!(
                    "cannot record injection output: \"{}\" escapes the \
                     injection root \"{}\" via a `..` component",
                    p.display(),
                    root_abs.display()
                )));
            }
            let identity = path_identity_key(p);
            match seen.get(&fold_path_case(&rel)) {
                Some((existing_identity, existing_path)) => {
                    if existing_identity != &identity {
                        return Err(LocustError::InjectionError(format!(
                            "two different written files collide on the same \
                             archive path \"{}\": \"{}\" and \"{}\" — they \
                             cannot both extract from the patch zip on a \
                             case-folding filesystem",
                            rel,
                            existing_path.display(),
                            p.display()
                        )));
                    }
                    continue; // same physical file listed twice
                }
                None => {
                    seen.insert(fold_path_case(&rel), (identity, p.clone()));
                }
            }
            let (hash, size) = sha256_file(p)?;
            rows.push((rel, hash, size));
        }

        let root_str = root_abs.to_string_lossy().to_string();
        let now = Utc::now().to_rfc3339();
        let conn = lock_connection(&self.conn);
        let tx = conn.unchecked_transaction()?;
        let mut registration_backups = HashMap::new();
        if let Some((expected, backup)) = extension {
            if Self::get_injection_from(&tx, lang)?.as_ref() != Some(expected) {
                return Err(LocustError::InjectionError(
                    "injection recording changed during language registration".into(),
                ));
            }
            let mut stmt =
                tx.prepare("SELECT rel, registration_backup FROM injected_files WHERE lang IS ?1")?;
            let saved = stmt.query_map(params![lang], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
            })?;
            for row in saved {
                let (rel, backup) = row?;
                registration_backups.insert(fold_path_case(&rel), backup);
            }
            let backup = backup.map(serde_json::to_string).transpose()?;
            for (rel, _, _) in &rows {
                registration_backups
                    .entry(fold_path_case(rel))
                    .or_insert_with(|| backup.clone());
            }
        }
        tx.execute("DELETE FROM injected_files WHERE lang IS ?1", params![lang])?;
        // Same class as save_entries/merge_entries: one plan for N file rows
        // (Unreal/Unity injects can record hundreds of written paths).
        {
            let mut insert = tx.prepare_cached(
                "INSERT INTO injected_files (lang, root, rel, hash, size, recorded_at, pristine_backup, registration_backup)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            )?;
            for (rel, hash, size) in &rows {
                insert.execute(params![
                    lang,
                    root_str,
                    rel,
                    hash,
                    *size as i64,
                    now,
                    pristine_backup,
                    registration_backups
                        .get(&fold_path_case(rel))
                        .and_then(Option::as_ref)
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// The recording persisted by [`Self::record_injection`] for exactly this
    /// key — `None` matches only the language-unspecified recording, never a
    /// named one, and vice versa. `Ok(None)` when nothing is recorded for it.
    pub fn get_injection(&self, lang: Option<&str>) -> Result<Option<InjectionRecording>> {
        let conn = lock_connection(&self.conn);
        Self::get_injection_from(&conn, lang)
    }

    fn get_injection_from(
        conn: &Connection,
        lang: Option<&str>,
    ) -> Result<Option<InjectionRecording>> {
        let mut stmt = conn.prepare(
            "SELECT root, rel, hash, size, recorded_at, pristine_backup FROM injected_files
             WHERE lang IS ?1 ORDER BY id ASC",
        )?;
        let rows = stmt.query_map(params![lang], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, Option<String>>(5)?,
            ))
        })?;
        let mut backup_json: Option<Option<String>> = None;
        let mut root = None;
        let mut recorded_at = String::new();
        let mut files = Vec::new();
        for row in rows {
            let (r, rel, hash, size, at, backup) = row?;
            if backup_json
                .as_ref()
                .is_some_and(|previous| previous != &backup)
                || root.as_ref().is_some_and(|previous| previous != &r)
                || (!recorded_at.is_empty() && recorded_at != at)
            {
                return Err(LocustError::InjectionError(
                    "inconsistent injection recording provenance".into(),
                ));
            }
            backup_json.get_or_insert(backup);
            root.get_or_insert(r);
            recorded_at = at;
            files.push(RecordedFile {
                rel,
                hash,
                size: size as u64,
            });
        }
        match root {
            Some(r) => Ok(Some(InjectionRecording {
                lang: lang.map(str::to_string),
                root: PathBuf::from(r),
                files,
                recorded_at,
                pristine_backup: backup_json
                    .flatten()
                    .map(|json| serde_json::from_str(&json))
                    .transpose()?,
            })),
            None => Ok(None),
        }
    }

    /// Exact verified backup used for each registration-only replaced file.
    pub fn registration_backups(
        &self,
        lang: Option<&str>,
    ) -> Result<Vec<(String, RecordedBackup)>> {
        let conn = lock_connection(&self.conn);
        let mut stmt = conn.prepare("SELECT rel, registration_backup FROM injected_files WHERE lang IS ?1 AND registration_backup IS NOT NULL")?;
        let rows = stmt.query_map(params![lang], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        rows.map(|row| {
            let (rel, backup) = row?;
            Ok((rel, serde_json::from_str(&backup)?))
        })
        .collect()
    }

    #[cfg(test)]
    pub(crate) fn fail_registration_recording(&self) {
        lock_connection(&self.conn)
            .execute_batch(
                "CREATE TRIGGER fail_registration BEFORE INSERT ON injected_files
             BEGIN SELECT RAISE(ABORT, 'recording failure'); END;",
            )
            .unwrap();
    }

    /// The pristine backup for each recorded language that has one.
    /// Decode through `get_injection` so provenance and JSON errors are preserved.
    pub fn recorded_backup_refs(&self) -> Result<Vec<(Option<String>, RecordedBackup)>> {
        let mut refs = Vec::new();
        for lang in self.list_recorded_langs()? {
            if let Some(backup) = self
                .get_injection(lang.as_deref())?
                .and_then(|recording| recording.pristine_backup)
            {
                refs.push((lang, backup));
            }
        }
        for lang in self.list_recorded_langs()? {
            for (_, backup) in self.registration_backups(lang.as_deref())? {
                refs.push((lang.clone(), backup));
            }
        }
        Ok(refs)
    }

    /// Every language key with a recording, named keys first, the reserved
    /// language-unspecified key (`None`) last. Empty when no injection has
    /// ever been recorded — `locust patch` must then hard-error with the
    /// exact inject command, never fall back to guessing from entries.
    pub fn list_recorded_langs(&self) -> Result<Vec<Option<String>>> {
        let conn = lock_connection(&self.conn);
        let mut stmt =
            conn.prepare("SELECT DISTINCT lang FROM injected_files ORDER BY (lang IS NULL), lang")?;
        let rows = stmt.query_map([], |row| row.get::<_, Option<String>>(0))?;
        let mut langs = Vec::new();
        for row in rows {
            langs.push(row?);
        }
        Ok(langs)
    }

    /// All ledger rows, oldest first (CLI `stats` chronological table).
    /// Callers that want newest-first (HTTP UI) reverse the slice.
    pub fn get_translation_runs(&self) -> Result<Vec<TranslationRun>> {
        let conn = lock_connection(&self.conn);
        let mut stmt = conn.prepare(
            "SELECT id, started_at, duration_secs, provider, source_lang, target_lang,
                    strings_translated, tokens_used, input_tokens, output_tokens, cost_usd, cost_is_complete
             FROM translation_runs ORDER BY started_at ASC, id ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            #[cfg(test)]
            TRANSLATION_RUN_ROWS_MATERIALIZED.with(|count| {
                if let Some(n) = count.get() {
                    count.set(Some(n + 1));
                }
            });
            Ok(TranslationRun {
                id: row.get(0)?,
                started_at: row.get(1)?,
                duration_secs: row.get(2)?,
                provider: row.get(3)?,
                source_lang: row.get(4)?,
                target_lang: row.get(5)?,
                strings_translated: row.get::<_, i64>(6)? as usize,
                tokens_used: row.get::<_, i64>(7)? as u64,
                input_tokens: row.get::<_, i64>(8)? as u64,
                output_tokens: row.get::<_, i64>(9)? as u64,
                cost_usd: row.get(10)?,
                cost_is_complete: row.get(11)?,
            })
        })?;
        let mut runs = Vec::new();
        for row in rows {
            runs.push(row?);
        }
        Ok(runs)
    }

    /// Source language for export headers: prefer the latest run that targeted
    /// `target_lang`, else the latest run of any target, else `fallback`.
    /// Config defaults are not project ground truth once a run exists.
    pub fn resolve_export_source_lang(&self, target_lang: &str, fallback: &str) -> Result<String> {
        use rusqlite::OptionalExtension;

        let conn = lock_connection(&self.conn);
        // Both language columns are NOT NULL; lower() folds ASCII only.
        if let Some(source_lang) = conn
            .query_row(
                "SELECT source_lang FROM translation_runs
                 WHERE lower(target_lang) = lower(?1) AND source_lang <> ''
                 ORDER BY started_at DESC, id DESC LIMIT 1",
                [target_lang],
                |row| row.get::<_, String>(0),
            )
            .optional()?
        {
            return Ok(source_lang);
        }
        Ok(conn
            .query_row(
                "SELECT source_lang FROM translation_runs WHERE source_lang <> ''
                 ORDER BY started_at DESC, id DESC LIMIT 1",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .unwrap_or_else(|| fallback.to_string()))
    }

    pub fn get_stats(&self) -> Result<ProjectStats> {
        let conn = lock_connection(&self.conn);
        // One table scan: the editor polls this after every page of work.
        let mut stmt =
            conn.prepare_cached("SELECT status, COUNT(*) FROM strings GROUP BY status")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, usize>(1)?))
        })?;
        let mut stats = ProjectStats::default();
        for row in rows {
            let (status, n) = row?;
            stats.total += n;
            match status.as_str() {
                "pending" => stats.pending = n,
                "translated" => stats.translated = n,
                "reviewed" => stats.reviewed = n,
                "approved" => stats.approved = n,
                "error" => stats.error = n,
                _ => {}
            }
        }
        Ok(stats)
    }

    /// Progress by status, ordered by file path. Pending rows may retain stale text.
    pub fn get_file_stats(&self) -> Result<Vec<FileStats>> {
        let conn = lock_connection(&self.conn);
        let mut stmt = conn.prepare_cached(
            "SELECT file_path, status, COUNT(*) FROM strings GROUP BY file_path, status",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, usize>(2)?,
            ))
        })?;
        let mut files = BTreeMap::new();
        for row in rows {
            let (file_path, status, n) = row?;
            let stats = files.entry(file_path.clone()).or_insert_with(|| FileStats {
                file_path,
                ..FileStats::default()
            });
            stats.total += n;
            match status.as_str() {
                "pending" => stats.pending = n,
                "translated" => stats.translated = n,
                "reviewed" => stats.reviewed = n,
                "approved" => stats.approved = n,
                "error" => stats.error = n,
                _ => {}
            }
        }
        Ok(files.into_values().collect())
    }

    pub fn save_glossary_entry(&self, entry: &GlossaryEntry) -> Result<()> {
        let conn = lock_connection(&self.conn);
        conn.execute(
            "INSERT INTO glossary (term, translation, lang_pair, context, case_sensitive)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(term, lang_pair) DO UPDATE SET
                 translation = excluded.translation,
                 context = excluded.context,
                 case_sensitive = excluded.case_sensitive",
            params![
                entry.term,
                entry.translation,
                entry.lang_pair,
                entry.context,
                entry.case_sensitive as i32,
            ],
        )?;
        Ok(())
    }

    /// Merge by (term, lang_pair) atomically, preserving existing edits unless
    /// overwrite is requested. Identical entries never issue a write.
    pub fn merge_glossary_entries(
        &self,
        entries: &[GlossaryEntry],
        overwrite: bool,
    ) -> Result<GlossaryMergeReport> {
        use rusqlite::OptionalExtension;

        let conn = lock_connection(&self.conn);
        let tx = conn.unchecked_transaction()?;
        let mut report = GlossaryMergeReport::default();
        {
            let mut current = tx.prepare_cached(
                "SELECT translation, context, case_sensitive FROM glossary
                 WHERE term = ?1 AND lang_pair = ?2",
            )?;
            let mut insert = tx.prepare_cached(
                "INSERT INTO glossary (term, translation, lang_pair, context, case_sensitive)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
            )?;
            let mut update = tx.prepare_cached(
                "UPDATE glossary SET translation = ?2, context = ?4, case_sensitive = ?5
                 WHERE term = ?1 AND lang_pair = ?3",
            )?;
            for entry in entries {
                let existing = current
                    .query_row(params![entry.term, entry.lang_pair], |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, Option<String>>(1)?,
                            row.get::<_, i32>(2)? != 0,
                        ))
                    })
                    .optional()?;
                if let Some((translation, context, case_sensitive)) = existing {
                    if translation == entry.translation
                        && context == entry.context
                        && case_sensitive == entry.case_sensitive
                    {
                        report.unchanged += 1;
                    } else if overwrite {
                        update.execute(params![
                            entry.term,
                            entry.translation,
                            entry.lang_pair,
                            entry.context,
                            entry.case_sensitive as i32,
                        ])?;
                        report.overwritten += 1;
                    } else {
                        report.kept_existing += 1;
                    }
                } else {
                    insert.execute(params![
                        entry.term,
                        entry.translation,
                        entry.lang_pair,
                        entry.context,
                        entry.case_sensitive as i32,
                    ])?;
                    report.added += 1;
                }
            }
        }
        tx.commit()?;
        Ok(report)
    }

    pub fn get_glossary(&self, lang_pair: &str) -> Result<Vec<GlossaryEntry>> {
        let conn = lock_connection(&self.conn);
        let mut stmt = conn.prepare(
            "SELECT term, translation, lang_pair, context, case_sensitive FROM glossary WHERE lang_pair = ?1",
        )?;
        let rows = stmt.query_map(params![lang_pair], |row| {
            Ok(GlossaryEntry {
                term: row.get(0)?,
                translation: row.get(1)?,
                lang_pair: row.get(2)?,
                context: row.get(3)?,
                case_sensitive: row.get::<_, i32>(4)? != 0,
            })
        })?;
        let mut entries = Vec::new();
        for row in rows {
            entries.push(row?);
        }
        Ok(entries)
    }

    pub fn delete_glossary_entry(&self, term: &str, lang_pair: &str) -> Result<()> {
        let conn = lock_connection(&self.conn);
        conn.execute(
            "DELETE FROM glossary WHERE term = ?1 AND lang_pair = ?2",
            params![term, lang_pair],
        )?;
        Ok(())
    }

    pub async fn save_validation_issues(&self, issues: &[ValidationIssue]) -> Result<()> {
        let conn = self.conn.clone();
        let issues: Vec<ValidationIssue> = issues.to_vec();
        tokio::task::spawn_blocking(move || {
            let conn = lock_connection(&conn);
            if issues.is_empty() {
                return Ok(());
            }
            let tx = conn.unchecked_transaction()?;
            {
                let mut insert = tx.prepare_cached(
                    "INSERT INTO validation_issues (entry_id, kind, message) VALUES (?1, ?2, ?3)",
                )?;
                for issue in &issues {
                    let kind_json = serde_json::to_string(&issue.kind)
                        .unwrap_or_else(|_| "unknown".to_string());
                    insert.execute(params![issue.entry_id, kind_json, issue.message])?;
                }
            }
            tx.commit()?;
            Ok(())
        })
        .await
        .unwrap()
    }

    pub fn get_validation_issues(&self, entry_id: Option<&str>) -> Result<Vec<ValidationIssue>> {
        let conn = lock_connection(&self.conn);
        let (sql, params_vec): (String, Vec<Box<dyn rusqlite::types::ToSql>>) = match entry_id {
            Some(id) => (
                "SELECT entry_id, kind, message FROM validation_issues WHERE entry_id = ?1"
                    .to_string(),
                vec![Box::new(id.to_string())],
            ),
            None => (
                "SELECT entry_id, kind, message FROM validation_issues".to_string(),
                vec![],
            ),
        };
        let params_refs: Vec<&dyn rusqlite::types::ToSql> =
            params_vec.iter().map(|p| p.as_ref()).collect();
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params_refs.as_slice(), |row| {
            let kind_str: String = row.get(1)?;
            let kind: ValidationKind =
                serde_json::from_str(&kind_str).unwrap_or(ValidationKind::EmptyTranslation);
            Ok(ValidationIssue {
                entry_id: row.get(0)?,
                kind,
                message: row.get(2)?,
                source: None,
            })
        })?;
        let mut issues = Vec::new();
        for row in rows {
            issues.push(row?);
        }
        Ok(issues)
    }

    pub fn clear_entries(&self) -> Result<()> {
        let conn = lock_connection(&self.conn);
        conn.execute("DELETE FROM strings", [])?;
        Ok(())
    }

    /// Merge a fresh extract into the live `strings` table without wiping
    /// translations. Existing ids keep translation / status / timestamps /
    /// provider; a changed `source` keeps the translation but forces `pending`.
    /// Pivot DBs instead validate incoming physical sources atomically, retain
    /// semantic sources/provenance, and only refresh valid locators.
    /// Ids missing from `entries` are deleted. One transaction.
    pub fn merge_entries(&self, entries: &[StringEntry]) -> Result<MergeStats> {
        self.merge_entries_impl(entries, false)
    }

    /// Merge an explicitly partial extraction without deleting unread resources.
    /// Matching pivot rows must still prove their physical source is unchanged.
    pub fn merge_entries_preserving_missing(&self, entries: &[StringEntry]) -> Result<MergeStats> {
        self.merge_entries_impl(entries, true)
    }

    fn merge_entries_impl(
        &self,
        entries: &[StringEntry],
        preserve_missing: bool,
    ) -> Result<MergeStats> {
        let conn = lock_connection(&self.conn);
        let tx = conn.unchecked_transaction()?;

        struct Stored {
            source: String,
            file_path: String,
            context: Option<String>,
            tags: String,
            char_limit: Option<i64>,
            metadata: HashMap<String, serde_json::Value>,
            translation: Option<String>,
            status: String,
            provider_used: Option<String>,
            created_at: String,
            translated_at: Option<String>,
            reviewed_at: Option<String>,
        }

        let mut existing: HashMap<String, Stored> = HashMap::new();
        {
            let mut stmt = tx.prepare(
                "SELECT id, source, file_path, context, tags, char_limit, metadata,
                        translation, status, provider_used, created_at, translated_at, reviewed_at
                 FROM strings",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, Option<i64>>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, Option<String>>(7)?,
                    row.get::<_, String>(8)?,
                    row.get::<_, Option<String>>(9)?,
                    row.get::<_, String>(10)?,
                    row.get::<_, Option<String>>(11)?,
                    row.get::<_, Option<String>>(12)?,
                ))
            })?;
            for row in rows {
                let (
                    id,
                    source,
                    file_path,
                    context,
                    tags,
                    char_limit,
                    metadata_json,
                    translation,
                    status,
                    provider_used,
                    created_at,
                    translated_at,
                    reviewed_at,
                ) = row?;
                let metadata = serde_json::from_str(&metadata_json).map_err(|error| {
                    LocustError::Other(anyhow::anyhow!(
                        "entry '{id}' has malformed metadata: {error}"
                    ))
                })?;
                existing.insert(
                    id,
                    Stored {
                        source,
                        file_path,
                        context,
                        tags,
                        char_limit,
                        metadata,
                        translation,
                        status,
                        provider_used,
                        created_at,
                        translated_at,
                        reviewed_at,
                    },
                );
            }
        }

        let mut incoming: HashMap<&str, &StringEntry> = HashMap::new();
        let mut originals = OriginalCache::new();
        for entry in entries {
            incoming.insert(entry.id.as_str(), entry);
        }

        // A pivot DB's source is semantic text, so a fresh extraction may only
        // refresh locators after every existing row proves the same untouched
        // physical baseline. Validate the complete overlap before DELETE/UPDATE.
        let database_is_pivoted = existing
            .values()
            .any(|stored| stored.metadata.contains_key(INJECTION_SOURCE_METADATA_KEY));
        if database_is_pivoted {
            let mut validation_originals = OriginalCache::new();
            let mut matched = 0usize;
            for (id, entry) in &incoming {
                let Some(old) = existing.get(*id) else {
                    continue; // no semantic pivot source exists for a newly extracted row
                };
                let expected = match old.metadata.get(INJECTION_SOURCE_METADATA_KEY) {
                    Some(serde_json::Value::String(value)) if !value.is_empty() => value,
                    _ => {
                        return Err(LocustError::Other(anyhow::anyhow!(
                            "pivot entry '{id}' has malformed {INJECTION_SOURCE_METADATA_KEY} metadata"
                        )));
                    }
                };
                if entry.source != expected.as_str() {
                    return Err(LocustError::Other(anyhow::anyhow!(
                        "source_changed for pivot entry '{id}': selected game does not match the original extraction baseline; extract a fresh project for this game copy"
                    )));
                }
                if let Some(original_hash) = old.metadata.get("locres_source_hash") {
                    if entry.metadata.get("locres_source_hash") != Some(original_hash) {
                        return Err(LocustError::Other(anyhow::anyhow!(
                            "source_changed for pivot locres entry '{id}': Unreal source hash no longer matches the original baseline"
                        )));
                    }
                }
                if old
                    .metadata
                    .contains_key(crate::textasset_group::GROUP_ORIGINAL_REF_KEY)
                    || old
                        .metadata
                        .contains_key(crate::textasset_group::GROUP_ORIGINAL_KEY)
                {
                    let mut old_entry = StringEntry::new(*id, &old.source, PathBuf::new());
                    old_entry.metadata = old.metadata.clone();
                    hydrate_original(&tx, &mut old_entry, &mut validation_originals)?;
                    let expected = crate::textasset_group::shared_original(&old_entry)
                        .map_err(original_error)?;
                    let actual =
                        crate::textasset_group::shared_original(entry).map_err(original_error)?;
                    if expected.as_ref().map(|o| o.sha256()) != actual.as_ref().map(|o| o.sha256())
                    {
                        return Err(original_error(format!("source_changed for pivot TextAsset '{id}': original blob no longer matches the physical baseline")));
                    }
                }
                matched += 1;
            }
            if !preserve_missing && matched != existing.len() {
                return Err(LocustError::Other(anyhow::anyhow!(
                    "source_changed: extraction is missing entries from this pivot's original baseline; extract a fresh project for this game copy"
                )));
            }
            // Do not mix newly extracted physical-language rows into a DB whose
            // source language is the prior pivot translation.
            incoming.retain(|id, _| existing.contains_key(*id));
        }

        let mut stats = MergeStats::default();

        {
            let mut delete = tx.prepare_cached("DELETE FROM strings WHERE id = ?1")?;
            for (id, stored) in &existing {
                if !preserve_missing && !incoming.contains_key(id.as_str()) {
                    delete.execute(params![id])?;
                    stats.removed += 1;
                    if has_saved_translation(&stored.translation) {
                        stats.lost_translations += 1;
                    }
                }
            }
        }

        {
            let mut update = tx.prepare_cached(
                "UPDATE strings SET
                        source = ?1,
                        file_path = ?2,
                        context = ?3,
                        tags = ?4,
                        metadata = ?5,
                        char_limit = ?6,
                        translation = ?7,
                        status = ?8,
                        provider_used = ?9,
                        created_at = ?10,
                        translated_at = ?11,
                        reviewed_at = ?12
                     WHERE id = ?13",
            )?;
            let mut insert = tx.prepare_cached(
                "INSERT INTO strings
                     (id, source, translation, status, file_path, context, tags, metadata,
                      char_limit, provider_used, created_at, translated_at, reviewed_at)
                     VALUES (?1, ?2, NULL, 'pending', ?3, ?4, ?5, ?6, ?7, NULL, ?8, NULL, NULL)",
            )?;

            for (id, entry) in incoming {
                let tags_json = serde_json::to_string(&entry.tags)?;
                let file_path_str = entry.file_path.to_string_lossy().to_string();
                let char_limit = entry.char_limit.map(|l| l as i64);

                if let Some(old) = existing.get(id) {
                    let pivoted = old.metadata.contains_key(INJECTION_SOURCE_METADATA_KEY);
                    let source_changed = !pivoted && old.source != entry.source;
                    let status = if source_changed {
                        stats.stale_source_reset += 1;
                        if has_saved_translation(&old.translation) {
                            stats.lost_translations += 1;
                        }
                        StringStatus::Pending.to_string()
                    } else {
                        old.status.clone()
                    };
                    let source = if pivoted {
                        old.source.as_str()
                    } else {
                        entry.source.as_str()
                    };
                    let mut metadata = persist_original_metadata(&tx, entry, &mut originals)?;
                    if let Some(marker) = old.metadata.get(STALE_TRANSLATION_METADATA_KEY) {
                        // Refreshing locators must never silently accept an old
                        // translation. Preserve its first source hash over any
                        // number of edits; corrupt/unknown markers remain errors.
                        let mut marker = marker.clone();
                        if source_changed
                            && marker.get("version").and_then(serde_json::Value::as_u64) == Some(1)
                        {
                            if let Some(object) = marker.as_object_mut() {
                                object.insert(
                                    "current_source_sha256".into(),
                                    serde_json::json!(sha256_hex(source.as_bytes())),
                                );
                            }
                        }
                        metadata.insert(STALE_TRANSLATION_METADATA_KEY.into(), marker);
                    } else if source_changed
                        && old
                            .translation
                            .as_deref()
                            .is_some_and(|text| !text.trim().is_empty())
                    {
                        metadata.insert(
                            STALE_TRANSLATION_METADATA_KEY.into(),
                            serde_json::json!({
                                "version": 1,
                                "translated_source_sha256": sha256_hex(old.source.as_bytes()),
                                "current_source_sha256": sha256_hex(source.as_bytes()),
                            }),
                        );
                    }
                    if pivoted {
                        for key in [
                            INJECTION_SOURCE_METADATA_KEY,
                            INJECTION_CAPACITY_METADATA_KEY,
                            "locres_source_hash",
                        ] {
                            if let Some(value) = old.metadata.get(key) {
                                metadata.insert(key.to_string(), value.clone());
                            }
                        }
                    }
                    if has_saved_translation(&old.translation) {
                        stats.preserved_translations += 1;
                    }
                    if old.source == source
                        && old.file_path == file_path_str
                        && old.context == entry.context
                        && old.tags == tags_json
                        && old.char_limit == char_limit
                        && old.metadata == metadata
                        && old.status == status
                    {
                        continue;
                    }
                    stats.updated += 1;
                    let metadata_json = serde_json::to_string(&metadata)?;
                    update.execute(params![
                        source,
                        file_path_str,
                        entry.context,
                        tags_json,
                        metadata_json,
                        char_limit,
                        old.translation,
                        status,
                        old.provider_used,
                        old.created_at,
                        old.translated_at,
                        old.reviewed_at,
                        entry.id,
                    ])?;
                } else {
                    let metadata_json = serde_json::to_string(&persist_original_metadata(
                        &tx,
                        entry,
                        &mut originals,
                    )?)?;
                    stats.added += 1;
                    let created_at = entry.created_at.to_rfc3339();
                    insert.execute(params![
                        entry.id,
                        entry.source,
                        file_path_str,
                        entry.context,
                        tags_json,
                        metadata_json,
                        char_limit,
                        created_at,
                    ])?;
                }
            }
        }

        tx.commit()?;
        Ok(stats)
    }

    pub fn memory_count(&self) -> Result<usize> {
        let conn = lock_connection(&self.conn);
        let count: usize =
            conn.query_row("SELECT COUNT(*) FROM translation_memory", [], |row| {
                row.get(0)
            })?;
        Ok(count)
    }

    pub fn list_memory(
        &self,
        search: Option<&str>,
        lang_pair: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> Result<(Vec<MemoryEntry>, usize)> {
        let conn = lock_connection(&self.conn);

        let mut where_clauses = Vec::new();
        let mut params_vec: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

        if let Some(s) = search {
            let like = format!("%{}%", s);
            params_vec.push(Box::new(like.clone()));
            params_vec.push(Box::new(like));
            where_clauses.push(format!(
                "(source LIKE ?{} OR translation LIKE ?{})",
                params_vec.len() - 1,
                params_vec.len()
            ));
        }
        if let Some(lp) = lang_pair {
            params_vec.push(Box::new(lp.to_string()));
            where_clauses.push(format!("lang_pair = ?{}", params_vec.len()));
        }

        let where_sql = if where_clauses.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", where_clauses.join(" AND "))
        };

        let count_sql = format!("SELECT COUNT(*) FROM translation_memory {}", where_sql);
        let total: usize = conn.query_row(
            &count_sql,
            rusqlite::params_from_iter(params_vec.iter().map(|p| p.as_ref())),
            |row| row.get(0),
        )?;

        let query_sql = format!(
            "SELECT source_hash, lang_pair, source, translation, uses, last_used
             FROM translation_memory {} ORDER BY last_used DESC LIMIT {} OFFSET {}",
            where_sql, limit, offset
        );
        let mut stmt = conn.prepare(&query_sql)?;
        let entries = stmt
            .query_map(
                rusqlite::params_from_iter(params_vec.iter().map(|p| p.as_ref())),
                |row| {
                    Ok(MemoryEntry {
                        source_hash: row.get(0)?,
                        lang_pair: row.get(1)?,
                        source: row.get(2)?,
                        translation: row.get(3)?,
                        uses: row.get(4)?,
                        last_used: row.get(5)?,
                    })
                },
            )?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok((entries, total))
    }

    pub fn delete_memory(&self, source_hash: &str, lang_pair: &str) -> Result<()> {
        let conn = lock_connection(&self.conn);
        conn.execute(
            "DELETE FROM translation_memory WHERE source_hash = ?1 AND lang_pair = ?2",
            params![source_hash, lang_pair],
        )?;
        Ok(())
    }

    pub fn clear_memory(&self) -> Result<()> {
        let conn = lock_connection(&self.conn);
        conn.execute("DELETE FROM translation_memory", [])?;
        Ok(())
    }

    pub fn memory_lang_pairs(&self) -> Result<Vec<String>> {
        let conn = lock_connection(&self.conn);
        let mut stmt =
            conn.prepare("SELECT DISTINCT lang_pair FROM translation_memory ORDER BY lang_pair")?;
        let pairs = stmt
            .query_map([], |row| row.get(0))?
            .collect::<std::result::Result<Vec<String>, _>>()?;
        Ok(pairs)
    }
}

/// Global translation memory database — shared across projects.
pub struct GlobalMemoryDb {
    db: Database,
}

impl GlobalMemoryDb {
    pub fn open(path: &Path) -> Result<Self> {
        let db = Database::open(path)?;
        Ok(Self { db })
    }

    pub fn open_in_memory() -> Result<Self> {
        let db = Database::open_in_memory()?;
        Ok(Self { db })
    }

    pub fn open_default() -> Result<Self> {
        let config_dir = crate::config::AppConfig::config_dir();
        let path = config_dir.join("global_memory.db");
        Self::open(&path)
    }

    pub fn lookup_memory(&self, source_hash: &str, lang_pair: &str) -> Result<Option<String>> {
        self.db.lookup_memory(source_hash, lang_pair)
    }

    pub async fn save_memory(
        &self,
        hash: &str,
        source: &str,
        translation: &str,
        lang_pair: &str,
    ) -> Result<()> {
        self.db
            .save_memory(hash, source, translation, lang_pair)
            .await
    }

    pub fn memory_count(&self) -> Result<usize> {
        self.db.memory_count()
    }

    pub fn list_memory(
        &self,
        search: Option<&str>,
        lang_pair: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> Result<(Vec<MemoryEntry>, usize)> {
        self.db.list_memory(search, lang_pair, limit, offset)
    }

    pub fn delete_memory(&self, source_hash: &str, lang_pair: &str) -> Result<()> {
        self.db.delete_memory(source_hash, lang_pair)
    }

    pub fn clear_memory(&self) -> Result<()> {
        self.db.clear_memory()
    }

    pub fn memory_lang_pairs(&self) -> Result<Vec<String>> {
        self.db.memory_lang_pairs()
    }
}

struct RawEntry {
    id: String,
    source: String,
    translation: Option<String>,
    status: String,
    file_path: String,
    context: Option<String>,
    tags: String,
    metadata: String,
    char_limit: Option<i64>,
    provider_used: Option<String>,
    created_at: String,
    translated_at: Option<String>,
    reviewed_at: Option<String>,
}

type OriginalCache = HashMap<String, Arc<crate::models::TextAssetOriginal>>;

const ENTRY_COLUMNS: &str = "id, source, translation, status, file_path, context, tags, metadata, char_limit, provider_used, created_at, translated_at, reviewed_at";

fn entry_where_clause(filter: &EntryFilter) -> (String, Vec<Value>) {
    let mut sql = String::from("WHERE 1=1");
    let mut params = Vec::new();
    if let Some(ref status) = filter.status {
        sql.push_str(" AND status = ?");
        params.push(Value::Text(status.to_string()));
    }
    if let Some(ref file_path) = filter.file_path {
        sql.push_str(" AND file_path = ?");
        params.push(Value::Text(file_path.clone()));
    }
    if let Some(ref tag) = filter.tag {
        sql.push_str(" AND tags LIKE ?");
        params.push(Value::Text(format!("%\"{}\"%", tag)));
    }
    if let Some(ref search) = filter.search {
        sql.push_str(" AND (source LIKE ? OR translation LIKE ?)");
        let pattern = format!("%{}%", search);
        params.push(Value::Text(pattern.clone()));
        params.push(Value::Text(pattern));
    }
    (sql, params)
}

fn append_entry_pagination(sql: &mut String, params: &mut Vec<Value>, filter: &EntryFilter) {
    sql.push_str(" ORDER BY id");
    if let Some(limit) = filter.limit {
        sql.push_str(" LIMIT ?");
        params.push(Value::Integer(limit as i64));
    }
    if let Some(offset) = filter.offset {
        if filter.limit.is_none() {
            sql.push_str(" LIMIT -1");
        }
        sql.push_str(" OFFSET ?");
        params.push(Value::Integer(offset as i64));
    }
}

fn count_filtered_entries(
    conn: &Connection,
    where_clause: &str,
    params: &[Value],
) -> Result<usize> {
    let sql = format!("SELECT COUNT(*) FROM strings {where_clause}");
    #[cfg(test)]
    record_strings_query();
    Ok(conn.query_row(&sql, rusqlite::params_from_iter(params), |row| row.get(0))?)
}

#[cfg(test)]
fn record_strings_query() {
    STRINGS_QUERIES.with(|count| {
        if let Some(n) = count.get() {
            count.set(Some(n + 1));
        }
    });
}

fn query_entries(
    conn: &Connection,
    sql: &str,
    params: &[&dyn rusqlite::types::ToSql],
    originals: &mut OriginalCache,
) -> Result<Vec<StringEntry>> {
    let mut stmt = conn.prepare(sql)?;
    #[cfg(test)]
    record_strings_query();
    let mut rows = stmt.query(params)?;
    let mut entries = Vec::new();
    while let Some(row) = rows.next()? {
        entries.push(entry_from_row(row, conn, originals)?);
    }
    Ok(entries)
}

fn entry_from_row(
    row: &rusqlite::Row<'_>,
    conn: &Connection,
    originals: &mut OriginalCache,
) -> Result<StringEntry> {
    let raw = RawEntry {
        id: row.get(0)?,
        source: row.get(1)?,
        translation: row.get(2)?,
        status: row.get(3)?,
        file_path: row.get(4)?,
        context: row.get(5)?,
        tags: row.get(6)?,
        metadata: row.get(7)?,
        char_limit: row.get(8)?,
        provider_used: row.get(9)?,
        created_at: row.get(10)?,
        translated_at: row.get(11)?,
        reviewed_at: row.get(12)?,
    };
    #[cfg(test)]
    ENTRY_ROWS_MATERIALIZED.with(|count| {
        if let Some(n) = count.get() {
            count.set(Some(n + 1));
        }
    });
    raw_to_entry(raw, conn, originals)
}

fn parse_entry_metadata(id: &str, json: &str) -> Result<HashMap<String, serde_json::Value>> {
    serde_json::from_str(json).map_err(|error| {
        LocustError::Other(anyhow::anyhow!(
            "entry '{id}' has malformed metadata: {error}"
        ))
    })
}

// Preserve get_entries' original-metadata errors even for unselected groups.
// Check shared payloads once per digest, retaining only their lengths; unrelated
// StringEntries are never hydrated; payloads are discarded after validation.
fn validate_unselected_original(
    conn: &Connection,
    metadata: &HashMap<String, serde_json::Value>,
    validated: &mut HashMap<String, usize>,
) -> Result<()> {
    use crate::textasset_group as group;
    let inline = metadata
        .get(group::GROUP_ORIGINAL_KEY)
        .map(|value| {
            let text = value
                .as_str()
                .ok_or_else(|| original_error("malformed TextAsset original".into()))?;
            crate::models::TextAssetOriginal::new(text).map_err(original_error)
        })
        .transpose()?;
    if let Some(value) = metadata.get(group::GROUP_ORIGINAL_REF_KEY) {
        let reference = value
            .as_str()
            .ok_or_else(|| original_error("malformed TextAsset reference".into()))?;
        let byte_len = if let Some(original) = inline {
            if reference != original.sha256() {
                return Err(original_error(
                    "shared TextAsset original digest or length does not match metadata".into(),
                ));
            }
            original.text().len()
        } else if let Some(byte_len) = validated.get(reference) {
            *byte_len
        } else {
            let original =
                load_original(conn, reference, &mut OriginalCache::new())?.ok_or_else(|| {
                    original_error(format!("missing shared TextAsset original {reference}"))
                })?;
            let byte_len = original.text().len();
            validated.insert(reference.to_owned(), byte_len);
            byte_len
        };
        if metadata
            .get(group::GROUP_ORIGINAL_BYTES_KEY)
            .and_then(|value| value.as_u64())
            != Some(byte_len as u64)
        {
            return Err(original_error(
                "shared TextAsset original digest or length does not match metadata".into(),
            ));
        }
    } else if metadata.contains_key(group::GROUP_ORIGINAL_BYTES_KEY) {
        return Err(original_error(
            "TextAsset original length has no digest reference".into(),
        ));
    }
    Ok(())
}

fn original_error(message: String) -> LocustError {
    LocustError::Other(anyhow::anyhow!(message))
}

fn load_original(
    conn: &Connection,
    reference: &str,
    cache: &mut OriginalCache,
) -> Result<Option<Arc<crate::models::TextAssetOriginal>>> {
    if let Some(original) = cache.get(reference) {
        return Ok(Some(Arc::clone(original)));
    }
    if reference.len() != 64
        || !reference
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(original_error(
            "malformed TextAsset SHA256 reference".into(),
        ));
    }
    let mut stmt = conn.prepare("SELECT byte_len, CASE WHEN length(CAST(payload AS BLOB)) <= 1048576 THEN payload ELSE NULL END FROM textasset_originals WHERE sha256 = ?1")?;
    let mut rows = stmt.query(params![reference])?;
    let Some(row) = rows.next()? else {
        return Ok(None);
    };
    let byte_len: i64 = row.get(0)?;
    let payload: Option<String> = row.get(1)?;
    let payload =
        payload.ok_or_else(|| original_error("oversized TextAsset original in database".into()))?;
    let original =
        Arc::new(crate::models::TextAssetOriginal::new(&payload).map_err(original_error)?);
    if byte_len != payload.len() as i64 || original.sha256() != reference {
        return Err(original_error(
            "corrupt shared TextAsset original: digest or byte length mismatch".into(),
        ));
    }
    cache.insert(reference.to_owned(), Arc::clone(&original));
    Ok(Some(original))
}

fn hydrate_original(
    conn: &Connection,
    entry: &mut StringEntry,
    cache: &mut OriginalCache,
) -> Result<()> {
    use crate::textasset_group as group;
    if entry.textasset_original.is_none() && !entry.metadata.contains_key(group::GROUP_ORIGINAL_KEY)
    {
        if let Some(value) = entry.metadata.get(group::GROUP_ORIGINAL_REF_KEY) {
            let reference = value
                .as_str()
                .ok_or_else(|| original_error("malformed TextAsset reference".into()))?;
            entry.textasset_original =
                Some(load_original(conn, reference, cache)?.ok_or_else(|| {
                    original_error(format!("missing shared TextAsset original {reference}"))
                })?);
        }
    }
    if let Some(original) = group::shared_original(entry).map_err(original_error)? {
        let shared = cache
            .entry(original.sha256().to_owned())
            .or_insert(original);
        group::attach_shared_original(entry, Arc::clone(shared));
    }
    Ok(())
}

fn persist_original_metadata(
    conn: &Connection,
    entry: &StringEntry,
    cache: &mut OriginalCache,
) -> Result<HashMap<String, serde_json::Value>> {
    use crate::textasset_group as group;
    // Skip legacy payload before cloning metadata: normalization must not make
    // another full original copy merely to remove it again.
    let mut normalized = StringEntry::new(&entry.id, "", PathBuf::new());
    normalized.metadata = entry
        .metadata
        .iter()
        .filter(|(key, _)| key.as_str() != group::GROUP_ORIGINAL_KEY)
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    if entry.textasset_original.is_none() && !entry.metadata.contains_key(group::GROUP_ORIGINAL_KEY)
    {
        hydrate_original(conn, &mut normalized, cache)?;
    } else {
        normalized.textasset_original = group::shared_original(entry).map_err(original_error)?;
    }
    if let Some(original) = &normalized.textasset_original {
        if !cache.contains_key(original.sha256())
            && load_original(conn, original.sha256(), cache)?.is_none()
        {
            conn.execute(
                "INSERT INTO textasset_originals(sha256, byte_len, payload) VALUES (?1, ?2, ?3)",
                params![
                    original.sha256(),
                    original.text().len() as i64,
                    original.text()
                ],
            )?;
            cache.insert(original.sha256().to_owned(), Arc::clone(original));
        }
        let original = Arc::clone(original);
        group::attach_shared_original(&mut normalized, original);
    } else {
        hydrate_original(conn, &mut normalized, cache)?;
    }
    Ok(normalized.metadata)
}

fn raw_to_entry(
    raw: RawEntry,
    conn: &Connection,
    originals: &mut OriginalCache,
) -> Result<StringEntry> {
    let status: StringStatus = raw.status.parse().unwrap_or(StringStatus::Pending);
    let tags: Vec<String> = serde_json::from_str(&raw.tags).unwrap_or_default();
    let metadata = parse_entry_metadata(&raw.id, &raw.metadata)?;
    let created_at: DateTime<Utc> = DateTime::parse_from_rfc3339(&raw.created_at)
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now());
    let translated_at = raw
        .translated_at
        .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
        .map(|d| d.with_timezone(&Utc));
    let reviewed_at = raw
        .reviewed_at
        .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
        .map(|d| d.with_timezone(&Utc));

    let mut entry = StringEntry {
        id: raw.id,
        source: raw.source,
        translation: raw.translation,
        file_path: PathBuf::from(raw.file_path),
        context: raw.context,
        tags,
        metadata,
        textasset_original: None,
        status,
        provider_used: raw.provider_used,
        char_limit: raw.char_limit.map(|l| l as usize),
        created_at,
        translated_at,
        reviewed_at,
    };
    hydrate_original(conn, &mut entry, originals)?;
    Ok(entry)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_entry(id: &str, source: &str) -> StringEntry {
        StringEntry::new(id, source, PathBuf::from("test.json"))
    }

    #[test]
    fn test_open_in_memory() {
        let db = Database::open_in_memory().unwrap();
        assert!(db.get_entries(&EntryFilter::default()).unwrap().is_empty());
    }

    #[test]
    fn poisoned_connection_allows_later_queries() {
        let db = Database::open_in_memory().unwrap();
        let conn = db.conn.clone();
        assert!(std::thread::spawn(move || {
            let _guard = conn.lock().unwrap();
            panic!("simulate a worker panic while holding the connection");
        })
        .join()
        .is_err());

        assert_eq!(db.get_project_metadata("after_panic").unwrap(), None);
        db.set_project_metadata("after_panic", &serde_json::json!(true))
            .unwrap();
        assert_eq!(
            db.get_project_metadata("after_panic").unwrap(),
            Some(serde_json::json!(true))
        );
    }

    #[test]
    fn partial_extract_preserves_absent_rows_and_pivot_provenance() {
        let db = Database::open_in_memory().unwrap();
        let mut a = make_entry("a", "日本語A");
        a.translation = Some("English A".into());
        let mut b = make_entry("b", "日本語B");
        b.translation = Some("English B".into());
        db.save_entries(&[a.clone(), b]).unwrap();
        let stats = db.merge_entries_preserving_missing(&[a.clone()]).unwrap();
        assert_eq!(stats.removed, 0);
        assert_eq!(
            db.get_entry("b").unwrap().unwrap().translation.as_deref(),
            Some("English B")
        );
        db.set_project_metadata(
            "extraction_warnings",
            &serde_json::json!(["Unread encrypted TOC"]),
        )
        .unwrap();
        let dir = recording_tempdir();
        let path = dir.join("partial-pivot.db");
        db.pivot_to(&path).unwrap();
        let pivot = Database::open(&path).unwrap();
        assert_eq!(
            pivot.get_project_metadata("extraction_warnings").unwrap(),
            Some(serde_json::json!(["Unread encrypted TOC"]))
        );
        pivot.merge_entries_preserving_missing(&[a]).unwrap();
        assert_eq!(pivot.get_entry("b").unwrap().unwrap().source, "English B");
        assert!(pivot
            .merge_entries_preserving_missing(&[make_entry("a", "changed")])
            .is_err());
        assert_eq!(pivot.get_entry("a").unwrap().unwrap().source, "English A");
        assert!(pivot.merge_entries(&[]).is_err());
        assert_eq!(db.merge_entries(&[]).unwrap().removed, 2);
    }

    #[test]
    fn project_metadata_roundtrips_reopen_and_update() {
        let dir = recording_tempdir();
        let path = dir.join("metadata.db");
        let db = Database::open(&path).unwrap();
        assert_eq!(db.get_project_metadata("missing").unwrap(), None);
        db.set_project_metadata("key", &serde_json::json!({"日本語": [1, false]}))
            .unwrap();
        drop(db);
        let db = Database::open(&path).unwrap();
        assert_eq!(
            db.get_project_metadata("key").unwrap(),
            Some(serde_json::json!({"日本語": [1, false]}))
        );
        db.set_project_metadata("key", &serde_json::Value::Null)
            .unwrap();
        assert_eq!(
            db.get_project_metadata("key").unwrap(),
            Some(serde_json::Value::Null)
        );
    }

    fn shared_textasset_rows(count: usize) -> (String, Vec<StringEntry>) {
        let mut original = String::new();
        let mut entries = Vec::new();
        for i in 0..count {
            let key = format!("Menu.K{i}");
            let value = format!("日本語の文章{i}{}", "あ".repeat(50));
            original.push_str(&format!("{key}: {value}\n"));
            let mut entry = make_entry(&format!("row{i:05}"), &value);
            entry.metadata.insert(
                "extraction_method".into(),
                serde_json::json!("textasset_loc_line"),
            );
            entry
                .metadata
                .insert("loc_key".into(), serde_json::json!(key));
            entry
                .metadata
                .insert("line_index".into(), serde_json::json!(i));
            entries.push(entry);
        }
        original.push_str("   ");
        crate::textasset_group::attach_to_entries(
            &mut entries,
            crate::textasset_group::GroupKind::LocLine,
            &original,
            original.len(),
            "shared-group",
        );
        (original, entries)
    }

    #[test]
    fn snapshot_to_preserves_shared_textasset_original() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(&dir.path().join("project.db")).unwrap();
        let (original, entries) = shared_textasset_rows(3);
        db.save_entries(&entries).unwrap();
        let expected = db.get_entries(&EntryFilter::default()).unwrap();
        let dest = dir.path().join("checkpoint's shared original.locust.db");

        db.snapshot_to(&dest).unwrap();

        let checkpoint = Database::open(&dest).unwrap();
        let loaded = checkpoint.get_entries(&EntryFilter::default()).unwrap();
        assert_eq!(
            serde_json::to_value(&loaded).unwrap(),
            serde_json::to_value(&expected).unwrap()
        );
        let head = loaded[0].textasset_original.as_ref().unwrap();
        for entry in &loaded {
            assert_eq!(
                crate::textasset_group::original_textasset(entry),
                Some(original.as_str())
            );
            assert!(Arc::ptr_eq(
                head,
                entry.textasset_original.as_ref().unwrap()
            ));
            assert!(crate::textasset_group::parse_group_meta(entry).is_ok());
        }
        let original_count: usize = lock_connection(&checkpoint.conn)
            .query_row("SELECT COUNT(*) FROM textasset_originals", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(original_count, 1);
    }

    #[test]
    fn readonly_snapshot_leaves_source_bytes_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("project.locust.db");
        let db = Database::open(&path).unwrap();
        db.save_entries(&[make_entry("a", "Hello")]).unwrap();
        drop(db);
        let before = std::fs::read(&path).unwrap();
        let wal = dir.path().join("project.locust.db-wal");
        let shm = dir.path().join("project.locust.db-shm");
        let wal_before = wal.is_file().then(|| std::fs::read(&wal).unwrap());
        let shm_before = shm.is_file().then(|| std::fs::read(&shm).unwrap());
        let dest = dir.path().join("copy.locust.db");
        Database::snapshot_existing(&path, &dest).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), before);
        let wal_after = wal.is_file().then(|| std::fs::read(&wal).unwrap());
        let shm_after = shm.is_file().then(|| std::fs::read(&shm).unwrap());
        assert_eq!(wal_after, wal_before);
        assert_eq!(shm_after, shm_before);
        let copied = Database::open(&dest).unwrap();
        assert_eq!(
            copied.get_entries(&EntryFilter::default()).unwrap().len(),
            1
        );
    }

    #[test]
    fn shared_textasset_database_large_roundtrip_has_one_blob_and_shared_memory() {
        let dir = recording_tempdir();
        let path = dir.join("shared.db");
        let (original, entries) = shared_textasset_rows(512);
        assert!(original.len() > 32_768);
        let db = Database::open(&path).unwrap();
        db.save_entries(&entries).unwrap();
        let conn = db.conn.lock().unwrap();
        let count: usize = conn
            .query_row("SELECT COUNT(*) FROM textasset_originals", [], |r| r.get(0))
            .unwrap();
        let metadata_size: usize = conn
            .query_row("SELECT MAX(length(metadata)) FROM strings", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(count, 1);
        assert!(metadata_size < 1024);
        drop(conn);
        drop(db);
        let db = Database::open(&path).unwrap();
        let loaded = db.get_entries(&EntryFilter::default()).unwrap();
        let head = loaded[0].textasset_original.as_ref().unwrap();
        for row in &loaded {
            assert!(Arc::ptr_eq(head, row.textasset_original.as_ref().unwrap()));
            assert_eq!(
                crate::textasset_group::original_textasset(row),
                Some(original.as_str())
            );
            assert!(crate::textasset_group::parse_group_meta(row).is_ok());
        }
        // JSON carries the digest only; saving back into its owning DB rehydrates.
        let serialized = serde_json::to_string(&loaded[0]).unwrap();
        let detached: StringEntry = serde_json::from_str(&serialized).unwrap();
        assert!(detached.textasset_original.is_none());
        db.save_entries(&[detached]).unwrap();
    }

    #[test]
    fn textasset_group_query_preserves_order_across_chunks_and_shared_originals() {
        let db = Database::open_in_memory().unwrap();
        let (original, mut entries) = shared_textasset_rows(601);
        for (i, entry) in entries.iter_mut().enumerate() {
            entry.metadata.insert(
                crate::textasset_group::GROUP_ID_KEY.into(),
                serde_json::json!(if i % 2 == 0 { "group-a'" } else { "group-b" }),
            );
        }
        db.save_entries(&entries).unwrap();
        let expected = db.get_entries(&EntryFilter::default()).unwrap();
        let actual = db
            .get_entries_for_textasset_groups(&HashSet::from(["group-b".into(), "group-a'".into()]))
            .unwrap();
        assert_eq!(
            serde_json::to_value(&actual).unwrap(),
            serde_json::to_value(&expected).unwrap()
        );
        let head = actual[0].textasset_original.as_ref().unwrap();
        for entry in &actual {
            assert!(Arc::ptr_eq(
                head,
                entry.textasset_original.as_ref().unwrap()
            ));
            assert_eq!(head.text(), original);
        }
        assert!(db
            .get_entries_for_textasset_groups(&HashSet::new())
            .unwrap()
            .is_empty());
        assert!(db
            .get_entries_for_textasset_groups(&HashSet::from(["absent".into()]))
            .unwrap()
            .is_empty());
    }

    #[test]
    fn textasset_group_query_rejects_corrupt_unselected_metadata() {
        let db = Database::open_in_memory().unwrap();
        let (_, entries) = shared_textasset_rows(3);
        db.save_entries(&entries).unwrap();
        db.save_entries(&[make_entry("unrelated", "Unrelated")])
            .unwrap();
        let selected = HashSet::from(["shared-group".into()]);
        let deep = format!("{{\"nested\":{}{}}}", "[".repeat(130), "]".repeat(130));
        for metadata in [
            "{",
            "[]",
            "null",
            "\"scalar\"",
            "{\"number\":1e9999}",
            &deep,
            "{\"textasset_group_original\":7}",
            "{\"textasset_group_original_ref\":7}",
            "{\"textasset_group_original_ref\":\"bad\"}",
            "{\"textasset_group_original_bytes\":1}",
        ] {
            db.conn
                .lock()
                .unwrap()
                .execute(
                    "UPDATE strings SET metadata=?1 WHERE id='unrelated'",
                    params![metadata],
                )
                .unwrap();
            assert!(
                db.get_entries(&EntryFilter::default()).is_err(),
                "{metadata}"
            );
            assert!(
                db.get_entries_for_textasset_groups(&selected).is_err(),
                "{metadata}"
            );
        }
        // serde uses the last duplicate key; SQLite json_extract uses the first.
        db.conn
            .lock()
            .unwrap()
            .execute(
                "UPDATE strings SET metadata=?1 WHERE id='unrelated'",
                params![
                    "{\"textasset_group_id\":\"other\",\"textasset_group_id\":\"shared-group\"}"
                ],
            )
            .unwrap();
        assert_eq!(
            db.get_entries_for_textasset_groups(&selected)
                .unwrap()
                .len(),
            4
        );
    }

    #[test]
    fn shared_textasset_partial_pivot_and_merge_preserve_physical_blob_without_leader() {
        let dir = recording_tempdir();
        let (original, mut entries) = shared_textasset_rows(256);
        // Only the final row is translated: the group's first row never enters
        // either pivot, so blob ownership cannot depend on a leader row.
        entries.last_mut().unwrap().translation = Some("English last row".into());
        let db = Database::open_in_memory().unwrap();
        db.save_entries(&entries).unwrap();
        let path = dir.join("pivot-one.db");
        db.pivot_to(&path).unwrap();
        let pivot = Database::open(&path).unwrap();
        let id = entries.last().unwrap().id.clone();
        let mut row = pivot.get_entry(&id).unwrap().unwrap();
        assert_eq!(row.source, "English last row");
        assert_eq!(
            crate::textasset_group::original_textasset(&row),
            Some(original.as_str())
        );
        assert_eq!(
            row.injection_source().unwrap(),
            entries.last().unwrap().source
        );
        row.translation = Some("Última fila española".into());
        pivot.save_entries(&[row]).unwrap();
        let path2 = dir.join("pivot-two.db");
        pivot.pivot_to(&path2).unwrap();
        let pivot2 = Database::open(&path2).unwrap();
        pivot2.merge_entries(&entries).unwrap();
        let row = pivot2.get_entry(&id).unwrap().unwrap();
        assert_eq!(row.source, "Última fila española");
        assert_eq!(
            crate::textasset_group::original_textasset(&row),
            Some(original.as_str())
        );
        let patch = crate::textasset_group::patch_from_entry(&row, "Texto final").unwrap();
        let meta = crate::textasset_group::parse_group_meta(&row).unwrap();
        let rebuilt = crate::textasset_group::apply_patches(&meta.original, meta.kind, &[patch]);
        assert!(rebuilt.outcomes[0].1.is_ok());
        assert!(rebuilt.text.starts_with("Menu.K0: 日本語"));
        assert!(rebuilt.text.contains("Menu.K255: Texto final"));
        // Change only an unselected row/padding, keeping the pivot row's source.
        let changed_original = original.replace("Menu.K0:", "Menu.X0:");
        crate::textasset_group::attach_to_entries(
            &mut entries,
            crate::textasset_group::GroupKind::LocLine,
            &changed_original,
            changed_original.len(),
            "shared-group",
        );
        assert!(pivot2
            .merge_entries(&entries)
            .unwrap_err()
            .to_string()
            .contains("source_changed"));
        assert_eq!(
            crate::textasset_group::original_textasset(&pivot2.get_entry(&id).unwrap().unwrap()),
            Some(original.as_str())
        );
    }

    #[test]
    fn shared_textasset_legacy_inline_database_is_readable_and_migrates_on_save() {
        use crate::textasset_group as group;
        let db = Database::open_in_memory().unwrap();
        let (original, entries) = shared_textasset_rows(2);
        db.save_entries(&entries).unwrap();
        let mut legacy_metadata = entries[0].metadata.clone();
        legacy_metadata.remove(group::GROUP_ORIGINAL_REF_KEY);
        legacy_metadata.remove(group::GROUP_ORIGINAL_BYTES_KEY);
        legacy_metadata.insert(
            group::GROUP_ORIGINAL_KEY.into(),
            serde_json::json!(original),
        );
        {
            let conn = db.conn.lock().unwrap();
            conn.execute(
                "UPDATE strings SET metadata=?1",
                params![serde_json::to_string(&legacy_metadata).unwrap()],
            )
            .unwrap();
            conn.execute("DELETE FROM textasset_originals", []).unwrap();
        }
        let loaded = db.get_entries(&EntryFilter::default()).unwrap();
        assert!(Arc::ptr_eq(
            loaded[0].textasset_original.as_ref().unwrap(),
            loaded[1].textasset_original.as_ref().unwrap()
        ));
        db.save_entries(&loaded).unwrap();
        let conn = db.conn.lock().unwrap();
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM textasset_originals", [], |r| r
                .get::<_, usize>(0))
                .unwrap(),
            1
        );
        assert!(!conn
            .query_row("SELECT metadata FROM strings LIMIT 1", [], |r| r
                .get::<_, String>(0))
            .unwrap()
            .contains("日本語"));
    }

    #[test]
    fn shared_textasset_corrupt_missing_and_oversized_blobs_fail_closed() {
        for attack in ["payload", "length", "missing", "oversized", "reference"] {
            let db = Database::open_in_memory().unwrap();
            let (_, entries) = shared_textasset_rows(2);
            db.save_entries(&entries).unwrap();
            {
                let conn = db.conn.lock().unwrap();
                match attack {
                    "payload" => {
                        conn.execute("UPDATE textasset_originals SET payload='corrupt'", [])
                            .unwrap();
                    }
                    "length" => {
                        conn.execute("UPDATE textasset_originals SET byte_len=1", [])
                            .unwrap();
                    }
                    "missing" => {
                        conn.execute("DELETE FROM textasset_originals", []).unwrap();
                    }
                    "oversized" => {
                        conn.execute(
                            "UPDATE textasset_originals SET payload=?1",
                            params!["x".repeat(1_048_577)],
                        )
                        .unwrap();
                    }
                    "reference" => {
                        conn.execute("UPDATE strings SET metadata=json_set(metadata, '$.textasset_group_original_ref', 'bad')", []).unwrap();
                    }
                    _ => unreachable!(),
                }
            }
            assert!(db.get_entry(&entries[0].id).is_err(), "{attack}");
            assert!(db.get_entries(&EntryFilter::default()).is_err(), "{attack}");
            assert!(
                db.get_entries_for_textasset_groups(&HashSet::from(["unselected".into()]))
                    .is_err(),
                "unselected original must still fail: {attack}"
            );
            if attack != "missing" && attack != "reference" {
                assert!(
                    db.save_entries(&entries).is_err(),
                    "save must not hide corrupt existing blob: {attack}"
                );
            }
        }
    }

    #[test]
    fn sha256_file_matches_in_memory_across_chunk_boundary() {
        // FILE_HASH_CHUNK is 1 MiB — force at least two reads.
        let dir = recording_tempdir();
        let path = dir.join("big.bin");
        let mut data = vec![0u8; FILE_HASH_CHUNK + 50_000];
        for (i, b) in data.iter_mut().enumerate() {
            *b = (i % 251) as u8;
        }
        std::fs::write(&path, &data).unwrap();
        let (hash, size) = sha256_file(&path).unwrap();
        assert_eq!(size, data.len() as u64);
        assert_eq!(hash, sha256_hex(&data));
        assert_eq!(sha256_path(&path).unwrap(), hash);
        // Negative: wrong length must not pass as equal to a truncated hash input.
        assert_ne!(hash, sha256_hex(&data[..data.len() - 1]));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn copy_path_chunked_roundtrip_multi_chunk() {
        let dir = recording_tempdir();
        let src = dir.join("src.bin");
        let dst = dir.join("dst.bin");
        let data = vec![0x5Au8; FILE_HASH_CHUNK + 12];
        std::fs::write(&src, &data).unwrap();
        let mut out = std::fs::File::create(&dst).unwrap();
        let n = copy_path_chunked(&src, &mut out).unwrap();
        drop(out);
        assert_eq!(n, data.len() as u64);
        assert_eq!(std::fs::read(&dst).unwrap(), data);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ─── injection recording (root + rel + hash per language) ──────────────

    fn recording_tempdir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("locust_db_rec_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A minimal injected tree: `<base>/root/game/script.rpy` with known bytes.
    fn make_recorded_tree(base: &Path) -> (PathBuf, PathBuf, Vec<u8>) {
        let root = base.join("root");
        let sub = root.join("game");
        std::fs::create_dir_all(&sub).unwrap();
        let file = sub.join("script.rpy");
        let bytes = b"label start:\n    \"Hola\"\n".to_vec();
        std::fs::write(&file, &bytes).unwrap();
        (root, file, bytes)
    }

    #[test]
    fn test_recorded_backup_refs_returns_one_per_language_with_a_backup() {
        let base = tempfile::tempdir().unwrap();
        let (root, file, _) = make_recorded_tree(base.path());
        let second = root.join("second.rpy");
        std::fs::write(&second, b"translated").unwrap();
        let files = [file, second];
        let db = Database::open_in_memory().unwrap();
        assert!(db.recorded_backup_refs().unwrap().is_empty());

        db.record_injection(Some("ja"), &root, &files).unwrap();
        assert!(db.recorded_backup_refs().unwrap().is_empty());

        let backup = RecordedBackup {
            id: "shared-backup".into(),
            source_path: root.clone(),
            storage_root: Some(base.path().join("backups")),
        };
        let legacy_backup = RecordedBackup {
            storage_root: None,
            ..backup.clone()
        };
        for lang in [Some("es"), Some("fr")] {
            db.record_injection_with_backup(lang, &root, &files, Some(&backup))
                .unwrap();
        }
        db.record_injection_with_backup(None, &root, &files, Some(&legacy_backup))
            .unwrap();

        assert_eq!(
            db.recorded_backup_refs().unwrap(),
            vec![
                (Some("es".into()), backup.clone()),
                (Some("fr".into()), backup),
                (None, legacy_backup),
            ]
        );
    }

    #[test]
    fn test_recorded_backup_refs_propagates_recording_decode_errors() {
        let base = tempfile::tempdir().unwrap();
        let (root, file, _) = make_recorded_tree(base.path());
        let second = root.join("second.rpy");
        std::fs::write(&second, b"translated").unwrap();
        let files = [file, second];
        for corruption in [
            "UPDATE injected_files SET pristine_backup = 'not json'",
            "UPDATE injected_files SET root = 'different' WHERE rel = 'second.rpy'",
        ] {
            let db = Database::open_in_memory().unwrap();
            db.record_injection(Some("es"), &root, &files).unwrap();
            lock_connection(&db.conn).execute(corruption, []).unwrap();
            let expected = db.get_injection(Some("es")).unwrap_err().to_string();
            assert_eq!(db.recorded_backup_refs().unwrap_err().to_string(), expected);
        }
    }

    #[test]
    fn test_record_injection_roundtrip_stores_rel_hash_and_size() {
        let base = recording_tempdir();
        let (root, file, bytes) = make_recorded_tree(&base);
        let db = Database::open_in_memory().unwrap();

        db.record_injection(Some("es"), &root, &[file]).unwrap();

        let rec = db
            .get_injection(Some("es"))
            .unwrap()
            .expect("a recording must exist for es");
        assert!(
            paths_identical(&rec.root, &root),
            "recorded root {} must name the injection root {}",
            rec.root.display(),
            root.display()
        );
        assert!(
            rec.root.is_absolute(),
            "the root must be absolutized at record time"
        );
        assert_eq!(rec.files.len(), 1);
        assert_eq!(
            rec.files[0].rel, "game/script.rpy",
            "rels are stored with forward slashes, relative to the root"
        );
        assert_eq!(rec.files[0].hash, sha256_hex(&bytes));
        assert_eq!(rec.files[0].size, bytes.len() as u64);
    }

    #[test]
    fn test_record_injection_many_files_all_persist() {
        // Pins the prepare_cached INSERT path: N rows in one transaction must
        // all round-trip (guards against accidentally emptying the insert loop).
        let base = recording_tempdir();
        let root = base.join("root");
        let sub = root.join("game");
        std::fs::create_dir_all(&sub).unwrap();
        let mut files = Vec::new();
        for i in 0..40 {
            let p = sub.join(format!("f{i}.rpy"));
            std::fs::write(&p, format!("line {i}")).unwrap();
            files.push(p);
        }
        let db = Database::open_in_memory().unwrap();
        db.record_injection(Some("es"), &root, &files).unwrap();
        let rec = db.get_injection(Some("es")).unwrap().unwrap();
        assert_eq!(rec.files.len(), 40);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn test_record_injection_null_key_is_its_own_recording() {
        // `--direct` without -l records under the reserved NULL key, matched
        // only by `patch` without -l — never silently by a named language.
        let base = recording_tempdir();
        let (root, file, _) = make_recorded_tree(&base);
        let db = Database::open_in_memory().unwrap();

        db.record_injection(None, &root, &[file]).unwrap();

        assert!(db.get_injection(None).unwrap().is_some());
        assert!(
            db.get_injection(Some("es")).unwrap().is_none(),
            "a named language must never match the language-unspecified recording"
        );
        assert_eq!(db.list_recorded_langs().unwrap(), vec![None]);
    }

    #[test]
    fn test_record_injection_replaces_only_its_own_key() {
        let base = recording_tempdir();
        let root = base.join("root");
        std::fs::create_dir_all(root.join("game")).unwrap();
        let a = root.join("game").join("a.rpy");
        let b = root.join("game").join("b.rpy");
        let c = root.join("game").join("c.rpy");
        for f in [&a, &b, &c] {
            std::fs::write(f, b"x").unwrap();
        }
        let db = Database::open_in_memory().unwrap();

        db.record_injection(Some("es"), &root, &[a]).unwrap();
        db.record_injection(Some("fr"), &root, &[b]).unwrap();
        db.record_injection(Some("es"), &root, &[c]).unwrap();

        let es = db.get_injection(Some("es")).unwrap().unwrap();
        assert_eq!(
            es.files.iter().map(|f| f.rel.as_str()).collect::<Vec<_>>(),
            vec!["game/c.rpy"],
            "a new recording replaces the previous one for the SAME key only"
        );
        let fr = db.get_injection(Some("fr")).unwrap().unwrap();
        assert_eq!(fr.files[0].rel, "game/b.rpy");
        assert_eq!(
            db.list_recorded_langs().unwrap(),
            vec![Some("es".to_string()), Some("fr".to_string())]
        );
    }

    #[test]
    fn test_record_injection_empty_report_keeps_previous_recording() {
        // An inject run that wrote nothing must not clobber the last good
        // recording — the files it previously wrote are still on disk.
        let base = recording_tempdir();
        let (root, file, _) = make_recorded_tree(&base);
        let db = Database::open_in_memory().unwrap();
        db.record_injection(Some("es"), &root, &[file]).unwrap();

        db.record_injection(Some("es"), &root, &[]).unwrap();

        let rec = db.get_injection(Some("es")).unwrap().unwrap();
        assert_eq!(rec.files[0].rel, "game/script.rpy");
    }

    #[test]
    fn test_record_injection_refuses_a_file_outside_the_root() {
        // Containment is checked at record time: a plugin that wrote into a
        // different tree (Unity/Unreal/Wolf/Ren'Py-loose in Replace mode) must
        // produce a hard error and record NOTHING — recording the paths would
        // silently re-create the packs-the-wrong-tree corruption.
        let base = recording_tempdir();
        let (root, file, _) = make_recorded_tree(&base);
        let outside = base.join("elsewhere.rpy");
        std::fs::write(&outside, b"y").unwrap();
        let db = Database::open_in_memory().unwrap();

        let err = db
            .record_injection(Some("es"), &root, &[file, outside.clone()])
            .expect_err("a write outside the root must refuse to record");
        let msg = err.to_string();
        assert!(
            msg.contains(&outside.display().to_string()),
            "error must name the escaping file: {msg}"
        );
        assert!(
            db.get_injection(Some("es")).unwrap().is_none(),
            "nothing may be recorded when any file escapes the root"
        );
    }

    #[test]
    fn test_record_injection_refuses_parent_dir_escape() {
        // Zip-slip guard at record time: a dot-dot path escaping the root
        // must refuse to record. On Windows `std::path::absolute` collapses
        // `..` lexically, so the CONTAINMENT branch catches it; on POSIX the
        // `..` survives resolution and the explicit `..` branch does. Either
        // way the refusal must come from the recording guards (shared
        // "cannot record injection output" contract), never from an
        // incidental read failure later on.
        let base = recording_tempdir();
        let (root, _file, _) = make_recorded_tree(&base);
        let sneaky = root.join("game").join("..").join("..").join("evil.txt");
        let db = Database::open_in_memory().unwrap();

        let err = db
            .record_injection(Some("es"), &root, &[sneaky])
            .expect_err("a dot-dot path escaping the root must refuse to record");
        assert!(
            err.to_string().contains("cannot record injection output"),
            "the refusal must come from the recording guards: {err}"
        );
        assert!(db.get_injection(Some("es")).unwrap().is_none());
    }

    #[cfg(any(windows, target_os = "macos"))]
    #[test]
    fn test_record_injection_dedupes_two_case_spellings_of_one_file() {
        // ONE physical file reached under two case spellings is a duplicate,
        // not a collision — NTFS/APFS fold case, so both spellings name the
        // same bytes and the file must be recorded exactly once.
        let base = recording_tempdir();
        let (root, file, _) = make_recorded_tree(&base);
        let respelled = root.join("game").join("SCRIPT.RPY");
        let db = Database::open_in_memory().unwrap();

        db.record_injection(Some("es"), &root, &[file, respelled])
            .unwrap();

        let rec = db.get_injection(Some("es")).unwrap().unwrap();
        assert_eq!(
            rec.files.len(),
            1,
            "one physical file must be recorded once"
        );
    }

    #[test]
    fn test_migration_drops_the_legacy_injected_files_table() {
        // The pre-recording table (file_path/lang, no root) cannot say which
        // tree injection targeted, so it is dropped on open; `locust patch`
        // then gives the exact inject command that rebuilds the recording.
        let base = recording_tempdir();
        let db_path = base.join("legacy.locust.db");
        {
            let conn = rusqlite::Connection::open(&db_path).unwrap();
            conn.execute_batch(
                "CREATE TABLE injected_files (
                     id INTEGER PRIMARY KEY AUTOINCREMENT,
                     file_path TEXT NOT NULL,
                     lang TEXT NOT NULL,
                     recorded_at TEXT NOT NULL
                 );
                 INSERT INTO injected_files (file_path, lang, recorded_at)
                 VALUES ('/g/game/a.rpy', 'es', '2026-01-01T00:00:00Z');",
            )
            .unwrap();
        }

        let db = Database::open(&db_path).unwrap();
        assert!(
            db.list_recorded_langs().unwrap().is_empty(),
            "legacy rows must be gone — they never recorded a root"
        );
        // And the new API works against the rebuilt table.
        let (root, file, _) = make_recorded_tree(&base);
        db.record_injection(Some("es"), &root, &[file]).unwrap();
        assert!(db.get_injection(Some("es")).unwrap().is_some());
    }

    #[test]
    fn test_migration_drops_an_injected_files_table_missing_any_expected_column() {
        // Keying the migration on a missing `root` alone under-detects: a
        // table WITH `root` but WITHOUT hash/size (an intermediate schema)
        // survives and then fails at runtime with a raw SQL error the first
        // time a recording is read or written.
        let base = recording_tempdir();
        let db_path = base.join("intermediate.locust.db");
        {
            let conn = rusqlite::Connection::open(&db_path).unwrap();
            conn.execute_batch(
                "CREATE TABLE injected_files (
                     id INTEGER PRIMARY KEY AUTOINCREMENT,
                     lang TEXT,
                     root TEXT NOT NULL,
                     rel TEXT NOT NULL,
                     recorded_at TEXT NOT NULL
                 );
                 INSERT INTO injected_files (lang, root, rel, recorded_at)
                 VALUES ('es', '/g/game', 'a.rpy', '2026-01-01T00:00:00Z');",
            )
            .unwrap();
        }

        let db = Database::open(&db_path).unwrap();
        assert!(
            db.list_recorded_langs().unwrap().is_empty(),
            "a table without the full column set must be rebuilt, not kept"
        );
        // The full round-trip works against the rebuilt table — this is what
        // raw-SQL-errored before the migration detected the column set.
        let (root, file, _) = make_recorded_tree(&base);
        db.record_injection(Some("es"), &root, &[file]).unwrap();
        let rec = db.get_injection(Some("es")).unwrap().unwrap();
        assert!(!rec.files[0].hash.is_empty());
    }

    // ─── path helpers backing the recording contract ────────────────────────

    #[test]
    fn test_rel_under_root_uses_forward_slashes() {
        let base = recording_tempdir();
        let (root, file, _) = make_recorded_tree(&base);
        assert_eq!(
            rel_under_root(&file, &root),
            Some("game/script.rpy".to_string())
        );
    }

    #[test]
    fn test_rel_under_root_resolves_relative_vs_absolute_spelling() {
        // The recording is absolutized at record time, but the file list a
        // plugin reports can be spelled relative when inject was invoked with
        // a relative game path. A purely lexical prefix match would fail.
        let cwd = std::env::current_dir().unwrap();
        let stored = cwd.join("mygame").join("Data").join("BasicData.wolf");
        assert_eq!(
            rel_under_root(&stored, Path::new("mygame")),
            Some("Data/BasicData.wolf".to_string())
        );
    }

    #[cfg(any(windows, target_os = "macos"))]
    #[test]
    fn test_rel_under_root_is_case_insensitive_where_the_filesystem_is() {
        assert_eq!(
            rel_under_root(
                Path::new("/games/MYGAME/Data/BasicData.wolf"),
                Path::new("/Games/MyGame")
            ),
            Some("Data/BasicData.wolf".to_string())
        );
    }

    #[cfg(windows)]
    #[test]
    fn test_rel_under_root_resolves_verbatim_prefix_spelling() {
        // `canonicalize()` yields `\\?\C:\...` verbatim paths on Windows. A
        // plainly spelled file must still match a verbatim-spelled root.
        let base = recording_tempdir();
        let (root, file, _) = make_recorded_tree(&base);
        let verbatim_root = root.canonicalize().unwrap();
        assert_eq!(
            rel_under_root(&file, &verbatim_root),
            Some("game/script.rpy".to_string())
        );
    }

    #[test]
    fn test_rel_under_root_is_none_outside_the_root_and_for_the_root_itself() {
        let base = recording_tempdir();
        let (root, _file, _) = make_recorded_tree(&base);
        assert_eq!(rel_under_root(&base.join("elsewhere.txt"), &root), None);
        assert_eq!(
            rel_under_root(&root, &root),
            None,
            "the root itself is not a file under the root"
        );
    }

    #[test]
    fn test_paths_identical_across_spellings() {
        let base = recording_tempdir();
        let (root, _file, _) = make_recorded_tree(&base);
        let canonical = root.canonicalize().unwrap();
        assert!(paths_identical(&root, &canonical));
        assert!(!paths_identical(&root, &base));
    }

    fn schema_tables(db: &Database) -> Vec<String> {
        let conn = db.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
            )
            .unwrap();
        stmt.query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
    }

    #[test]
    fn test_reopen_swaps_connection_and_preserves_original() {
        let base = recording_tempdir();
        let path_a = base.join("a.locust.db");
        let path_b = base.join("b.locust.db");

        let db = Database::open(&path_a).unwrap();
        assert_eq!(db.path(), path_a);
        db.save_entries(&[make_entry("keep-me", "Hello")]).unwrap();

        db.reopen(&path_b).unwrap();
        assert_eq!(db.path(), path_b);
        assert!(
            db.get_entries(&EntryFilter::default()).unwrap().is_empty(),
            "reopened path B must start empty"
        );
        let tables = schema_tables(&db);
        for required in [
            "strings",
            "glossary",
            "translation_memory",
            "validation_issues",
            "translation_runs",
            "injected_files",
        ] {
            assert!(
                tables.iter().any(|t| t == required),
                "reopened DB missing table {required}, have {tables:?}"
            );
        }

        db.reopen(&path_a).unwrap();
        assert_eq!(db.path(), path_a);
        let entries = db.get_entries(&EntryFilter::default()).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, "keep-me");
        assert_eq!(entries[0].source, "Hello");
    }

    #[tokio::test]
    async fn test_merge_entries_preserves_translations_and_status() {
        let db = Database::open_in_memory().unwrap();
        let mut first = make_entry("hero", "Hello");
        first.context = Some("old-ctx".into());
        db.save_entries(&[first, make_entry("gone", "Disappears")])
            .unwrap();
        assert!(db.save_translation("hero", "Hola", "mock").await.unwrap());
        db.update_entry_status("hero", StringStatus::Approved)
            .await
            .unwrap();

        let mut refreshed = make_entry("hero", "Hello");
        refreshed.context = Some("new-ctx".into());
        refreshed.file_path = PathBuf::from("data/Actors.json");
        refreshed.tags = vec!["ui".into()];
        let fresh = make_entry("mage", "Spell");
        let stats = db.merge_entries(&[refreshed, fresh]).unwrap();

        assert_eq!(stats.added, 1);
        assert_eq!(stats.updated, 1);
        assert_eq!(stats.removed, 1);
        assert_eq!(stats.stale_source_reset, 0);
        assert_eq!(stats.preserved_translations, 1);
        assert_eq!(stats.lost_translations, 0);

        let hero = db.get_entry("hero").unwrap().unwrap();
        assert_eq!(hero.translation.as_deref(), Some("Hola"));
        assert_eq!(hero.status, StringStatus::Approved);
        assert_eq!(hero.provider_used.as_deref(), Some("mock"));
        assert_eq!(hero.context.as_deref(), Some("new-ctx"));
        assert_eq!(hero.file_path, PathBuf::from("data/Actors.json"));
        assert_eq!(hero.tags, vec!["ui".to_string()]);
        assert!(hero.translated_at.is_some());

        assert!(db.get_entry("gone").unwrap().is_none());
        let mage = db.get_entry("mage").unwrap().unwrap();
        assert_eq!(mage.status, StringStatus::Pending);
        assert!(mage.translation.is_none());
    }

    #[test]
    fn merge_entries_writes_only_changed_rows() {
        let db = Database::open_in_memory().unwrap();
        let unchanged = make_entry("unchanged", "Hello");
        let mut changed = make_entry("changed", "Goodbye");
        db.save_entries(&[unchanged.clone(), changed.clone()])
            .unwrap();

        changed.context = Some("new context".into());
        let before: i64 = lock_connection(&db.conn)
            .query_row("SELECT total_changes()", [], |row| row.get(0))
            .unwrap();
        let stats = db
            .merge_entries(&[unchanged.clone(), changed.clone()])
            .unwrap();
        let after: i64 = lock_connection(&db.conn)
            .query_row("SELECT total_changes()", [], |row| row.get(0))
            .unwrap();
        assert_eq!(stats.updated, 1);
        assert_eq!(after - before, 1, "only the changed row should be written");

        let stats = db.merge_entries(&[unchanged, changed]).unwrap();
        let final_changes: i64 = lock_connection(&db.conn)
            .query_row("SELECT total_changes()", [], |row| row.get(0))
            .unwrap();
        assert_eq!(stats.updated, 0);
        assert_eq!(
            final_changes - after,
            0,
            "an unchanged reopen should write no rows"
        );
    }

    #[tokio::test]
    async fn test_merge_entries_stale_source_keeps_translation_resets_status() {
        let db = Database::open_in_memory().unwrap();
        db.save_entries(&[make_entry("npc", "Welcome")]).unwrap();
        assert!(db
            .save_translation("npc", "Bienvenido", "mock")
            .await
            .unwrap());
        db.update_entry_status("npc", StringStatus::Approved)
            .await
            .unwrap();

        let stats = db
            .merge_entries(&[make_entry("npc", "Welcome, traveler")])
            .unwrap();
        assert_eq!(stats.stale_source_reset, 1);
        assert_eq!(stats.updated, 1);
        assert_eq!(stats.preserved_translations, 1);
        assert_eq!(stats.lost_translations, 1);
        assert_eq!(stats.added, 0);
        assert_eq!(stats.removed, 0);

        let npc = db.get_entry("npc").unwrap().unwrap();
        assert_eq!(npc.source, "Welcome, traveler");
        assert_eq!(npc.translation.as_deref(), Some("Bienvenido"));
        assert_eq!(npc.status, StringStatus::Pending);
        assert_eq!(npc.provider_used.as_deref(), Some("mock"));
    }

    #[tokio::test]
    async fn merge_entries_counts_translations_lost_to_removal_or_pending_reset() {
        let db = Database::open_in_memory().unwrap();
        db.save_entries(&[
            make_entry("reset", "Hello"),
            make_entry("drop", "Goodbye"),
            make_entry("plain", "Stay"),
        ])
        .unwrap();
        assert!(db.save_translation("reset", "Hola", "mock").await.unwrap());
        assert!(db.save_translation("drop", "Adiós", "mock").await.unwrap());

        let stats = db
            .merge_entries(&[make_entry("reset", "Hello!"), make_entry("plain", "Stay")])
            .unwrap();
        assert_eq!(stats.stale_source_reset, 1);
        assert_eq!(stats.removed, 1);
        assert_eq!(stats.lost_translations, 2);
        assert_eq!(stats.preserved_translations, 1);

        let db = Database::open_in_memory().unwrap();
        db.save_entries(&[make_entry("reset", "Hello"), make_entry("keep", "Goodbye")])
            .unwrap();
        assert!(db.save_translation("reset", "Hola", "mock").await.unwrap());
        assert!(db.save_translation("keep", "Adiós", "mock").await.unwrap());
        let stats = db
            .merge_entries_preserving_missing(&[make_entry("reset", "Hello!")])
            .unwrap();
        assert_eq!(stats.removed, 0);
        assert_eq!(stats.stale_source_reset, 1);
        assert_eq!(stats.lost_translations, 1);
    }

    #[test]
    fn test_save_and_get_entries() {
        let db = Database::open_in_memory().unwrap();
        let entries = vec![
            make_entry("a", "Hello"),
            make_entry("b", "World"),
            make_entry("c", "Test"),
        ];
        let count = db.save_entries(&entries).unwrap();
        assert_eq!(count, 3);
        let all = db.get_entries(&EntryFilter::default()).unwrap();
        assert_eq!(all.len(), 3);
    }

    #[test]
    fn test_save_entries_deduplication() {
        let db = Database::open_in_memory().unwrap();
        db.save_entries(&[make_entry("dup", "First")]).unwrap();
        db.save_entries(&[make_entry("dup", "Second")]).unwrap();
        let all = db.get_entries(&EntryFilter::default()).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].source, "Second");
    }

    #[test]
    fn test_filter_by_status() {
        let db = Database::open_in_memory().unwrap();
        let mut translated = make_entry("t1", "Translated one");
        translated.status = StringStatus::Translated;
        db.save_entries(&[
            make_entry("p1", "Pending one"),
            make_entry("p2", "Pending two"),
            translated,
        ])
        .unwrap();
        let filter = EntryFilter {
            status: Some(StringStatus::Pending),
            ..Default::default()
        };
        let results = db.get_entries(&filter).unwrap();
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn test_filter_by_search() {
        let db = Database::open_in_memory().unwrap();
        db.save_entries(&[make_entry("s1", "hello world"), make_entry("s2", "goodbye")])
            .unwrap();
        let filter = EntryFilter {
            search: Some("hello".to_string()),
            ..Default::default()
        };
        let results = db.get_entries(&filter).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "s1");
    }

    #[test]
    fn test_filter_limit_offset() {
        let db = Database::open_in_memory().unwrap();
        let entries: Vec<StringEntry> = (0..5)
            .map(|i| make_entry(&format!("e{}", i), &format!("Entry {}", i)))
            .collect();
        db.save_entries(&entries).unwrap();
        let filter = EntryFilter {
            limit: Some(2),
            offset: Some(2),
            ..Default::default()
        };
        let results = db.get_entries(&filter).unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].id, "e2");
        assert_eq!(results[1].id, "e3");
    }

    #[test]
    fn test_get_entries_page_strings_query_count() {
        let db = Database::open_in_memory().unwrap();
        db.save_entries(&[
            make_entry("c", "hello three"),
            make_entry("a", "hello one"),
            make_entry("b", "hello two"),
            make_entry("d", "unrelated"),
        ])
        .unwrap();
        for (search, limit, offset, expected_len, expected_total, expected_queries) in [
            ("hello", 1, 1, 1, 3, 1),
            ("hello", 1, 3, 0, 3, 2),
            ("hello", 1, 30, 0, 3, 2),
            ("missing", 1, 0, 0, 0, 2),
            ("hello", 0, 0, 0, 3, 2),
        ] {
            let filter = EntryFilter {
                search: Some(search.into()),
                limit: Some(limit),
                offset: Some(offset),
                ..Default::default()
            };
            STRINGS_QUERIES.with(|count| count.set(Some(0)));
            let result = db.get_entries_page(&filter);
            let queries = STRINGS_QUERIES.with(|count| count.replace(None));
            let (entries, total) = result.unwrap();
            assert_eq!(
                queries,
                Some(expected_queries),
                "strings query count for {filter:?}"
            );
            assert_eq!(entries.len(), expected_len, "{filter:?}");
            assert_eq!(total, expected_total, "{filter:?}");
        }
    }

    #[test]
    fn test_get_entries_page_matches_entries_and_count() {
        let db = Database::open_in_memory().unwrap();
        let mut a = make_entry("a", "source-only needle");
        a.file_path = PathBuf::from("data/menu's.json");
        a.tags = vec!["ui".into(), "dialogue".into()];
        let mut b = make_entry("b", "Other source");
        b.translation = Some("translation-only needle".into());
        b.status = StringStatus::Translated;
        b.file_path = PathBuf::from("data/other.json");
        b.tags = vec!["ui_label".into()];
        let mut c = a.clone();
        c.id = "c".into();
        c.source = "Another needle".into();
        c.translation = Some("Listo".into());
        c.status = StringStatus::Translated;
        c.context = Some("Menu title".into());
        c.char_limit = Some(42);
        c.provider_used = Some("test-provider".into());
        c.translated_at = Some(Utc::now());
        c.reviewed_at = Some(Utc::now());
        c.metadata
            .insert("custom".into(), serde_json::json!([1, "two"]));
        let mut d = make_entry("d", "Unrelated");
        d.status = StringStatus::Approved;
        db.save_entries(&[d, c, b, a]).unwrap();

        let cases: Vec<(EntryFilter, &[&str], usize)> = vec![
            (EntryFilter::default(), &["a", "b", "c", "d"], 4),
            (
                EntryFilter {
                    status: Some(StringStatus::Translated),
                    ..Default::default()
                },
                &["b", "c"],
                2,
            ),
            (
                EntryFilter {
                    file_path: Some("data/menu's.json".into()),
                    ..Default::default()
                },
                &["a", "c"],
                2,
            ),
            (
                EntryFilter {
                    tag: Some("ui".into()),
                    ..Default::default()
                },
                &["a", "c"],
                2,
            ),
            (
                EntryFilter {
                    search: Some("source-only".into()),
                    ..Default::default()
                },
                &["a"],
                1,
            ),
            (
                EntryFilter {
                    search: Some("translation-only".into()),
                    ..Default::default()
                },
                &["b"],
                1,
            ),
            (
                EntryFilter {
                    search: Some("missing".into()),
                    ..Default::default()
                },
                &[],
                0,
            ),
            (
                EntryFilter {
                    limit: Some(2),
                    offset: Some(1),
                    ..Default::default()
                },
                &["b", "c"],
                4,
            ),
            (
                EntryFilter {
                    limit: Some(2),
                    offset: Some(4),
                    ..Default::default()
                },
                &[],
                4,
            ),
            (
                EntryFilter {
                    limit: Some(2),
                    offset: Some(9),
                    ..Default::default()
                },
                &[],
                4,
            ),
            (
                EntryFilter {
                    offset: Some(2),
                    ..Default::default()
                },
                &["c", "d"],
                4,
            ),
            (
                EntryFilter {
                    limit: Some(0),
                    ..Default::default()
                },
                &[],
                4,
            ),
            (
                EntryFilter {
                    search: Some("needle".into()),
                    limit: Some(1),
                    offset: Some(1),
                    ..Default::default()
                },
                &["b"],
                3,
            ),
            (
                EntryFilter {
                    search: Some("needle".into()),
                    limit: Some(1),
                    offset: Some(3),
                    ..Default::default()
                },
                &[],
                3,
            ),
            (
                EntryFilter {
                    search: Some("needle".into()),
                    offset: Some(9),
                    ..Default::default()
                },
                &[],
                3,
            ),
            (
                EntryFilter {
                    search: Some("source%only".into()),
                    ..Default::default()
                },
                &["a"],
                1,
            ),
            (
                EntryFilter {
                    search: Some(String::new()),
                    ..Default::default()
                },
                &["a", "b", "c", "d"],
                4,
            ),
            (
                EntryFilter {
                    status: Some(StringStatus::Translated),
                    file_path: Some("data/menu's.json".into()),
                    tag: Some("ui".into()),
                    search: Some("needle".into()),
                    limit: Some(1),
                    offset: Some(0),
                },
                &["c"],
                1,
            ),
        ];
        for (filter, ids, expected_total) in cases {
            let expected_entries = db.get_entries(&filter).unwrap();
            let count = db.count_entries(&filter).unwrap();
            let (entries, total) = db.get_entries_page(&filter).unwrap();
            assert_eq!(total, count, "{filter:?}");
            assert_eq!(total, expected_total, "{filter:?}");
            assert_eq!(
                entries
                    .iter()
                    .map(|entry| entry.id.as_str())
                    .collect::<Vec<_>>(),
                ids,
                "{filter:?}"
            );
            assert_eq!(
                serde_json::to_value(&entries).unwrap(),
                serde_json::to_value(&expected_entries).unwrap(),
                "{filter:?}"
            );
        }
        let empty = Database::open_in_memory().unwrap();
        let (entries, total) = empty.get_entries_page(&EntryFilter::default()).unwrap();
        assert!(entries.is_empty());
        assert_eq!(total, 0);
    }

    #[test]
    fn test_get_entries_page_preserves_shared_originals_and_errors() {
        let db = Database::open_in_memory().unwrap();
        let (original, entries) = shared_textasset_rows(4);
        db.save_entries(&entries).unwrap();
        let filter = EntryFilter {
            limit: Some(2),
            offset: Some(1),
            ..Default::default()
        };
        let expected = db.get_entries(&filter).unwrap();
        let (page, total) = db.get_entries_page(&filter).unwrap();
        assert_eq!(total, 4);
        assert_eq!(page.len(), 2);
        assert_eq!(
            serde_json::to_value(&page).unwrap(),
            serde_json::to_value(&expected).unwrap()
        );
        let head = page[0].textasset_original.as_ref().unwrap();
        for entry in &page {
            let restored = entry.textasset_original.as_ref().unwrap();
            assert!(Arc::ptr_eq(head, restored));
            assert_eq!(restored.text(), original);
        }
        for metadata in ["{", r#"{"textasset_group_original_ref":"missing"}"#] {
            lock_connection(&db.conn)
                .execute(
                    "UPDATE strings SET metadata = ?1 WHERE id = ?2",
                    params![metadata, page[0].id],
                )
                .unwrap();
            assert_eq!(
                db.get_entries_page(&filter).unwrap_err().to_string(),
                db.get_entries(&filter).unwrap_err().to_string()
            );
        }
    }

    #[tokio::test]
    async fn test_save_translation_updates_status() {
        let db = Database::open_in_memory().unwrap();
        db.save_entries(&[make_entry("tr1", "Hello")]).unwrap();
        assert!(db
            .save_translation("tr1", "Hola", "test-provider")
            .await
            .unwrap());
        let entry = db.get_entry("tr1").unwrap().unwrap();
        assert_eq!(entry.translation, Some("Hola".to_string()));
        assert_eq!(entry.status, StringStatus::Translated);
        assert_eq!(entry.provider_used, Some("test-provider".to_string()));
    }

    #[tokio::test]
    async fn test_save_translation_unknown_id_returns_false() {
        let db = Database::open_in_memory().unwrap();
        assert!(!db
            .save_translation("missing", "Hola", "import")
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn test_save_translations_batch_applies_known_skips_unknown() {
        let db = Database::open_in_memory().unwrap();
        db.save_entries(&[make_entry("a", "Hello"), make_entry("b", "World")])
            .unwrap();
        let applied = db
            .save_translations_batch(
                vec![
                    ("a".into(), "Hola".into()),
                    ("missing".into(), "X".into()),
                    ("b".into(), "Mundo".into()),
                ],
                "batch",
            )
            .await
            .unwrap();
        assert_eq!(applied, 2);
        assert_eq!(
            db.get_entry("a").unwrap().unwrap().translation.as_deref(),
            Some("Hola")
        );
        assert_eq!(
            db.get_entry("b").unwrap().unwrap().translation.as_deref(),
            Some("Mundo")
        );
        assert!(db.get_entry("missing").unwrap().is_none());
    }

    #[tokio::test]
    async fn guarded_batch_preserves_intervening_edits_and_applies_other_rows() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("project.db");
        let db = Database::open(&path).unwrap();
        let desktop = Database::open(&path).unwrap();
        db.save_entries(&[make_entry("a", "A"), make_entry("b", "B")])
            .unwrap();
        db.save_translation("a", "old", "initial").await.unwrap();
        let expected = db.get_entry("a").unwrap().unwrap().translation;
        desktop
            .save_translation("a", "desktop edit", "manual")
            .await
            .unwrap();
        desktop
            .update_entry_status("a", StringStatus::Approved)
            .await
            .unwrap();
        let before = serde_json::to_value(db.get_entry("a").unwrap().unwrap()).unwrap();
        let report = db
            .save_translations_batch_if_unchanged(
                vec![
                    TranslationBatchItem {
                        id: "a".into(),
                        translation: "stale replacement".into(),
                        expected_translation: Some(expected),
                    },
                    TranslationBatchItem {
                        id: "b".into(),
                        translation: "filled".into(),
                        expected_translation: Some(None),
                    },
                    TranslationBatchItem {
                        id: "missing".into(),
                        translation: "unknown".into(),
                        expected_translation: Some(None),
                    },
                ],
                "batch",
            )
            .await
            .unwrap();
        assert_eq!(
            serde_json::to_value(db.get_entry("a").unwrap().unwrap()).unwrap(),
            before
        );
        assert_eq!(
            serde_json::to_value(report).unwrap(),
            serde_json::json!({
                "requested":3, "applied":1, "skipped":2, "conflicts":["a"]
            })
        );
        let saved = db.get_entry("b").unwrap().unwrap();
        assert_eq!(saved.translation.as_deref(), Some("filled"));
        assert_eq!(saved.provider_used.as_deref(), Some("batch"));
        assert_eq!(saved.status, StringStatus::Translated);
    }

    #[tokio::test]
    async fn guarded_batch_distinguishes_absent_null_and_empty_guards() {
        let db = Database::open_in_memory().unwrap();
        db.save_entries(&[
            make_entry("null", "A"),
            make_entry("empty", "B"),
            make_entry("legacy", "C"),
        ])
        .unwrap();
        db.save_translation("empty", "", "manual").await.unwrap();
        db.save_translation("legacy", "desktop edit", "manual")
            .await
            .unwrap();
        let updates = serde_json::from_value(serde_json::json!([
            {"id":"empty", "translation":"stale", "expected_translation":null},
            {"id":"null", "translation":"stale", "expected_translation":""},
            {"id":"legacy", "translation":"unconditional"},
            {"id":"empty", "translation":"filled empty", "expected_translation":""},
            {"id":"null", "translation":"filled null", "expected_translation":null}
        ]))
        .unwrap();
        let report = db
            .save_translations_batch_if_unchanged(updates, "batch")
            .await
            .unwrap();
        assert_eq!(report.conflicts, ["empty", "null"]);
        assert_eq!(
            (report.requested, report.applied, report.skipped),
            (5, 3, 2)
        );
        for (id, expected) in [
            ("legacy", "unconditional"),
            ("empty", "filled empty"),
            ("null", "filled null"),
        ] {
            assert_eq!(
                db.get_entry(id).unwrap().unwrap().translation.as_deref(),
                Some(expected)
            );
        }
    }

    #[tokio::test]
    async fn guarded_batch_rolls_back_on_error() {
        let db = Database::open_in_memory().unwrap();
        db.save_entries(&[make_entry("a", "A"), make_entry("b", "B")])
            .unwrap();
        lock_connection(&db.conn)
            .execute_batch(
                "CREATE TRIGGER reject_second BEFORE UPDATE ON strings
            WHEN NEW.id = 'b' BEGIN SELECT RAISE(ABORT, 'fixture failure'); END;",
            )
            .unwrap();
        let updates = ["a", "b"]
            .into_iter()
            .map(|id| TranslationBatchItem {
                id: id.into(),
                translation: "filled".into(),
                expected_translation: Some(None),
            })
            .collect();
        assert!(db
            .save_translations_batch_if_unchanged(updates, "batch")
            .await
            .is_err());
        for id in ["a", "b"] {
            assert!(db.get_entry(id).unwrap().unwrap().translation.is_none());
        }
    }

    #[tokio::test]
    async fn test_export_import_po_multi_hash_id_through_db() {
        use crate::export::{export_po, import_po};
        use crate::models::StringEntry;
        use std::path::PathBuf;

        let db = Database::open_in_memory().unwrap();
        let id = "S004b.ks.json#0#message";
        let mut entry =
            StringEntry::new(id, "Hello there", PathBuf::from(r"C:\work\S004b.ks.json"));
        entry.translation = Some("PLACEHOLDER".to_string());
        db.save_entries(&[entry]).unwrap();

        // External CAT tool "edits" the PO.
        let mut e = db.get_entry(id).unwrap().unwrap();
        e.translation = Some("Hola alli".to_string());
        let po = export_po(std::slice::from_ref(&e), "en", "es");
        let imported = import_po(&po).unwrap();
        assert_eq!(imported[0].id.as_deref(), Some(id));

        let (updates, pre_skipped) = crate::export::po_entries_for_batch(&imported);
        let attempted = updates.len();
        let report = db.save_imported_translations_batch(updates).await.unwrap();
        let (imported_n, missed) =
            crate::export::import_counts_after_batch(pre_skipped, attempted, report.imported);
        assert_eq!(imported_n, 1);
        assert_eq!(missed, 0);
        let again = db.get_entry(id).unwrap().unwrap();
        assert_eq!(again.translation.as_deref(), Some("Hola alli"));
        assert_eq!(again.status, StringStatus::Translated);
    }

    fn import_rows_bytes(db: &Database) -> Vec<u8> {
        // JSON values sort metadata keys, so fresh HashMaps serialize identically.
        let rows = serde_json::to_value(db.get_entries(&EntryFilter::default()).unwrap()).unwrap();
        serde_json::to_vec(&rows).unwrap()
    }

    fn identical_import_catalog() -> (Database, Vec<crate::export::ImportedTranslation>) {
        let db = Database::open_in_memory().unwrap();
        let entries: Vec<_> = (0..2_000)
            .rev()
            .map(|i| reviewed_import_entry(&format!("row{i:04}"), "Hello"))
            .collect();
        db.save_entries(&entries).unwrap();
        let updates = entries
            .into_iter()
            .map(|entry| crate::export::ImportedTranslation {
                id: entry.id,
                source: entry.source,
                translation: entry.translation.unwrap(),
            })
            .collect();
        (db, updates)
    }

    #[test]
    fn import_preview_reads_at_most_four_chunks_for_2000_identical_rows() {
        let (db, updates) = identical_import_catalog();
        let before = import_rows_bytes(&db);
        IMPORT_READ_QUERIES.with(|count| count.set(0));
        let preview = db.preview_imported_translations(&updates, false).unwrap();
        let reads = IMPORT_READ_QUERIES.with(|count| count.replace(0));
        assert!(
            (1..=4).contains(&reads),
            "preview issued {reads} SELECTs against strings for 2000 rows; expected at most 4"
        );
        assert_eq!(
            serde_json::to_value(preview).unwrap(),
            serde_json::json!({
                "report": {
                    "imported": 2000, "unchanged": 2000, "kept_existing": 0,
                    "stale_sources": 0, "unknown_ids": 0,
                },
                "replacements": [], "confirmations": [], "fill_ids": [], "kept_ids": [],
                "unchanged_ids": updates.iter().map(|entry| &entry.id).collect::<Vec<_>>(),
            })
        );
        assert_eq!(import_rows_bytes(&db), before);
    }

    #[tokio::test]
    async fn import_save_reads_at_most_four_chunks_for_2000_identical_rows() {
        let (db, updates) = identical_import_catalog();
        let before = import_rows_bytes(&db);
        IMPORT_READ_QUERIES.with(|count| count.set(0));
        let report = db
            .save_imported_translations_batch_with(updates, false)
            .await
            .unwrap();
        let reads = IMPORT_READ_QUERIES.with(|count| count.replace(0));
        assert!(
            (1..=4).contains(&reads),
            "save issued {reads} SELECTs against strings for 2000 rows; expected at most 4"
        );
        assert_eq!(
            serde_json::to_value(report).unwrap(),
            serde_json::json!({
                "imported": 2000, "unchanged": 2000, "kept_existing": 0,
                "stale_sources": 0, "unknown_ids": 0,
            })
        );
        assert_eq!(import_rows_bytes(&db), before);
    }

    #[tokio::test]
    async fn import_reads_all_1201_rows_across_chunk_boundaries_in_catalog_order() {
        use crate::export::ImportedTranslation;

        let db = Database::open_in_memory().unwrap();
        let updates: Vec<_> = (0..1_201)
            .rev()
            .map(|i| ImportedTranslation {
                // Quotes and punctuation must remain bound parameters.
                id: format!("row'{i:04},?"),
                source: format!("Source {i}"),
                translation: format!("Translation {i}"),
            })
            .collect();
        db.save_entries(
            &updates
                .iter()
                .map(|entry| make_entry(&entry.id, &entry.source))
                .collect::<Vec<_>>(),
        )
        .unwrap();
        let expected_report = serde_json::json!({
            "imported": 1201, "unchanged": 0, "kept_existing": 0,
            "stale_sources": 0, "unknown_ids": 0,
        });
        IMPORT_READ_QUERIES.with(|count| count.set(0));
        let preview = db.preview_imported_translations(&updates, true).unwrap();
        assert_eq!(IMPORT_READ_QUERIES.with(|count| count.replace(0)), 3);
        assert_eq!(
            serde_json::to_value(preview).unwrap(),
            serde_json::json!({
                "report": expected_report,
                "replacements": [], "confirmations": [], "kept_ids": [], "unchanged_ids": [],
                "fill_ids": updates.iter().map(|entry| &entry.id).collect::<Vec<_>>(),
            })
        );
        let report = db
            .save_imported_translations_batch_with(updates.clone(), true)
            .await
            .unwrap();
        assert_eq!(IMPORT_READ_QUERIES.with(|count| count.replace(0)), 3);
        assert_eq!(serde_json::to_value(report).unwrap(), expected_report);
        for update in &updates {
            let saved = db.get_entry(&update.id).unwrap().unwrap();
            assert_eq!(saved.source, update.source);
            assert_eq!(saved.translation.as_ref(), Some(&update.translation));
            assert_eq!(saved.status, StringStatus::Translated);
            assert_eq!(saved.provider_used.as_deref(), Some("import"));
        }
    }

    #[tokio::test]
    async fn import_save_does_not_count_ignored_writes() {
        let db = Database::open_in_memory().unwrap();
        let mut confirm = reviewed_import_entry("confirm", "Hello again");
        confirm.status = StringStatus::Pending;
        let skipped = [
            make_entry("fill", "Goodbye"),
            confirm,
            reviewed_import_entry("replace", "Welcome"),
        ];
        db.save_entries(&skipped).unwrap();
        db.save_entries(&[make_entry("saved", "Thanks")]).unwrap();
        lock_connection(&db.conn)
            .execute_batch(
                "CREATE TRIGGER ignore_import BEFORE UPDATE ON strings
                 WHEN OLD.id IN ('fill', 'confirm', 'replace')
                 BEGIN SELECT RAISE(IGNORE); END;",
            )
            .unwrap();
        let updates = [
            ("fill", "Goodbye", "Adiós"),
            ("confirm", "Hello again", "Hola"),
            ("replace", "Welcome", "Bienvenido"),
            ("saved", "Thanks", "Gracias"),
        ]
        .into_iter()
        .map(
            |(id, source, translation)| crate::export::ImportedTranslation {
                id: id.into(),
                source: source.into(),
                translation: translation.into(),
            },
        )
        .collect();
        let report = db
            .save_imported_translations_batch_with(updates, false)
            .await
            .unwrap();
        assert_eq!(
            serde_json::to_value(report).unwrap(),
            serde_json::json!({
                "imported": 1, "unchanged": 0, "kept_existing": 0,
                "stale_sources": 0, "unknown_ids": 0,
            })
        );
        for before in skipped {
            assert_eq!(
                serde_json::to_value(db.get_entry(&before.id).unwrap().unwrap()).unwrap(),
                serde_json::to_value(before).unwrap(),
            );
        }
        assert_eq!(
            db.get_entry("saved")
                .unwrap()
                .unwrap()
                .translation
                .as_deref(),
            Some("Gracias")
        );
    }

    #[tokio::test]
    async fn import_preview_matches_apply_without_changing_any_rows() {
        use crate::export::ImportedTranslation;

        for keep_existing in [false, true] {
            let db = Database::open_in_memory().unwrap();
            let mut confirm = reviewed_import_entry("confirm", "Hello again");
            confirm.status = StringStatus::Pending;
            let mut blank = make_entry("blank", "Thanks");
            blank.translation = Some(" \t\r\n\u{2003}".into());
            let mut same = reviewed_import_entry("same", "Hello");
            same.status = StringStatus::Translated;
            db.save_entries(&[
                same,
                confirm,
                make_entry("empty", "Goodbye"),
                blank,
                reviewed_import_entry("different", "Welcome"),
                reviewed_import_entry("stale", "Current source"),
                reviewed_import_entry("untouched", "Outside the catalog"),
            ])
            .unwrap();
            let updates: Vec<_> = [
                ("unknown", "Unknown", "Missing"),
                ("stale", "Old source", "Hola"),
                ("same", "Hello", "Hola"),
                ("confirm", "Hello again", "Hola"),
                ("empty", "Goodbye", "Adiós"),
                ("blank", "Thanks", "Gracias"),
                ("different", "Welcome", "Bienvenido"),
            ]
            .into_iter()
            .map(|(id, source, translation)| ImportedTranslation {
                id: id.into(),
                source: source.into(),
                translation: translation.into(),
            })
            .collect();
            let before = import_rows_bytes(&db);
            lock_connection(&db.conn)
                .execute_batch("PRAGMA query_only = ON")
                .unwrap();

            let preview = db
                .preview_imported_translations(&updates, keep_existing)
                .unwrap();

            assert_eq!(import_rows_bytes(&db), before);
            lock_connection(&db.conn)
                .execute_batch("PRAGMA query_only = OFF")
                .unwrap();
            let applied = db
                .save_imported_translations_batch_with(updates, keep_existing)
                .await
                .unwrap();
            assert_eq!(
                serde_json::to_value(&preview.report).unwrap(),
                serde_json::to_value(&applied).unwrap(),
                "preview counts must match the following real import (keep_existing={keep_existing})"
            );
            assert_eq!(
                serde_json::to_value(&preview.report).unwrap(),
                serde_json::json!({
                    "imported": if keep_existing { 4 } else { 5 },
                    "unchanged": 1,
                    "kept_existing": usize::from(keep_existing),
                    "stale_sources": 1,
                    "unknown_ids": 1,
                })
            );
            assert_eq!(preview.fill_ids, ["empty", "blank"]);
            assert_eq!(preview.unchanged_ids, ["same"]);
            assert_eq!(preview.confirmations.len(), 1);
            let confirmation = &preview.confirmations[0];
            assert_eq!(confirmation.id, "confirm");
            assert_eq!(confirmation.previous, "Hola");
            assert_eq!(confirmation.previous_status, "pending");
            assert_eq!(confirmation.new_text, "Hola");
            if keep_existing {
                assert!(preview.replacements.is_empty());
                assert_eq!(preview.kept_ids, ["different"]);
            } else {
                assert!(preview.kept_ids.is_empty());
                assert_eq!(preview.replacements.len(), 1);
                let replacement = &preview.replacements[0];
                assert_eq!(replacement.id, "different");
                assert_eq!(replacement.previous, "Hola");
                assert_eq!(replacement.previous_status, "approved");
                assert_eq!(replacement.new_text, "Bienvenido");
            }
        }
    }

    #[tokio::test]
    async fn import_preview_refuses_duplicate_ids_and_accepts_empty_batch() {
        use crate::export::ImportedTranslation;

        let db = Database::open_in_memory().unwrap();
        db.save_entries(&[reviewed_import_entry("same", "Hello")])
            .unwrap();
        let before = import_rows_bytes(&db);
        for keep_existing in [false, true] {
            let update = ImportedTranslation {
                id: "same".into(),
                source: "Hello".into(),
                translation: "New translation".into(),
            };
            let updates = vec![update.clone(), update];
            let preview_error = db
                .preview_imported_translations(&updates, keep_existing)
                .unwrap_err();
            let save_error = db
                .save_imported_translations_batch_with(updates, keep_existing)
                .await
                .unwrap_err();
            assert!(preview_error.to_string().contains("duplicate import id"));
            assert_eq!(preview_error.to_string(), save_error.to_string());
            assert_eq!(
                serde_json::to_value(
                    db.preview_imported_translations(&[], keep_existing)
                        .unwrap()
                )
                .unwrap(),
                serde_json::to_value(ImportPreview::default()).unwrap()
            );
            assert_eq!(import_rows_bytes(&db), before);
        }
    }

    #[tokio::test]
    async fn import_identical_translation_keeps_approval() {
        use crate::export::ImportedTranslation;

        for keep_existing in [false, true] {
            for status in [
                StringStatus::Approved,
                StringStatus::Reviewed,
                StringStatus::Translated,
            ] {
                let db = Database::open_in_memory().unwrap();
                let mut entry = reviewed_import_entry("same", "Hello");
                entry.status = status;
                db.save_entries(&[entry]).unwrap();
                let before = db.get_entry("same").unwrap().unwrap();
                let updates = vec![ImportedTranslation {
                    id: before.id.clone(),
                    source: before.source.clone(),
                    translation: before.translation.clone().unwrap(),
                }];
                let report = if keep_existing {
                    db.save_imported_translations_batch_with(updates, true)
                        .await
                } else {
                    db.save_imported_translations_batch(updates).await
                }
                .unwrap();

                let after = db.get_entry("same").unwrap().unwrap();
                assert_eq!(after.status, before.status);
                assert_eq!(after.provider_used, before.provider_used);
                assert_eq!(after.translated_at, before.translated_at);
                assert_eq!(after.reviewed_at, before.reviewed_at);
                assert_eq!(
                    serde_json::to_value(&after).unwrap(),
                    serde_json::to_value(&before).unwrap()
                );
                assert_eq!(report.imported, 1);
                assert_eq!(report.unchanged, 1);
                assert_eq!(report.kept_existing, 0);
                assert_eq!(report.stale_sources, 0);
                assert_eq!(report.unknown_ids, 0);
            }
        }
    }

    #[tokio::test]
    async fn import_identical_translation_confirms_pending_stale_row() {
        use crate::export::ImportedTranslation;

        for keep_existing in [false, true] {
            let db = Database::open_in_memory().unwrap();
            let mut entry = make_entry("stale", "Hello");
            entry.translation = Some("Hola".into());
            entry.status = StringStatus::Translated;
            entry.provider_used = Some("original-provider".into());
            entry.translated_at = Some("2025-01-01T00:00:00Z".parse().unwrap());
            db.save_entries(&[entry]).unwrap();
            let stats = db
                .merge_entries(&[make_entry("stale", "Hello again")])
                .unwrap();
            assert_eq!(stats.stale_source_reset, 1);
            let before = db.get_entry("stale").unwrap().unwrap();
            assert_eq!(before.status, StringStatus::Pending);
            assert_eq!(before.translation.as_deref(), Some("Hola"));
            assert!(before.metadata.contains_key(STALE_TRANSLATION_METADATA_KEY));

            let report = db
                .save_imported_translations_batch_with(
                    vec![ImportedTranslation {
                        id: before.id.clone(),
                        source: before.source.clone(),
                        translation: "Hola".into(),
                    }],
                    keep_existing,
                )
                .await
                .unwrap();

            let after = db.get_entry("stale").unwrap().unwrap();
            assert_eq!(after.translation.as_deref(), Some("Hola"));
            assert_eq!(after.status, StringStatus::Translated);
            assert_eq!(after.provider_used.as_deref(), Some("import"));
            assert!(after.translated_at.unwrap() > before.translated_at.unwrap());
            assert!(!after.metadata.contains_key(STALE_TRANSLATION_METADATA_KEY));
            assert_eq!(report.imported, 1);
            assert_eq!(report.unchanged, 0);
            assert_eq!(report.kept_existing, 0);
            assert_eq!(report.stale_sources, 0);
            assert_eq!(report.unknown_ids, 0);
        }
    }

    fn reviewed_import_entry(id: &str, source: &str) -> StringEntry {
        let mut entry = make_entry(id, source);
        entry.translation = Some("Hola".into());
        entry.status = StringStatus::Approved;
        entry.provider_used = Some("reviewed-provider".into());
        entry.translated_at = Some("2025-01-01T00:00:00Z".parse().unwrap());
        entry.reviewed_at = Some("2025-01-02T00:00:00Z".parse().unwrap());
        entry.metadata.insert(
            STALE_TRANSLATION_METADATA_KEY.into(),
            serde_json::json!("preserve on no-op"),
        );
        entry.metadata.insert(
            INJECTION_SOURCE_METADATA_KEY.into(),
            serde_json::json!("日本語"),
        );
        entry
    }

    #[tokio::test]
    async fn import_keep_existing_preserves_translated_and_fills_empty() {
        use crate::export::ImportedTranslation;

        for empty_translation in [None, Some(""), Some(" \t\r\n\u{2003}")] {
            let db = Database::open_in_memory().unwrap();
            let approved = reviewed_import_entry("approved", "Hello");
            let stale = reviewed_import_entry("stale", "Current source");
            let mut empty = make_entry("empty", "Goodbye");
            empty.translation = empty_translation.map(str::to_owned);
            db.save_entries(&[approved.clone(), empty, stale.clone()])
                .unwrap();
            let report = db
                .save_imported_translations_batch_with(
                    vec![
                        ImportedTranslation {
                            id: "approved".into(),
                            source: "Hello".into(),
                            translation: "Collaborator correction".into(),
                        },
                        ImportedTranslation {
                            id: "empty".into(),
                            source: "Goodbye".into(),
                            translation: "Adiós".into(),
                        },
                        ImportedTranslation {
                            id: "stale".into(),
                            source: "Old source".into(),
                            // Source identity takes precedence even for identical text.
                            translation: "Hola".into(),
                        },
                        ImportedTranslation {
                            id: "unknown".into(),
                            source: "Unknown".into(),
                            translation: "Missing".into(),
                        },
                    ],
                    true,
                )
                .await
                .unwrap();

            for before in [approved, stale] {
                assert_eq!(
                    serde_json::to_value(db.get_entry(&before.id).unwrap().unwrap()).unwrap(),
                    serde_json::to_value(before).unwrap()
                );
            }
            let filled = db.get_entry("empty").unwrap().unwrap();
            assert_eq!(filled.translation.as_deref(), Some("Adiós"));
            assert_eq!(filled.status, StringStatus::Translated);
            assert_eq!(filled.provider_used.as_deref(), Some("import"));
            assert!(filled.translated_at.is_some());
            assert!(filled.reviewed_at.is_none());
            assert!(db.get_entry("unknown").unwrap().is_none());
            assert_eq!(report.imported, 1);
            assert_eq!(report.unchanged, 0);
            assert_eq!(report.kept_existing, 1);
            assert_eq!(report.stale_sources, 1);
            assert_eq!(report.unknown_ids, 1);
        }
    }

    #[tokio::test]
    async fn import_without_keep_existing_overwrites_differing_translation() {
        use crate::export::ImportedTranslation;

        let db = Database::open_in_memory().unwrap();
        let before = reviewed_import_entry("approved", "Hello");
        db.save_entries(std::slice::from_ref(&before)).unwrap();
        let report = db
            .save_imported_translations_batch(vec![ImportedTranslation {
                id: before.id.clone(),
                source: before.source.clone(),
                // Whitespace differences are not byte-identical.
                translation: "Hola ".into(),
            }])
            .await
            .unwrap();

        let after = db.get_entry(&before.id).unwrap().unwrap();
        assert_eq!(after.translation.as_deref(), Some("Hola "));
        assert_eq!(after.status, StringStatus::Translated);
        assert_eq!(after.provider_used.as_deref(), Some("import"));
        assert!(after.translated_at.unwrap() > before.translated_at.unwrap());
        assert_eq!(after.reviewed_at, before.reviewed_at);
        assert!(!after.metadata.contains_key(STALE_TRANSLATION_METADATA_KEY));
        assert_eq!(
            after.metadata.get(INJECTION_SOURCE_METADATA_KEY),
            before.metadata.get(INJECTION_SOURCE_METADATA_KEY)
        );
        assert_eq!(report.imported, 1);
        assert_eq!(report.unchanged, 0);
        assert_eq!(report.kept_existing, 0);
        assert_eq!(report.stale_sources, 0);
        assert_eq!(report.unknown_ids, 0);
    }

    #[test]
    fn import_report_defaults_new_counters_for_older_json() {
        let report: ImportApplyReport = serde_json::from_value(serde_json::json!({
            "imported": 1, "stale_sources": 2, "unknown_ids": 3
        }))
        .unwrap();
        assert_eq!(report.unchanged, 0);
        assert_eq!(report.kept_existing, 0);
    }

    #[tokio::test]
    async fn imports_skip_stale_sources_without_clearing_review_or_physical_metadata() {
        use crate::export::ImportedTranslation;
        let db = Database::open_in_memory().unwrap();
        let mut stale = StringEntry::new("stale", "Current English source", "story.html".into());
        stale.translation = Some("Reviewed current translation".into());
        stale.status = StringStatus::Approved;
        stale.metadata.insert(
            STALE_TRANSLATION_METADATA_KEY.into(),
            serde_json::json!("needs review"),
        );
        stale.metadata.insert(
            INJECTION_SOURCE_METADATA_KEY.into(),
            serde_json::json!("日本語"),
        );
        let current = StringEntry::new("current", "Fresh source", "story.html".into());
        db.save_entries(&[stale, current]).unwrap();
        let before = serde_json::to_value(db.get_entry("stale").unwrap().unwrap()).unwrap();
        let report = db
            .save_imported_translations_batch(vec![
                ImportedTranslation {
                    id: "stale".into(),
                    source: "Previous English source".into(),
                    translation: "Wrong old translation".into(),
                },
                ImportedTranslation {
                    id: "current".into(),
                    source: "Fresh source".into(),
                    translation: "Traducción actual".into(),
                },
                ImportedTranslation {
                    id: "unknown".into(),
                    source: "Unknown".into(),
                    translation: "Unknown translation".into(),
                },
            ])
            .await
            .unwrap();
        assert_eq!(
            (report.imported, report.stale_sources, report.unknown_ids),
            (1, 1, 1)
        );
        assert_eq!(
            serde_json::to_value(db.get_entry("stale").unwrap().unwrap()).unwrap(),
            before
        );
        assert_eq!(
            db.get_entry("current")
                .unwrap()
                .unwrap()
                .translation
                .as_deref(),
            Some("Traducción actual")
        );
        let duplicate = ImportedTranslation {
            id: "current".into(),
            source: "Fresh source".into(),
            translation: "Must not overwrite".into(),
        };
        assert!(db
            .save_imported_translations_batch(vec![duplicate.clone(), duplicate])
            .await
            .is_err());
        assert_eq!(
            db.get_entry("current")
                .unwrap()
                .unwrap()
                .translation
                .as_deref(),
            Some("Traducción actual")
        );
    }

    #[test]
    fn test_translation_memory_roundtrip() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let db = Database::open_in_memory().unwrap();
        rt.block_on(async {
            db.save_memory("hash1", "Hello", "Hola", "en-es")
                .await
                .unwrap();
        });
        let result = db.lookup_memory("hash1", "en-es").unwrap();
        assert_eq!(result, Some("Hola".to_string()));
    }

    #[tokio::test]
    async fn save_memory_batch_preserves_sequential_upserts() {
        let sequential = Database::open_in_memory().unwrap();
        let batched = Database::open_in_memory().unwrap();
        let items: Vec<(String, String, String)> = [
            ("duplicate", "First source", "First translation"),
            ("other", "Other source", "Other translation"),
            ("duplicate", "Changed source", "Second translation"),
            ("duplicate", "Last source", "Last translation"),
        ]
        .into_iter()
        .map(|(h, s, t)| (h.into(), s.into(), t.into()))
        .collect();
        for db in [&sequential, &batched] {
            db.save_memory("duplicate", "Existing source", "Old translation", "en-es")
                .await
                .unwrap();
            db.save_memory("duplicate", "Other language", "Autre", "en-fr")
                .await
                .unwrap();
        }
        let before = Utc::now();
        MEMORY_TRANSACTIONS.with(|count| count.set(Some(0)));
        for (hash, source, translation) in &items {
            sequential
                .save_memory(hash, source, translation, "en-es")
                .await
                .unwrap();
        }
        assert_eq!(
            MEMORY_TRANSACTIONS.with(|count| count.replace(Some(0))),
            Some(4)
        );
        batched.save_memory_batch(&items, "en-es").await.unwrap();
        batched.save_memory_batch(&[], "en-es").await.unwrap();
        assert_eq!(
            MEMORY_TRANSACTIONS.with(|count| count.replace(None)),
            Some(1)
        );
        let after = Utc::now();
        let snapshot = |db: &Database| {
            let (mut rows, _) = db.list_memory(None, None, 100, 0).unwrap();
            rows.sort_by(|a, b| {
                (&a.lang_pair, &a.source_hash).cmp(&(&b.lang_pair, &b.source_hash))
            });
            for row in &mut rows {
                if row.lang_pair == "en-es" {
                    let used = DateTime::parse_from_rfc3339(&row.last_used).unwrap();
                    assert!(used >= before && used <= after);
                }
                // Separate executions have different wall-clock timestamps.
                row.last_used.clear();
            }
            serde_json::to_value(rows).unwrap()
        };
        assert_eq!(snapshot(&batched), snapshot(&sequential));
        let (rows, _) = batched.list_memory(None, Some("en-es"), 100, 0).unwrap();
        let duplicate = rows
            .iter()
            .find(|row| row.source_hash == "duplicate")
            .unwrap();
        assert_eq!(duplicate.source, "Existing source");
        assert_eq!(duplicate.translation, "Last translation");
        assert_eq!(duplicate.uses, 4);
    }

    #[tokio::test]
    async fn project_memory_queries_filter_source_translation_and_paginate() {
        let db = Database::open_in_memory().unwrap();
        db.save_memory_batch(
            &[
                ("hello".into(), "Hello".into(), "Hola".into()),
                ("world".into(), "World".into(), "Mundo".into()),
            ],
            "en-es",
        )
        .await
        .unwrap();
        db.save_memory("hello", "Hello", "Bonjour", "en-fr")
            .await
            .unwrap();
        for (search, pair, expected) in [
            (None, None, 3),
            (Some("Hello"), None, 2),
            (Some("Mundo"), None, 1),
            (None, Some("en-es"), 2),
            (Some("Hello"), Some("en-es"), 1),
            (Some("Mundo"), Some("en-fr"), 0),
            (Some("' OR 1=1 --"), None, 0),
        ] {
            let (rows, total) = db.list_memory(search, pair, 50, 0).unwrap();
            assert_eq!(total, expected);
            assert_eq!(rows.len(), expected);
        }
        let (all, _) = db.list_memory(None, Some("en-es"), 50, 0).unwrap();
        let (page, total) = db.list_memory(None, Some("en-es"), 1, 1).unwrap();
        assert_eq!(total, 2);
        assert_eq!(page.len(), 1);
        assert_eq!(page[0].source_hash, all[1].source_hash);
        let (past_end, total) = db.list_memory(None, None, 50, 3).unwrap();
        assert!(past_end.is_empty());
        assert_eq!(total, 3);
    }

    #[tokio::test]
    async fn project_memory_delete_removes_only_the_requested_engine_hit() {
        let db = Database::open_in_memory().unwrap();
        db.save_memory("shared", "Hello", "Hola", "en-es")
            .await
            .unwrap();
        db.save_memory("shared", "Hello", "Bonjour", "en-fr")
            .await
            .unwrap();
        db.delete_memory("shared", "en-es").unwrap();
        db.delete_memory("missing", "en-es").unwrap();
        assert!(db
            .lookup_memory_batch(&["shared".into()], "en-es")
            .unwrap()
            .is_empty());
        assert_eq!(
            db.lookup_memory("shared", "en-fr").unwrap().as_deref(),
            Some("Bonjour")
        );
        assert_eq!(db.list_memory(None, None, 50, 0).unwrap().1, 1);
    }

    #[tokio::test]
    async fn project_memory_language_pairs_are_distinct_sorted_and_clear_with_rows() {
        let db = Database::open_in_memory().unwrap();
        assert!(db.memory_lang_pairs().unwrap().is_empty());
        for (hash, pair) in [("a", "en-fr"), ("b", "en-es"), ("c", "en-es")] {
            db.save_memory(hash, "Source", "Translation", pair)
                .await
                .unwrap();
        }
        assert_eq!(db.memory_lang_pairs().unwrap(), ["en-es", "en-fr"]);
        db.delete_memory("a", "en-fr").unwrap();
        assert_eq!(db.memory_lang_pairs().unwrap(), ["en-es"]);
        db.clear_memory().unwrap();
        assert!(db.memory_lang_pairs().unwrap().is_empty());
        assert_eq!(db.list_memory(None, None, 50, 0).unwrap().1, 0);
        assert_eq!(db.memory_count().unwrap(), 0);
    }

    #[tokio::test]
    async fn lookup_memory_batch_matches_single_lookups_across_chunk_sizes() {
        let db = Database::open_in_memory().unwrap();
        let mut hashes: Vec<_> = (0..501).map(|i| format!("hash'{i}")).collect();
        let items: Vec<_> = hashes
            .iter()
            .enumerate()
            .filter(|(i, _)| i % 2 == 0)
            .map(|(i, hash)| {
                (
                    hash.clone(),
                    format!("Source {i}"),
                    format!("Translation {i}"),
                )
            })
            .collect();
        db.save_memory_batch(&items, "en-es").await.unwrap();
        db.save_memory(&hashes[0], "Source 0", "Other language", "en-fr")
            .await
            .unwrap();
        hashes.push(hashes[0].clone());
        hashes.push("missing".into());
        let expected: HashMap<_, _> = hashes
            .iter()
            .filter_map(|hash| {
                db.lookup_memory(hash, "en-es")
                    .unwrap()
                    .map(|text| (hash.clone(), text))
            })
            .collect();
        MEMORY_LOOKUP_QUERIES.with(|count| count.set(Some(0)));
        assert_eq!(db.lookup_memory_batch(&hashes, "en-es").unwrap(), expected);
        assert!(db.lookup_memory_batch(&[], "en-es").unwrap().is_empty());
        assert_eq!(
            MEMORY_LOOKUP_QUERIES.with(|count| count.replace(None)),
            Some(2)
        );
        assert_eq!(
            db.lookup_memory_batch(&hashes, "en-fr").unwrap(),
            HashMap::from([(hashes[0].clone(), "Other language".into())])
        );
        assert!(db.lookup_memory_batch(&hashes, "en-de").unwrap().is_empty());
    }

    #[test]
    fn test_translation_memory_miss() {
        let db = Database::open_in_memory().unwrap();
        let result = db.lookup_memory("nonexistent", "en-es").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_stats_accuracy() {
        let db = Database::open_in_memory().unwrap();
        let mut t1 = make_entry("t1", "One");
        t1.status = StringStatus::Translated;
        let mut t2 = make_entry("t2", "Two");
        t2.status = StringStatus::Translated;
        db.save_entries(&[
            make_entry("p1", "A"),
            make_entry("p2", "B"),
            make_entry("p3", "C"),
            t1,
            t2,
        ])
        .unwrap();
        let stats = db.get_stats().unwrap();
        assert_eq!(stats.total, 5);
        assert_eq!(stats.pending, 3);
        assert_eq!(stats.translated, 2);
        assert_eq!(stats.reviewed, 0);
        assert_eq!(stats.approved, 0);
        assert_eq!(stats.error, 0);

        // Unknown status still counts toward total (GROUP BY must not drop it).
        {
            let conn = db.conn.lock().unwrap();
            conn.execute(
                "INSERT INTO strings (id, source, file_path, status, created_at) VALUES ('x1', 'X', 'f.json', 'weird', 't')",
                [],
            )
            .unwrap();
        }
        let stats = db.get_stats().unwrap();
        assert_eq!(stats.total, 6);
        assert_eq!(stats.pending, 3);
        assert_eq!(stats.translated, 2);
    }

    #[test]
    fn file_stats_counts_statuses_in_file_order_including_unknown() {
        let db = Database::open_in_memory().unwrap();
        assert!(db.get_file_stats().unwrap().is_empty());
        let entries: Vec<_> = [
            (
                "b-translated",
                "b.rpy",
                StringStatus::Translated,
                Some("Hola"),
            ),
            ("b-error", "b.rpy", StringStatus::Error, None),
            (
                "a-pending",
                "a.rpy",
                StringStatus::Pending,
                Some("Stale text"),
            ),
            (
                "a-approved",
                "a.rpy",
                StringStatus::Approved,
                Some("Aprobado"),
            ),
        ]
        .into_iter()
        .map(|(id, file, status, translation)| {
            let mut entry = StringEntry::new(id, id, file.into());
            entry.status = status;
            entry.translation = translation.map(str::to_owned);
            entry
        })
        .collect();
        db.save_entries(&entries).unwrap();
        let mut expected = vec![
            FileStats {
                file_path: "a.rpy".into(),
                total: 2,
                pending: 1,
                translated: 0,
                reviewed: 0,
                approved: 1,
                error: 0,
            },
            FileStats {
                file_path: "b.rpy".into(),
                total: 2,
                pending: 0,
                translated: 1,
                reviewed: 0,
                approved: 0,
                error: 1,
            },
        ];
        assert_eq!(db.get_file_stats().unwrap(), expected);

        db.conn
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO strings (id, source, translation, file_path, status, created_at)
                 VALUES ('unknown', 'Unknown', 'Retained text', 'a.rpy', 'weird', 't')",
                [],
            )
            .unwrap();
        expected[0].total += 1;
        assert_eq!(db.get_file_stats().unwrap(), expected);
        let project = db.get_stats().unwrap();
        assert_eq!(project.total, 5);
        assert_eq!(project.pending, 1);
        assert_eq!(project.translated, 1);
        assert_eq!(project.reviewed, 0);
        assert_eq!(project.approved, 1);
        assert_eq!(project.error, 1);
    }

    #[test]
    fn file_stats_counts_multiple_reviewed_rows() {
        let db = Database::open_in_memory().unwrap();
        let entries: Vec<_> = ["reviewed-1", "reviewed-2"]
            .into_iter()
            .map(|id| {
                let mut entry = StringEntry::new(id, id, "review.rpy".into());
                entry.status = StringStatus::Reviewed;
                entry
            })
            .collect();
        db.save_entries(&entries).unwrap();
        assert_eq!(
            db.get_file_stats().unwrap(),
            vec![FileStats {
                file_path: "review.rpy".into(),
                total: 2,
                reviewed: 2,
                ..FileStats::default()
            }]
        );
    }

    #[test]
    fn test_glossary_add_and_get() {
        let db = Database::open_in_memory().unwrap();
        db.save_glossary_entry(&GlossaryEntry {
            term: "HP".to_string(),
            translation: "PV".to_string(),
            lang_pair: "en-es".to_string(),
            context: None,
            case_sensitive: false,
        })
        .unwrap();
        db.save_glossary_entry(&GlossaryEntry {
            term: "MP".to_string(),
            translation: "PM".to_string(),
            lang_pair: "en-es".to_string(),
            context: None,
            case_sensitive: false,
        })
        .unwrap();
        let glossary = db.get_glossary("en-es").unwrap();
        assert_eq!(glossary.len(), 2);
    }

    #[test]
    fn test_glossary_duplicate_upserts() {
        let db = Database::open_in_memory().unwrap();
        let entry = GlossaryEntry {
            term: "HP".to_string(),
            translation: "PV".to_string(),
            lang_pair: "en-es".to_string(),
            context: None,
            case_sensitive: false,
        };
        db.save_glossary_entry(&entry).unwrap();
        db.save_glossary_entry(&GlossaryEntry {
            translation: "Puntos de Vida".to_string(),
            ..entry
        })
        .unwrap();
        let glossary = db.get_glossary("en-es").unwrap();
        assert_eq!(glossary.len(), 1);
    }

    #[test]
    fn test_merge_glossary_entries_compares_all_fields() {
        let db = Database::open_in_memory().unwrap();
        let original: Vec<_> = ["translation", "context", "case", "clear-context"]
            .into_iter()
            .map(|term| GlossaryEntry {
                term: term.into(),
                translation: "Original".into(),
                lang_pair: "en-es".into(),
                context: (term == "clear-context").then(|| "Old context".into()),
                case_sensitive: false,
            })
            .collect();
        for entry in &original {
            db.save_glossary_entry(entry).unwrap();
        }
        let before = serde_json::to_value(db.get_glossary("en-es").unwrap()).unwrap();
        let changes = || {
            lock_connection(&db.conn)
                .query_row("SELECT total_changes()", [], |row| row.get::<_, i64>(0))
                .unwrap()
        };
        let mut incoming = original;
        incoming[0].translation = "Replacement".into();
        // NULL and an empty context are distinct, even with identical text.
        incoming[1].context = Some(String::new());
        incoming[2].case_sensitive = true;
        incoming[3].context = None;

        let writes_before = changes();
        assert_eq!(
            db.merge_glossary_entries(&incoming, false).unwrap(),
            GlossaryMergeReport {
                kept_existing: 4,
                ..Default::default()
            }
        );
        assert_eq!(changes(), writes_before);
        assert_eq!(
            serde_json::to_value(db.get_glossary("en-es").unwrap()).unwrap(),
            before
        );
        assert_eq!(
            db.merge_glossary_entries(&incoming, true).unwrap(),
            GlossaryMergeReport {
                overwritten: 4,
                ..Default::default()
            }
        );
        let mut actual = db.get_glossary("en-es").unwrap();
        actual.sort_by(|a, b| a.term.cmp(&b.term));
        incoming.sort_by(|a, b| a.term.cmp(&b.term));
        assert_eq!(
            serde_json::to_value(actual).unwrap(),
            serde_json::to_value(&incoming).unwrap()
        );

        let writes_before = changes();
        for overwrite in [false, true] {
            assert_eq!(
                db.merge_glossary_entries(&incoming, overwrite).unwrap(),
                GlossaryMergeReport {
                    unchanged: 4,
                    ..Default::default()
                }
            );
            assert_eq!(
                db.merge_glossary_entries(&[], overwrite).unwrap(),
                GlossaryMergeReport::default()
            );
        }
        assert_eq!(changes(), writes_before);
    }

    #[test]
    fn test_merge_glossary_entries_uses_composite_key_within_batch() {
        let db = Database::open_in_memory().unwrap();
        let entry = GlossaryEntry {
            term: "HP".into(),
            translation: "PV".into(),
            lang_pair: "en-es".into(),
            context: Some("Combat".into()),
            case_sensitive: true,
        };
        let other_pair = GlossaryEntry {
            translation: "Vie".into(),
            lang_pair: "en-fr".into(),
            ..entry.clone()
        };
        assert_eq!(
            db.merge_glossary_entries(&[entry.clone(), other_pair.clone(), entry.clone()], false)
                .unwrap(),
            GlossaryMergeReport {
                added: 2,
                unchanged: 1,
                ..Default::default()
            }
        );
        for expected in [entry, other_pair] {
            assert_eq!(
                serde_json::to_value(db.get_glossary(&expected.lang_pair).unwrap()).unwrap(),
                serde_json::to_value([expected]).unwrap()
            );
        }
    }

    #[test]
    fn test_merge_glossary_entries_rolls_back_entire_batch() {
        for overwrite in [false, true] {
            let db = Database::open_in_memory().unwrap();
            let original = GlossaryEntry {
                term: "HP".into(),
                translation: "Tuned translation".into(),
                lang_pair: "en-es".into(),
                context: Some("Tuned context".into()),
                case_sensitive: true,
            };
            db.save_glossary_entry(&original).unwrap();
            lock_connection(&db.conn)
                .execute_batch(
                    "CREATE TRIGGER fail_glossary_insert BEFORE INSERT ON glossary
                     WHEN NEW.term = 'fail'
                     BEGIN SELECT RAISE(ABORT, 'forced glossary failure'); END;",
                )
                .unwrap();
            let before = serde_json::to_value(db.get_glossary("en-es").unwrap()).unwrap();
            let error = db
                .merge_glossary_entries(
                    &[
                        GlossaryEntry {
                            term: "new".into(),
                            ..original.clone()
                        },
                        GlossaryEntry {
                            translation: "Replacement".into(),
                            context: None,
                            case_sensitive: false,
                            ..original.clone()
                        },
                        GlossaryEntry {
                            term: "fail".into(),
                            ..original.clone()
                        },
                        GlossaryEntry {
                            term: "after-failure".into(),
                            ..original
                        },
                    ],
                    overwrite,
                )
                .unwrap_err();
            assert!(error.to_string().contains("forced glossary failure"));
            assert_eq!(
                serde_json::to_value(db.get_glossary("en-es").unwrap()).unwrap(),
                before,
                "the insert and any overwrite before the failing entry must roll back"
            );
        }
    }

    #[test]
    fn test_validation_issues_save_and_get() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let db = Database::open_in_memory().unwrap();
        let issues = vec![
            ValidationIssue {
                entry_id: "e1".to_string(),
                kind: ValidationKind::EmptyTranslation,
                message: "empty".to_string(),
                source: None,
            },
            ValidationIssue {
                entry_id: "e2".to_string(),
                kind: ValidationKind::IdenticalToSource,
                message: "identical".to_string(),
                source: None,
            },
        ];
        rt.block_on(async {
            db.save_validation_issues(&issues).await.unwrap();
        });
        let all = db.get_validation_issues(None).unwrap();
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn test_save_validation_issues_many_all_persist() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let db = Database::open_in_memory().unwrap();
        let issues: Vec<ValidationIssue> = (0..40)
            .map(|i| ValidationIssue {
                entry_id: format!("e{i}"),
                kind: ValidationKind::EmptyTranslation,
                message: format!("empty-{i}"),
                source: None,
            })
            .collect();
        rt.block_on(async {
            db.save_validation_issues(&issues).await.unwrap();
        });
        let all = db.get_validation_issues(None).unwrap();
        assert_eq!(all.len(), 40);
    }

    #[test]
    fn test_count_entries() {
        let db = Database::open_in_memory().unwrap();
        let entries: Vec<StringEntry> = (0..4)
            .map(|i| make_entry(&format!("c{}", i), &format!("Count {}", i)))
            .collect();
        db.save_entries(&entries).unwrap();
        let count = db.count_entries(&EntryFilter::default()).unwrap();
        assert_eq!(count, 4);
    }

    #[test]
    fn test_global_memory_saves_across_calls() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let gm = GlobalMemoryDb::open_in_memory().unwrap();
        rt.block_on(async {
            gm.save_memory("hash_g1", "Hello", "Hola", "en-es")
                .await
                .unwrap();
        });
        let result = gm.lookup_memory("hash_g1", "en-es").unwrap();
        assert_eq!(result, Some("Hola".to_string()));
        assert_eq!(gm.memory_count().unwrap(), 1);
    }

    #[test]
    fn test_global_memory_used_in_new_project() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let gm = GlobalMemoryDb::open_in_memory().unwrap();
        rt.block_on(async {
            gm.save_memory("hash_g2", "World", "Mundo", "en-es")
                .await
                .unwrap();
        });
        // Simulate new project checking global memory
        let result = gm.lookup_memory("hash_g2", "en-es").unwrap();
        assert_eq!(result, Some("Mundo".to_string()));
    }

    // Keep the original materializing resolver as an equivalence oracle.
    fn resolve_export_source_lang_from_runs(
        db: &Database,
        target_lang: &str,
        fallback: &str,
    ) -> Result<String> {
        let runs = db.get_translation_runs()?;
        if let Some(run) = runs
            .iter()
            .rev()
            .find(|r| r.target_lang.eq_ignore_ascii_case(target_lang) && !r.source_lang.is_empty())
        {
            return Ok(run.source_lang.clone());
        }
        if let Some(run) = runs.iter().rev().find(|r| !r.source_lang.is_empty()) {
            return Ok(run.source_lang.clone());
        }
        Ok(fallback.to_string())
    }

    #[test]
    fn test_resolve_export_source_lang_matches_original_order_and_case_rules() {
        let db = Database::open_in_memory().unwrap();
        {
            let conn = lock_connection(&db.conn);
            // Insert tied timestamps out of ID order; the highest ID wins.
            for (id, started_at, source_lang, target_lang) in [
                (2, "2026-01-01T00:00:00Z", "ja", "ES"),
                (9, "2026-01-02T00:00:00Z", "de", "eS"),
                (5, "2026-01-02T00:00:00Z", "en", "es"),
                (3, "2026-01-03T00:00:00Z", "it", "É"),
                (4, "2026-01-04T00:00:00Z", "pt", "é"),
                (11, "2026-01-05T00:00:00Z", "sv", "de"),
                (6, "2026-01-05T00:00:00Z", "nl", "fr"),
                (1, "2026-01-06T00:00:00Z", "", "ES"),
                (7, "2026-01-06T00:00:00Z", "", "fr"),
                (8, "2026-01-07T00:00:00Z", "", "empty-only"),
                // A larger ID must not outrank a later timestamp.
                (12, "2025-12-31T00:00:00Z", "ru", "es"),
            ] {
                conn.execute(
                    "INSERT INTO translation_runs
                     (id, started_at, duration_secs, provider, source_lang, target_lang,
                      strings_translated)
                     VALUES (?1, ?2, 0, 'mock', ?3, ?4, 1)",
                    params![id, started_at, source_lang, target_lang],
                )
                .unwrap();
            }
        }

        for (target, expected) in [
            ("es", "de"),
            ("ES", "de"),
            ("Es", "de"),
            ("É", "it"),
            ("é", "pt"),
            ("FR", "nl"),
            ("unknown", "sv"),
            ("empty-only", "sv"),
        ] {
            let original = resolve_export_source_lang_from_runs(&db, target, "fallback").unwrap();
            assert_eq!(original, expected, "original resolver for {target:?}");
            assert_eq!(
                db.resolve_export_source_lang(target, "fallback").unwrap(),
                original,
                "target {target:?}"
            );
        }
    }

    #[test]
    fn test_resolve_export_source_lang_matches_original_without_nonempty_sources() {
        for run_count in [0, 3] {
            let db = Database::open_in_memory().unwrap();
            {
                let conn = lock_connection(&db.conn);
                for _ in 0..run_count {
                    conn.execute(
                        "INSERT INTO translation_runs
                         (started_at, duration_secs, provider, source_lang, target_lang,
                          strings_translated)
                         VALUES ('2026-01-01T00:00:00Z', 0, 'mock', '', 'ES', 1)",
                        [],
                    )
                    .unwrap();
                }
            }
            for target in ["es", "unknown"] {
                for fallback in ["ja", ""] {
                    let original =
                        resolve_export_source_lang_from_runs(&db, target, fallback).unwrap();
                    assert_eq!(original, fallback);
                    assert_eq!(
                        db.resolve_export_source_lang(target, fallback).unwrap(),
                        original,
                        "{run_count} empty-source runs, target {target:?}, fallback {fallback:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn test_resolve_export_source_lang_does_not_materialize_ledger() {
        let db = Database::open_in_memory().unwrap();
        {
            let conn = lock_connection(&db.conn);
            conn.execute_batch(
                "WITH RECURSIVE runs(id) AS (
                     SELECT 1 UNION ALL SELECT id + 1 FROM runs WHERE id < 2000
                 )
                 INSERT INTO translation_runs
                 (id, started_at, duration_secs, provider, source_lang, target_lang,
                  strings_translated)
                 SELECT id, '2026-01-01T00:00:00Z', 0, 'mock',
                        CASE WHEN id = 2000 THEN 'en' ELSE 'ja' END,
                        CASE WHEN id = 2000 THEN 'fr' ELSE 'es' END, 1
                 FROM runs",
            )
            .unwrap();
        }

        // Negative control: the full-ledger path must trip the same counter.
        TRANSLATION_RUN_ROWS_MATERIALIZED.with(|count| count.set(Some(0)));
        let runs = db.get_translation_runs().unwrap();
        let rows = TRANSLATION_RUN_ROWS_MATERIALIZED.with(|count| count.replace(None).unwrap());
        assert_eq!(runs.len(), 2000);
        assert_eq!(rows, 2000, "the counter must observe every decoded run");

        for (target, expected) in [("ES", "ja"), ("unknown", "en")] {
            TRANSLATION_RUN_ROWS_MATERIALIZED.with(|count| count.set(Some(0)));
            let source = db.resolve_export_source_lang(target, "fallback").unwrap();
            let rows = TRANSLATION_RUN_ROWS_MATERIALIZED.with(|count| count.replace(None).unwrap());
            assert_eq!(source, expected);
            assert!(
                rows <= 2,
                "export for {target:?} materialized {rows} ledger rows; expected at most 2"
            );
        }
    }

    #[test]
    fn test_resolve_export_source_lang_prefers_matching_target_run() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let db = Database::open_in_memory().unwrap();
        rt.block_on(async {
            db.record_translation_run(&TranslationRun {
                id: 0,
                started_at: "2026-01-01T00:00:00Z".into(),
                duration_secs: 1.0,
                provider: "mock".into(),
                source_lang: "ja".into(),
                target_lang: "en".into(),
                strings_translated: 1,
                tokens_used: 0,
                input_tokens: 0,
                output_tokens: 0,
                cost_usd: 0.0,
                cost_is_complete: true,
            })
            .await
            .unwrap();
            db.record_translation_run(&TranslationRun {
                id: 0,
                started_at: "2026-01-02T00:00:00Z".into(),
                duration_secs: 1.0,
                provider: "mock".into(),
                source_lang: "en".into(),
                target_lang: "es".into(),
                strings_translated: 1,
                tokens_used: 0,
                input_tokens: 0,
                output_tokens: 0,
                cost_usd: 0.0,
                cost_is_complete: true,
            })
            .await
            .unwrap();
        });
        assert_eq!(db.resolve_export_source_lang("es", "ja").unwrap(), "en");
        assert_eq!(
            db.resolve_export_source_lang("fr", "xx").unwrap(),
            "en",
            "unknown target falls back to latest run overall"
        );
        let empty = Database::open_in_memory().unwrap();
        assert_eq!(empty.resolve_export_source_lang("es", "ja").unwrap(), "ja");
    }

    #[test]
    fn test_string_facets_are_distinct_sorted_and_project_wide() {
        let db = Database::open_in_memory().unwrap();
        let mut entries = Vec::new();
        // More rows than a typical 100-row page so facets cannot come from
        // whatever is currently on screen.
        for i in 0..120 {
            let file = match i % 3 {
                0 => "data/Map001.json",
                1 => "data/Actors.json",
                _ => "data/Map002.json",
            };
            let mut e = StringEntry::new(format!("e{i}"), format!("S{i}"), PathBuf::from(file));
            e.tags = match i % 3 {
                0 => vec!["dialogue".into(), "ui_label".into()],
                1 => vec!["ui_label".into()],
                _ => vec!["dialogue".into()],
            };
            entries.push(e);
        }
        let mut extra = StringEntry::new("zz", "Last", PathBuf::from("data/System.json"));
        extra.tags = vec!["system".into()];
        entries.push(extra);
        db.save_entries(&entries).unwrap();

        let facets = db.get_string_facets().unwrap();
        assert_eq!(
            facets.file_paths,
            vec![
                "data/Actors.json",
                "data/Map001.json",
                "data/Map002.json",
                "data/System.json",
            ]
        );
        assert_eq!(facets.tags, vec!["dialogue", "system", "ui_label"]);
    }

    #[test]
    fn test_string_facets_empty_project() {
        let db = Database::open_in_memory().unwrap();
        let facets = db.get_string_facets().unwrap();
        assert!(facets.file_paths.is_empty());
        assert!(facets.tags.is_empty());
    }

    fn translated_entry(id: &str, source: &str, file: &str, translation: &str) -> StringEntry {
        let mut e = StringEntry::new(id, source, PathBuf::from(file));
        e.translation = Some(translation.into());
        e.status = StringStatus::Translated;
        e
    }

    #[test]
    fn test_pivot_to_uses_translations_as_sources_and_skips_pending() {
        let src = Database::open_in_memory().unwrap();
        let mut done = translated_entry("a", "Hello", "data/Actors.json", "Hola");
        done.context = Some("npc".into());
        done.tags = vec!["dialogue".into()];
        done.char_limit = Some(20);
        let pending = StringEntry::new("b", "World", PathBuf::from("data/Actors.json"));
        let whitespace = translated_entry("c", "Skip me", "data/Actors.json", "   ");
        src.save_entries(&[done, pending, whitespace]).unwrap();

        let dir = recording_tempdir();
        let out = dir.join("pivoted.locust.db");
        let result = src.pivot_to(&out).unwrap();
        assert_eq!(result.entries, 1);
        assert_eq!(result.database_path, out.to_string_lossy());

        let new_db = Database::open(&out).unwrap();
        let entries = new_db.get_entries(&EntryFilter::default()).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, "a");
        assert_eq!(entries[0].source, "Hola");
        assert!(entries[0].translation.is_none());
        assert_eq!(entries[0].status, StringStatus::Pending);
        assert_eq!(entries[0].context.as_deref(), Some("npc"));
        assert_eq!(entries[0].tags, vec!["dialogue".to_string()]);
        assert_eq!(entries[0].char_limit, Some(20));
        assert_eq!(entries[0].injection_source().unwrap(), "Hello");

        let original = src.get_entries(&EntryFilter::default()).unwrap();
        assert_eq!(original.len(), 3);
        let hello = original.iter().find(|e| e.id == "a").unwrap();
        assert_eq!(hello.source, "Hello");
        assert_eq!(hello.translation.as_deref(), Some("Hola"));
        assert_eq!(hello.status, StringStatus::Translated);
        let world = original.iter().find(|e| e.id == "b").unwrap();
        assert_eq!(world.source, "World");
        assert!(world.translation.is_none());
    }

    #[tokio::test]
    async fn repeated_pivot_keeps_original_source_and_binary_capacity() {
        let src = Database::open_in_memory().unwrap();
        let mut japanese = translated_entry("slot", "日本語文", "resources.assets", "English");
        japanese
            .metadata
            .insert("binary_slot".into(), serde_json::json!("utf8"));
        src.save_entries(&[japanese]).unwrap();

        let dir = recording_tempdir();
        let first_path = dir.join("en.locust.db");
        src.pivot_to(&first_path).unwrap();
        let first = Database::open(&first_path).unwrap();
        let first_entry = first.get_entry("slot").unwrap().unwrap();
        assert_eq!(first_entry.source, "English");
        assert_eq!(first_entry.injection_source().unwrap(), "日本語文");
        assert_eq!(
            first_entry.injection_capacity().unwrap(),
            Some(("utf8", 12))
        );
        first
            .save_translation("slot", "1234567890", "manual")
            .await
            .unwrap();

        let second_path = dir.join("es.locust.db");
        first.pivot_to(&second_path).unwrap();
        let second = Database::open(&second_path)
            .unwrap()
            .get_entry("slot")
            .unwrap()
            .unwrap();
        assert_eq!(second.source, "1234567890");
        assert_eq!(second.injection_source().unwrap(), "日本語文");
        assert_eq!(second.injection_capacity().unwrap(), Some(("utf8", 12)));
    }

    #[test]
    fn pivot_preserves_grouped_textasset_original_and_does_not_invent_cell_capacity() {
        let src = Database::open_in_memory().unwrap();
        let original = "Menu.A: 日本語\nMenu.B: 行く\n";
        let mut a = translated_entry("a", "日本語", "resources.assets", "English A");
        let mut b = translated_entry("b", "行く", "resources.assets", "Go");
        for (entry, key, index) in [(&mut a, "Menu.A", 0usize), (&mut b, "Menu.B", 1usize)] {
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
        }
        let mut members = vec![a, b];
        crate::textasset_group::attach_to_entries(
            &mut members,
            crate::textasset_group::GroupKind::LocLine,
            original,
            original.len(),
            "g-pivot",
        );
        src.save_entries(&members).unwrap();

        let dir = recording_tempdir();
        let out = dir.join("pivot-group.locust.db");
        src.pivot_to(&out).unwrap();
        let pivoted = Database::open(&out).unwrap();
        let first = pivoted.get_entry("a").unwrap().unwrap();
        assert_eq!(first.source, "English A");
        assert_eq!(first.injection_source().unwrap(), "日本語");
        assert!(first.injection_capacity().unwrap().is_none());
        assert_eq!(
            crate::textasset_group::original_textasset(&first),
            Some(original)
        );
        assert_eq!(
            first
                .metadata
                .get(crate::textasset_group::GROUP_CAPACITY_KEY)
                .and_then(|v| v.as_u64()),
            Some(original.len() as u64)
        );
        assert_eq!(crate::validation::binary_slot_budget(&first).unwrap(), None);
    }

    #[tokio::test]
    async fn pivot_merge_preserves_semantic_source_and_rejects_changed_baseline_atomically() {
        let src = Database::open_in_memory().unwrap();
        src.save_entries(&[
            translated_entry("a", "日本語A", "old-a", "English A"),
            translated_entry("b", "日本語B", "old-b", "English B"),
        ])
        .unwrap();
        let dir = recording_tempdir();
        let pivot_path = dir.join("merge.locust.db");
        src.pivot_to(&pivot_path).unwrap();
        let pivot = Database::open(&pivot_path).unwrap();
        pivot
            .save_translation("a", "Español A", "manual")
            .await
            .unwrap();

        let mut same_a = make_entry("a", "日本語A");
        same_a.file_path = PathBuf::from("new-a");
        same_a
            .metadata
            .insert("fresh_locator".into(), serde_json::json!(7));
        let mut same_b = make_entry("b", "日本語B");
        same_b.file_path = PathBuf::from("new-b");
        pivot.merge_entries(&[same_a, same_b]).unwrap();
        let kept = pivot.get_entry("a").unwrap().unwrap();
        assert_eq!(kept.source, "English A");
        assert_eq!(kept.translation.as_deref(), Some("Español A"));
        assert_eq!(kept.injection_source().unwrap(), "日本語A");
        assert_eq!(kept.file_path, PathBuf::from("new-a"));
        assert_eq!(
            kept.metadata.get("fresh_locator"),
            Some(&serde_json::json!(7))
        );

        let before_a = pivot.get_entry("a").unwrap().unwrap();
        let before_b = pivot.get_entry("b").unwrap().unwrap();
        let mut would_update_a = make_entry("a", "日本語A");
        would_update_a.file_path = PathBuf::from("must-not-write");
        let changed_b = make_entry("b", "different baseline");
        let error = pivot
            .merge_entries(&[would_update_a, changed_b])
            .expect_err("changed physical baseline must fail before writes");
        assert!(error.to_string().contains("source_changed"));
        assert_eq!(
            pivot.get_entry("a").unwrap().unwrap().file_path,
            before_a.file_path
        );
        assert_eq!(
            pivot.get_entry("b").unwrap().unwrap().file_path,
            before_b.file_path
        );
        // Some formats encode source text in IDs. A changed baseline may
        // therefore look like a missing row, not an overlapping changed source.
        let error = pivot
            .merge_entries(&[make_entry("a", "日本語A")])
            .expect_err("a missing physical slot must not delete a pivot translation");
        assert!(error.to_string().contains("missing entries"));
        assert_eq!(pivot.get_entry("a").unwrap().unwrap().source, "English A");
        assert!(pivot.get_entry("b").unwrap().is_some());
    }

    /// Pin: pivot's source query must not materialize the pending bulk that
    /// `get_entries` still returns (negative: equal lengths would mean the
    /// SQL filter is gone and large projects pay O(n) for mostly-empty rows).
    #[test]
    fn test_entries_with_nonempty_translation_skips_pending_bulk() {
        let src = Database::open_in_memory().unwrap();
        let mut bulk: Vec<StringEntry> = (0..200)
            .map(|i| {
                StringEntry::new(
                    format!("p{i}"),
                    format!("Pending {i}"),
                    PathBuf::from("f.json"),
                )
            })
            .collect();
        bulk.push(translated_entry("done", "Hello", "f.json", "Hola"));
        bulk.push(translated_entry("ws", "Whitespace", "f.json", "   "));
        src.save_entries(&bulk).unwrap();

        let pivoted_src = src.entries_with_nonempty_translation().unwrap();
        assert_eq!(pivoted_src.len(), 1);
        assert_eq!(pivoted_src[0].id, "done");
        assert_eq!(
            src.get_entries(&EntryFilter::default()).unwrap().len(),
            202,
            "full table scan still sees every row; pivot query must stay narrower"
        );
    }

    #[test]
    fn test_pivot_to_refuses_existing_output_path() {
        let src = Database::open_in_memory().unwrap();
        src.save_entries(&[translated_entry("a", "Hello", "f.json", "Hola")])
            .unwrap();

        let dir = recording_tempdir();
        let out = dir.join("exists.locust.db");
        std::fs::write(&out, b"nope").unwrap();
        let err = src.pivot_to(&out).expect_err("must refuse overwrite");
        let msg = err.to_string().to_lowercase();
        assert!(msg.contains("exist") || msg.contains("overwrite"), "{msg}");
        assert_eq!(std::fs::read(&out).unwrap(), b"nope");
    }

    #[test]
    fn test_pivot_to_errors_when_nothing_is_translated() {
        let src = Database::open_in_memory().unwrap();
        src.save_entries(&[StringEntry::new("b", "World", PathBuf::from("f.json"))])
            .unwrap();
        let dir = recording_tempdir();
        let out = dir.join("empty-pivot.locust.db");
        let err = src.pivot_to(&out).expect_err("must refuse empty pivot");
        assert!(
            err.to_string().to_lowercase().contains("no translated"),
            "{err}"
        );
        assert!(!out.exists(), "must not create an empty output db");
    }
    #[test]
    fn control_repro_pivot_refuses_missing_controls_before_creating_output() {
        let temp = tempfile::tempdir().unwrap();
        let db = Database::open(&temp.path().join("source.db")).unwrap();
        let mut entry = StringEntry::new("line", "JA {name}", PathBuf::from("script.rpy"));
        entry.translation = Some("Hello".into());
        db.save_entries(&[entry]).unwrap();
        let output = temp.path().join("pivot.db");
        assert!(
            db.pivot_to(&output).is_err(),
            "invalid English must not become a pivot source"
        );
        assert!(!output.exists());
    }
    #[tokio::test]
    async fn control_manual_and_imported_results_remain_reportable_and_block_pivot() {
        let temp = tempfile::tempdir().unwrap();
        let db = Database::open(&temp.path().join("source.db")).unwrap();
        let entry = StringEntry::new("line", r"JA \V[1]", PathBuf::from("data.json"));
        db.save_entries(&[entry]).unwrap();
        db.save_translation("line", "Hello", "manual")
            .await
            .unwrap();
        let saved = db.get_entries(&EntryFilter::default()).unwrap().remove(0);
        assert!(!crate::validation::Validator::validate_entry(&saved).is_empty());
        assert!(db.pivot_to(&temp.path().join("invalid.db")).is_err());
        // Batch import shares the same report/gate, while allowing stored draft
        // work to be repaired explicitly rather than deleting the user's text.
        db.save_translations_batch(vec![("line".into(), r"Hello \V[1]".into())], "import")
            .await
            .unwrap();
        let es = temp.path().join("es.db");
        let fr = temp.path().join("fr.db");
        db.pivot_to(&es).unwrap();
        db.pivot_to(&fr).unwrap();
        let es = Database::open(&es).unwrap();
        let fr = Database::open(&fr).unwrap();
        es.save_translation("line", r"Hola \V[1]", "manual")
            .await
            .unwrap();
        let saved = es.get_entries(&EntryFilter::default()).unwrap().remove(0);
        assert!(saved.require_preserved_translation_controls().is_ok());
        assert_eq!(saved.source, r"Hello \V[1]");
        assert_eq!(saved.injection_source().unwrap(), r"JA \V[1]");
        assert!(fr.get_entries(&EntryFilter::default()).unwrap()[0]
            .translation
            .is_none());
    }
}
