use std::collections::HashMap;

use chrono::Utc;
use rusqlite::{params, OptionalExtension};

use crate::error::Result;

use super::{lock_connection, Database, MemoryEntry};
#[cfg(test)]
use super::{MEMORY_LOOKUP_QUERIES, MEMORY_TRANSACTIONS};

impl Database {
    /// Bounded, length-prefiltered candidates for fuzzy TM suggestions. SQLite
    /// length(TEXT) and source_len both count Unicode characters, not bytes.
    pub fn memory_candidates(
        &self,
        lang_pair: &str,
        source_len: usize,
        max_rows: usize,
    ) -> Result<Vec<MemoryEntry>> {
        let radius = source_len.saturating_mul(2).div_ceil(5).max(3);
        let lower = source_len.saturating_sub(radius).min(i64::MAX as usize) as i64;
        let upper = source_len.saturating_add(radius).min(i64::MAX as usize) as i64;
        let conn = lock_connection(&self.conn);
        let mut stmt = conn.prepare(
            "SELECT source_hash, lang_pair, source, translation, uses, last_used
             FROM translation_memory
             WHERE lang_pair = ?1 AND length(source) BETWEEN ?2 AND ?3
             ORDER BY uses DESC, last_used DESC LIMIT ?4",
        )?;
        let rows = stmt
            .query_map(
                params![lang_pair, lower, upper, max_rows.min(3000)],
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
        Ok(rows)
    }

    /// Read an exact suggestion through the (source_hash, lang_pair) primary key.
    pub fn lookup_memory_entry(
        &self,
        source_hash: &str,
        lang_pair: &str,
    ) -> Result<Option<MemoryEntry>> {
        let conn = lock_connection(&self.conn);
        Ok(conn
            .query_row(
                "SELECT source_hash, lang_pair, source, translation, uses, last_used
                 FROM translation_memory WHERE source_hash = ?1 AND lang_pair = ?2",
                params![source_hash, lang_pair],
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
            )
            .optional()?)
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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::DateTime;

    #[test]
    fn memory_candidates_filter_character_lengths_languages_and_order() {
        let db = Database::open_in_memory().unwrap();
        {
            let conn = lock_connection(&db.conn);
            for (hash, source, pair, uses, last_used) in [
                ("lower", "界".repeat(6), "en-es", 2, "2026-01-01"),
                ("upper", "é".repeat(14), "en-es", 2, "2026-02-01"),
                ("popular", "a".repeat(10), "en-es", 3, "2025-01-01"),
                ("short", "a".repeat(5), "en-es", 99, "2026-01-01"),
                ("long", "a".repeat(15), "en-es", 99, "2026-01-01"),
                ("other", "a".repeat(10), "en-fr", 99, "2026-01-01"),
                ("empty", String::new(), "en-es", 1, "2026-01-01"),
                ("tiny", "界".repeat(4), "en-es", 1, "2026-01-01"),
            ] {
                conn.execute(
                    "INSERT INTO translation_memory VALUES (?1, ?2, ?3, 'translation', ?4, ?5)",
                    params![hash, pair, source, uses, last_used],
                )
                .unwrap();
            }
        }
        let hashes = |len, limit| {
            db.memory_candidates("en-es", len, limit)
                .unwrap()
                .into_iter()
                .map(|row| row.source_hash)
                .collect::<Vec<_>>()
        };
        assert_eq!(hashes(10, 50), ["popular", "upper", "lower"]);
        assert_eq!(hashes(10, 2), ["popular", "upper"]);
        assert!(hashes(10, 0).is_empty());
        assert_eq!(hashes(1, 50), ["empty", "tiny"]);
        assert_eq!(hashes(0, 50), ["empty"]);
        assert!(db
            .memory_candidates("' OR 1=1 --", 10, 50)
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn memory_candidates_cap_at_three_thousand() {
        let db = Database::open_in_memory().unwrap();
        let items = (0..3001)
            .map(|i| (format!("hash{i}"), "Hello".into(), "Hola".into()))
            .collect::<Vec<_>>();
        db.save_memory_batch(&items, "en-es").await.unwrap();
        assert_eq!(
            db.memory_candidates("en-es", 5, usize::MAX).unwrap().len(),
            3000
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
}
