use rusqlite::params;

use crate::error::Result;

#[cfg(test)]
use super::TRANSLATION_RUN_ROWS_MATERIALIZED;
use super::{lock_connection, Database, TranslationRun};

impl Database {
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
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
