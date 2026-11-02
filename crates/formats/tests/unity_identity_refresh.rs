use std::fs;
use std::path::PathBuf;

use locust_core::database::{Database, EntryFilter};
use locust_core::extraction::FormatPlugin;
use locust_core::models::{StringEntry, StringStatus, STALE_TRANSLATION_METADATA_KEY};
use locust_formats::unity::UnityPlugin;

struct Fixture {
    _temp: tempfile::TempDir,
    game: PathBuf,
    db_path: PathBuf,
    db: Database,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let game = temp.path().join("game");
        fs::create_dir_all(game.join("Test_Data")).unwrap();
        let db_path = temp.path().join("project.sqlite");
        let db = Database::open(&db_path).unwrap();
        Self {
            _temp: temp,
            game,
            db_path,
            db,
        }
    }

    fn mesh(&self, name: &str, text: &str) {
        fs::write(self.game.join("Test_Data").join(name), textmesh(text)).unwrap();
    }

    fn script(&self, directory: &str, text: &str) {
        let directory = self.game.join("Test_Data/SCRIPTS~").join(directory);
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("a.txt"), format!("L {text}\n")).unwrap();
    }

    fn extract(&self) -> Vec<StringEntry> {
        UnityPlugin::new().extract(&self.game).unwrap()
    }

    fn rows(&self) -> Vec<StringEntry> {
        self.db.get_entries(&EntryFilter::default()).unwrap()
    }

    fn row(&self, file: &str) -> StringEntry {
        self.rows()
            .into_iter()
            .find(|row| row.file_path.ends_with(file))
            .unwrap()
    }

    async fn translate(&self, id: &str, text: &str) -> StringEntry {
        self.db
            .save_translation_and_status_if_unchanged(
                id,
                text,
                "manual",
                Some(StringStatus::Approved),
                None,
            )
            .await
            .unwrap();
        self.db.get_entry(id).unwrap().unwrap()
    }
}

// Minimal v17 SerializedFile, matching unity_serialized.rs's TextMesh fixture:
// class-141 objects, no type tree, and m_Text after the GameObject PPtr.
fn textmesh(text: &str) -> Vec<u8> {
    textmeshes(&[text])
}

fn textmeshes(texts: &[&str]) -> Vec<u8> {
    let mut payload = Vec::new();
    let mut ranges = Vec::new();
    for text in texts {
        let start = payload.len();
        payload.extend_from_slice(&0i32.to_le_bytes());
        payload.extend_from_slice(&1i64.to_le_bytes());
        payload.extend_from_slice(&(text.len() as u32).to_le_bytes());
        payload.extend_from_slice(text.as_bytes());
        while !payload.len().is_multiple_of(4) {
            payload.push(0);
        }
        payload.extend_from_slice(&0f32.to_le_bytes());
        payload.extend_from_slice(&1f32.to_le_bytes());
        ranges.push((start, payload.len() - start));
    }
    let mut meta = b"2019.4.0f1\0".to_vec();
    meta.extend_from_slice(&1u32.to_le_bytes());
    meta.push(0);
    meta.extend_from_slice(&1i32.to_le_bytes());
    meta.extend_from_slice(&141i32.to_le_bytes());
    meta.push(0);
    meta.extend_from_slice(&(-1i16).to_le_bytes());
    meta.extend_from_slice(&[0; 16]);
    meta.extend_from_slice(&(ranges.len() as i32).to_le_bytes());
    for (start, len) in ranges {
        while !meta.len().is_multiple_of(4) {
            meta.push(0);
        }
        meta.extend_from_slice(&42i64.to_le_bytes());
        meta.extend_from_slice(&(start as u32).to_le_bytes());
        meta.extend_from_slice(&(len as u32).to_le_bytes());
        meta.extend_from_slice(&0i32.to_le_bytes());
    }
    let data_offset = (20 + meta.len() + 15) & !15;
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&(meta.len() as u32).to_be_bytes());
    bytes.extend_from_slice(&((data_offset + payload.len()) as u32).to_be_bytes());
    bytes.extend_from_slice(&17u32.to_be_bytes());
    bytes.extend_from_slice(&(data_offset as u32).to_be_bytes());
    bytes.extend_from_slice(&[0; 4]);
    bytes.extend_from_slice(&meta);
    bytes.resize(data_offset, 0);
    bytes.extend_from_slice(&payload);
    bytes
}

