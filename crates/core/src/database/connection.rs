use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rusqlite::{params, Connection, OpenFlags};

use crate::error::{LocustError, Result};

use super::schema::init_schema;
use super::{lock_connection, Database};

#[cfg(test)]
thread_local! {
    // Rendezvous after sampling sidecars, before opening SQLite.
    static SNAPSHOT_BEFORE_OPEN: std::cell::RefCell<Option<Arc<std::sync::Barrier>>> = const { std::cell::RefCell::new(None) };
}

impl Database {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let conn = Connection::open(path)?;
        init_schema(&conn)?;
        let db = Self {
            conn: Arc::new(Mutex::new(conn)),
            path: Mutex::new(path.to_path_buf()),
        };
        if let Err(error) = db.index_backup_references() {
            tracing::warn!(database = %path.display(), %error,
                "Backup reference indexing is unavailable; deletion requires verified references");
        }
        Ok(db)
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

    /// Copy `src` with [`Database::snapshot_to`] using a read-only connection.
    /// Never unlink source sidecars. If WAL/SHM were initially absent, a brief
    /// read-write connection lets SQLite clean up empty files on last close.
    /// With another connection open, SQLite leaves its live sidecars alone.
    /// Cleanup is best-effort when the source cannot be opened for writing.
    pub fn snapshot_existing(src: &Path, dest: &Path) -> Result<()> {
        let mut missing_sidecar = false;
        for suffix in ["-wal", "-shm"] {
            missing_sidecar |= !sqlite_sidecar(src, suffix).try_exists()?;
        }
        #[cfg(test)]
        SNAPSHOT_BEFORE_OPEN.with(|hook| {
            if let Some(barrier) = hook.borrow_mut().take() {
                barrier.wait();
                barrier.wait();
            }
        });
        let db = Self::open_for_snapshot(src)?;
        let result = db.snapshot_to(dest);
        drop(db);
        if missing_sidecar {
            // Do not use Self::open: schema initialization would write to src.
            // READ_WRITE without CREATE cannot recreate a removed source. A
            // schema read activates WAL handling; merely opening and closing
            // an otherwise unused connection does not trigger WAL cleanup.
            if let Ok(conn) = Connection::open_with_flags(src, OpenFlags::SQLITE_OPEN_READ_WRITE) {
                let _ = conn.pragma_query_value(None, "schema_version", |row| row.get::<_, i64>(0));
                // SQLite's last-close locking/checkpoint semantics decide
                // whether these sidecars can safely be removed. This does not
                // run migrations, change journal mode or manually unlink files.
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

    fn synchronous(db: &Database) -> i64 {
        lock_connection(&db.conn)
            .pragma_query_value(None, "synchronous", |row| row.get(0))
            .unwrap()
    }

    #[tokio::test]
    async fn w10_database_file_commits_use_full_on_open_and_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("project.db");
        let db = Database::open(&path).unwrap();
        assert_eq!(synchronous(&db), 2, "file-backed commits must use FULL");
        let journal: String = lock_connection(&db.conn)
            .pragma_query_value(None, "journal_mode", |row| row.get(0))
            .unwrap();
        assert_eq!(journal, "wal");
        db.save_entries(&[make_entry("saved", "Hello")]).unwrap();
        assert!(db
            .save_translation("saved", "Hola", "manual")
            .await
            .unwrap());

        // synchronous is connection-local: reopening must set it again.
        lock_connection(&db.conn)
            .pragma_update(None, "synchronous", "NORMAL")
            .unwrap();
        db.reopen(&path).unwrap();
        assert_eq!(synchronous(&db), 2);
        let check_saved = |db: &Database| {
            let entries = db.get_entries(&EntryFilter::default()).unwrap();
            assert_eq!(entries.len(), 1);
            assert_eq!(entries[0].translation.as_deref(), Some("Hola"));
            let history = db.get_string_history("saved", 50).unwrap();
            assert_eq!(history.len(), 1);
            assert_eq!(history[0].new_translation.as_deref(), Some("Hola"));
        };
        check_saved(&db);
        drop(db);
        let reopened = Database::open(&path).unwrap();
        assert_eq!(synchronous(&reopened), 2);
        check_saved(&reopened);
    }

    #[test]
    fn w10_database_memory_and_scratch_keep_normal_on_open_and_reopen() {
        let db = Database::open_in_memory().unwrap();
        assert_eq!(synchronous(&db), 1);
        for path in [Path::new(":memory:"), Path::new("")] {
            let scratch = Database::open(path).unwrap();
            assert_eq!(synchronous(&scratch), 1);
            scratch.reopen(path).unwrap();
            assert_eq!(synchronous(&scratch), 1);
        }
        db.reopen(Path::new(":memory:")).unwrap();
        assert_eq!(synchronous(&db), 1);
    }

    #[test]
    fn w10_database_snapshot_preserves_late_writer_sidecars_and_commits() {
        // Exercise cleanup after both a successful snapshot and a refused one.
        for destination_exists in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("project.db");
            let dest = dir.path().join("snapshot.db");
            let seed = Database::open(&path).unwrap();
            seed.save_entries(&[make_entry("seed", "Before snapshot")])
                .unwrap();
            drop(seed);
            for suffix in ["-wal", "-shm", "-journal"] {
                assert!(!sqlite_sidecar(&path, suffix).try_exists().unwrap());
            }
            if destination_exists {
                std::fs::write(&dest, b"existing destination").unwrap();
            }

            let barrier = Arc::new(std::sync::Barrier::new(2));
            let writer_barrier = barrier.clone();
            let writer_path = path.clone();
            let writer = std::thread::spawn(move || {
                // snapshot_existing sampled absence; only now may a writer open.
                writer_barrier.wait();
                let result = (|| -> Result<Database> {
                    let db = Database::open(&writer_path)?;
                    db.save_entries(&[make_entry("concurrent", "Committed during preview")])?;
                    Ok(db)
                })();
                writer_barrier.wait();
                // Keep this connection open through snapshot completion/cleanup.
                writer_barrier.wait();
                result
            });
            SNAPSHOT_BEFORE_OPEN.with(|hook| *hook.borrow_mut() = Some(barrier.clone()));
            let result = Database::snapshot_existing(&path, &dest);
            // Collect observations before allowing the writer to close, but join
            // before asserting so a failed assertion cannot strand its barrier.
            let wal_len = std::fs::metadata(sqlite_sidecar(&path, "-wal")).map(|m| m.len());
            let shm_exists = sqlite_sidecar(&path, "-shm").is_file();
            let live_rows = Database::open_for_snapshot(&path)
                .and_then(|db| db.get_entries(&EntryFilter::default()));
            barrier.wait();
            let writer_db = writer.join().unwrap().unwrap();
            assert_eq!(result.is_err(), destination_exists, "{result:?}");
            assert!(wal_len.unwrap() > 0, "the committed WAL must remain live");
            assert!(shm_exists, "the writer's SHM must survive snapshot cleanup");
            let assert_rows = |rows: Vec<crate::models::StringEntry>| {
                assert_eq!(rows.len(), 2);
                assert!(rows.iter().any(|row| row.id == "concurrent"));
            };
            assert_rows(live_rows.unwrap());
            drop(writer_db);
            let reopened = Database::open(&path).unwrap();
            assert_rows(reopened.get_entries(&EntryFilter::default()).unwrap());
            if destination_exists {
                assert_eq!(std::fs::read(&dest).unwrap(), b"existing destination");
            } else {
                let snapshot = Database::open(&dest).unwrap();
                assert_rows(snapshot.get_entries(&EntryFilter::default()).unwrap());
            }
        }
    }

    #[tokio::test]
    #[ignore = "manual 1000-translation NORMAL/FULL timing comparison"]
    async fn w10_database_translation_save_timing_1000() {
        // Same public save APIs, fresh on-disk WAL DBs, five trials per mode.
        // Time only saves, excluding setup, reopening and verification.
        let dir = tempfile::tempdir().unwrap();
        let entries: Vec<_> = (0..1000)
            .map(|i| make_entry(&format!("row-{i}"), "Hello from the benchmark"))
            .collect();
        for bulk in [false, true] {
            let mut normal = Vec::new();
            let mut full = Vec::new();
            for trial in 0..5 {
                // Alternate order to reduce warm-cache/order bias.
                let modes = if trial % 2 == 0 {
                    ["NORMAL", "FULL"]
                } else {
                    ["FULL", "NORMAL"]
                };
                for mode in modes {
                    let path = dir.path().join(format!("{bulk}-{trial}-{mode}.db"));
                    let db = Database::open(&path).unwrap();
                    db.save_entries(&entries).unwrap();
                    lock_connection(&db.conn)
                        .execute_batch(&format!(
                            "PRAGMA wal_checkpoint(TRUNCATE); PRAGMA synchronous = {mode};"
                        ))
                        .unwrap();
                    assert_eq!(synchronous(&db), if mode == "FULL" { 2 } else { 1 });
                    let updates: Vec<_> = entries
                        .iter()
                        .map(|entry| (entry.id.clone(), "Hola desde el benchmark".to_owned()))
                        .collect();
                    let started = std::time::Instant::now();
                    if bulk {
                        assert_eq!(
                            db.save_translations_batch(updates, "benchmark")
                                .await
                                .unwrap(),
                            1000
                        );
                    } else {
                        for (id, translation) in updates {
                            assert!(db
                                .save_translation(&id, &translation, "benchmark")
                                .await
                                .unwrap());
                        }
                    }
                    let ms = started.elapsed().as_secs_f64() * 1000.0;
                    if mode == "NORMAL" {
                        normal.push(ms);
                    } else {
                        full.push(ms);
                    }
                    drop(db);
                    let reopened = Database::open(&path).unwrap();
                    assert_eq!(reopened.get_stats().unwrap().translated, 1000);
                }
            }
            eprintln!("1000 translations, bulk={bulk}: NORMAL ms={normal:?}; FULL ms={full:?}");
            normal.sort_by(f64::total_cmp);
            full.sort_by(f64::total_cmp);
            eprintln!("1000 translations, bulk={bulk}: median NORMAL={:.3} ms; FULL={:.3} ms; ratio={:.3}x", normal[2], full[2], full[2] / normal[2]);
        }
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
