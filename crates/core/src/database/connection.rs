use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rusqlite::{params, Connection, OpenFlags};

use crate::error::{LocustError, Result};

use super::schema::init_schema;
use super::{lock_connection, Database};

impl Database {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let conn = Connection::open(path)?;
        init_schema(&conn)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            path: Mutex::new(path.to_path_buf()),
        })
    }

    /// Open an existing database without creating directories or migrating it.
    fn open_for_snapshot(path: &Path) -> Result<Self> {
        let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            path: Mutex::new(path.to_path_buf()),
        })
    }

    /// Consistent saved-project reads without schema/configuration writes.
    /// Uses normal read-only SQLite (including committed WAL), not immutable
    /// mode. SQLite may create WAL/SHM sidecars alongside the saved database.
    pub(crate) fn read_project_snapshot(path: &Path) -> Result<Self> {
        let db = Self::open_for_snapshot(path)?;
        lock_connection(&db.conn).execute_batch("BEGIN DEFERRED TRANSACTION")?;
        Ok(db)
    }

    /// Copy `src` with [`Database::snapshot_to`] without modifying that file.
    /// A read-only snapshot can leave an empty `-wal`/`-shm` behind; those are
    /// removed when this call created them. Sidecars that were already there stay.
    pub fn snapshot_existing(src: &Path, dest: &Path) -> Result<()> {
        let mut prior = Vec::new();
        for suffix in ["-wal", "-shm", "-journal"] {
            let path = sqlite_sidecar(src, suffix);
            let existed = path.try_exists()?;
            prior.push((path, existed));
        }
        let db = Self::open_for_snapshot(src)?;
        let result = db.snapshot_to(dest);
        drop(db);
        for (path, existed) in prior {
            if !existed {
                let _ = std::fs::remove_file(path);
            }
        }
        result
    }

    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        init_schema(&conn)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            path: Mutex::new(PathBuf::from(":memory:")),
        })
    }

    /// On-disk path of the live connection (`:memory:` for an in-memory DB).
    pub fn path(&self) -> PathBuf {
        self.path.lock().unwrap().clone()
    }

    /// Write a consistent standalone checkpoint, including committed WAL data.
    pub fn snapshot_to(&self, dest: &Path) -> Result<()> {
        crate::export::check_export_destination(dest, &self.path())?;
        if dest.try_exists()? {
            return Err(LocustError::Other(anyhow::anyhow!(
                "checkpoint destination already exists: {}",
                dest.display()
            )));
        }
        let dest = dest.to_str().ok_or_else(|| {
            LocustError::Other(anyhow::anyhow!("checkpoint destination is not valid UTF-8"))
        })?;
        lock_connection(&self.conn).execute("VACUUM INTO ?1", params![dest])?;
        Ok(())
    }

    /// Swap the live connection to `path` in place and run the same schema
    /// init as [`Database::open`]. Shared `Arc<Database>` handlers keep working.
    pub fn reopen(&self, path: &Path) -> Result<()> {
        self.replace_with(Self::open(path)?);
        Ok(())
    }

    /// Publish a fully prepared candidate without reopening or other fallible I/O.
    /// Callers must finish validation and merge work before replacing the live DB.
    pub(crate) fn replace_with(&self, candidate: Self) {
        let candidate_path = candidate.path();
        let mut conn = lock_connection(&self.conn);
        let mut path = self.path.lock().unwrap();
        std::mem::swap(&mut *conn, &mut *lock_connection(&candidate.conn));
        *path = candidate_path;
    }
}

fn sqlite_sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    path.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::super::tests::{make_entry, recording_tempdir};
    use super::super::EntryFilter;
    use super::*;

    #[test]
    fn test_open_in_memory() {
        let db = Database::open_in_memory().unwrap();
        assert!(db.get_entries(&EntryFilter::default()).unwrap().is_empty());
    }

    #[test]
    fn poisoned_connection_allows_later_queries() {
        let db = Database::open_in_memory().unwrap();
        let conn = db.conn.clone();
        assert!(std::thread::spawn(move || {
            let _guard = conn.lock().unwrap();
            panic!("simulate a worker panic while holding the connection");
        })
        .join()
        .is_err());

        assert_eq!(db.get_project_metadata("after_panic").unwrap(), None);
        db.set_project_metadata("after_panic", &serde_json::json!(true))
            .unwrap();
        assert_eq!(
            db.get_project_metadata("after_panic").unwrap(),
            Some(serde_json::json!(true))
        );
    }

    #[test]
    fn readonly_snapshot_leaves_source_bytes_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("project.locust.db");
        let db = Database::open(&path).unwrap();
        db.save_entries(&[make_entry("a", "Hello")]).unwrap();
        drop(db);
        let before = std::fs::read(&path).unwrap();
        let wal = dir.path().join("project.locust.db-wal");
        let shm = dir.path().join("project.locust.db-shm");
        let wal_before = wal.is_file().then(|| std::fs::read(&wal).unwrap());
        let shm_before = shm.is_file().then(|| std::fs::read(&shm).unwrap());
        let dest = dir.path().join("copy.locust.db");
        Database::snapshot_existing(&path, &dest).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), before);
        let wal_after = wal.is_file().then(|| std::fs::read(&wal).unwrap());
        let shm_after = shm.is_file().then(|| std::fs::read(&shm).unwrap());
        assert_eq!(wal_after, wal_before);
        assert_eq!(shm_after, shm_before);
        let copied = Database::open(&dest).unwrap();
        assert_eq!(
            copied.get_entries(&EntryFilter::default()).unwrap().len(),
            1
        );
    }

    fn schema_tables(db: &Database) -> Vec<String> {
        let conn = db.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
            )
            .unwrap();
        stmt.query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
    }

    #[test]
    fn test_reopen_swaps_connection_and_preserves_original() {
        let base_guard = recording_tempdir();
        let base = base_guard.path().to_path_buf();
        let path_a = base.join("a.locust.db");
        let path_b = base.join("b.locust.db");

        let db = Database::open(&path_a).unwrap();
        assert_eq!(db.path(), path_a);
        db.save_entries(&[make_entry("keep-me", "Hello")]).unwrap();

        db.reopen(&path_b).unwrap();
        assert_eq!(db.path(), path_b);
        assert!(
            db.get_entries(&EntryFilter::default()).unwrap().is_empty(),
            "reopened path B must start empty"
        );
        let tables = schema_tables(&db);
        for required in [
            "strings",
            "glossary",
            "translation_memory",
            "validation_issues",
            "translation_runs",
            "injected_files",
        ] {
            assert!(
                tables.iter().any(|t| t == required),
                "reopened DB missing table {required}, have {tables:?}"
            );
        }

        db.reopen(&path_a).unwrap();
        assert_eq!(db.path(), path_a);
        let entries = db.get_entries(&EntryFilter::default()).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, "keep-me");
        assert_eq!(entries[0].source, "Hello");
    }
}
