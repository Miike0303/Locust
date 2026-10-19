use super::*;
use crate::models::StringEntry;
use std::path::Path;

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
