use locust_core::database::Database;
use locust_core::models::{StringEntry, StringStatus};

/// Process-global env vars — serialize tests that select backup/data roots.
pub(crate) static BACKUP_ROOT_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

pub(crate) struct EnvVarRestore {
    name: &'static str,
    previous: Option<std::ffi::OsString>,
}

impl EnvVarRestore {
    pub(crate) fn capture(name: &'static str) -> Self {
        Self {
            name,
            previous: std::env::var_os(name),
        }
    }
}

impl Drop for EnvVarRestore {
    fn drop(&mut self) {
        match self.previous.take() {
            Some(value) => std::env::set_var(self.name, value),
            None => std::env::remove_var(self.name),
        }
    }
}

pub(crate) struct TestProfile {
    _backup_root_restore: EnvVarRestore,
    _data_dir_restore: EnvVarRestore,
    _root: tempfile::TempDir,
    _lock: tokio::sync::MutexGuard<'static, ()>,
}

impl TestProfile {
    fn new_with_lock(lock: tokio::sync::MutexGuard<'static, ()>) -> Self {
        let root = tempfile::tempdir().unwrap();
        let backup_root_restore = EnvVarRestore::capture("LOCUST_BACKUP_ROOT");
        let data_dir_restore = EnvVarRestore::capture("LOCUST_DATA_DIR");
        std::env::set_var("LOCUST_DATA_DIR", root.path().join("profile"));
        std::env::set_var("LOCUST_BACKUP_ROOT", root.path().join("backups"));
        Self {
            _backup_root_restore: backup_root_restore,
            _data_dir_restore: data_dir_restore,
            _root: root,
            _lock: lock,
        }
    }

    pub(crate) fn new() -> Self {
        Self::new_with_lock(BACKUP_ROOT_LOCK.blocking_lock())
    }

    pub(crate) async fn new_async() -> Self {
        Self::new_with_lock(BACKUP_ROOT_LOCK.lock().await)
    }
}

pub(crate) fn patch_test_tempdir() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("locust_patch_test_")
        .tempdir()
        .unwrap()
}

/// One translated entry, in the exact shape `locust extract` +
/// `locust translate`/`locust import` leave in the database.
pub(crate) fn save_translated(
    db: &Database,
    id: &str,
    source: &str,
    path: &std::path::Path,
    t: &str,
) {
    let mut entry = StringEntry::new(id, source, path.to_path_buf());
    entry.translation = Some(t.to_string());
    entry.status = StringStatus::Translated;
    db.save_entries(&[entry]).unwrap();
}
