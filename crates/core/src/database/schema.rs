use std::time::{Duration, Instant};

use rusqlite::{Connection, Transaction, TransactionBehavior};

use crate::error::Result;

/// Shared schema + migration used by [`super::Database::open`] and [`super::Database::reopen`].
pub(super) fn init_schema(conn: &Connection) -> Result<()> {
    // These connection settings cannot be changed inside a transaction.
    enable_wal(conn)?;
    conn.execute_batch("PRAGMA synchronous = NORMAL; PRAGMA foreign_keys = ON;")?;
    // Acquire the write lock before inspecting the schema. Concurrent openers
    // wait using the connection's existing busy timeout; failures roll back.
    let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    // The injected_files table was rebuilt (lang/root/rel/hash/size) when
    // `locust patch` moved to packing exclusively from recordings. Any
    // table missing part of that column set — the legacy file_path
    // schema, or an intermediate one — cannot serve the new contract and
    // would fail at runtime with a raw SQL error the first time a
    // recording is read or written, so it is dropped whole. Recordings
    // are reproducible caches: the next `locust inject` rebuilds one,
    // and `locust patch` on a missing recording names that exact
    // command.
    let legacy: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master
         WHERE type = 'table' AND name = 'injected_files'
           AND (SELECT COUNT(*) FROM pragma_table_info('injected_files')
                WHERE name IN ('lang', 'root', 'rel', 'hash', 'size',
                               'recorded_at')) < 6",
        [],
        |row| row.get(0),
    )?;
    if legacy > 0 {
        conn.execute("DROP TABLE injected_files", [])?;
    }
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS project_metadata (
            key TEXT PRIMARY KEY NOT NULL,
            value TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS textasset_originals (
            sha256 TEXT PRIMARY KEY NOT NULL,
            byte_len INTEGER NOT NULL CHECK(byte_len > 0 AND byte_len <= 1048576),
            payload TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS strings (
            id TEXT PRIMARY KEY,
            source TEXT NOT NULL,
            translation TEXT,
            status TEXT NOT NULL DEFAULT 'pending',
            file_path TEXT NOT NULL,
            context TEXT,
            tags TEXT NOT NULL DEFAULT '[]',
            metadata TEXT NOT NULL DEFAULT '{}',
            char_limit INTEGER,
            provider_used TEXT,
            created_at TEXT NOT NULL,
            translated_at TEXT,
            reviewed_at TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_strings_status ON strings(status);
        CREATE INDEX IF NOT EXISTS idx_strings_file ON strings(file_path);

        CREATE TABLE IF NOT EXISTS glossary (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            term TEXT NOT NULL,
            translation TEXT NOT NULL,
            lang_pair TEXT NOT NULL,
            context TEXT,
            case_sensitive INTEGER NOT NULL DEFAULT 0,
            UNIQUE(term, lang_pair)
        );

        CREATE TABLE IF NOT EXISTS translation_memory (
            source_hash TEXT NOT NULL,
            lang_pair TEXT NOT NULL,
            source TEXT NOT NULL,
            translation TEXT NOT NULL,
            uses INTEGER NOT NULL DEFAULT 1,
            last_used TEXT NOT NULL,
            PRIMARY KEY (source_hash, lang_pair)
        );

        CREATE TABLE IF NOT EXISTS validation_issues (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            entry_id TEXT NOT NULL,
            kind TEXT NOT NULL,
            message TEXT NOT NULL,
            resolved INTEGER NOT NULL DEFAULT 0
        );

        CREATE TABLE IF NOT EXISTS translation_runs (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            started_at TEXT NOT NULL,
            duration_secs REAL NOT NULL,
            provider TEXT NOT NULL,
            source_lang TEXT NOT NULL,
            target_lang TEXT NOT NULL,
            strings_translated INTEGER NOT NULL,
            tokens_used INTEGER NOT NULL DEFAULT 0,
            input_tokens INTEGER NOT NULL DEFAULT 0,
            output_tokens INTEGER NOT NULL DEFAULT 0,
            cost_usd REAL NOT NULL DEFAULT 0
        );

        CREATE TABLE IF NOT EXISTS injected_files (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            lang TEXT,
            root TEXT NOT NULL,
            rel TEXT NOT NULL,
            hash TEXT NOT NULL,
            size INTEGER NOT NULL,
            recorded_at TEXT NOT NULL
        );
        ",
    )?;
    // Preserve older recordings; absent provenance must never be guessed.
    add_column_if_missing(
        conn,
        "injected_files",
        "pristine_backup",
        "ALTER TABLE injected_files ADD COLUMN pristine_backup TEXT",
    )?;
    add_column_if_missing(
        conn,
        "injected_files",
        "registration_backup",
        "ALTER TABLE injected_files ADD COLUMN registration_backup TEXT",
    )?;
    // Migrate older DBs that predate the input/output token columns.
    add_column_if_missing(
        conn,
        "translation_runs",
        "input_tokens",
        "ALTER TABLE translation_runs ADD COLUMN input_tokens INTEGER NOT NULL DEFAULT 0",
    )?;
    add_column_if_missing(
        conn,
        "translation_runs",
        "output_tokens",
        "ALTER TABLE translation_runs ADD COLUMN output_tokens INTEGER NOT NULL DEFAULT 0",
    )?;
    // Missing historical metadata cannot prove a zero or complete bill.
    add_column_if_missing(
        conn,
        "translation_runs",
        "cost_is_complete",
        "ALTER TABLE translation_runs ADD COLUMN cost_is_complete INTEGER NOT NULL DEFAULT 0",
    )?;
    // Additive history migration: also runs when opening an existing project.
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS string_revisions (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            entry_id TEXT NOT NULL,
            old_translation TEXT,
            new_translation TEXT,
            old_status TEXT,
            new_status TEXT,
            provider TEXT,
            changed_at TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_string_revisions_entry_id
            ON string_revisions(entry_id, id);

        CREATE TRIGGER IF NOT EXISTS strings_record_revision
        AFTER UPDATE OF translation, status ON strings
        WHEN old.translation IS NOT new.translation OR old.status IS NOT new.status
        BEGIN
            INSERT INTO string_revisions
                (entry_id, old_translation, new_translation, old_status, new_status,
                 provider, changed_at)
            VALUES
                (new.id, old.translation, new.translation, old.status, new.status,
                 new.provider_used, strftime('%Y-%m-%dT%H:%M:%fZ','now'));
            DELETE FROM string_revisions
            WHERE entry_id = new.id AND id NOT IN (
                SELECT id FROM string_revisions WHERE entry_id = new.id
                ORDER BY id DESC LIMIT 50
            );
        END;

        CREATE TRIGGER IF NOT EXISTS strings_delete_revisions
        AFTER DELETE ON strings
        BEGIN
            DELETE FROM string_revisions WHERE entry_id = old.id;
        END;",
    )?;
    tx.commit()?;
    Ok(())
}

fn enable_wal(conn: &Connection) -> Result<()> {
    let timeout_ms = conn.pragma_query_value(None, "busy_timeout", |row| row.get(0))?;
    let timeout = Duration::from_millis(timeout_ms);
    let started = Instant::now();
    loop {
        match conn.execute_batch("PRAGMA journal_mode = WAL;") {
            // SQLite can skip its busy handler during a journal-mode lock
            // upgrade. Retry that contention within the existing timeout.
            Err(error)
                if error.sqlite_error_code() == Some(rusqlite::ErrorCode::DatabaseBusy)
                    && started.elapsed() < timeout =>
            {
                std::thread::sleep(
                    Duration::from_millis(10).min(timeout.saturating_sub(started.elapsed())),
                );
            }
            result => return result.map_err(Into::into),
        }
    }
}

fn add_column_if_missing(
    conn: &Connection,
    table: &str,
    column: &str,
    statement: &str,
) -> Result<()> {
    let exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info(?1) WHERE name = ?2 COLLATE NOCASE)",
        [table, column],
        |row| row.get(0),
    )?;
    if exists {
        return Ok(());
    }
    match conn.execute(statement, []) {
        Ok(_) => Ok(()),
        // Defensive fallback for a duplicate-column race only.
        Err(error) if is_duplicate_column(&error, column) => Ok(()),
        Err(rusqlite::Error::SqliteFailure(code, message)) => {
            let detail = message.unwrap_or_else(|| code.to_string());
            Err(rusqlite::Error::SqliteFailure(code, Some(format!("{statement}: {detail}"))).into())
        }
        // SQLite input errors already include the offending SQL statement.
        Err(error) => Err(error.into()),
    }
}

