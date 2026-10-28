use std::collections::HashSet;

use rusqlite::params;

use crate::error::Result;
use crate::models::{ValidationIssue, ValidationKind};
use crate::validation::{
    StoredValidationIssue, ValidationIssueFilter, ValidationIssueUpdateError, ValidationIssuesPage,
    ValidationKindCounts, ValidationSummary,
};

use super::{lock_connection, Database};

// Existing kinds use serde's externally tagged enum representation: a JSON
// string for unit variants and a one-key object for variants carrying details.
const ISSUE_ROWS: &str = "SELECT id, entry_id, message, resolved, kind AS kind_data,
    CASE WHEN json_type(kind) = 'text' THEN json_extract(kind, '$')
         ELSE (SELECT key FROM json_each(kind) LIMIT 1) END AS kind
    FROM validation_issues";

// The most common translation is display data, not part of the ignored issue's
// identity. Keep ignored rows matching after an upgrade or a new common value.
fn ignored_kind_key(kind_json: String) -> String {
    match serde_json::from_str::<ValidationKind>(&kind_json) {
        Ok(ValidationKind::InconsistentTranslation { variants, .. }) => {
            format!("InconsistentTranslation:{variants}")
        }
        _ => kind_json,
    }
}

impl Database {
    pub async fn save_validation_issues(&self, issues: &[ValidationIssue]) -> Result<()> {
        let conn = self.conn.clone();
        let issues: Vec<ValidationIssue> = issues.to_vec();
        tokio::task::spawn_blocking(move || {
            let conn = lock_connection(&conn);
            let tx = conn.unchecked_transaction()?;
            let ignored = {
                let mut stmt =
                    tx.prepare("SELECT entry_id, kind FROM validation_issues WHERE resolved = 1")?;
                let rows =
                    stmt.query_map([], |row| Ok((row.get(0)?, ignored_kind_key(row.get(1)?))))?;
                rows.collect::<rusqlite::Result<HashSet<(String, String)>>>()?
            };
            tx.execute("DELETE FROM validation_issues", [])?;
            {
                let mut insert = tx.prepare_cached(
                    "INSERT INTO validation_issues (entry_id, kind, message, resolved)
                     VALUES (?1, ?2, ?3, ?4)",
                )?;
                for issue in &issues {
                    let kind_json = serde_json::to_string(&issue.kind)?;
                    let resolved = ignored
                        .contains(&(issue.entry_id.clone(), ignored_kind_key(kind_json.clone())));
                    insert.execute(params![issue.entry_id, kind_json, issue.message, resolved])?;
                }
            }
            tx.commit()?;
            Ok(())
        })
        .await
        .unwrap()
    }

    pub fn list_validation_issues(
        &self,
        filter: &ValidationIssueFilter,
    ) -> Result<ValidationIssuesPage> {
        let conn = lock_connection(&self.conn);
        let condition = "(?1 IS NULL OR v.kind = ?1) AND (?2 OR v.resolved = 0)";
        let total = conn.query_row(
            &format!(
                "WITH issues AS ({ISSUE_ROWS}) SELECT COUNT(*) FROM issues v WHERE {condition}"
            ),
            params![filter.kind, filter.include_ignored],
            |row| row.get(0),
        )?;
        let limit = filter.limit.min(500);
        let mut stmt = conn.prepare(&format!(
            "WITH issues AS ({ISSUE_ROWS})
             SELECT v.id, v.entry_id, v.kind, v.message, v.resolved,
                    s.source, s.translation, s.file_path, v.kind_data
             FROM issues v LEFT JOIN strings s ON s.id = v.entry_id
             WHERE {condition} ORDER BY v.id LIMIT ?3 OFFSET ?4"
        ))?;
        let rows = stmt.query_map(
            params![
                filter.kind,
                filter.include_ignored,
                limit as i64,
                i64::try_from(filter.offset).unwrap_or(i64::MAX)
            ],
            |row| {
                Ok(StoredValidationIssue {
                    id: row.get(0)?,
                    entry_id: row.get(1)?,
                    kind: row.get(2)?,
                    kind_data: serde_json::from_str(&row.get::<_, String>(8)?).unwrap_or_default(),
                    message: row.get(3)?,
                    ignored: row.get(4)?,
                    source: row.get(5)?,
                    translation: row.get(6)?,
                    file_path: row.get(7)?,
                })
            },
        )?;
        Ok(ValidationIssuesPage {
            issues: rows.collect::<rusqlite::Result<_>>()?,
            total,
            offset: filter.offset,
            limit,
        })
    }

