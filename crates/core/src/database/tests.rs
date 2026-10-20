use super::*;
use crate::models::StringEntry;
use std::path::Path;

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