fn assert_saved(actual: &StringEntry, saved: &StringEntry) {
    assert_eq!(actual.id, saved.id, "keep the saved key and its history");
    assert_eq!(actual.translation, saved.translation);
    assert_eq!(actual.status, saved.status);
    assert_eq!(actual.provider_used, saved.provider_used);
    assert_eq!(actual.created_at, saved.created_at);
    assert_eq!(actual.translated_at, saved.translated_at);
    assert_eq!(actual.reviewed_at, saved.reviewed_at);
}

fn strip_identity(rows: &mut [StringEntry]) {
    for row in rows {
        row.metadata.remove("unity_identity");
        row.metadata.remove("format_id");
    }
}

#[tokio::test]
async fn textmesh_translation_survives_collision_appearing_and_disappearing() {
    let mut f = Fixture::new();
    f.mesh("A.assets", "Welcome traveler!");
    let initial = f.extract();
    assert_eq!(initial.len(), 1);
    assert_eq!(initial[0].id, "textmesh/42");
    f.db.merge_entries(&initial).unwrap();
    let saved = f.translate(&initial[0].id, "¡Bienvenido!").await;
    let history = serde_json::to_value(f.db.get_string_history(&saved.id, 50).unwrap()).unwrap();
    f.mesh("B.assets", "Welcome traveler!");
    let colliding = f.extract();
    assert!(colliding.iter().all(|row| row.id.starts_with("unity:")));
    let stats = f.db.merge_entries(&colliding).unwrap();
    assert_eq!(stats.preserved_translations, 1);
    assert_eq!(stats.lost_translations, 0);
    assert_eq!(stats.added, 1);
    assert_saved(&f.row("A.assets"), &saved);
    let b = f.row("B.assets");
    assert_eq!(b.translation, None);
    assert_eq!(b.status, StringStatus::Pending);
    assert_ne!(b.id, saved.id);
    // Exercise persisted metadata, rather than retaining an in-memory alias map.
    f.db = Database::open(&f.db_path).unwrap();
    fs::remove_file(f.game.join("Test_Data/B.assets")).unwrap();
    let stats = f.db.merge_entries(&f.extract()).unwrap();
    assert_eq!(stats.removed, 1);
    assert_eq!(stats.lost_translations, 0);
    assert_eq!(f.rows().len(), 1);
    assert_saved(&f.row("A.assets"), &saved);
    assert_eq!(
        serde_json::to_value(f.db.get_string_history(&saved.id, 50).unwrap()).unwrap(),
        history
    );
}

#[tokio::test]
async fn script_translation_survives_basename_collision_churn() {
    let f = Fixture::new();
    f.script("chapter", "Welcome traveler!");
    f.db.merge_entries(&f.extract()).unwrap();
    let saved = f.translate("a.txt#1", "¡Bienvenido!").await;
    f.script("extra", "Welcome traveler!");
    f.db.merge_entries(&f.extract()).unwrap();
    assert_saved(&f.row("chapter/a.txt"), &saved);
    assert_eq!(f.row("extra/a.txt").translation, None);
    fs::remove_file(f.game.join("Test_Data/SCRIPTS~/extra/a.txt")).unwrap();
    f.db.merge_entries(&f.extract()).unwrap();
    assert_saved(&f.row("chapter/a.txt"), &saved);
}

#[tokio::test]
async fn legacy_unqualified_row_is_upgraded_without_losing_translation() {
    let f = Fixture::new();
    f.mesh("A.assets", "Welcome traveler!");
    let mut legacy = f.extract();
    strip_identity(&mut legacy);
    f.db.save_entries(&legacy).unwrap();
    let saved = f.translate("textmesh/42", "¡Bienvenido!").await;
    f.mesh("B.assets", "Welcome traveler!");
    f.db.merge_entries(&f.extract()).unwrap();
    assert_saved(&f.row("A.assets"), &saved);
    assert!(f.row("A.assets").metadata.contains_key("unity_identity"));
    assert_eq!(f.row("B.assets").translation, None);
}

#[tokio::test]
async fn legacy_qualified_row_survives_collision_disappearing() {
    let f = Fixture::new();
    f.mesh("A.assets", "Welcome traveler!");
    f.mesh("B.assets", "Welcome traveler!");
    let mut legacy = f.extract();
    strip_identity(&mut legacy);
    f.db.save_entries(&legacy).unwrap();
    let saved = f.translate(&f.row("A.assets").id, "¡Bienvenido!").await;
    fs::remove_file(f.game.join("Test_Data/B.assets")).unwrap();
    f.db.merge_entries(&f.extract()).unwrap();
    assert_saved(&f.row("A.assets"), &saved);
}

