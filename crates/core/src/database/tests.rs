use super::*;
use crate::models::StringEntry;
use std::path::Path;

fn content_revision_token(db: &Database) -> (String, u64) {
    let stats = serde_json::to_value(db.get_stats().unwrap()).unwrap();
    (
        stats["content_epoch"]
            .as_str()
            .expect("persistent epoch")
            .to_owned(),
        stats["content_revision"]
            .as_u64()
            .expect("monotonic revision"),
    )
}

#[test]
fn content_revision_bumps_on_insert_delete_and_every_string_column_update() {
    let db = Database::open_in_memory().unwrap();
    let (epoch, mut revision) = content_revision_token(&db);
    db.save_entries(&[make_entry("revision", "Hello")]).unwrap();
    assert_eq!(content_revision_token(&db), (epoch.clone(), revision + 1));
    revision += 1;

    // Discover all columns so a later schema addition remains covered too.
    let columns: Vec<String> = lock_connection(&db.conn)
        .prepare("SELECT name FROM pragma_table_info('strings')")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    for column in columns {
        let value = match column.as_str() {
            "status" => "'translated'",
            "tags" => "'[\"new\"]'",
            "metadata" => "'{\"provenance\":\"new\"}'",
            "char_limit" => "99",
            _ => "'changed'",
        };
        lock_connection(&db.conn)
            .execute(&format!("UPDATE strings SET \"{column}\" = {value}"), [])
            .unwrap();
        revision += 1;
        assert_eq!(
            content_revision_token(&db),
            (epoch.clone(), revision),
            "{column}"
        );
    }
    // Even identical-value UPDATEs are conservatively invalidating.
    lock_connection(&db.conn)
        .execute("UPDATE strings SET source = source", [])
        .unwrap();
    revision += 1;
    assert_eq!(content_revision_token(&db), (epoch.clone(), revision));
    lock_connection(&db.conn)
        .execute("DELETE FROM strings", [])
        .unwrap();
    assert_eq!(content_revision_token(&db), (epoch, revision + 1));
}

#[test]
fn content_revision_bumps_for_run_insert_delete_from_another_connection() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("project.db");
    let db = Database::open(&path).unwrap();
    let (epoch, revision) = content_revision_token(&db);
    let writer = rusqlite::Connection::open(&path).unwrap();
    writer
        .execute_batch(
            "INSERT INTO strings (id, source, file_path, created_at)
         VALUES ('external', 'Hello', 'script.rpy', '2026-11-01');
         INSERT INTO translation_runs
            (started_at, duration_secs, provider, source_lang, target_lang, strings_translated)
         VALUES ('2026-11-01', 1, 'test', 'en', 'es', 1);",
        )
        .unwrap();
    assert_eq!(content_revision_token(&db), (epoch.clone(), revision + 2));
    writer.execute("DELETE FROM translation_runs", []).unwrap();
    assert_eq!(content_revision_token(&db), (epoch.clone(), revision + 3));
    writer
        .execute("UPDATE strings SET provider_used = 'external'", [])
        .unwrap();
    assert_eq!(content_revision_token(&db), (epoch.clone(), revision + 4));
    writer.execute("DELETE FROM strings", []).unwrap();
    assert_eq!(content_revision_token(&db), (epoch, revision + 5));
}

#[test]
fn content_revision_reads_do_not_bump() {
    let db = Database::open_in_memory().unwrap();
    db.save_entries(&[make_entry("revision", "Hello")]).unwrap();
    let before = content_revision_token(&db);
    for _ in 0..3 {
        db.get_stats().unwrap();
        db.get_entries(&EntryFilter::default()).unwrap();
        db.get_translation_runs().unwrap();
        db.get_string_history("revision", 50).unwrap();
    }
    assert_eq!(content_revision_token(&db), before);
}

