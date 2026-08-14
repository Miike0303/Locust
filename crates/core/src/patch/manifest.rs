//! Patch archive manifest, backup manifest, and apply receipt schemas.

use serde::{Deserialize, Serialize};

/// `locust-patch.json` at the root of a patch zip.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PatchManifest {
    pub schema_version: u32,
    pub patch_id: String,
    pub game_name: String,
    pub engine: String,
    pub language: String,
    pub patch_version: String,
    pub generator_version: String,
    pub created_at: String,
    pub files: Vec<PatchFileEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PatchFileEntry {
    /// Game-root-relative path using `/` separators in the archive.
    pub path: String,
    pub patched_sha256: String,
    pub size: u64,
    /// Present only when the packager had a pristine source (`--pristine` or
    /// a committed `.locust/backup/`). Absence degrades verify to structural.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_sha256: Option<String>,
}

impl PatchManifest {
    pub const SCHEMA_VERSION: u32 = 1;
    pub const FILENAME: &'static str = "locust-patch.json";

    /// True when the packager supplied at least one pristine hash → strict
    /// tier is available. Added-only entries legitimately omit
    /// `original_sha256`; structural tier is only for archives where *no*
    /// file carries an original hash.
    pub fn supports_strict_tier(&self) -> bool {
        self.files
            .iter()
            .any(|f| f.original_sha256.as_ref().is_some_and(|h| !h.is_empty()))
    }

    /// Paths that already exist in a pristine game (will be replaced).
    /// Without original hashes we still treat all existing targets as replaced
    /// at structural tier; this helper is for plan-time when originals exist.
    pub fn replaced_paths(&self) -> impl Iterator<Item = &PatchFileEntry> {
        self.files.iter().filter(|f| f.original_sha256.is_some())
    }
}

/// Whether a backup/receipt claims factory-pristine originals.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BackupBaseline {
    Pristine,
    Unverified,
}

/// `.locust/backup/manifest.json` — the backup's commit marker.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BackupManifest {
    pub schema_version: u32,
    pub created_at: String,
    pub baseline: BackupBaseline,
    pub files: Vec<BackupFileEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BackupFileEntry {
    pub path: String,
    pub sha256: String,
    pub size: u64,
}

impl BackupManifest {
    pub const SCHEMA_VERSION: u32 = 1;
}

/// `.locust/receipt.json` written after a successful apply.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Receipt {
    pub schema_version: u32,
    pub patch_id: String,
    pub patch_version: String,
    pub generator_version: String,
    pub language: String,
    pub engine: String,
    pub applied_at: String,
    pub verification: VerificationTier,
    pub forced: bool,
    pub baseline: BackupBaseline,
    pub created_dirs: Vec<String>,
    pub replaced: Vec<ReceiptReplaced>,
    pub added: Vec<ReceiptAdded>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VerificationTier {
    Strict,
    Structural,
    Legacy,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReceiptReplaced {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_sha256: Option<String>,
    pub patched_sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReceiptAdded {
    pub path: String,
    pub patched_sha256: String,
}

impl Receipt {
    pub const SCHEMA_VERSION: u32 = 1;
    pub const FILENAME: &'static str = "receipt.json";
}

/// Mid-operation journal `.locust/journal.json`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Journal {
    pub schema_version: u32,
    pub state: JournalState,
    pub patch_id: String,
    pub plan: ApplyPlan,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum JournalState {
    Applying,
    RollingBack,
}

/// Planned file operations for one apply (persisted in the journal).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ApplyPlan {
    pub patch_version: String,
    pub language: String,
    pub engine: String,
    pub generator_version: String,
    pub verification: VerificationTier,
    pub forced: bool,
    pub baseline: BackupBaseline,
    pub replaced: Vec<ReceiptReplaced>,
    pub added: Vec<ReceiptAdded>,
    pub created_dirs: Vec<String>,
}

impl Journal {
    pub const SCHEMA_VERSION: u32 = 1;
    pub const FILENAME: &'static str = "journal.json";
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_round_trip() {
        let m = PatchManifest {
            schema_version: 1,
            patch_id: "id".into(),
            game_name: "g".into(),
            engine: "renpy".into(),
            language: "es".into(),
            patch_version: "1.0.0".into(),
            generator_version: "0.1.0".into(),
            created_at: "t".into(),
            files: vec![PatchFileEntry {
                path: "game/a.rpy".into(),
                patched_sha256: "aa".into(),
                size: 1,
                original_sha256: Some("bb".into()),
            }],
        };
        let j = serde_json::to_string(&m).unwrap();
        let back: PatchManifest = serde_json::from_str(&j).unwrap();
        assert_eq!(m, back);
        assert!(m.supports_strict_tier());
    }

    #[test]
    fn structural_when_originals_missing() {
        let m = PatchManifest {
            schema_version: 1,
            patch_id: "id".into(),
            game_name: "g".into(),
            engine: "rpgmaker_mv".into(),
            language: "es".into(),
            patch_version: "1.0.0".into(),
            generator_version: "0.1.0".into(),
            created_at: "t".into(),
            files: vec![PatchFileEntry {
                path: "www/data/Map001.json".into(),
                patched_sha256: "aa".into(),
                size: 1,
                original_sha256: None,
            }],
        };
        assert!(!m.supports_strict_tier());
    }

    #[test]
    fn strict_when_at_least_one_original_even_with_adds() {
        let m = PatchManifest {
            schema_version: 1,
            patch_id: "id".into(),
            game_name: "g".into(),
            engine: "renpy".into(),
            language: "es".into(),
            patch_version: "1.0.0".into(),
            generator_version: "0.1.0".into(),
            created_at: "t".into(),
            files: vec![
                PatchFileEntry {
                    path: "game/a.rpy".into(),
                    patched_sha256: "aa".into(),
                    size: 1,
                    original_sha256: Some("bb".into()),
                },
                PatchFileEntry {
                    path: "game/new.rpy".into(),
                    patched_sha256: "cc".into(),
                    size: 1,
                    original_sha256: None,
                },
            ],
        };
        assert!(m.supports_strict_tier());
    }
}