#[tokio::test]
async fn changed_source_uses_existing_stale_translation_handling_after_remap() {
    let f = Fixture::new();
    f.mesh("A.assets", "Welcome traveler!");
    f.db.merge_entries(&f.extract()).unwrap();
    let saved = f.translate("textmesh/42", "¡Bienvenido!").await;
    f.mesh("A.assets", "Goodbye traveler!");
    f.mesh("B.assets", "Welcome traveler!");
    let stats = f.db.merge_entries(&f.extract()).unwrap();
    assert_eq!(stats.stale_source_reset, 1);
    assert_eq!(stats.lost_translations, 1);
    let stale = f.row("A.assets");
    assert_eq!(stale.translation, saved.translation);
    assert_eq!(stale.status, StringStatus::Pending);
    let marker = stale.metadata[STALE_TRANSLATION_METADATA_KEY].clone();
    fs::remove_file(f.game.join("Test_Data/B.assets")).unwrap();
    f.db.merge_entries(&f.extract()).unwrap();
    let refreshed = f.row("A.assets");
    assert_eq!(refreshed.translation, saved.translation);
    assert_eq!(refreshed.status, StringStatus::Pending);
    assert_eq!(refreshed.metadata[STALE_TRANSLATION_METADATA_KEY], marker);
}

#[tokio::test]
async fn removing_a_preserves_bs_own_translation_without_alias_reuse() {
    let f = Fixture::new();
    f.mesh("A.assets", "Welcome traveler!");
    f.db.merge_entries(&f.extract()).unwrap();
    f.translate("textmesh/42", "Traducción de A").await;
    f.mesh("B.assets", "Welcome traveler!");
    f.db.merge_entries(&f.extract()).unwrap();
    let b = f.translate(&f.row("B.assets").id, "Traducción de B").await;
    fs::remove_file(f.game.join("Test_Data/A.assets")).unwrap();
    f.db.merge_entries(&f.extract()).unwrap();
    assert_eq!(f.rows().len(), 1);
    assert_saved(&f.row("B.assets"), &b);
}

#[tokio::test]
async fn same_local_id_in_replacement_file_never_inherits_saved_translation() {
    let f = Fixture::new();
    f.mesh("A.assets", "Welcome traveler!");
    f.db.merge_entries(&f.extract()).unwrap();
    f.translate("textmesh/42", "Traducción de A").await;
    fs::remove_file(f.game.join("Test_Data/A.assets")).unwrap();
    f.mesh("B.assets", "Welcome traveler!");
    let stats = f.db.merge_entries(&f.extract()).unwrap();
    let b = f.row("B.assets");
    assert_eq!(b.translation, None);
    assert_eq!(b.status, StringStatus::Pending);
    assert!(f.db.get_string_history(&b.id, 50).unwrap().is_empty());
    assert_eq!(
        (stats.added, stats.removed, stats.lost_translations),
        (1, 1, 1)
    );
}

#[tokio::test]
async fn ambiguous_saved_identity_uses_plain_ids_without_guessing() {
    // An incomplete extraction can leave both the legacy and qualified aliases
    // saved. Exercise both an exact incoming ID match and no matching alias.
    for relative_path in ["Test_Data/A.assets", "A.assets"] {
        let f = Fixture::new();
        f.mesh("A.assets", "Welcome traveler!");
        let mut old = f.extract();
        let mut duplicate = old[0].clone();
        duplicate.id = format!(
            "unity:{}:textmesh/42",
            serde_json::json!([relative_path, 0])
        );
        old.push(duplicate);
        strip_identity(&mut old);
        f.db.save_entries(&old).unwrap();
        let first = f.translate(&old[0].id, "Primera traducción").await;
        let second = f.translate(&old[1].id, "Segunda traducción").await;
        f.mesh("B.assets", "Welcome traveler!");
        let stats = f.db.merge_entries_preserving_missing(&f.extract()).unwrap();
        assert_eq!(stats.removed, 0);
        assert_eq!(stats.lost_translations, 0);
        assert_saved(&f.db.get_entry(&first.id).unwrap().unwrap(), &first);
        assert_saved(&f.db.get_entry(&second.id).unwrap().unwrap(), &second);
        for row in f.rows() {
            if row.id != first.id && row.id != second.id {
                assert_eq!(row.translation, None);
                assert_eq!(row.status, StringStatus::Pending);
            }
        }
        assert_eq!(f.row("B.assets").translation, None);
        assert_eq!(f.row("B.assets").status, StringStatus::Pending);
    }
}