#[test]
fn content_revision_rolls_back_with_every_write_kind() {
    let db = Database::open_in_memory().unwrap();
    db.save_entries(&[make_entry("revision", "Hello")]).unwrap();
    lock_connection(&db.conn)
        .execute_batch(
            "INSERT INTO translation_runs
            (started_at, duration_secs, provider, source_lang, target_lang, strings_translated)
         VALUES ('2026-11-01', 1, 'test', 'en', 'es', 1);",
        )
        .unwrap();
    let before = content_revision_token(&db);
    {
        let mut conn = lock_connection(&db.conn);
        let tx = conn.transaction().unwrap();
        tx.execute_batch(
            "INSERT INTO strings (id, source, file_path, created_at)
             VALUES ('rolled-back', 'Hello', 'script.rpy', '2026-11-01');
             UPDATE strings SET translation = 'Hola';
             DELETE FROM strings;
             INSERT INTO translation_runs
                (started_at, duration_secs, provider, source_lang, target_lang, strings_translated)
             VALUES ('2026-11-01', 1, 'test', 'en', 'es', 1);
             DELETE FROM translation_runs;",
        )
        .unwrap();
        let during: u64 = tx
            .query_row(
                "SELECT content_revision FROM project_content_revision WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            during > before.1,
            "the trigger runs inside the writer transaction"
        );
        // Dropping the uncommitted transaction rolls back content and revision.
    }
    assert_eq!(content_revision_token(&db), before);
    assert_eq!(db.get_stats().unwrap().total, 1);
    assert_eq!(db.get_translation_runs().unwrap().len(), 1);
}

#[test]
fn content_revision_epoch_survives_reopen_and_changes_on_database_recreation() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("project.db");
    let db = Database::open(&path).unwrap();
    db.save_entries(&[make_entry("revision", "Hello")]).unwrap();
    let before = content_revision_token(&db);
    assert_eq!(before.0.len(), 32);
    db.reopen(&path).unwrap();
    assert_eq!(content_revision_token(&db), before);
    drop(db);
    let db = Database::open(&path).unwrap();
    assert_eq!(content_revision_token(&db), before);
    drop(db);
    std::fs::remove_file(&path).unwrap();
    let db = Database::open(&path).unwrap();
    db.save_entries(&[make_entry("revision", "Hello")]).unwrap();
    let recreated = content_revision_token(&db);
    assert_eq!(recreated.1, before.1);
    assert_ne!(recreated.0, before.0);
}

#[test]
fn content_revision_migrates_existing_database_without_revision_table() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("old.db");
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch(
        "CREATE TABLE strings (
            id TEXT PRIMARY KEY, source TEXT NOT NULL, translation TEXT,
            status TEXT NOT NULL DEFAULT 'pending', file_path TEXT NOT NULL,
            context TEXT, tags TEXT NOT NULL DEFAULT '[]', metadata TEXT NOT NULL DEFAULT '{}',
            char_limit INTEGER, provider_used TEXT, created_at TEXT NOT NULL,
            translated_at TEXT, reviewed_at TEXT
         );
         INSERT INTO strings (id, source, file_path, created_at)
         VALUES ('old', 'Hello', 'script.rpy', '2026-11-01');",
    )
    .unwrap();
    drop(conn);
    // Saved-project preflight reads old DBs without running migrations.
    let snapshot = Database::read_project_snapshot(&path).unwrap();
    let legacy = serde_json::to_value(snapshot.get_stats().unwrap()).unwrap();
    assert_eq!(legacy["total"], 1);
    assert!(legacy.get("content_epoch").is_none());
    assert!(legacy.get("content_revision").is_none());
    let tables: usize = lock_connection(&snapshot.conn)
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE name = 'project_content_revision'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        tables, 0,
        "read-only preflight must not migrate the database"
    );
    drop(snapshot);
    let db = Database::open(&path).unwrap();
    let before = content_revision_token(&db);
    assert_eq!(db.get_stats().unwrap().total, 1);
    lock_connection(&db.conn)
        .execute("UPDATE strings SET metadata = '{\"old\":true}'", [])
        .unwrap();
    assert_eq!(
        content_revision_token(&db),
        (before.0.clone(), before.1 + 1)
    );
    db.reopen(&path).unwrap();
    assert_eq!(content_revision_token(&db), (before.0, before.1 + 1));
}