fn is_duplicate_column(error: &rusqlite::Error, column: &str) -> bool {
    matches!(
        error,
        rusqlite::Error::SqliteFailure(code, Some(message))
            if code.extended_code == rusqlite::ffi::SQLITE_ERROR
                && message == &format!("duplicate column name: {column}")
    )
}

#[cfg(test)]
mod tests {
    use super::super::tests::{make_recorded_tree, recording_tempdir};
    use super::super::Database;

    fn schema_snapshot(conn: &rusqlite::Connection) -> Vec<(String, String)> {
        conn.prepare("SELECT name, sql FROM sqlite_master WHERE sql IS NOT NULL ORDER BY name")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    }

    #[test]
    fn migration_duplicate_fallback_matches_only_the_exact_column_error() {
        let error = |code, message: &str| {
            rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(code),
                Some(message.to_owned()),
            )
        };
        assert!(super::is_duplicate_column(
            &error(
                rusqlite::ffi::SQLITE_ERROR,
                "duplicate column name: input_tokens"
            ),
            "input_tokens",
        ));
        for (code, message) in [
            (
                rusqlite::ffi::SQLITE_READONLY,
                "duplicate column name: input_tokens",
            ),
            (
                rusqlite::ffi::SQLITE_ERROR,
                "duplicate column name: output_tokens",
            ),
            (
                rusqlite::ffi::SQLITE_ERROR,
                "duplicate column name: input_tokens_extra",
            ),
            (
                rusqlite::ffi::SQLITE_ERROR,
                "schema error: duplicate column name: input_tokens",
            ),
        ] {
            assert!(!super::is_duplicate_column(
                &error(code, message),
                "input_tokens"
            ));
        }
    }

    #[test]
    fn migration_second_open_is_a_noop() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("project.db");
        let db = Database::open(&path).unwrap();
        db.set_project_metadata("retained", &serde_json::json!(42))
            .unwrap();
        let before = {
            let conn = db.conn.lock().unwrap();
            let version: i64 = conn
                .query_row("PRAGMA schema_version", [], |r| r.get(0))
                .unwrap();
            (version, schema_snapshot(&conn))
        };
        drop(db);

        let db = Database::open(&path).unwrap();
        let conn = db.conn.lock().unwrap();
        let version: i64 = conn
            .query_row("PRAGMA schema_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!((version, schema_snapshot(&conn)), before);
        drop(conn);
        assert_eq!(
            db.get_project_metadata("retained").unwrap(),
            Some(serde_json::json!(42))
        );
    }

    #[test]
    fn migration_concurrent_fresh_opens_complete_the_schema() {
        let reference = Database::open_in_memory().unwrap();
        let expected = schema_snapshot(&reference.conn.lock().unwrap());
        let dir = tempfile::tempdir().unwrap();
        for attempt in 0..32 {
            let path = dir.path().join(format!("concurrent-{attempt}.db"));
            let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
            let handles: Vec<_> = (0..2)
                .map(|_| {
                    let path = path.clone();
                    let barrier = barrier.clone();
                    std::thread::spawn(move || {
                        barrier.wait();
                        Database::open(&path).map(|db| schema_snapshot(&db.conn.lock().unwrap()))
                    })
                })
                .collect();
            // Join both workers before asserting, including when one open fails.
            let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
            for result in results {
                assert_eq!(result.unwrap(), expected);
            }
        }
    }

    #[test]
    fn migration_respects_busy_timeout_on_locked_database() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("locked.db");
        let writer = rusqlite::Connection::open(&path).unwrap();
        writer
            .execute_batch("CREATE TABLE existing (id INTEGER); BEGIN IMMEDIATE;")
            .unwrap();
        for timeout_ms in [0, 20] {
            let conn = rusqlite::Connection::open(&path).unwrap();
            conn.busy_timeout(std::time::Duration::from_millis(timeout_ms))
                .unwrap();
            let err =
                super::init_schema(&conn).expect_err("a held write lock must surface an error");
            match err {
                crate::error::LocustError::DatabaseError(error) => assert_eq!(
                    error.sqlite_error_code(),
                    Some(rusqlite::ErrorCode::DatabaseBusy),
                ),
                error => panic!("expected a database error, got {error}"),
            }
            let configured: u64 = conn
                .pragma_query_value(None, "busy_timeout", |row| row.get(0))
                .unwrap();
            assert_eq!(configured, timeout_ms);
            assert!(conn.is_autocommit());
        }
    }

    #[test]
    fn migration_alter_errors_include_statement_context() {
        for (table, column) in [
            ("injected_files", "pristine_backup"),
            ("injected_files", "registration_backup"),
            ("translation_runs", "input_tokens"),
            ("translation_runs", "output_tokens"),
            ("translation_runs", "cost_is_complete"),
        ] {
            let conn = rusqlite::Connection::open_in_memory().unwrap();
            super::init_schema(&conn).unwrap();
            let columns: Vec<String> = conn
                .prepare("SELECT name FROM pragma_table_info(?1) WHERE name != ?2")
                .unwrap()
                .query_map([table, column], |row| row.get(0))
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap();
            let projection = columns
                .iter()
                .map(|name| format!("NULL AS {name}"))
                .collect::<Vec<_>>()
                .join(", ");
            conn.execute_batch(&format!(
                "DROP TABLE {table}; CREATE VIEW {table} AS SELECT {projection};"
            ))
            .unwrap();

            let err = super::init_schema(&conn).expect_err("ALTER on a view must fail");
            assert!(matches!(err, crate::error::LocustError::DatabaseError(_)));
            let message = err.to_string();
            assert!(
                message.contains(&format!("ALTER TABLE {table} ADD COLUMN {column}")),
                "{message}"
            );
            assert!(message.contains("view"), "{message}");
            assert!(
                conn.is_autocommit(),
                "a failed migration must end its transaction"
            );
        }
    }

    #[test]
    fn migration_failure_rolls_back_earlier_schema_changes() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        super::init_schema(&conn).unwrap();
        conn.execute_batch(
            "DROP TABLE injected_files;
             CREATE TABLE injected_files (file_path TEXT NOT NULL);
             INSERT INTO injected_files VALUES ('keep-on-failure');
             DROP TABLE translation_runs;
             CREATE VIEW translation_runs AS SELECT 0 AS output_tokens, 0 AS cost_is_complete;",
        )
        .unwrap();
        let before = schema_snapshot(&conn);

        assert!(super::init_schema(&conn).is_err());
        assert_eq!(schema_snapshot(&conn), before);
        let value: String = conn
            .query_row("SELECT file_path FROM injected_files", [], |row| row.get(0))
            .unwrap();
        assert_eq!(value, "keep-on-failure");
        assert!(conn.is_autocommit());
    }

    #[test]
    fn migration_older_schema_preserves_rows_and_adds_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("old.db");
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE translation_runs (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                started_at TEXT NOT NULL,
                duration_secs REAL NOT NULL,
                provider TEXT NOT NULL,
                source_lang TEXT NOT NULL,
                target_lang TEXT NOT NULL,
                strings_translated INTEGER NOT NULL,
                tokens_used INTEGER NOT NULL DEFAULT 0,
                cost_usd REAL NOT NULL DEFAULT 0
             );
             INSERT INTO translation_runs VALUES
                (7, '2026-01-01', 1.5, 'test', 'en', 'es', 3, 17, 0.25);
             CREATE TABLE injected_files (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                lang TEXT,
                root TEXT NOT NULL,
                rel TEXT NOT NULL,
                hash TEXT NOT NULL,
                size INTEGER NOT NULL,
                recorded_at TEXT NOT NULL
             );
             INSERT INTO injected_files VALUES
                (9, 'es', '/game', 'a.rpy', 'original-hash', 123, '2026-01-01');
             CREATE TABLE strings (
                id TEXT PRIMARY KEY,
                source TEXT NOT NULL,
                translation TEXT,
                status TEXT NOT NULL DEFAULT 'pending',
                file_path TEXT NOT NULL,
                context TEXT,
                tags TEXT NOT NULL DEFAULT '[]',
                metadata TEXT NOT NULL DEFAULT '{}',
                char_limit INTEGER,
                provider_used TEXT,
                created_at TEXT NOT NULL,
                translated_at TEXT,
                reviewed_at TEXT
             );
             INSERT INTO strings (id, source, translation, file_path, created_at)
             VALUES ('old', 'Hello', 'Hola', 'a.rpy', '2026-01-01');",
        )
        .unwrap();
        drop(conn);

        let db = Database::open(&path).unwrap();
        let conn = db.conn.lock().unwrap();
        let run: (i64, i64, f64, i64, i64, i64) = conn.query_row(
            "SELECT id, tokens_used, cost_usd, input_tokens, output_tokens, cost_is_complete FROM translation_runs",
            [], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?)),
        ).unwrap();
        assert_eq!(run, (7, 17, 0.25, 0, 0, 0));
        let recording: (i64, String, i64, Option<String>, Option<String>) = conn
            .query_row(
                "SELECT id, hash, size, pristine_backup, registration_backup FROM injected_files",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(recording, (9, "original-hash".into(), 123, None, None));
        let entry: (String, String) = conn
            .query_row(
                "SELECT source, translation FROM strings WHERE id = 'old'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(entry, ("Hello".into(), "Hola".into()));
    }

    #[test]
    fn test_migration_drops_the_legacy_injected_files_table() {
        // The pre-recording table (file_path/lang, no root) cannot say which
        // tree injection targeted, so it is dropped on open; `locust patch`
        // then gives the exact inject command that rebuilds the recording.
        let base_guard = recording_tempdir();
        let base = base_guard.path().to_path_buf();
        let db_path = base.join("legacy.locust.db");
        {
            let conn = rusqlite::Connection::open(&db_path).unwrap();
            conn.execute_batch(
                "CREATE TABLE injected_files (
                     id INTEGER PRIMARY KEY AUTOINCREMENT,
                     file_path TEXT NOT NULL,
                     lang TEXT NOT NULL,
                     recorded_at TEXT NOT NULL
                 );
                 INSERT INTO injected_files (file_path, lang, recorded_at)
                 VALUES ('/g/game/a.rpy', 'es', '2026-01-01T00:00:00Z');",
            )
            .unwrap();
        }

        let db = Database::open(&db_path).unwrap();
        assert!(
            db.list_recorded_langs().unwrap().is_empty(),
            "legacy rows must be gone — they never recorded a root"
        );
        // And the new API works against the rebuilt table.
        let (root, file, _) = make_recorded_tree(&base);
        db.record_injection(Some("es"), &root, &[file]).unwrap();
        assert!(db.get_injection(Some("es")).unwrap().is_some());
    }

    #[test]
    fn test_migration_drops_an_injected_files_table_missing_any_expected_column() {
        // Keying the migration on a missing `root` alone under-detects: a
        // table WITH `root` but WITHOUT hash/size (an intermediate schema)
        // survives and then fails at runtime with a raw SQL error the first
        // time a recording is read or written.
        let base_guard = recording_tempdir();
        let base = base_guard.path().to_path_buf();
        let db_path = base.join("intermediate.locust.db");
        {
            let conn = rusqlite::Connection::open(&db_path).unwrap();
            conn.execute_batch(
                "CREATE TABLE injected_files (
                     id INTEGER PRIMARY KEY AUTOINCREMENT,
                     lang TEXT,
                     root TEXT NOT NULL,
                     rel TEXT NOT NULL,
                     recorded_at TEXT NOT NULL
                 );
                 INSERT INTO injected_files (lang, root, rel, recorded_at)
                 VALUES ('es', '/g/game', 'a.rpy', '2026-01-01T00:00:00Z');",
            )
            .unwrap();
        }

        let db = Database::open(&db_path).unwrap();
        assert!(
            db.list_recorded_langs().unwrap().is_empty(),
            "a table without the full column set must be rebuilt, not kept"
        );
        // The full round-trip works against the rebuilt table — this is what
        // raw-SQL-errored before the migration detected the column set.
        let (root, file, _) = make_recorded_tree(&base);
        db.record_injection(Some("es"), &root, &[file]).unwrap();
        let rec = db.get_injection(Some("es")).unwrap().unwrap();
        assert!(!rec.files[0].hash.is_empty());
    }
}
