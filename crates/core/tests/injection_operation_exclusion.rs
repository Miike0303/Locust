use locust_core::{
    backup::{BackupManager, BackupManifest},
    database::Database,
    injection_transaction,
    models::{StringEntry, StringStatus},
    patch::{self, ApplyOptions, PackOptions, RollbackOptions},
};
use serde_json::json;
use std::{fs, path::Path};

fn recognized_store(game: &Path) {
    let root = game.canonicalize().unwrap();
    let store = game.join(injection_transaction::STORE_DIR);
    fs::create_dir(&store).unwrap();
    fs::write(
        store.join("store.json"),
        serde_json::to_vec(&json!({
            "schema_version":1,"kind":"locust-injection-store","game_root":root
        }))
        .unwrap(),
    )
    .unwrap();
}

fn interrupted_preparation(game: &Path) {
    recognized_store(game);
    let id = uuid::Uuid::new_v4().to_string();
    let store = game.join(injection_transaction::STORE_DIR);
    let operation = store.join("operations").join(&id);
    fs::create_dir_all(&operation).unwrap();
    fs::write(operation.join("phase.json"), b"\"preparing\"").unwrap();
    fs::write(
        store.join("active.json"),
        serde_json::to_vec(&json!({
            "schema_version":1,"transaction_id":id,"game_root":game.canonicalize().unwrap(),
            "format":"html-game","language":"es"
        }))
        .unwrap(),
    )
    .unwrap();
}

#[test]
fn pending_insertion_blocks_other_operations_until_public_recovery() {
    let temp = tempfile::tempdir().unwrap();
    let game = temp.path().join("game");
    fs::create_dir(&game).unwrap();
    let story = game.join("story.html");
    fs::write(&story, b"Hola").unwrap();
    let manager = BackupManager::new(temp.path().join("backups"));
    let backup = manager.create_backup(&game).unwrap();
    let db = Database::open_in_memory().unwrap();
    let mut entry = StringEntry::new("line", "Hello", story.clone());
    entry.translation = Some("Hola".into());
    entry.status = StringStatus::Translated;
    db.save_entries(&[entry]).unwrap();
    db.record_injection(Some("es"), &game, std::slice::from_ref(&story))
        .unwrap();
    interrupted_preparation(&game);
    let status = injection_transaction::status(&game).unwrap();
    assert!(status.pending.is_some());
    let marker = fs::read(game.join(".locust-injections/active.json")).unwrap();
    assert!(patch::apply(
        &game,
        &temp.path().join("missing.zip"),
        ApplyOptions::default(),
        |_| {}
    )
    .is_err());
    assert!(patch::rollback(&game, RollbackOptions::default()).is_err());
    assert!(manager.restore(&backup.id).is_err());
    assert!(locust_core::project::lock_game_source(&game).is_err());
    let output = temp.path().join("output/patch.zip");
    let options = PackOptions {
        game: None,
        game_path: game.clone(),
        lang: Some("es".into()),
        output: output.clone(),
        pristine: None,
        engine: Some("html-game".into()),
        project: temp.path().join("project.db"),
        require_pristine: false,
    };
    assert!(patch::pack_injection_recording(&db, options.clone()).is_err());
    assert!(!output.parent().unwrap().exists());
    assert!(!game.join(".locust").exists());
    assert_eq!(fs::read(&story).unwrap(), b"Hola");
    assert_eq!(
        fs::read(game.join(".locust-injections/active.json")).unwrap(),
        marker
    );
    injection_transaction::recover(&game, Default::default()).unwrap();
    assert!(injection_transaction::status(&game)
        .unwrap()
        .pending
        .is_none());
    manager.restore(&backup.id).unwrap();
    assert!(patch::pack_injection_recording(&db, options).is_ok());
}

#[test]
fn backup_excludes_recognized_insertion_history_and_restore_preserves_it() {
    let temp = tempfile::tempdir().unwrap();
    let game = temp.path().join("game");
    fs::create_dir(&game).unwrap();
    fs::write(game.join("story.html"), "Original").unwrap();
    fs::create_dir(game.join(".locust-injections-user")).unwrap();
    fs::write(game.join(".locust-injections-user/data"), "user").unwrap();
    recognized_store(&game);
    fs::write(game.join(".locust-injections/history-sentinel"), "history").unwrap();
    let manager = BackupManager::new(temp.path().join("backups"));
    let backup = manager.create_backup(&game).unwrap();
    assert_eq!(backup.file_count, 2);
    assert!(!backup.path.join("payload/.locust-injections").exists());
    assert!(backup
        .path
        .join("payload/.locust-injections-user/data")
        .exists());
    fs::write(game.join("story.html"), "Changed").unwrap();
    manager.restore(&backup.id).unwrap();
    assert_eq!(
        fs::read_to_string(game.join("story.html")).unwrap(),
        "Original"
    );
    assert_eq!(
        fs::read_to_string(game.join(".locust-injections/history-sentinel")).unwrap(),
        "history"
    );
}

#[test]
fn unknown_namespace_and_legacy_journal_payload_are_refused_without_writing() {
    let temp = tempfile::tempdir().unwrap();
    let game = temp.path().join("game");
    fs::create_dir(&game).unwrap();
    fs::write(game.join("a"), "sentinel").unwrap();
    let namespace = game.join(".locust-injections");
    fs::create_dir(&namespace).unwrap();
    fs::write(namespace.join("store.json"), "unknown user bytes").unwrap();
    let manager = BackupManager::new(temp.path().join("backups"));
    assert!(manager.create_backup(&game).is_err());
    assert!(!temp.path().join("backups").exists());
    // Move the fixture's unknown directory aside; no recovery adopts it.
    fs::rename(&namespace, game.join("unknown-preserved")).unwrap();
    let old = temp.path().join("backups/legacy");
    fs::create_dir_all(old.join(".locust-injections")).unwrap();
    fs::write(old.join("a"), "old").unwrap();
    fs::write(old.join(".locust-injections/active.json"), "{}").unwrap();
    fs::write(
        old.join("manifest.json"),
        serde_json::to_vec(&BackupManifest {
            source_path: game.canonicalize().unwrap(),
            created_at: chrono::Utc::now(),
            file_count: 2,
            size_bytes: 5,
        })
        .unwrap(),
    )
    .unwrap();
    assert!(manager
        .restore("legacy")
        .unwrap_err()
        .to_string()
        .contains("insertion recovery metadata"));
    assert_eq!(fs::read_to_string(game.join("a")).unwrap(), "sentinel");
    assert!(!namespace.exists());
}