    pub fn validation_summary(&self) -> Result<ValidationSummary> {
        let conn = lock_connection(&self.conn);
        let mut stmt = conn.prepare(&format!(
            "WITH issues AS ({ISSUE_ROWS})
             SELECT kind, SUM(resolved = 0), SUM(resolved = 1) FROM issues GROUP BY kind"
        ))?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get(0)?,
                ValidationKindCounts {
                    open: row.get(1)?,
                    ignored: row.get(2)?,
                },
            ))
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn set_validation_issue_ignored(
        &self,
        id: i64,
        ignored: bool,
    ) -> std::result::Result<(), ValidationIssueUpdateError> {
        let conn = lock_connection(&self.conn);
        let changed = conn.execute(
            "UPDATE validation_issues SET resolved = ?2 WHERE id = ?1",
            params![id, ignored],
        )?;
        if changed == 0 {
            return Err(ValidationIssueUpdateError::NotFound);
        }
        Ok(())
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
}

#[cfg(test)]
mod tests {
    use super::*;

    fn issue(entry_id: &str, kind: ValidationKind) -> ValidationIssue {
        ValidationIssue {
            entry_id: entry_id.into(),
            kind,
            message: "validation message".into(),
            source: None,
        }
    }

    #[tokio::test]
    async fn validation_inconsistent_translation_wire_form_round_trips_through_storage() {
        let db = Database::open_in_memory().unwrap();
        let kind = ValidationKind::InconsistentTranslation {
            variants: 2,
            common: None,
        };
        let expected_json = r#"{"InconsistentTranslation":{"variants":2}}"#;
        assert_eq!(serde_json::to_string(&kind).unwrap(), expected_json);
        db.save_validation_issues(&[issue("a", kind)])
            .await
            .unwrap();

        let stored_json: String = lock_connection(&db.conn)
            .query_row(
                "SELECT kind FROM validation_issues WHERE entry_id = ?1",
                ["a"],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(stored_json, expected_json);
        let issues = db.get_validation_issues(Some("a")).unwrap();
        assert_eq!(issues.len(), 1);
        assert_eq!(
            serde_json::to_string(&issues[0].kind).unwrap(),
            expected_json
        );

        let page = db
            .list_validation_issues(&ValidationIssueFilter {
                kind: Some("InconsistentTranslation".into()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(page.total, 1);
        assert_eq!(page.issues.len(), 1);
        assert_eq!(page.issues[0].entry_id, "a");
        assert_eq!(page.issues[0].kind, "InconsistentTranslation");
        assert_eq!(
            page.issues[0].kind_data,
            serde_json::from_str::<serde_json::Value>(expected_json).unwrap()
        );
        let summary = db.validation_summary().unwrap();
        assert_eq!(summary.len(), 1);
        assert_eq!(
            summary["InconsistentTranslation"],
            ValidationKindCounts {
                open: 1,
                ignored: 0
            }
        );
    }

    #[tokio::test]
    async fn validation_legacy_consistency_details_and_ignored_state_survive_upgrade() {
        let db = Database::open_in_memory().unwrap();
        let legacy_kind = r#"{"InconsistentTranslation":{"variants":2}}"#;
        let legacy_message = "legacy English consistency diagnostic";
        lock_connection(&db.conn)
            .execute(
                "INSERT INTO validation_issues (entry_id, kind, message, resolved)
                 VALUES (?1, ?2, ?3, 1)",
                params!["a", legacy_kind, legacy_message],
            )
            .unwrap();
        let issues = db.get_validation_issues(Some("a")).unwrap();
        assert!(matches!(
            &issues[0].kind,
            ValidationKind::InconsistentTranslation {
                variants: 2,
                common: None
            }
        ));
        assert_eq!(issues[0].message, legacy_message);
        let filter = ValidationIssueFilter {
            include_ignored: true,
            ..Default::default()
        };
        let page = db.list_validation_issues(&filter).unwrap();
        assert_eq!(page.issues[0].message, legacy_message);
        assert_eq!(
            page.issues[0].kind_data,
            serde_json::json!({"InconsistentTranslation": {"variants": 2}})
        );
        for common in ["Guardar", "Conservar"] {
            db.save_validation_issues(&[issue(
                "a",
                ValidationKind::InconsistentTranslation {
                    variants: 2,
                    common: Some(common.into()),
                },
            )])
            .await
            .unwrap();
            let page = db.list_validation_issues(&filter).unwrap();
            assert!(page.issues[0].ignored);
            assert_eq!(
                page.issues[0].kind_data,
                serde_json::json!({"InconsistentTranslation": {"variants": 2, "common": common}})
            );
        }
    }

    #[test]
    fn validation_list_preserves_unknown_kind_payload_and_message() {
        let db = Database::open_in_memory().unwrap();
        let kind = serde_json::json!({"FutureKind": {"detail": "example"}});
        lock_connection(&db.conn)
            .execute(
                "INSERT INTO validation_issues (entry_id, kind, message, resolved)
                 VALUES (?1, ?2, ?3, 0)",
                params!["a", kind.to_string(), "future English diagnostic"],
            )
            .unwrap();
        let page = db.list_validation_issues(&Default::default()).unwrap();
        let value = serde_json::to_value(&page.issues[0]).unwrap();
        assert_eq!(value["kind"], "FutureKind");
        assert_eq!(value["kind_data"], kind);
        assert_eq!(value["message"], "future English diagnostic");
    }

    #[tokio::test]
    async fn validation_consecutive_saves_replace_instead_of_accumulating() {
        let db = Database::open_in_memory().unwrap();
        let issues = [issue("a", ValidationKind::EmptyTranslation)];
        db.save_validation_issues(&issues).await.unwrap();
        db.save_validation_issues(&issues).await.unwrap();
        assert_eq!(db.get_validation_issues(None).unwrap().len(), 1);
    }

    #[tokio::test]
    async fn validation_ignored_pairs_survive_with_updated_messages() {
        let db = Database::open_in_memory().unwrap();
        let mut issues = [
            issue("a", ValidationKind::EmptyTranslation),
            issue("b", ValidationKind::EmptyTranslation),
        ];
        db.save_validation_issues(&issues).await.unwrap();
        let page = db.list_validation_issues(&Default::default()).unwrap();
        db.set_validation_issue_ignored(page.issues[0].id, true)
            .unwrap();
        issues[0].message = "new message".into();
        db.save_validation_issues(&issues).await.unwrap();
        let open = db.list_validation_issues(&Default::default()).unwrap();
        assert_eq!(open.total, 1);
        assert_eq!(open.issues[0].entry_id, "b");
        let all = db
            .list_validation_issues(&ValidationIssueFilter {
                include_ignored: true,
                ..Default::default()
            })
            .unwrap();
        assert!(all.issues[0].ignored);
        assert_eq!(all.issues[0].message, "new message");
        // Legacy callers still see all issues, including ignored rows.
        assert_eq!(db.get_validation_issues(Some("a")).unwrap().len(), 1);
        assert_eq!(db.get_validation_issues(None).unwrap().len(), 2);
    }

    #[tokio::test]
    async fn validation_disappeared_issues_and_empty_runs_clear_stored_rows() {
        let db = Database::open_in_memory().unwrap();
        let issues = [
            issue("a", ValidationKind::EmptyTranslation),
            issue("b", ValidationKind::IdenticalToSource),
        ];
        db.save_validation_issues(&issues).await.unwrap();
        let id = db
            .list_validation_issues(&Default::default())
            .unwrap()
            .issues[0]
            .id;
        db.set_validation_issue_ignored(id, true).unwrap();
        db.save_validation_issues(&issues[1..]).await.unwrap();
        assert!(db.get_validation_issues(Some("a")).unwrap().is_empty());
        assert_eq!(db.get_validation_issues(None).unwrap().len(), 1);
        db.save_validation_issues(&[]).await.unwrap();
        assert!(db.validation_summary().unwrap().is_empty());
        // A newly reappearing issue is open after its ignored row disappeared.
        db.save_validation_issues(&issues[..1]).await.unwrap();
        assert_eq!(
            db.list_validation_issues(&Default::default())
                .unwrap()
                .total,
            1
        );
    }

    #[tokio::test]
    async fn validation_replacement_rolls_back_delete_on_insert_failure() {
        let db = Database::open_in_memory().unwrap();
        db.save_validation_issues(&[issue("a", ValidationKind::EmptyTranslation)])
            .await
            .unwrap();
        lock_connection(&db.conn)
            .execute_batch(
                "CREATE TRIGGER refuse_issue BEFORE INSERT ON validation_issues
                 WHEN NEW.entry_id = 'fail' BEGIN SELECT RAISE(ABORT, 'test failure'); END;",
            )
            .unwrap();
        assert!(db
            .save_validation_issues(&[
                issue("b", ValidationKind::EmptyTranslation),
                issue("fail", ValidationKind::EmptyTranslation),
            ])
            .await
            .is_err());
        let all = db.get_validation_issues(None).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].entry_id, "a");
    }

    #[tokio::test]
    async fn validation_list_joins_filters_pages_and_summarizes_payload_kinds() {
        use crate::models::StringEntry;
        use std::path::PathBuf;

        let db = Database::open_in_memory().unwrap();
        let mut entry = StringEntry::new("a", "Hello {0}", PathBuf::from("data/a.json"));
        entry.translation = Some("Hola".into());
        db.save_entries(&[entry]).unwrap();
        db.save_validation_issues(&[
            issue(
                "a",
                ValidationKind::MissingPlaceholder {
                    placeholder: "{0}".into(),
                },
            ),
            issue(
                "orphan",
                ValidationKind::MissingPlaceholder {
                    placeholder: "{1}".into(),
                },
            ),
            issue("b", ValidationKind::EmptyTranslation),
        ])
        .await
        .unwrap();
        let filter = ValidationIssueFilter {
            kind: Some("MissingPlaceholder".into()),
            limit: 1,
            ..Default::default()
        };
        let first = db.list_validation_issues(&filter).unwrap();
        assert_eq!(first.total, 2);
        assert_eq!(first.issues.len(), 1);
        assert_eq!(first.issues[0].source.as_deref(), Some("Hello {0}"));
        assert_eq!(first.issues[0].translation.as_deref(), Some("Hola"));
        assert_eq!(first.issues[0].file_path.as_deref(), Some("data/a.json"));
        assert_eq!(
            first.issues[0].kind_data,
            serde_json::json!({"MissingPlaceholder": {"placeholder": "{0}"}})
        );
        let next = db
            .list_validation_issues(&ValidationIssueFilter {
                offset: 1,
                ..filter.clone()
            })
            .unwrap();
        assert_eq!(next.total, 2);
        assert_eq!(next.issues[0].entry_id, "orphan");
        assert!(next.issues[0].source.is_none());
        assert!(next.issues[0].translation.is_none());
        assert!(next.issues[0].file_path.is_none());
        let ignored_id = first.issues[0].id;
        db.set_validation_issue_ignored(ignored_id, true).unwrap();
        assert_eq!(db.list_validation_issues(&filter).unwrap().total, 1);
        assert_eq!(
            db.list_validation_issues(&ValidationIssueFilter {
                include_ignored: true,
                ..filter
            })
            .unwrap()
            .total,
            2
        );
        let summary = db.validation_summary().unwrap();
        assert_eq!(
            summary["MissingPlaceholder"],
            ValidationKindCounts {
                open: 1,
                ignored: 1
            }
        );
        assert_eq!(
            summary["EmptyTranslation"],
            ValidationKindCounts {
                open: 1,
                ignored: 0
            }
        );
        db.set_validation_issue_ignored(ignored_id, false).unwrap();
        assert_eq!(
            db.validation_summary().unwrap()["MissingPlaceholder"].open,
            2
        );
        assert!(matches!(
            db.set_validation_issue_ignored(-1, true),
            Err(ValidationIssueUpdateError::NotFound)
        ));
    }

    #[tokio::test]
    async fn validation_list_caps_limit_and_keeps_total_beyond_last_page() {
        let db = Database::open_in_memory().unwrap();
        let issues: Vec<_> = (0..503)
            .map(|i| issue(&format!("e{i}"), ValidationKind::EmptyTranslation))
            .collect();
        db.save_validation_issues(&issues).await.unwrap();
        let filter = ValidationIssueFilter {
            limit: usize::MAX,
            ..Default::default()
        };
        let first = db.list_validation_issues(&filter).unwrap();
        assert_eq!(first.issues.len(), 500);
        assert_eq!(first.limit, 500);
        assert_eq!(first.total, 503);
        let last = db
            .list_validation_issues(&ValidationIssueFilter {
                offset: 500,
                ..filter
            })
            .unwrap();
        assert_eq!(last.issues.len(), 3);
        let past = db
            .list_validation_issues(&ValidationIssueFilter {
                offset: usize::MAX,
                ..Default::default()
            })
            .unwrap();
        assert!(past.issues.is_empty());
        assert_eq!(past.total, 503);
    }

    #[tokio::test]
    async fn validation_legacy_duplicates_keep_any_ignored_exact_kind_pair() {
        let db = Database::open_in_memory().unwrap();
        let kind = ValidationKind::MissingPlaceholder {
            placeholder: "{0}".into(),
        };
        let issues = [issue("a", kind.clone()), issue("a", kind)];
        db.save_validation_issues(&issues).await.unwrap();
        let page = db.list_validation_issues(&Default::default()).unwrap();
        db.set_validation_issue_ignored(page.issues[1].id, true)
            .unwrap();
        db.save_validation_issues(&[
            issues[0].clone(),
            issue(
                "a",
                ValidationKind::MissingPlaceholder {
                    placeholder: "{1}".into(),
                },
            ),
            issue("a", ValidationKind::EmptyTranslation),
        ])
        .await
        .unwrap();
        let all = db
            .list_validation_issues(&ValidationIssueFilter {
                include_ignored: true,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(all.total, 3);
        assert!(all.issues[0].ignored);
        assert!(!all.issues[1].ignored);
        assert!(!all.issues[2].ignored);
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
}
