use rusqlite::params;

use crate::error::Result;
use crate::models::{ValidationIssue, ValidationKind};

use super::{lock_connection, Database};

impl Database {
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
}

#[cfg(test)]
mod tests {
    use super::*;

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
