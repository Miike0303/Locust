use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[cfg(test)]
thread_local! {
    pub(crate) static ENTRY_ROWS_MATERIALIZED: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
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

fn lock_connection(conn: &Mutex<Connection>) -> MutexGuard<'_, Connection> {
    conn.lock().unwrap_or_else(PoisonError::into_inner)
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct ImportApplyReport {
    pub imported: usize,
    pub stale_sources: usize,
    pub unknown_ids: usize,
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
        let mut sql = String::from("SELECT id, source, translation, status, file_path, context, tags, metadata, char_limit, provider_used, created_at, translated_at, reviewed_at FROM strings WHERE 1=1");
        let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

        if let Some(ref status) = filter.status {
            sql.push_str(" AND status = ?");
            param_values.push(Box::new(status.to_string()));
        }
        if let Some(ref fp) = filter.file_path {
            sql.push_str(" AND file_path = ?");
            param_values.push(Box::new(fp.clone()));
        }
        if let Some(ref tag) = filter.tag {
            sql.push_str(" AND tags LIKE ?");
            param_values.push(Box::new(format!("%\"{}\"%", tag)));
        }
        if let Some(ref search) = filter.search {
            sql.push_str(" AND (source LIKE ? OR translation LIKE ?)");
            let pattern = format!("%{}%", search);
            param_values.push(Box::new(pattern.clone()));
            param_values.push(Box::new(pattern));
        }

        sql.push_str(" ORDER BY id");

        if let Some(limit) = filter.limit {
            sql.push_str(" LIMIT ?");
            param_values.push(Box::new(limit as i64));
        }
        if let Some(offset) = filter.offset {
            if filter.limit.is_none() {
                sql.push_str(" LIMIT -1");
            }
            sql.push_str(" OFFSET ?");
            param_values.push(Box::new(offset as i64));
        }

        let params_refs: Vec<&dyn rusqlite::types::ToSql> =
            param_values.iter().map(|p| p.as_ref()).collect();
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params_refs.as_slice(), |row| {
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
            let raw = row?;
            #[cfg(test)]
            ENTRY_ROWS_MATERIALIZED.with(|count| {
                if let Some(n) = count.get() {
                    count.set(Some(n + 1));
                }
            });
            entries.push(raw_to_entry(raw, &conn, &mut originals)?);
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

    pub fn count_entries(&self, filter: &EntryFilter) -> Result<usize> {
        let conn = lock_connection(&self.conn);
        let mut sql = String::from("SELECT COUNT(*) FROM strings WHERE 1=1");
        let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

        if let Some(ref status) = filter.status {
            sql.push_str(" AND status = ?");
            param_values.push(Box::new(status.to_string()));
        }
        if let Some(ref fp) = filter.file_path {
            sql.push_str(" AND file_path = ?");
            param_values.push(Box::new(fp.clone()));
        }
        if let Some(ref tag) = filter.tag {
            sql.push_str(" AND tags LIKE ?");
            param_values.push(Box::new(format!("%\"{}\"%", tag)));
        }
        if let Some(ref search) = filter.search {
            sql.push_str(" AND (source LIKE ? OR translation LIKE ?)");
            let pattern = format!("%{}%", search);
            param_values.push(Box::new(pattern.clone()));
            param_values.push(Box::new(pattern));
        }

        let params_refs: Vec<&dyn rusqlite::types::ToSql> =
            param_values.iter().map(|p| p.as_ref()).collect();
        let count: usize = conn.query_row(&sql, params_refs.as_slice(), |row| row.get(0))?;
        Ok(count)
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
        let conn = self.conn.clone();
        let entry_id = entry_id.to_string();
        let translation = translation.to_string();
        let provider = provider.to_string();
        tokio::task::spawn_blocking(move || {
            let conn = lock_connection(&conn);
            let now = Utc::now().to_rfc3339();
            let n = conn.execute(
                "UPDATE strings SET translation = ?1, status = 'translated', provider_used = ?2, translated_at = ?3, metadata = CASE WHEN ?5 THEN json_remove(metadata, '$.locust_stale_translation') ELSE metadata END WHERE id = ?4",
                params![translation, provider, now, entry_id, !translation.trim().is_empty()],
            )?;
            Ok(n > 0)
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
        if updates.is_empty() {
            return Ok(0);
        }
        let conn = self.conn.clone();
        let provider = provider.to_string();
        tokio::task::spawn_blocking(move || {
            let conn = lock_connection(&conn);
            let now = Utc::now().to_rfc3339();
            let tx = conn.unchecked_transaction()?;
            let mut applied = 0usize;
            {
                let mut stmt = tx.prepare_cached(
                    "UPDATE strings SET translation = ?1, status = 'translated', provider_used = ?2, translated_at = ?3, metadata = CASE WHEN ?5 THEN json_remove(metadata, '$.locust_stale_translation') ELSE metadata END WHERE id = ?4",
                )?;
                for (id, translation) in &updates {
                    let n = stmt.execute(params![translation, provider, now, id, !translation.trim().is_empty()])?;
                    if n > 0 {
                        applied += 1;
                    }
                }
            }
            tx.commit()?;
            Ok(applied)
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
        let mut seen = std::collections::HashSet::new();
        for entry in &updates {
            if !seen.insert(&entry.id) {
                return Err(LocustError::ValidationError {
                    entry_id: entry.id.clone(),
                    message: "duplicate import id; no translations were saved".into(),
                });
            }
        }
        if updates.is_empty() {
            return Ok(ImportApplyReport::default());
        }
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let conn = lock_connection(&conn);
            let tx = conn.unchecked_transaction()?;
            let now = Utc::now().to_rfc3339();
            let mut report = ImportApplyReport::default();
            {
                let mut write = tx.prepare_cached(
                    "UPDATE strings SET translation = ?1, status = 'translated', provider_used = 'import', translated_at = ?2, metadata = CASE WHEN ?5 THEN json_remove(metadata, '$.locust_stale_translation') ELSE metadata END WHERE id = ?3 AND source = ?4",
                )?;
                let mut exists = tx.prepare_cached("SELECT EXISTS(SELECT 1 FROM strings WHERE id = ?1)")?;
                for entry in updates {
                    if write.execute(params![entry.translation, now, entry.id, entry.source, !entry.translation.trim().is_empty()])? > 0 {
                        report.imported += 1;
                    } else if exists.query_row(params![entry.id], |row| row.get::<_, bool>(0))? {
                        report.stale_sources += 1;
                    } else {
                        report.unknown_ids += 1;
                    }
                }
            }
            tx.commit()?;
            Ok(report)
        }).await.map_err(|e| LocustError::Other(anyhow::anyhow!("import save task failed: {e}")))?
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

    async fn save_guarded_updates(
        &self,
        updates: Vec<(String, String, String, TranslationSaveGuard)>,
    ) -> Result<()> {
        if updates.is_empty() {
            return Ok(());
        }
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
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
        }).await.map_err(|e| LocustError::ProviderError(format!("translation save task failed: {e}")))?
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

    pub async fn save_memory(
        &self,
        hash: &str,
        source: &str,
        translation: &str,
        lang_pair: &str,
    ) -> Result<()> {
        let conn = self.conn.clone();
        let hash = hash.to_string();
        let source = source.to_string();
        let translation = translation.to_string();
        let lang_pair = lang_pair.to_string();
        tokio::task::spawn_blocking(move || {
            let conn = lock_connection(&conn);
            let now = Utc::now().to_rfc3339();
            conn.execute(
                "INSERT INTO translation_memory (source_hash, lang_pair, source, translation, uses, last_used)
                 VALUES (?1, ?2, ?3, ?4, 1, ?5)
                 ON CONFLICT(source_hash, lang_pair) DO UPDATE SET
                     translation = excluded.translation,
                     uses = uses + 1,
                     last_used = excluded.last_used",
                params![hash, lang_pair, source, translation, now],
            )?;
            Ok(())
        })
        .await
        .unwrap()
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
        tx.execute("DELETE FROM injected_files WHERE lang IS ?1", params![lang])?;
        // Same class as save_entries/merge_entries: one plan for N file rows
        // (Unreal/Unity injects can record hundreds of written paths).
        {
            let mut insert = tx.prepare_cached(
                "INSERT INTO injected_files (lang, root, rel, hash, size, recorded_at, pristine_backup)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )?;
            for (rel, hash, size) in &rows {
                insert.execute(params![
                    lang,
                    root_str,
                    rel,
                    hash,
                    *size as i64,
                    now,
                    pristine_backup
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
        let runs = self.get_translation_runs()?;
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
            for id in existing.keys() {
                if !preserve_missing && !incoming.contains_key(id.as_str()) {
                    delete.execute(params![id])?;
                    stats.removed += 1;
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
                    if old.translation.as_ref().is_some_and(|t| !t.is_empty()) {
                        stats.preserved_translations += 1;
                    }
                    stats.updated += 1;
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
    let metadata: HashMap<String, serde_json::Value> = serde_json::from_str(&raw.metadata)
        .map_err(|error| {
            LocustError::Other(anyhow::anyhow!(
                "entry '{}' has malformed metadata: {error}",
                raw.id
            ))
        })?;
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
        assert_eq!(stats.updated, 2);
        assert_eq!(after - before, 1, "only the changed row should be written");

        let stats = db.merge_entries(&[unchanged, changed]).unwrap();
        let final_changes: i64 = lock_connection(&db.conn)
            .query_row("SELECT total_changes()", [], |row| row.get(0))
            .unwrap();
        assert_eq!(stats.updated, 2);
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
        assert_eq!(stats.added, 0);
        assert_eq!(stats.removed, 0);

        let npc = db.get_entry("npc").unwrap().unwrap();
        assert_eq!(npc.source, "Welcome, traveler");
        assert_eq!(npc.translation.as_deref(), Some("Bienvenido"));
        assert_eq!(npc.status, StringStatus::Pending);
        assert_eq!(npc.provider_used.as_deref(), Some("mock"));
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
