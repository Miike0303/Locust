use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StringEntry {
    pub id: String,
    pub source: String,
    pub translation: Option<String>,
    pub file_path: PathBuf,
    pub context: Option<String>,
    pub tags: Vec<String>,
    pub metadata: HashMap<String, serde_json::Value>,
    pub status: StringStatus,
    pub provider_used: Option<String>,
    pub char_limit: Option<usize>,
    pub created_at: DateTime<Utc>,
    pub translated_at: Option<DateTime<Utc>>,
    pub reviewed_at: Option<DateTime<Utc>>,
}

impl StringEntry {
    pub fn new(id: impl Into<String>, source: impl Into<String>, file_path: PathBuf) -> Self {
        Self {
            id: id.into(),
            source: source.into(),
            translation: None,
            file_path,
            context: None,
            tags: Vec::new(),
            metadata: HashMap::new(),
            status: StringStatus::Pending,
            provider_used: None,
            char_limit: None,
            created_at: Utc::now(),
            translated_at: None,
            reviewed_at: None,
        }
    }

    pub fn with_context(mut self, ctx: impl Into<String>) -> Self {
        self.context = Some(ctx.into());
        self
    }

    pub fn with_tags(mut self, tags: Vec<String>) -> Self {
        self.tags = tags;
        self
    }

    pub fn with_char_limit(mut self, limit: usize) -> Self {
        self.char_limit = Some(limit);
        self
    }

    pub fn source_hash(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(self.source.as_bytes());
        hex::encode(hasher.finalize())
    }

    pub fn is_translatable(&self) -> bool {
        !self.source.trim().is_empty() && self.status != StringStatus::Approved
    }

    pub fn translation_exceeds_limit(&self) -> bool {
        match (&self.translation, self.char_limit) {
            (Some(t), Some(limit)) => t.len() > limit,
            _ => false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StringStatus {
    Pending,
    Translated,
    Reviewed,
    Approved,
    Error,
}

impl fmt::Display for StringStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            StringStatus::Pending => "pending",
            StringStatus::Translated => "translated",
            StringStatus::Reviewed => "reviewed",
            StringStatus::Approved => "approved",
            StringStatus::Error => "error",
        };
        write!(f, "{}", s)
    }
}