#[tokio::test]
async fn ambiguous_saved_identity_complete_refresh_keeps_exact_id_translation() {
    let f = Fixture::new();
    f.mesh("A.assets", "Welcome traveler!");
    let mut old = f.extract();
    let mut duplicate = old[0].clone();
    duplicate.id = "unity:[\"Test_Data/A.assets\",0]:textmesh/42".into();
    old.push(duplicate);
    strip_identity(&mut old);
    f.db.save_entries(&old).unwrap();
    f.translate(&old[0].id, "Legacy translation").await;
    let qualified = f.translate(&old[1].id, "Exact ID translation").await;
    f.mesh("B.assets", "Welcome traveler!");
    let stats = f.db.merge_entries(&f.extract()).unwrap();
    // Plain ID merging removes the absent alias, while the matching qualified
    // row keeps its own translation. Ambiguous aliases never get combined.
    assert_eq!(stats.removed, 1);
    assert_saved(&f.row("A.assets"), &qualified);
    assert_eq!(f.row("B.assets").translation, None);
    assert_eq!(f.row("B.assets").status, StringStatus::Pending);
}

#[tokio::test]
async fn conflicting_saved_aliases_preserve_saved_field_in_any_incoming_order() {
    for reverse in [false, true] {
        let f = Fixture::new();
        f.mesh("A.assets", "Welcome traveler!");
        let mut old = f.extract();
        // A historical saved alias can collide with a newly extracted row's
        // local key even when the incoming physical identities are unique.
        old[0].id = "a.txt#1".into();
        f.db.save_entries(&old).unwrap();
        let saved = f.translate("a.txt#1", "Traducción de A").await;
        let before = serde_json::to_value(f.rows()).unwrap();
        f.script("chapter", "An unrelated field");
        let mut incoming = f.extract();
        if reverse {
            incoming.reverse();
        }
        let stats = f.db.merge_entries(&incoming).unwrap();
        assert_eq!(
            (stats.added, stats.removed, stats.lost_translations),
            (0, 0, 0)
        );
        assert_eq!(stats.preserved_translations, 1);
        assert_eq!(serde_json::to_value(f.rows()).unwrap(), before);
        assert_saved(&f.row("A.assets"), &saved);
    }
}

#[test]
fn duplicate_incoming_identity_is_refused_before_id_deduplication() {
    let f = Fixture::new();
    f.mesh("A.assets", "Welcome traveler!");
    let mut incoming = f.extract();
    incoming.push(incoming[0].clone());
    assert!(f.db.merge_entries(&incoming).is_err());
    assert!(f.rows().is_empty());
}

#[tokio::test]
async fn partial_refresh_remaps_without_deleting_unread_rows() {
    let f = Fixture::new();
    f.mesh("A.assets", "Welcome traveler!");
    f.mesh("B.assets", "Welcome traveler!");
    f.db.merge_entries(&f.extract()).unwrap();
    let a = f.translate(&f.row("A.assets").id, "Traducción de A").await;
    let b = f.translate(&f.row("B.assets").id, "Traducción de B").await;
    fs::remove_file(f.game.join("Test_Data/B.assets")).unwrap();
    let stats = f.db.merge_entries_preserving_missing(&f.extract()).unwrap();
    assert_eq!(stats.removed, 0);
    assert_eq!(f.rows().len(), 2);
    assert_saved(&f.row("A.assets"), &a);
    assert_saved(&f.row("B.assets"), &b);
}

#[test]
fn extracted_identity_records_path_local_id_and_occurrence_without_changing_legacy_id() {
    let f = Fixture::new();
    f.mesh("A.assets", "Welcome traveler!");
    let rows = f.extract();
    assert_eq!(rows[0].id, "textmesh/42");
    let identity = &rows[0].metadata["unity_identity"];
    assert_eq!(identity["version"], 1);
    assert_eq!(identity["local_id"], "textmesh/42");
    assert_eq!(identity["occurrence"], 0);
    assert_eq!(
        identity["physical_path"],
        rows[0].file_path.to_string_lossy().replace('\\', "/")
    );
}

