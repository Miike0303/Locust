use thiserror::Error;

#[derive(Error, Debug)]
pub enum LocustError {
    #[error("unsupported format: {0}")]
    UnsupportedFormat(String),

    #[error("parse error in {file}: {message}")]
    ParseError { file: String, message: String },

    #[error("injection error: {0}")]
    InjectionError(String),

    #[error("provider error: {0}")]
    ProviderError(String),

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

pub type Result<T> = std::result::Result<T, LocustError>;