impl FromStr for StringStatus {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "pending" => Ok(StringStatus::Pending),
            "translated" => Ok(StringStatus::Translated),
            "reviewed" => Ok(StringStatus::Reviewed),
            "approved" => Ok(StringStatus::Approved),
            "error" => Ok(StringStatus::Error),
            _ => Err(anyhow::anyhow!("unknown status: {}", s)),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputMode {
    Replace,
    Add,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TranslationRequest {
    pub entry_id: String,
    pub source: String,
    pub source_lang: String,
    pub target_lang: String,
    pub context: Option<String>,
    pub glossary_hint: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TranslationResult {
    pub entry_id: String,
    pub translation: String,
    pub detected_source_lang: Option<String>,
    pub provider: String,
    pub tokens_used: Option<u32>,
    pub cost_usd: Option<f64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ValidationIssue {
    pub entry_id: String,
    pub kind: ValidationKind,
    pub message: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ValidationKind {
    MissingPlaceholder { placeholder: String },
    ExtraPlaceholder { placeholder: String },
    ExceedsCharLimit { limit: usize, actual: usize },
    EmptyTranslation,
    IdenticalToSource,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProgressEvent {
    Started {
        total: usize,
        job_id: String,
    },
    BatchCompleted {
        completed: usize,
        total: usize,
        cost_so_far: f64,
        language: Option<String>,
    },
    StringTranslated {
        entry_id: String,
        translation: String,
    },
    ValidationFailed {
        issues: Vec<ValidationIssue>,
    },
    Paused,
    Resumed,
    Completed {
        total_translated: usize,
        total_cost: f64,
        duration_secs: f64,
    },
    Failed {
        entry_id: Option<String>,
        error: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_string_entry_new() {
        let entry = StringEntry::new("test_id", "Hello world", PathBuf::from("test.json"));
        assert_eq!(entry.id, "test_id");
        assert_eq!(entry.source, "Hello world");
        assert_eq!(entry.file_path, PathBuf::from("test.json"));
        assert!(entry.translation.is_none());
        assert!(entry.context.is_none());
        assert!(entry.tags.is_empty());
        assert!(entry.metadata.is_empty());
        assert_eq!(entry.status, StringStatus::Pending);
        assert!(entry.provider_used.is_none());
        assert!(entry.char_limit.is_none());
        assert!(entry.translated_at.is_none());
        assert!(entry.reviewed_at.is_none());
    }

    #[test]
    fn test_string_entry_is_translatable_empty_source() {
        let entry = StringEntry::new("id", "   ", PathBuf::from("f.json"));
        assert!(!entry.is_translatable());
    }

    #[test]
    fn test_string_entry_is_translatable_approved() {
        let mut entry = StringEntry::new("id", "Hello", PathBuf::from("f.json"));
        entry.status = StringStatus::Approved;
        assert!(!entry.is_translatable());
    }

    #[test]
    fn test_string_entry_is_translatable_pending() {
        let entry = StringEntry::new("id", "Hello", PathBuf::from("f.json"));
        assert!(entry.is_translatable());
    }

    #[test]
    fn test_source_hash_deterministic() {
        let a = StringEntry::new("id", "Hello", PathBuf::from("f.json"));
        let b = StringEntry::new("id", "Hello", PathBuf::from("f.json"));
        assert_eq!(a.source_hash(), b.source_hash());
    }

    #[test]
    fn test_source_hash_different() {
        let a = StringEntry::new("id", "Hello", PathBuf::from("f.json"));
        let b = StringEntry::new("id", "World", PathBuf::from("f.json"));
        assert_ne!(a.source_hash(), b.source_hash());
    }

    #[test]
    fn test_status_roundtrip() {
        let variants = vec![
            StringStatus::Pending,
            StringStatus::Translated,
            StringStatus::Reviewed,
            StringStatus::Approved,
            StringStatus::Error,
        ];
        for status in variants {
            let s = status.to_string();
            let parsed: StringStatus = s.parse().unwrap();
            assert_eq!(parsed, status);
        }
    }

    #[test]
    fn test_translation_exceeds_limit() {
        let mut entry = StringEntry::new("id", "Hi", PathBuf::from("f.json"))
            .with_char_limit(10);
        entry.translation = Some("hello world".to_string()); // 11 chars
        assert!(entry.translation_exceeds_limit());
    }

    #[test]
    fn test_progress_event_serialize() {
        let started = ProgressEvent::Started {
            total: 100,
            job_id: "job-1".to_string(),
        };
        let json = serde_json::to_string(&started).unwrap();
        let back: ProgressEvent = serde_json::from_str(&json).unwrap();
        match back {
            ProgressEvent::Started { total, job_id } => {
                assert_eq!(total, 100);
                assert_eq!(job_id, "job-1");
            }
            _ => panic!("wrong variant"),
        }

        let completed = ProgressEvent::Completed {
            total_translated: 50,
            total_cost: 1.23,
            duration_secs: 45.0,
        };
        let json = serde_json::to_string(&completed).unwrap();
        let back: ProgressEvent = serde_json::from_str(&json).unwrap();
        match back {
            ProgressEvent::Completed {
                total_translated,
                total_cost,
                duration_secs,
            } => {
                assert_eq!(total_translated, 50);
                assert!((total_cost - 1.23).abs() < f64::EPSILON);
                assert!((duration_secs - 45.0).abs() < f64::EPSILON);
            }
            _ => panic!("wrong variant"),
        }

        let failed = ProgressEvent::Failed {
            entry_id: Some("e1".to_string()),
            error: "timeout".to_string(),
        };
        let json = serde_json::to_string(&failed).unwrap();
        let back: ProgressEvent = serde_json::from_str(&json).unwrap();
        match back {
            ProgressEvent::Failed { entry_id, error } => {
                assert_eq!(entry_id, Some("e1".to_string()));
                assert_eq!(error, "timeout");
            }
            _ => panic!("wrong variant"),
        }
    }
}