#[tokio::test]
async fn non_unity_rows_retain_existing_id_based_merge_semantics() {
    let f = Fixture::new();
    let mut entry = StringEntry::new("shared-id", "Welcome traveler!", "old.json".into())
        .with_format_id("rpgmaker-mv");
    // Unrelated metadata must not opt another format into Unity reconciliation.
    entry.metadata.insert("unity_identity".into(), true.into());
    f.db.merge_entries(std::slice::from_ref(&entry)).unwrap();
    let saved = f.translate(&entry.id, "¡Bienvenido!").await;
    let mut incoming = entry;
    incoming.file_path = "new.json".into();
    f.db.merge_entries(&[incoming]).unwrap();
    assert_saved(&f.db.get_entry(&saved.id).unwrap().unwrap(), &saved);
}

#[tokio::test]
async fn occurrences_keep_identical_fields_in_one_file_separate() {
    let f = Fixture::new();
    fs::write(
        f.game.join("Test_Data/A.assets"),
        textmeshes(&["Welcome traveler!", "Welcome traveler!"]),
    )
    .unwrap();
    let initial = f.extract();
    assert_eq!(initial.len(), 2);
    assert_eq!(initial[0].metadata["unity_identity"]["occurrence"], 0);
    assert_eq!(initial[1].metadata["unity_identity"]["occurrence"], 1);
    let mut legacy = initial.clone();
    strip_identity(&mut legacy);
    f.db.save_entries(&legacy).unwrap();
    let first = f.translate(&initial[0].id, "Primera traducción").await;
    let second = f.translate(&initial[1].id, "Segunda traducción").await;
    f.mesh("B.assets", "Welcome traveler!");
    let mut incoming = f.extract();
    incoming.reverse();
    f.db.merge_entries(&incoming).unwrap();
    assert_eq!(f.rows().len(), 3);
    assert_saved(&f.db.get_entry(&first.id).unwrap().unwrap(), &first);
    assert_saved(&f.db.get_entry(&second.id).unwrap().unwrap(), &second);
    assert_eq!(f.row("B.assets").translation, None);
}

#[tokio::test]
async fn partial_refresh_skips_reused_id_and_preserves_saved_field() {
    let f = Fixture::new();
    f.mesh("A.assets", "Welcome traveler!");
    f.db.merge_entries(&f.extract()).unwrap();
    let saved = f.translate("textmesh/42", "Traducción de A").await;
    let before = serde_json::to_value(f.rows()).unwrap();
    let history = serde_json::to_value(f.db.get_string_history(&saved.id, 50).unwrap()).unwrap();
    fs::remove_file(f.game.join("Test_Data/A.assets")).unwrap();
    f.mesh("B.assets", "Welcome traveler!");
    let stats = f.db.merge_entries_preserving_missing(&f.extract()).unwrap();
    assert_eq!(
        (stats.added, stats.removed, stats.lost_translations),
        (0, 0, 0)
    );
    assert_eq!(serde_json::to_value(f.rows()).unwrap(), before);
    assert_eq!(
        serde_json::to_value(f.db.get_string_history(&saved.id, 50).unwrap()).unwrap(),
        history
    );
    assert_saved(&f.row("A.assets"), &saved);
    // A later complete extraction can safely replace A with a fresh pending B.
    f.db.merge_entries(&f.extract()).unwrap();
    assert_eq!(f.row("B.assets").translation, None);
    assert_eq!(f.row("B.assets").status, StringStatus::Pending);
}

