use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const INJECTION_SOURCE_METADATA_KEY: &str = "locust_injection_source";
pub const INJECTION_CAPACITY_METADATA_KEY: &str = "locust_injection_capacity";
pub const STALE_TRANSLATION_METADATA_KEY: &str = "locust_stale_translation";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StringEntry {
    pub id: String,
    pub source: String,
    pub translation: Option<String>,
    pub file_path: PathBuf,
    pub context: Option<String>,
    pub tags: Vec<String>,
    pub metadata: HashMap<String, serde_json::Value>,
    /// Immutable physical TextAsset shared by its localization rows. Persisted
    /// separately by Database; JSON carries only its content-addressed reference.
    #[serde(skip)]
    pub textasset_original: Option<Arc<TextAssetOriginal>>,
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
            textasset_original: None,
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

    /// Re-extraction may retain an old translation for review, but it must not
    /// become an injected translation or a new pivot source without acceptance.
    /// Legacy rows without this provenance marker remain compatible regardless
    /// of their status. Any malformed marker also fails closed.
    pub fn require_current_translation(&self) -> std::result::Result<(), String> {
        let Some(marker) = self.metadata.get(STALE_TRANSLATION_METADATA_KEY) else {
            return Ok(());
        };
        let hash_is_valid = |key| {
            marker
                .get(key)
                .and_then(serde_json::Value::as_str)
                .is_some_and(|hash| {
                    hash.len() == 64
                        && hash
                            .bytes()
                            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                })
        };
        let valid = marker.get("version").and_then(serde_json::Value::as_u64) == Some(1)
            && hash_is_valid("translated_source_sha256")
            && hash_is_valid("current_source_sha256")
            && marker
                .get("current_source_sha256")
                .and_then(serde_json::Value::as_str)
                == Some(self.source_hash().as_str());
        if valid {
            Err(format!("stale translation for entry '{}': source changed after translation; translate again or explicitly mark Reviewed/Approved after checking the current source", self.id))
        } else {
            Err(format!("entry '{}' has malformed {STALE_TRANSLATION_METADATA_KEY} metadata; save a current translation or explicitly review it before injection or pivot", self.id))
        }
    }

    /// Validate controls against both the semantic request source and the
    /// immutable physical baseline. A pivot must not hide a control lost by an
    /// earlier translation. None is pending; Some("") still deletes controls.
    pub fn translation_control_mismatches(
        &self,
    ) -> std::result::Result<Vec<crate::placeholder::PlaceholderMismatch>, String> {
        let Some(translation) = self.translation.as_deref() else {
            return Ok(Vec::new());
        };
        let physical = self.injection_source()?;
        let mut mismatches =
            crate::placeholder::PlaceholderProcessor::validate(&self.source, translation);
        if physical != self.source {
            for mismatch in
                crate::placeholder::PlaceholderProcessor::validate(physical, translation)
            {
                if !mismatches.contains(&mismatch) {
                    mismatches.push(mismatch);
                }
            }
        }
        Ok(mismatches)
    }

    /// Gate for pivot/insertion callers; invoke before changing `source` from
    /// its semantic value to physical bytes and before writing backup/game data.
    pub fn require_preserved_translation_controls(&self) -> std::result::Result<(), String> {
        let mismatches = self.translation_control_mismatches()?;
        if mismatches.is_empty() {
            return Ok(());
        }
        let details = mismatches
            .iter()
            .map(|m| format!("{:?}: {}", m.kind, m.placeholder))
            .collect::<Vec<_>>()
            .join("; ");
        Err(format!("translation for entry '{}' changed protected controls relative to its semantic or physical source: {details}", self.id))
    }

    /// Text that must exist in the untouched game when this row is injected.
    /// Pivoted projects keep their translated `source` for translation/export,
    /// while this value remains anchored to the first extracted baseline.
    pub fn injection_source(&self) -> std::result::Result<&str, String> {
        match self.metadata.get(INJECTION_SOURCE_METADATA_KEY) {
            None => Ok(&self.source),
            Some(serde_json::Value::String(source)) if !source.is_empty() => Ok(source),
            Some(_) => Err(format!(
                "entry '{}' has malformed {} metadata",
                self.id, INJECTION_SOURCE_METADATA_KEY
            )),
        }
    }

    /// Explicit immutable binary capacity recorded by the first pivot.
    pub fn injection_capacity(&self) -> std::result::Result<Option<(&str, usize)>, String> {
        let Some(value) = self.metadata.get(INJECTION_CAPACITY_METADATA_KEY) else {
            return Ok(None);
        };
        let Some(object) = value.as_object() else {
            return Err(format!(
                "entry '{}' has malformed {} metadata",
                self.id, INJECTION_CAPACITY_METADATA_KEY
            ));
        };
        let Some(encoding) = object.get("encoding").and_then(|v| v.as_str()) else {
            return Err(format!(
                "entry '{}' has malformed {}.encoding metadata",
                self.id, INJECTION_CAPACITY_METADATA_KEY
            ));
        };
        let Some(bytes) = object
            .get("bytes")
            .and_then(|v| v.as_u64())
            .and_then(|n| usize::try_from(n).ok())
        else {
            return Err(format!(
                "entry '{}' has malformed {}.bytes metadata",
                self.id, INJECTION_CAPACITY_METADATA_KEY
            ));
        };
        Ok(Some((encoding, bytes)))
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

/// Verified once when created or loaded; private fields keep the cached digest
/// bound to immutable text. Cloning a StringEntry never duplicates the payload.
#[derive(Clone, Debug)]
pub struct TextAssetOriginal {
    text: Arc<str>,
    sha256: String,
}

impl TextAssetOriginal {
    pub fn new(text: &str) -> std::result::Result<Self, String> {
        if text.is_empty() || text.len() > crate::textasset_group::MAX_GROUP_ORIGINAL_BYTES {
            return Err("TextAsset original must contain 1..=1048576 UTF-8 bytes".into());
        }
        Ok(Self {
            text: Arc::from(text),
            sha256: hex::encode(Sha256::digest(text.as_bytes())),
        })
    }

    pub fn text(&self) -> &str {
        &self.text
    }
    pub fn shared_text(&self) -> Arc<str> {
        Arc::clone(&self.text)
    }
    pub fn sha256(&self) -> &str {
        &self.sha256
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
    /// Prompt (input) tokens for the batch, when the provider reports them.
    #[serde(default)]
    pub input_tokens: Option<u32>,
    /// Completion (output) tokens for the batch, when the provider reports them.
    #[serde(default)]
    pub output_tokens: Option<u32>,
    pub cost_usd: Option<f64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ValidationIssue {
    pub entry_id: String,
    pub kind: ValidationKind,
    pub message: String,
    /// Optional source snippet for UI (not persisted in validation table).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ValidationKind {
    MissingPlaceholder {
        placeholder: String,
    },
    ExtraPlaceholder {
        placeholder: String,
    },
    ExceedsCharLimit {
        limit: usize,
        actual: usize,
    },
    /// Binary inject slot overflow (Unity UTF-8 / Unreal UTF-16LE / Wolf Shift-JIS).
    ExceedsBinarySlot {
        encoding: String,
        limit: usize,
        actual: usize,
    },
    EmptyTranslation,
    IdenticalToSource,
    InvalidInjectionProvenance,
    StaleTranslation,
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
        /// False means the numeric amount is only the observed subtotal.
        #[serde(default)]
        cost_is_complete: bool,
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
        #[serde(default)]
        cost_is_complete: bool,
        duration_secs: f64,
    },
    /// Recoverable failure: remaining batches and provider fallbacks may continue.
    BatchFailed {
        entry_id: Option<String>,
        error: String,
    },
    /// Terminal job failure; consumers may close the progress stream.
    Failed {
        entry_id: Option<String>,
        error: String,
    },
    /// Emitted when a multi-provider fallback chain advances to the next provider.
    ProviderSwitched {
        provider_id: String,
        provider_name: String,
        remaining_pending: usize,
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
    fn injection_provenance_accessors_are_fail_closed() {
        let mut entry = StringEntry::new("id", "semantic", PathBuf::from("f"));
        assert_eq!(entry.injection_source().unwrap(), "semantic");
        entry.metadata.insert(
            INJECTION_SOURCE_METADATA_KEY.into(),
            serde_json::json!("physical"),
        );
        entry.metadata.insert(
            INJECTION_CAPACITY_METADATA_KEY.into(),
            serde_json::json!({"encoding": "utf8", "bytes": 12}),
        );
        assert_eq!(entry.injection_source().unwrap(), "physical");
        assert_eq!(entry.injection_capacity().unwrap(), Some(("utf8", 12)));

        entry.metadata.insert(
            INJECTION_SOURCE_METADATA_KEY.into(),
            serde_json::Value::Null,
        );
        assert!(entry.injection_source().is_err());
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
        let mut entry = StringEntry::new("id", "Hi", PathBuf::from("f.json")).with_char_limit(10);
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
            cost_is_complete: true,
            duration_secs: 45.0,
        };
        let json = serde_json::to_string(&completed).unwrap();
        let back: ProgressEvent = serde_json::from_str(&json).unwrap();
        match back {
            ProgressEvent::Completed {
                total_translated,
                total_cost,
                duration_secs,
                ..
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

        let switched = ProgressEvent::ProviderSwitched {
            provider_id: "lmstudio".into(),
            provider_name: "LM Studio".into(),
            remaining_pending: 12,
        };
        let json = serde_json::to_string(&switched).unwrap();
        assert!(json.contains("provider_switched"), "{json}");
        let back: ProgressEvent = serde_json::from_str(&json).unwrap();
        match back {
            ProgressEvent::ProviderSwitched {
                provider_id,
                remaining_pending,
                ..
            } => {
                assert_eq!(provider_id, "lmstudio");
                assert_eq!(remaining_pending, 12);
            }
            _ => panic!("wrong variant"),
        }
    }
    #[test]
    fn control_gate_distinguishes_pending_empty_and_physical_baseline() {
        let mut entry = StringEntry::new("id", "Hello", PathBuf::from("script.rpy"));
        entry.metadata.insert(
            INJECTION_SOURCE_METADATA_KEY.into(),
            serde_json::json!("JA {name}"),
        );
        assert!(entry.require_preserved_translation_controls().is_ok());
        for target in ["", "   ", "Hola"] {
            entry.translation = Some(target.into());
            assert!(entry.require_preserved_translation_controls().is_err());
        }
        entry.source = "Hello {name}".into();
        entry.translation = Some("Hola {name}".into());
        assert!(entry.require_preserved_translation_controls().is_ok());
        assert_eq!(entry.source, "Hello {name}");
        assert_eq!(entry.injection_source().unwrap(), "JA {name}");
        entry
            .metadata
            .insert(INJECTION_SOURCE_METADATA_KEY.into(), serde_json::json!(123));
        assert!(entry.require_preserved_translation_controls().is_err());
    }
}