#[test]
fn content_revision_stats_serde_is_additive() {
    let old_json = serde_json::json!({
        "total": 0, "pending": 0, "translated": 0, "reviewed": 0,
        "approved": 0, "error": 0, "total_cost_usd": 0.0
    });
    let old: ProjectStats = serde_json::from_value(old_json.clone()).unwrap();
    assert_eq!(serde_json::to_value(old).unwrap(), old_json);
    let db = Database::open_in_memory().unwrap();
    let mut new_json = old_json;
    let (epoch, revision) = content_revision_token(&db);
    new_json["content_epoch"] = epoch.into();
    new_json["content_revision"] = revision.into();
    let decoded: ProjectStats = serde_json::from_value(new_json.clone()).unwrap();
    assert_eq!(serde_json::to_value(decoded).unwrap(), new_json);
}

#[test]
#[ignore = "manual 100k-row get_stats timing"]
fn content_revision_stats_timing_100k() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::open(&dir.path().join("benchmark.db")).unwrap();
    lock_connection(&db.conn)
        .execute_batch(
            "WITH RECURSIVE n(i) AS (VALUES(1) UNION ALL SELECT i + 1 FROM n WHERE i < 100000)
         INSERT INTO strings (id, source, translation, status, file_path, created_at)
         SELECT printf('row-%06d', i), 'Hello', 'Hola',
                CASE i % 3 WHEN 0 THEN 'pending' WHEN 1 THEN 'translated' ELSE 'approved' END,
                'script.rpy', '2026-11-01' FROM n;",
        )
        .unwrap();
    for _ in 0..5 {
        assert_eq!(db.get_stats().unwrap().total, 100000);
    }
    let mut samples = Vec::new();
    for _ in 0..100 {
        let started = std::time::Instant::now();
        std::hint::black_box(db.get_stats().unwrap());
        samples.push(started.elapsed().as_secs_f64() * 1000.0);
    }
    samples.sort_by(f64::total_cmp);
    eprintln!(
        "get_stats 100k rows (debug, on-disk WAL, 100 warm reads): median={:.3} ms, mean={:.3} ms, min={:.3} ms, max={:.3} ms",
        samples[50], samples.iter().sum::<f64>() / samples.len() as f64, samples[0], samples[99]
    );
}

#[tokio::test]
async fn string_history_records_translation_save() {
    let db = Database::open_in_memory().unwrap();
    db.save_entries(&[make_entry("history", "Hello")]).unwrap();
    assert!(db.get_string_history("history", 50).unwrap().is_empty());
    db.save_translation("history", "Hola", "test-provider")
        .await
        .unwrap();
    let history = db.get_string_history("history", 50).unwrap();
    assert_eq!(history.len(), 1);
    let revision = &history[0];
    assert_eq!(revision.entry_id, "history");
    assert_eq!(revision.old_translation, None);
    assert_eq!(revision.new_translation.as_deref(), Some("Hola"));
    assert_eq!(revision.old_status.as_deref(), Some("pending"));
    assert_eq!(revision.new_status.as_deref(), Some("translated"));
    assert_eq!(revision.provider.as_deref(), Some("test-provider"));
    assert!(chrono::DateTime::parse_from_rfc3339(&revision.changed_at).is_ok());
    assert!(revision.changed_at.ends_with('Z'));
}

#[tokio::test]
async fn string_history_records_status_only_change() {
    let db = Database::open_in_memory().unwrap();
    db.save_entries(&[make_entry("history", "Hello")]).unwrap();
    db.update_entry_status("history", crate::models::StringStatus::Approved)
        .await
        .unwrap();
    let history = db.get_string_history("history", 50).unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].old_status.as_deref(), Some("pending"));
    assert_eq!(history[0].new_status.as_deref(), Some("approved"));
    assert_eq!(history[0].old_translation, None);
    assert_eq!(history[0].new_translation, None);
    assert_eq!(history[0].provider, None);
}

