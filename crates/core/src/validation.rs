use std::collections::HashMap;

use serde::Serialize;

use crate::database::Database;
use crate::error::Result;
use crate::models::{StringEntry, StringStatus, ValidationIssue, ValidationKind};
use crate::placeholder::PlaceholderProcessor;

pub struct Validator;

/// Byte length of `text` under a binary inject encoding.
/// Returns `None` if the text cannot be encoded (e.g. unmappable Shift-JIS).
pub fn encoded_byte_len(encoding: &str, text: &str) -> Option<usize> {
    match encoding {
        "utf8" => Some(text.len()),
        "utf16le" => Some(text.encode_utf16().count() * 2),
        "sjis" | "shift_jis" | "shift-jis" => {
            let (bytes, _, had_errors) = encoding_rs::SHIFT_JIS.encode(text);
            if had_errors {
                None
            } else {
                Some(bytes.len())
            }
        }
        _ => None,
    }
}

/// Issues where translation exceeds a tagged binary inject slot
/// (`metadata.binary_slot` = utf8 / utf16le / sjis). Shared by CLI inject
/// preflight and [`crate::extraction::MultiLangInjector`] so every inject seam
/// surfaces the same problem.
pub fn binary_slot_oversize_issues(entries: &[StringEntry]) -> Vec<ValidationIssue> {
    Validator::validate_all(entries)
        .into_iter()
        .filter(|i| matches!(i.kind, ValidationKind::ExceedsBinarySlot { .. }))
        .collect()
}

pub fn count_binary_slot_oversize(entries: &[StringEntry]) -> usize {
    binary_slot_oversize_issues(entries).len()
}

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

        // Check 5 — Binary inject slot (Unity UTF-8 / Unreal UTF-16LE / Wolf SJIS)
        if !translation.is_empty() {
            if let Some(enc) = entry
                .metadata
                .get("binary_slot")
                .and_then(|v| v.as_str())
            {
                if let (Some(src_len), Some(tr_len)) = (
                    encoded_byte_len(enc, &entry.source),
                    encoded_byte_len(enc, translation),
                ) {
                    if tr_len > src_len {
                        issues.push(ValidationIssue {
                            entry_id: entry.id.clone(),
                            kind: ValidationKind::ExceedsBinarySlot {
                                encoding: enc.to_string(),
                                limit: src_len,
                                actual: tr_len,
                            },
                            message: format!(
                                "translation exceeds binary inject slot ({enc}): {tr_len} > {src_len} bytes"
                            ),
                        });
                    }
                }
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
                ValidationKind::ExceedsBinarySlot { .. } => "ExceedsBinarySlot",
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
    fn test_validate_exceeds_binary_slot_utf8() {
        let mut entry = make_entry("e1", "Hi", Some("Hola amigos"));
        entry.metadata.insert(
            "binary_slot".to_string(),
            serde_json::Value::String("utf8".to_string()),
        );
        let issues = Validator::validate_entry(&entry);
        assert!(
            issues
                .iter()
                .any(|i| matches!(i.kind, ValidationKind::ExceedsBinarySlot { .. })),
            "expected ExceedsBinarySlot, got {issues:?}"
        );
    }

    #[test]
    fn test_validate_binary_slot_utf16_ok_when_fits() {
        // Same char count → same UTF-16LE length for BMP.
        let mut entry = make_entry("e1", "Hero", Some("oreH"));
        entry.metadata.insert(
            "binary_slot".to_string(),
            serde_json::Value::String("utf16le".to_string()),
        );
        let issues = Validator::validate_entry(&entry);
        assert!(
            !issues
                .iter()
                .any(|i| matches!(i.kind, ValidationKind::ExceedsBinarySlot { .. })),
            "unexpected binary slot issues: {issues:?}"
        );
    }

    #[test]
    fn test_encoded_byte_len_sjis() {
        let n = encoded_byte_len("sjis", "テスト").unwrap();
        assert_eq!(n, 6); // 3 CJK × 2 SJIS
        assert!(encoded_byte_len("utf16le", "テスト").unwrap() == 6);
        assert!(encoded_byte_len("utf8", "テスト").unwrap() == 9);
    }

    #[test]
    fn test_count_binary_slot_oversize() {
        let mut over = make_entry("e1", "Hi", Some("Hola amigos"));
        over.metadata.insert(
            "binary_slot".to_string(),
            serde_json::Value::String("utf8".to_string()),
        );
        let mut ok = make_entry("e2", "Hello", Some("Hola!"));
        ok.metadata.insert(
            "binary_slot".to_string(),
            serde_json::Value::String("utf8".to_string()),
        );
        assert_eq!(count_binary_slot_oversize(&[over, ok]), 1);
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
