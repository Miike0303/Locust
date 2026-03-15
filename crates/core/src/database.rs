use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::models::{StringEntry, StringStatus, ValidationIssue, ValidationKind};

pub struct Database {
    conn: Arc<Mutex<Connection>>,
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
pub struct GlossaryEntry {
    pub term: String,
    pub translation: String,
    pub lang_pair: String,
    pub context: Option<String>,
    pub case_sensitive: bool,
}

impl Database {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        let db = Self {
            conn: Arc::new(Mutex::new(conn)),
        };
        db.migrate()?;
        Ok(db)
    }

    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        let db = Self {
            conn: Arc::new(Mutex::new(conn)),
        };
        db.migrate()?;
        Ok(db)
    }

    fn migrate(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute_batch(
            "
            PRAGMA journal_mode = WAL;
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
            ",
        )?;
        Ok(())
    }

    pub fn save_entries(&self, entries: &[StringEntry]) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
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

            conn.execute(
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

    pub async fn save_translation(
        &self,
        entry_id: &str,
        translation: &str,
        provider: &str,
    ) -> Result<()> {
        let conn = self.conn.clone();
        let entry_id = entry_id.to_string();
        let translation = translation.to_string();
        let provider = provider.to_string();
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().unwrap();
            let now = Utc::now().to_rfc3339();
            conn.execute(
                "UPDATE strings SET translation = ?1, status = 'translated', provider_used = ?2, translated_at = ?3 WHERE id = ?4",
                params![translation, provider, now, entry_id],
            )?;
            Ok(())
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

    pub fn get_stats(&self) -> Result<ProjectStats> {
        let conn = self.conn.lock().unwrap();
        let total: usize =
            conn.query_row("SELECT COUNT(*) FROM strings", [], |row| row.get(0))?;
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
                let kind_json = serde_json::to_string(&issue.kind)
                    .unwrap_or_else(|_| "unknown".to_string());
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
    let status: StringStatus = raw
        .status
        .parse()
        .unwrap_or(StringStatus::Pending);
    let tags: Vec<String> =
        serde_json::from_str(&raw.tags).unwrap_or_default();
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
        db.save_entries(&[
            make_entry("s1", "hello world"),
            make_entry("s2", "goodbye"),
        ])
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
        db.save_translation("tr1", "Hola", "test-provider")
            .await
            .unwrap();
        let entry = db.get_entry("tr1").unwrap().unwrap();
        assert_eq!(entry.translation, Some("Hola".to_string()));
        assert_eq!(entry.status, StringStatus::Translated);
        assert_eq!(entry.provider_used, Some("test-provider".to_string()));
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
            },
            ValidationIssue {
                entry_id: "e2".to_string(),
                kind: ValidationKind::IdenticalToSource,
                message: "identical".to_string(),
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
}
