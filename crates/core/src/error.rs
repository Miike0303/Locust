use thiserror::Error;

/// Structured QA detail travels with the original, readable diagnostic.
#[derive(Error, Clone, Debug)]
#[error("{message}")]
pub struct InjectionProvenanceError {
    pub reason: crate::models::InjectionProvenanceReason,
    pub message: String,
}

impl InjectionProvenanceError {
    pub(crate) fn new(
        reason: crate::models::InjectionProvenanceReason,
        message: impl Into<String>,
    ) -> Self {
        Self {
            reason,
            message: message.into(),
        }
    }
}

#[derive(Error, Debug)]
pub enum LocustError {
    #[error("unsupported format: {0}")]
    UnsupportedFormat(String),

    #[error("parse error in {file}: {message}")]
    ParseError { file: String, message: String },

    #[error("injection error: {0}")]
    InjectionError(String),

    #[error(transparent)]
    InjectionProvenance(#[from] InjectionProvenanceError),

    #[error("provider error: {0}")]
    ProviderError(String),

    /// A rejected provider answer can still carry a billable batch receipt.
    /// Display delegates to the original error so diagnostics stay unchanged.
    #[error("{error}")]
    ProviderErrorWithUsage {
        #[source]
        error: Box<LocustError>,
        usage: Box<crate::models::TranslationResult>,
    },

    #[error("provider not configured: {0}")]
    ProviderNotConfigured(String),

    #[error("cost limit exceeded: estimated ${estimated:.4} exceeds limit ${limit:.4}")]
    CostLimitExceeded { estimated: f64, limit: f64 },

    #[error("encoding error: {0}")]
    EncodingError(String),

    #[error("placeholder error in entry {entry_id}: {message}")]
    PlaceholderError { entry_id: String, message: String },

    #[error("validation error in entry {entry_id}: {message}")]
    ValidationError { entry_id: String, message: String },

    #[error("backup error: {0}")]
    BackupError(String),

    #[error("patch verification failed: {0}")]
    PatchVerificationFailed(String),

    #[error("patch downgrade blocked: installed {installed}, incoming {incoming}")]
    PatchDowngradeBlocked { installed: String, incoming: String },

    #[error("unsafe zip entry: {0}")]
    PatchUnsafeEntry(String),

    #[error("legacy patch requires explicit confirmation: {0}")]
    PatchLegacyUnconfirmed(String),

    #[error("patch already applied: {0}")]
    PatchAlreadyApplied(String),

    #[error("patch apply interrupted — run rollback first: {0}")]
    PatchInterrupted(String),

    #[error("game directory not writable: {0}")]
    GameDirNotWritable(String),

    #[error("patch backup incomplete: {0}")]
    PatchBackupIncomplete(String),

    #[error("patch error: {0}")]
    PatchError(String),

    #[error("database error: {0}")]
    DatabaseError(#[from] rusqlite::Error),

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    JsonError(#[from] serde_json::Error),

    #[error("project not found: {0}")]
    ProjectNotFound(String),

    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl LocustError {
    /// Attach usage independently of whether any translation can be accepted.
    pub fn with_usage(self, usage: crate::models::TranslationResult) -> Self {
        Self::ProviderErrorWithUsage {
            error: Box::new(self),
            usage: Box::new(usage),
        }
    }

    pub fn usage(&self) -> Option<&crate::models::TranslationResult> {
        match self {
            Self::ProviderErrorWithUsage { usage, .. } => Some(usage),
            _ => None,
        }
    }

    pub fn usage_mut(&mut self) -> Option<&mut crate::models::TranslationResult> {
        match self {
            Self::ProviderErrorWithUsage { usage, .. } => Some(usage),
            _ => None,
        }
    }

    pub(crate) fn without_usage(self) -> Self {
        match self {
            Self::ProviderErrorWithUsage { error, .. } => *error,
            other => other,
        }
    }
}

pub type Result<T> = std::result::Result<T, LocustError>;
