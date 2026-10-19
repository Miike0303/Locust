use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use rusqlite::Connection;

mod connection;
mod files;
mod global_memory;
mod glossary;
mod memory;
mod provenance;
mod runs;
mod schema;
mod strings;
mod types;
mod validation;

pub use files::{
    copy_path_chunked, fold_path_case, paths_identical, rel_under_root, sha256_file, sha256_hex,
    sha256_path,
};
pub use global_memory::GlobalMemoryDb;
pub use types::{
    deserialize_expected_translation, EntryFilter, FileStats, GlossaryEntry, GlossaryMergeReport,
    ImportApplyReport, ImportChange, ImportPreview, InjectionRecording, MemoryEntry, MergeStats,
    PivotResult, ProjectStats, RecordedBackup, RecordedFile, StringFacets, TranslationBatchItem,
    TranslationBatchReport, TranslationRun, TranslationSaveOutcome, TRANSLATION_CONFLICT_MESSAGE,
};

pub(crate) use files::path_identity_key;
#[cfg(test)]
pub(crate) use files::SHA256_PATH_CALLS;
pub(crate) use types::TranslationSaveGuard;

pub struct Database {
    conn: Arc<Mutex<Connection>>,
    path: Mutex<PathBuf>,
}

fn lock_connection(conn: &Mutex<Connection>) -> MutexGuard<'_, Connection> {
    conn.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
thread_local! {
    pub(crate) static ENTRY_ROWS_MATERIALIZED: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
    static TRANSLATION_RUN_ROWS_MATERIALIZED: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
    pub(crate) static MEMORY_TRANSACTIONS: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
    pub(crate) static MEMORY_LOOKUP_QUERIES: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
    static STRINGS_QUERIES: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
    static IMPORT_READ_QUERIES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    pub(crate) static GUARDED_TRANSLATION_TRANSACTIONS: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
}

#[cfg(test)]
mod tests;
