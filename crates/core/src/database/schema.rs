use rusqlite::Connection;

use crate::error::Result;

/// Shared schema + migration used by [`super::Database::open`] and [`super::Database::reopen`].
pub(super) fn init_schema(conn: &Connection) -> Result<()> {
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
        PRAGMA journal_mode = WAL;
        PRAGMA synchronous = NORMAL;
        PRAGMA foreign_keys = ON;

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
    let has_pristine_backup = {
        let mut stmt = conn.prepare("PRAGMA table_info(injected_files)")?;
        let names = stmt.query_map([], |row| row.get::<_, String>(1))?;
        names
            .collect::<std::result::Result<Vec<_>, _>>()?
            .iter()
            .any(|name| name == "pristine_backup")
    };
    if !has_pristine_backup {
        conn.execute(
            "ALTER TABLE injected_files ADD COLUMN pristine_backup TEXT",
            [],
        )?;
    }
    let has_registration_backup = conn
        .prepare("PRAGMA table_info(injected_files)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<std::result::Result<Vec<_>, _>>()?
        .iter()
        .any(|name| name == "registration_backup");
    if !has_registration_backup {
        conn.execute(
            "ALTER TABLE injected_files ADD COLUMN registration_backup TEXT",
            [],
        )?;
    }
    // Migrate older DBs that predate the input/output token columns.
    // ADD COLUMN errors if the column already exists — ignore that.
    let _ = conn.execute(
        "ALTER TABLE translation_runs ADD COLUMN input_tokens INTEGER NOT NULL DEFAULT 0",
        [],
    );
    let _ = conn.execute(
        "ALTER TABLE translation_runs ADD COLUMN output_tokens INTEGER NOT NULL DEFAULT 0",
        [],
    );
    // Missing historical metadata cannot prove a zero or complete bill.
    let has_cost_completeness = {
        let mut stmt = conn.prepare("PRAGMA table_info(translation_runs)")?;
        let names = stmt.query_map([], |row| row.get::<_, String>(1))?;
        names
            .collect::<std::result::Result<Vec<_>, _>>()?
            .iter()
            .any(|name| name == "cost_is_complete")
    };
    if !has_cost_completeness {
        conn.execute(
            "ALTER TABLE translation_runs ADD COLUMN cost_is_complete INTEGER NOT NULL DEFAULT 0",
            [],
        )?;
    }
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
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::tests::{make_recorded_tree, recording_tempdir};
    use super::super::Database;

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
