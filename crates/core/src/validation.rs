use std::collections::HashMap;

use serde::Serialize;

use crate::database::Database;
use crate::error::Result;
use crate::models::{StringEntry, StringStatus, ValidationIssue, ValidationKind};
use crate::placeholder::PlaceholderProcessor;

pub struct Validator;

impl Validator {
    pub fn validate_entry(entry: &StringEntry) -> Vec<ValidationIssue> {
        let mut issues = Vec::new();
        let translation = entry.translation.as_deref().unwrap_or("");

        // Check 1 — EmptyTranslation
        if translation.trim().is_empty() && entry.status == StringStatus::Translated {
            issues.push(ValidationIssue {
                entry_id: entry.id.clone(),
                kind: ValidationKind::EmptyTranslation,
                message: "translation is empty".to_string(),
            });
        }

        // Check 2 — IdenticalToSource
        if !translation.is_empty() && translation.trim() == entry.source.trim() {
            issues.push(ValidationIssue {
                entry_id: entry.id.clone(),
                kind: ValidationKind::IdenticalToSource,
                message: "translation is identical to source".to_string(),
            });
        }

        // Check 3 — ExceedsCharLimit
        if let Some(limit) = entry.char_limit {
            let actual = translation.chars().count();
            if actual > limit {
                issues.push(ValidationIssue {
                    entry_id: entry.id.clone(),
                    kind: ValidationKind::ExceedsCharLimit { limit, actual },
                    message: format!(
                        "translation exceeds char limit: {} > {}",
                        actual, limit
                    ),
                });
            }
        }

        // Check 4 — Placeholder mismatches
        if !translation.is_empty() {
            let mismatches = PlaceholderProcessor::validate(&entry.source, translation);
            for m in mismatches {
                let kind = match m.kind {
                    crate::placeholder::MismatchKind::Missing => {
                        ValidationKind::MissingPlaceholder {
                            placeholder: m.placeholder.clone(),
                        }
                    }
                    crate::placeholder::MismatchKind::Extra => {
                        ValidationKind::ExtraPlaceholder {
                            placeholder: m.placeholder.clone(),
                        }
                    }
                };
                issues.push(ValidationIssue {
                    entry_id: entry.id.clone(),
                    kind,
                    message: format!("placeholder mismatch: {}", m.placeholder),
                });
            }
        }

        issues
    }

    pub fn validate_all(entries: &[StringEntry]) -> Vec<ValidationIssue> {
        entries
            .iter()
            .flat_map(|e| Self::validate_entry(e))
            .collect()
    }

    pub async fn validate_and_save(
        entries: &[StringEntry],
        db: &Database,
    ) -> Result<ValidationReport> {
        let issues = Self::validate_all(entries);

        db.save_validation_issues(&issues).await?;

        let mut entries_with_issues = std::collections::HashSet::new();
        let mut by_kind: HashMap<String, usize> = HashMap::new();
        for issue in &issues {
            entries_with_issues.insert(&issue.entry_id);
            let kind_name = match &issue.kind {
                ValidationKind::MissingPlaceholder { .. } => "MissingPlaceholder",
                ValidationKind::ExtraPlaceholder { .. } => "ExtraPlaceholder",
                ValidationKind::ExceedsCharLimit { .. } => "ExceedsCharLimit",
                ValidationKind::EmptyTranslation => "EmptyTranslation",
                ValidationKind::IdenticalToSource => "IdenticalToSource",
            };
            *by_kind.entry(kind_name.to_string()).or_insert(0) += 1;
        }

        Ok(ValidationReport {
            total_checked: entries.len(),
            issues_found: issues.len(),
            entries_with_issues: entries_with_issues.len(),
            by_kind,
        })
    }
}

#[derive(Debug, Serialize)]
pub struct ValidationReport {
    pub total_checked: usize,
    pub issues_found: usize,
    pub entries_with_issues: usize,
    pub by_kind: HashMap<String, usize>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::Arc;

    fn make_entry(id: &str, source: &str, translation: Option<&str>) -> StringEntry {
        let mut e = StringEntry::new(id, source, PathBuf::from("test.json"));
        if let Some(t) = translation {
            e.translation = Some(t.to_string());
            e.status = StringStatus::Translated;
        }
        e
    }

    #[test]
    fn test_validate_empty_translation() {
        let entry = make_entry("e1", "Hello", Some(""));
        let issues = Validator::validate_entry(&entry);
        assert!(issues
            .iter()
            .any(|i| matches!(i.kind, ValidationKind::EmptyTranslation)));
    }

    #[test]
    fn test_validate_identical_to_source() {
        let entry = make_entry("e1", "Hello", Some("Hello"));
        let issues = Validator::validate_entry(&entry);
        assert!(issues
            .iter()
            .any(|i| matches!(i.kind, ValidationKind::IdenticalToSource)));
    }

    #[test]
    fn test_validate_exceeds_char_limit() {
        let mut entry = make_entry("e1", "Hi", Some("This is a long translation"));
        entry.char_limit = Some(10);
        let issues = Validator::validate_entry(&entry);
        assert!(issues
            .iter()
            .any(|i| matches!(i.kind, ValidationKind::ExceedsCharLimit { .. })));
    }

    #[test]
    fn test_validate_missing_placeholder() {
        let entry = make_entry("e1", r"\c[2]Hello", Some("Hola"));
        let issues = Validator::validate_entry(&entry);
        assert!(issues
            .iter()
            .any(|i| matches!(i.kind, ValidationKind::MissingPlaceholder { .. })));
    }

    #[test]
    fn test_validate_clean_entry() {
        let entry = make_entry("e1", "Hello", Some("Hola"));
        let issues = Validator::validate_entry(&entry);
        assert!(issues.is_empty());
    }

    #[test]
    fn test_validate_all_aggregates() {
        let entries = vec![
            make_entry("e1", "Hello", Some("")),   // EmptyTranslation
            make_entry("e2", "World", Some("World")), // IdenticalToSource
            {
                let mut e = make_entry("e3", "Hi", Some("Very long translation here!"));
                e.char_limit = Some(5);
                e
            }, // ExceedsCharLimit
        ];
        let issues = Validator::validate_all(&entries);
        assert_eq!(issues.len(), 3);
    }

    #[tokio::test]
    async fn test_validation_report_counts() {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let entries = vec![
            make_entry("e1", "Hello", Some("")),
            make_entry("e2", "World", Some("Mundo")),
        ];
        db.save_entries(&entries).unwrap();

        let report = Validator::validate_and_save(&entries, &db).await.unwrap();

        assert_eq!(report.total_checked, 2);
        assert_eq!(report.issues_found, 1);
        assert_eq!(report.entries_with_issues, 1);
    }
}