#[tokio::test]
async fn string_history_ignores_identical_values_and_provider_only_changes() {
    let db = Database::open_in_memory().unwrap();
    db.save_entries(&[make_entry("history", "Hello")]).unwrap();
    db.update_entry_status("history", crate::models::StringStatus::Pending)
        .await
        .unwrap();
    assert!(db.get_string_history("history", 50).unwrap().is_empty());
    db.save_translation("history", "Hola", "first")
        .await
        .unwrap();
    db.save_translation("history", "Hola", "second")
        .await
        .unwrap();
    db.update_entry_status("history", crate::models::StringStatus::Translated)
        .await
        .unwrap();
    assert_eq!(db.get_string_history("history", 50).unwrap().len(), 1);
}

#[tokio::test]
async fn string_history_trims_per_entry_and_caps_read_limit() {
    let db = Database::open_in_memory().unwrap();
    db.save_entries(&[make_entry("history", "Hello"), make_entry("other", "World")])
        .unwrap();
    db.save_translation("other", "Mundo", "manual")
        .await
        .unwrap();
    for i in 1..=51 {
        db.save_translation("history", &format!("version {i}"), "manual")
            .await
            .unwrap();
    }
    let history = db.get_string_history("history", usize::MAX).unwrap();
    assert_eq!(history.len(), 50);
    assert_eq!(history[0].new_translation.as_deref(), Some("version 51"));
    assert_eq!(history[49].new_translation.as_deref(), Some("version 2"));
    assert!(history.windows(2).all(|pair| pair[0].id > pair[1].id));
    assert_eq!(db.get_string_history("history", 2).unwrap().len(), 2);
    assert!(db.get_string_history("history", 0).unwrap().is_empty());
    assert_eq!(db.get_string_history("other", 50).unwrap().len(), 1);
    let conn = lock_connection(&db.conn);
    let retained: usize = conn
        .query_row(
            "SELECT COUNT(*) FROM string_revisions WHERE entry_id = 'history'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        retained, 50,
        "retention must delete rows, not just hide them"
    );
}

#[tokio::test]
async fn string_history_deletion_cleans_only_the_deleted_entry() {
    let db = Database::open_in_memory().unwrap();
    db.save_entries(&[make_entry("history", "Hello"), make_entry("other", "World")])
        .unwrap();
    db.save_translations_batch(
        vec![
            ("history".into(), "Hola".into()),
            ("other".into(), "Mundo".into()),
        ],
        "manual",
    )
    .await
    .unwrap();
    db.merge_entries(&[make_entry("other", "World")]).unwrap();
    assert!(db.get_entry("history").unwrap().is_none());
    assert!(db.get_string_history("history", 50).unwrap().is_empty());
    assert_eq!(db.get_string_history("other", 50).unwrap().len(), 1);
}

#[tokio::test]
async fn string_history_records_batch_updates() {
    let db = Database::open_in_memory().unwrap();
    db.save_entries(&[make_entry("a", "Hello"), make_entry("b", "World")])
        .unwrap();
    db.save_translations_batch(
        vec![("a".into(), "Hola".into()), ("b".into(), "Mundo".into())],
        "batch",
    )
    .await
    .unwrap();
    for (id, text) in [("a", "Hola"), ("b", "Mundo")] {
        let history = db.get_string_history(id, 50).unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].new_translation.as_deref(), Some(text));
        assert_eq!(history[0].provider.as_deref(), Some("batch"));
    }
}

#[tokio::test]
async fn string_history_records_import_updates() {
    let db = Database::open_in_memory().unwrap();
    db.save_entries(&[make_entry("history", "Hello")]).unwrap();
    db.save_imported_translations_batch(vec![crate::export::ImportedTranslation {
        id: "history".into(),
        source: "Hello".into(),
        translation: "Hola".into(),
    }])
    .await
    .unwrap();
    let history = db.get_string_history("history", 50).unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].new_translation.as_deref(), Some("Hola"));
    assert_eq!(history[0].provider.as_deref(), Some("import"));
}