#[tokio::test]
async fn pivot_refresh_skips_reused_id_and_preserves_saved_field() {
    let mut f = Fixture::new();
    f.mesh("A.assets", "Welcome traveler!");
    f.script("chapter", "An unchanged companion field");
    f.db.merge_entries(&f.extract()).unwrap();
    f.translate("textmesh/42", "¡Bienvenido!").await;
    f.translate("a.txt#1", "Otro campo sin cambios").await;
    let pivot_path = f._temp.path().join("pivot.sqlite");
    f.db.pivot_to(&pivot_path).unwrap();
    f.db = Database::open(&pivot_path).unwrap();
    let saved = f.translate("textmesh/42", "Bienvenue !").await;
    f.translate("a.txt#1", "Un autre champ inchangé").await;
    let before = serde_json::to_value(f.rows()).unwrap();
    let history = serde_json::to_value(f.db.get_string_history(&saved.id, 50).unwrap()).unwrap();
    fs::remove_file(f.game.join("Test_Data/A.assets")).unwrap();
    // Deliberately different source: a skipped field must not enter pivot
    // baseline validation under another physical field's saved ID.
    f.mesh("B.assets", "A completely different field");
    for preserve_missing in [false, true] {
        let incoming = f.extract();
        let stats = if preserve_missing {
            f.db.merge_entries_preserving_missing(&incoming)
        } else {
            f.db.merge_entries(&incoming)
        }
        .unwrap();
        assert_eq!(
            (stats.added, stats.removed, stats.lost_translations),
            (0, 0, 0)
        );
        assert_eq!(serde_json::to_value(f.rows()).unwrap(), before);
        assert_eq!(
            serde_json::to_value(f.db.get_string_history(&saved.id, 50).unwrap()).unwrap(),
            history
        );
        assert_saved(&f.row("A.assets"), &saved);
    }
    // Skipping B must not relax validation of any other saved pivot field.
    f.script("chapter", "A changed companion field");
    assert!(f.db.merge_entries(&f.extract()).is_err());
    assert_eq!(serde_json::to_value(f.rows()).unwrap(), before);
    fs::remove_file(f.game.join("Test_Data/SCRIPTS~/chapter/a.txt")).unwrap();
    assert!(f.db.merge_entries(&f.extract()).is_err());
    assert_eq!(serde_json::to_value(f.rows()).unwrap(), before);
}

#[tokio::test]
async fn malformed_saved_identity_skips_incoming_row_without_losing_translation() {
    let f = Fixture::new();
    f.mesh("A.assets", "Welcome traveler!");
    let mut old = f.extract();
    old[0].metadata.get_mut("unity_identity").unwrap()["occurrence"] = "unknown".into();
    f.db.save_entries(&old).unwrap();
    let saved = f.translate("textmesh/42", "¡Bienvenido!").await;
    let before = serde_json::to_value(f.rows()).unwrap();
    for preserve_missing in [false, true] {
        let incoming = f.extract();
        let stats = if preserve_missing {
            f.db.merge_entries_preserving_missing(&incoming)
        } else {
            f.db.merge_entries(&incoming)
        }
        .unwrap();
        assert_eq!(
            (stats.added, stats.removed, stats.lost_translations),
            (0, 0, 0)
        );
        assert_eq!(serde_json::to_value(f.rows()).unwrap(), before);
        assert_saved(&f.row("A.assets"), &saved);
    }
}

#[tokio::test]
async fn pivot_refresh_reconciles_aliases_and_still_refuses_changed_baseline() {
    let mut f = Fixture::new();
    f.mesh("A.assets", "Welcome traveler!");
    f.db.merge_entries(&f.extract()).unwrap();
    f.translate("textmesh/42", "¡Bienvenido!").await;
    let pivot_path = f._temp.path().join("pivot.sqlite");
    f.db.pivot_to(&pivot_path).unwrap();
    f.db = Database::open(&pivot_path).unwrap();
    let onward = f.translate("textmesh/42", "Bienvenue !").await;
    f.mesh("B.assets", "Welcome traveler!");
    let stats = f.db.merge_entries(&f.extract()).unwrap();
    assert_eq!(stats.added, 0);
    assert_eq!(f.rows().len(), 1);
    assert_eq!(f.row("A.assets").source, "¡Bienvenido!");
    assert_saved(&f.row("A.assets"), &onward);
    let before = serde_json::to_value(f.rows()).unwrap();
    f.mesh("A.assets", "Goodbye traveler!");
    assert!(f.db.merge_entries(&f.extract()).is_err());
    assert_eq!(serde_json::to_value(f.rows()).unwrap(), before);
}

#[tokio::test]
async fn malformed_incoming_identity_preserves_database_atomically() {
    let f = Fixture::new();
    f.mesh("A.assets", "Welcome traveler!");
    f.db.merge_entries(&f.extract()).unwrap();
    f.translate("textmesh/42", "¡Bienvenido!").await;
    let before = serde_json::to_value(f.rows()).unwrap();
    f.mesh("B.assets", "Welcome traveler!");
    let mut incoming = f.extract();
    incoming[0].metadata.get_mut("unity_identity").unwrap()["occurrence"] = "unknown".into();
    assert!(f.db.merge_entries(&incoming).is_err());
    assert_eq!(serde_json::to_value(f.rows()).unwrap(), before);
}
