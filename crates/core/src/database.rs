use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::{LocustError, Result};
use crate::models::{StringEntry, StringStatus, ValidationIssue, ValidationKind};

pub struct Database {
    conn: Arc<Mutex<Connection>>,
    path: Mutex<PathBuf>,
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
    pub cost_usd: f64,
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

/// Everything one `locust inject` run recorded for one language key: the
/// absolutized root of the tree it wrote into and the files it wrote there.
/// `lang: None` is the reserved language-unspecified key (`--direct` without
/// `-l`), rendered as "(unspecified)" and matched only by `patch` without `-l`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InjectionRecording {
    pub lang: Option<String>,
    pub root: PathBuf,
    pub files: Vec<RecordedFile>,
    pub recorded_at: String,
}

/// Lowercase hex SHA-256 of `bytes` — the hash stored in injection recordings.
/// SHA-256 over BLAKE3 because `sha2` is already in the dependency tree; the
/// column is TEXT, so nothing else depends on the choice.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
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
fn path_identity_key(p: &Path) -> String {
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
        *self.conn.lock().unwrap() = new_conn;
        *self.path.lock().unwrap() = path.to_path_buf();
        Ok(())
    }

    pub fn save_entries(&self, entries: &[StringEntry]) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        // Single transaction: per-row implicit transactions fsync each insert,
        // which takes minutes for a full game extraction.
        let tx = conn.unchecked_transaction()?;
        let mut count = 0usize;
        for entry in entries {
            let tags_json = serde_json::to_string(&entry.tags)?;
            let metadata_json = serde_json::to_string(&entry.metadata)?;
            let status_str = entry.status.to_string();
            let file_path_str = entry.file_path.to_string_lossy().to_string();
            let created_at_str = entry.created_at.to_rfc3339();
            let translated_at_str = entry.translated_at.map(|d| d.to_rfc3339());
            let reviewed_at_str = entry.reviewed_at.map(|d| d.to_rfc3339());
            let char_limit = entry.char_limit.map(|l| l as i64);

            tx.execute(
                "INSERT OR REPLACE INTO strings
                 (id, source, translation, status, file_path, context, tags, metadata,
                  char_limit, provider_used, created_at, translated_at, reviewed_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                params![
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
                ],
            )?;
            count += 1;
        }
        tx.commit()?;
        Ok(count)
    }

    pub fn get_entries(&self, filter: &EntryFilter) -> Result<Vec<StringEntry>> {
        let conn = self.conn.lock().unwrap();
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
        for row in rows {
            let raw = row?;
            entries.push(raw_to_entry(raw)?);
        }
        Ok(entries)
    }

    pub fn get_entry(&self, id: &str) -> Result<Option<StringEntry>> {
        let conn = self.conn.lock().unwrap();
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
            Some(row) => Ok(Some(raw_to_entry(row?)?)),
            None => Ok(None),
        }
    }

    pub fn count_entries(&self, filter: &EntryFilter) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
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
            let conn = conn.lock().unwrap();
            let now = Utc::now().to_rfc3339();
            let n = conn.execute(
                "UPDATE strings SET translation = ?1, status = 'translated', provider_used = ?2, translated_at = ?3 WHERE id = ?4",
                params![translation, provider, now, entry_id],
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
            let conn = conn.lock().unwrap();
            let now = Utc::now().to_rfc3339();
            let tx = conn.unchecked_transaction()?;
            let mut applied = 0usize;
            {
                let mut stmt = tx.prepare(
                    "UPDATE strings SET translation = ?1, status = 'translated', provider_used = ?2, translated_at = ?3 WHERE id = ?4",
                )?;
                for (id, translation) in &updates {
                    let n = stmt.execute(params![translation, provider, now, id])?;
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

    pub async fn update_entry_status(&self, entry_id: &str, status: StringStatus) -> Result<()> {
        let conn = self.conn.clone();
        let entry_id = entry_id.to_string();
        let status_str = status.to_string();
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().unwrap();
            conn.execute(
                "UPDATE strings SET status = ?1 WHERE id = ?2",
                params![status_str, entry_id],
            )?;
            Ok(())
        })
        .await
        .unwrap()
    }

    pub fn lookup_memory(&self, source_hash: &str, lang_pair: &str) -> Result<Option<String>> {
        let conn = self.conn.lock().unwrap();
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
            let conn = conn.lock().unwrap();
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
            let conn = conn.lock().unwrap();
            conn.execute(
                "INSERT INTO translation_runs
                 (started_at, duration_secs, provider, source_lang, target_lang,
                  strings_translated, tokens_used, input_tokens, output_tokens, cost_usd)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
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
        if written.is_empty() {
            return Ok(());
        }
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
            let bytes = std::fs::read(p)?;
            rows.push((rel, sha256_hex(&bytes), bytes.len() as u64));
        }

        let root_str = root_abs.to_string_lossy().to_string();
        let now = Utc::now().to_rfc3339();
        let conn = self.conn.lock().unwrap();
        let tx = conn.unchecked_transaction()?;
        tx.execute("DELETE FROM injected_files WHERE lang IS ?1", params![lang])?;
        for (rel, hash, size) in &rows {
            tx.execute(
                "INSERT INTO injected_files (lang, root, rel, hash, size, recorded_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![lang, root_str, rel, hash, *size as i64, now],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// The recording persisted by [`Self::record_injection`] for exactly this
    /// key — `None` matches only the language-unspecified recording, never a
    /// named one, and vice versa. `Ok(None)` when nothing is recorded for it.
    pub fn get_injection(&self, lang: Option<&str>) -> Result<Option<InjectionRecording>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT root, rel, hash, size, recorded_at FROM injected_files
             WHERE lang IS ?1 ORDER BY id ASC",
        )?;
        let rows = stmt.query_map(params![lang], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, String>(4)?,
            ))
        })?;
        let mut root = None;
        let mut recorded_at = String::new();
        let mut files = Vec::new();
        for row in rows {
            let (r, rel, hash, size, at) = row?;
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
            })),
            None => Ok(None),
        }
    }

    /// Every language key with a recording, named keys first, the reserved
    /// language-unspecified key (`None`) last. Empty when no injection has
    /// ever been recorded — `locust patch` must then hard-error with the
    /// exact inject command, never fall back to guessing from entries.
    pub fn list_recorded_langs(&self) -> Result<Vec<Option<String>>> {
        let conn = self.conn.lock().unwrap();
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
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, started_at, duration_secs, provider, source_lang, target_lang,
                    strings_translated, tokens_used, input_tokens, output_tokens, cost_usd
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
        let conn = self.conn.lock().unwrap();
        let total: usize = conn.query_row("SELECT COUNT(*) FROM strings", [], |row| row.get(0))?;
        let pending: usize = conn.query_row(
            "SELECT COUNT(*) FROM strings WHERE status = 'pending'",
            [],
            |row| row.get(0),
        )?;
        let translated: usize = conn.query_row(
            "SELECT COUNT(*) FROM strings WHERE status = 'translated'",
            [],
            |row| row.get(0),
        )?;
        let reviewed: usize = conn.query_row(
            "SELECT COUNT(*) FROM strings WHERE status = 'reviewed'",
            [],
            |row| row.get(0),
        )?;
        let approved: usize = conn.query_row(
            "SELECT COUNT(*) FROM strings WHERE status = 'approved'",
            [],
            |row| row.get(0),
        )?;
        let error: usize = conn.query_row(
            "SELECT COUNT(*) FROM strings WHERE status = 'error'",
            [],
            |row| row.get(0),
        )?;

        Ok(ProjectStats {
            total,
            pending,
            translated,
            reviewed,
            approved,
            error,
            total_cost_usd: 0.0,
        })
    }

    pub fn save_glossary_entry(&self, entry: &GlossaryEntry) -> Result<()> {
        let conn = self.conn.lock().unwrap();
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
        let conn = self.conn.lock().unwrap();
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
        let conn = self.conn.lock().unwrap();
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
            let conn = conn.lock().unwrap();
            for issue in &issues {
                let kind_json =
                    serde_json::to_string(&issue.kind).unwrap_or_else(|_| "unknown".to_string());
                conn.execute(
                    "INSERT INTO validation_issues (entry_id, kind, message) VALUES (?1, ?2, ?3)",
                    params![issue.entry_id, kind_json, issue.message],
                )?;
            }
            Ok(())
        })
        .await
        .unwrap()
    }

    pub fn get_validation_issues(&self, entry_id: Option<&str>) -> Result<Vec<ValidationIssue>> {
        let conn = self.conn.lock().unwrap();
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
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM strings", [])?;
        Ok(())
    }

    /// Merge a fresh extract into the live `strings` table without wiping
    /// translations. Existing ids keep translation / status / timestamps /
    /// provider; a changed `source` keeps the translation but forces
    /// `pending`. Ids missing from `entries` are deleted. One transaction.
    pub fn merge_entries(&self, entries: &[StringEntry]) -> Result<MergeStats> {
        let conn = self.conn.lock().unwrap();
        let tx = conn.unchecked_transaction()?;

        struct Stored {
            source: String,
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
                "SELECT id, source, translation, status, provider_used,
                        created_at, translated_at, reviewed_at
                 FROM strings",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    Stored {
                        source: row.get(1)?,
                        translation: row.get(2)?,
                        status: row.get(3)?,
                        provider_used: row.get(4)?,
                        created_at: row.get(5)?,
                        translated_at: row.get(6)?,
                        reviewed_at: row.get(7)?,
                    },
                ))
            })?;
            for row in rows {
                let (id, stored) = row?;
                existing.insert(id, stored);
            }
        }

        let mut incoming: HashMap<&str, &StringEntry> = HashMap::new();
        for entry in entries {
            incoming.insert(entry.id.as_str(), entry);
        }

        let mut stats = MergeStats::default();

        for id in existing.keys() {
            if !incoming.contains_key(id.as_str()) {
                tx.execute("DELETE FROM strings WHERE id = ?1", params![id])?;
                stats.removed += 1;
            }
        }

        for (id, entry) in incoming {
            let tags_json = serde_json::to_string(&entry.tags)?;
            let metadata_json = serde_json::to_string(&entry.metadata)?;
            let file_path_str = entry.file_path.to_string_lossy().to_string();
            let char_limit = entry.char_limit.map(|l| l as i64);

            if let Some(old) = existing.get(id) {
                let source_changed = old.source != entry.source;
                let status = if source_changed {
                    stats.stale_source_reset += 1;
                    StringStatus::Pending.to_string()
                } else {
                    old.status.clone()
                };
                if old.translation.as_ref().is_some_and(|t| !t.is_empty()) {
                    stats.preserved_translations += 1;
                }
                stats.updated += 1;
                tx.execute(
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
                    params![
                        entry.source,
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
                    ],
                )?;
            } else {
                stats.added += 1;
                let created_at = entry.created_at.to_rfc3339();
                tx.execute(
                    "INSERT INTO strings
                     (id, source, translation, status, file_path, context, tags, metadata,
                      char_limit, provider_used, created_at, translated_at, reviewed_at)
                     VALUES (?1, ?2, NULL, 'pending', ?3, ?4, ?5, ?6, ?7, NULL, ?8, NULL, NULL)",
                    params![
                        entry.id,
                        entry.source,
                        file_path_str,
                        entry.context,
                        tags_json,
                        metadata_json,
                        char_limit,
                        created_at,
                    ],
                )?;
            }
        }

        tx.commit()?;
        Ok(stats)
    }

    pub fn memory_count(&self) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
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
        let conn = self.conn.lock().unwrap();

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
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM translation_memory WHERE source_hash = ?1 AND lang_pair = ?2",
            params![source_hash, lang_pair],
        )?;
        Ok(())
    }

    pub fn clear_memory(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM translation_memory", [])?;
        Ok(())
    }

    pub fn memory_lang_pairs(&self) -> Result<Vec<String>> {
        let conn = self.conn.lock().unwrap();
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

fn raw_to_entry(raw: RawEntry) -> Result<StringEntry> {
    let status: StringStatus = raw.status.parse().unwrap_or(StringStatus::Pending);
    let tags: Vec<String> = serde_json::from_str(&raw.tags).unwrap_or_default();
    let metadata: HashMap<String, serde_json::Value> =
        serde_json::from_str(&raw.metadata).unwrap_or_default();
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

    Ok(StringEntry {
        id: raw.id,
        source: raw.source,
        translation: raw.translation,
        file_path: PathBuf::from(raw.file_path),
        context: raw.context,
        tags,
        metadata,
        status,
        provider_used: raw.provider_used,
        char_limit: raw.char_limit.map(|l| l as usize),
        created_at,
        translated_at,
        reviewed_at,
    })
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

        let mut applied = 0usize;
        let mut missed = 0usize;
        for pe in &imported {
            if pe.translation.is_empty() {
                continue;
            }
            if let Some(ref pe_id) = pe.id {
                if db
                    .save_translation(pe_id, &pe.translation, "import")
                    .await
                    .unwrap()
                {
                    applied += 1;
                } else {
                    missed += 1;
                }
            }
        }
        assert_eq!(applied, 1);
        assert_eq!(missed, 0);
        let again = db.get_entry(id).unwrap().unwrap();
        assert_eq!(again.translation.as_deref(), Some("Hola alli"));
        assert_eq!(again.status, StringStatus::Translated);
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
}