#[tokio::test]
async fn string_history_migrates_existing_database_and_reopens_idempotently() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("pre-history.db");
    {
        let db = Database::open(&path).unwrap();
        db.save_entries(&[make_entry("history", "Hello")]).unwrap();
        // Model a pre-history database without changing its existing strings.
        lock_connection(&db.conn)
            .execute_batch(
                "DROP TRIGGER IF EXISTS strings_record_revision;
             DROP TRIGGER IF EXISTS strings_delete_revisions;
             DROP TABLE IF EXISTS string_revisions;",
            )
            .unwrap();
    }
    {
        let db = Database::open(&path).unwrap();
        assert!(db.get_string_history("history", 50).unwrap().is_empty());
        db.save_translation("history", "Hola", "manual")
            .await
            .unwrap();
        assert_eq!(db.get_string_history("history", 50).unwrap().len(), 1);
    }
    let db = Database::open(&path).unwrap();
    assert_eq!(db.get_string_history("history", 50).unwrap().len(), 1);
    db.save_translation("history", "Saludos", "manual")
        .await
        .unwrap();
    assert_eq!(db.get_string_history("history", 50).unwrap().len(), 2);
    db.merge_entries(&[]).unwrap();
    assert!(db.get_string_history("history", 50).unwrap().is_empty());
}

pub(super) fn make_entry(id: &str, source: &str) -> StringEntry {
    StringEntry::new(id, source, PathBuf::from("test.json"))
}

pub(super) fn shared_textasset_rows(count: usize) -> (String, Vec<StringEntry>) {
    let mut original = String::new();
    let mut entries = Vec::new();
    for i in 0..count {
        let key = format!("Menu.K{i}");
        let value = format!("日本語の文章{i}{}", "あ".repeat(50));
        original.push_str(&format!("{key}: {value}\n"));
        let mut entry = make_entry(&format!("row{i:05}"), &value);
        entry.metadata.insert(
            "extraction_method".into(),
            serde_json::json!("textasset_loc_line"),
        );
        entry
            .metadata
            .insert("loc_key".into(), serde_json::json!(key));
        entry
            .metadata
            .insert("line_index".into(), serde_json::json!(i));
        entries.push(entry);
    }
    original.push_str("   ");
    crate::textasset_group::attach_to_entries(
        &mut entries,
        crate::textasset_group::GroupKind::LocLine,
        &original,
        original.len(),
        "shared-group",
    );
    (original, entries)
}

#[test]
fn snapshot_to_preserves_shared_textasset_original() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::open(&dir.path().join("project.db")).unwrap();
    let (original, entries) = shared_textasset_rows(3);
    db.save_entries(&entries).unwrap();
    let expected = db.get_entries(&EntryFilter::default()).unwrap();
    let dest = dir.path().join("checkpoint's shared original.locust.db");

    db.snapshot_to(&dest).unwrap();

    let checkpoint = Database::open(&dest).unwrap();
    let loaded = checkpoint.get_entries(&EntryFilter::default()).unwrap();
    assert_eq!(
        serde_json::to_value(&loaded).unwrap(),
        serde_json::to_value(&expected).unwrap()
    );
    let head = loaded[0].textasset_original.as_ref().unwrap();
    for entry in &loaded {
        assert_eq!(
            crate::textasset_group::original_textasset(entry),
            Some(original.as_str())
        );
        assert!(Arc::ptr_eq(
            head,
            entry.textasset_original.as_ref().unwrap()
        ));
        assert!(crate::textasset_group::parse_group_meta(entry).is_ok());
    }
    let original_count: usize = lock_connection(&checkpoint.conn)
        .query_row("SELECT COUNT(*) FROM textasset_originals", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(original_count, 1);
}

// ─── injection recording (root + rel + hash per language) ──────────────

pub(super) fn recording_tempdir() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("locust_db_rec_")
        .tempdir()
        .unwrap()
}

/// A minimal injected tree: `<base>/root/game/script.rpy` with known bytes.
pub(super) fn make_recorded_tree(base: &Path) -> (PathBuf, PathBuf, Vec<u8>) {
    let root = base.join("root");
    let sub = root.join("game");
    std::fs::create_dir_all(&sub).unwrap();
    let file = sub.join("script.rpy");
    let bytes = b"label start:\n    \"Hola\"\n".to_vec();
    std::fs::write(&file, &bytes).unwrap();
    (root, file, bytes)
}
