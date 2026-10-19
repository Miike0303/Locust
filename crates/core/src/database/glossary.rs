use rusqlite::params;

use crate::error::Result;

use super::{lock_connection, Database, GlossaryEntry, GlossaryMergeReport};

impl Database {
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
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
