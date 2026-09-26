use locust_core::database::{Database, RecordedBackup};
use std::fs;

#[test]
fn provenance_survives_reopen_noop_and_failed_recording() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("game");
    fs::create_dir(&root).unwrap();
    let file = root.join("story.txt");
    fs::write(&file, "translated").unwrap();
    let path = temp.path().join("project.db");
    let backup = RecordedBackup {
        id: "original-copy".into(),
        source_path: root.clone(),
        storage_root: None,
    };
    {
        let db = Database::open(&path).unwrap();
        db.record_injection_with_backup(
            Some("es"),
            &root,
            std::slice::from_ref(&file),
            Some(&backup),
        )
        .unwrap();
        let different = RecordedBackup {
            id: "already-translated-copy".into(),
            source_path: root.clone(),
            storage_root: None,
        };
        db.record_injection_with_backup(Some("es"), &root, &[], Some(&different))
            .unwrap();
        assert_eq!(
            db.get_injection(Some("es"))
                .unwrap()
                .unwrap()
                .pristine_backup,
            Some(backup.clone())
        );
        let outsider = temp.path().join("outside.txt");
        fs::write(&outsider, "unrelated").unwrap();
        assert!(db
            .record_injection_with_backup(Some("es"), &root, &[outsider], Some(&different))
            .is_err());
    }
    let db = Database::open(&path).unwrap();
    assert_eq!(
        db.get_injection(Some("es"))
            .unwrap()
            .unwrap()
            .pristine_backup,
        Some(backup)
    );
    db.record_injection(None, &root, std::slice::from_ref(&file))
        .unwrap();
    assert!(db
        .get_injection(None)
        .unwrap()
        .unwrap()
        .pristine_backup
        .is_none());
    assert!(db
        .get_injection(Some("es"))
        .unwrap()
        .unwrap()
        .pristine_backup
        .is_some());
    db.record_injection(Some("es"), &root, &[file]).unwrap();
    assert!(db
        .get_injection(Some("es"))
        .unwrap()
        .unwrap()
        .pristine_backup
        .is_none());
}

#[test]
fn old_recording_schema_migrates_without_losing_hashes() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("old.db");
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch("CREATE TABLE injected_files (id INTEGER PRIMARY KEY, lang TEXT, root TEXT, rel TEXT, hash TEXT, size INTEGER, recorded_at TEXT);
        INSERT INTO injected_files VALUES(1,NULL,'game','story.txt','saved-hash',9,'saved-date');").unwrap();
    }
    let db = Database::open(&path).unwrap();
    let recording = db.get_injection(None).unwrap().unwrap();
    assert_eq!(recording.files[0].hash, "saved-hash");
    assert_eq!(recording.recorded_at, "saved-date");
    assert!(recording.pristine_backup.is_none());
}

#[test]
fn inconsistent_backup_rows_are_rejected() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("project.db");
    let root = temp.path().join("game");
    fs::create_dir(&root).unwrap();
    let first = root.join("first.txt");
    let second = root.join("second.txt");
    fs::write(&first, "one").unwrap();
    fs::write(&second, "two").unwrap();
    let db = Database::open(&path).unwrap();
    let backup = RecordedBackup {
        id: "verified-copy".into(),
        source_path: root.clone(),
        storage_root: None,
    };
    db.record_injection_with_backup(Some("es"), &root, &[first, second], Some(&backup))
        .unwrap();
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute(
        "UPDATE injected_files SET pristine_backup=NULL WHERE rel='second.txt'",
        [],
    )
    .unwrap();
    assert!(db
        .get_injection(Some("es"))
        .unwrap_err()
        .to_string()
        .contains("inconsistent injection recording"));
}

#[test]
fn pack_rejects_a_changed_generation_before_publishing_output() {
    use locust_core::{
        models::{StringEntry, StringStatus},
        patch::{pack_recorded_generation, PackOptions},
    };
    let temp = tempfile::tempdir().unwrap();
    let game = temp.path().join("game");
    fs::create_dir(&game).unwrap();
    let source = game.join("story.txt");
    fs::write(&source, "Hola").unwrap();
    let db = Database::open_in_memory().unwrap();
    let mut entry = StringEntry::new("line", "Hello", source.clone());
    entry.translation = Some("Hola".into());
    entry.status = StringStatus::Translated;
    db.save_entries(&[entry]).unwrap();
    let old = RecordedBackup {
        id: "first-original".into(),
        source_path: game.clone(),
        storage_root: None,
    };
    db.record_injection_with_backup(Some("es"), &game, std::slice::from_ref(&source), Some(&old))
        .unwrap();
    let snapshot = db.get_injection(Some("es")).unwrap().unwrap();
    let new = RecordedBackup {
        id: "other-baseline".into(),
        source_path: game.clone(),
        storage_root: None,
    };
    db.record_injection_with_backup(Some("es"), &game, std::slice::from_ref(&source), Some(&new))
        .unwrap();
    let output = temp.path().join("prior.zip");
    fs::write(&output, "existing archive").unwrap();
    let result = pack_recorded_generation(
        &db,
        PackOptions {
            game_path: game,
            lang: Some("es".into()),
            output: output.clone(),
            pristine: None,
            engine: None,
            project: temp.path().join("db"),
            require_pristine: false,
        },
        &snapshot,
    );
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("recording changed"));
    assert_eq!(fs::read_to_string(output).unwrap(), "existing archive");
}

#[test]
fn pack_recovers_the_recorded_store_after_reopen_for_directory_and_file() {
    use locust_core::backup::BackupManager;
    use locust_core::models::{StringEntry, StringStatus};
    use locust_core::patch::{pack_with_pristine_backup, verify, PackOptions, VerificationOutcome};
    for single_file in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let game = temp.path().join("game");
        fs::create_dir(&game).unwrap();
        let source = game.join("story.html");
        fs::write(&source, "Original Japanese").unwrap();
        let selection = if single_file { &source } else { &game };
        let original_store = BackupManager::new(temp.path().join("original-store"));
        let backup = original_store.create_backup(selection).unwrap();
        fs::write(&source, "Texto traducido").unwrap();
        let db_path = temp.path().join("project.db");
        let provenance = RecordedBackup {
            id: backup.id.clone(),
            source_path: backup.source_path.clone(),
            storage_root: Some(original_store.root().to_path_buf()),
        };
        {
            let db = Database::open(&db_path).unwrap();
            let mut entry = StringEntry::new("line", "Original Japanese", source.clone());
            entry.translation = Some("Texto traducido".into());
            entry.status = StringStatus::Translated;
            db.save_entries(&[entry]).unwrap();
            db.record_injection_with_backup(
                Some("es"),
                &game,
                std::slice::from_ref(&source),
                Some(&provenance),
            )
            .unwrap();
        }
        let db = Database::open(&db_path).unwrap();
        let other_store = BackupManager::new(temp.path().join("another-profile"));
        let options = PackOptions {
            game_path: selection.clone(),
            lang: None,
            output: temp.path().join("patch.zip"),
            pristine: None,
            engine: Some("html-game".into()),
            project: db_path,
            require_pristine: false,
        };
        let report =
            pack_with_pristine_backup(&db, options.clone(), &other_store, None, true).unwrap();
        assert_eq!(report.tier, "strict");
        let target = temp.path().join("target");
        fs::create_dir(&target).unwrap();
        fs::write(target.join("story.html"), "Original Japanese").unwrap();
        let verification = verify(&target, &options.output).unwrap();
        assert_eq!(verification.outcome, VerificationOutcome::Clean);
        assert_eq!(
            verification.manifest.unwrap().files[0]
                .original_sha256
                .as_deref(),
            Some(locust_core::database::sha256_hex(b"Original Japanese").as_str())
        );
        let saved_zip = fs::read(&options.output).unwrap();
        let bad =
            pack_with_pristine_backup(&db, options.clone(), &other_store, Some("wrong-id"), true);
        assert!(bad.unwrap_err().to_string().contains("does not match"));
        assert_eq!(fs::read(&options.output).unwrap(), saved_zip);
        for root in [original_store.root(), other_store.root()] {
            let mut forbidden = options.clone();
            forbidden.output = root.join("new-folder").join("overwrite.zip");
            assert!(pack_with_pristine_backup(&db, forbidden, &other_store, None, true).is_err());
            assert!(!root.join("new-folder").exists());
        }
        // The new optional location is backward compatible with the initial schema.
        let mut legacy = provenance.clone();
        legacy.storage_root = None;
        db.record_injection_with_backup(
            Some("es"),
            &game,
            std::slice::from_ref(&source),
            Some(&legacy),
        )
        .unwrap();
        assert_eq!(
            pack_with_pristine_backup(&db, options.clone(), &original_store, None, true)
                .unwrap()
                .tier,
            "strict"
        );
        db.record_injection_with_backup(Some("es"), &game, &[source], Some(&provenance))
            .unwrap();
        let saved_zip = fs::read(&options.output).unwrap();
        original_store.delete_backup(&backup.id).unwrap();
        assert!(pack_with_pristine_backup(&db, options.clone(), &other_store, None, true).is_err());
        assert_eq!(fs::read(&options.output).unwrap(), saved_zip);
        // An explicitly supplied intact copy remains a supported recovery route.
        let mut manual = options;
        manual.pristine = Some(target);
        assert_eq!(
            pack_with_pristine_backup(&db, manual, &other_store, None, true)
                .unwrap()
                .tier,
            "strict"
        );
    }
}
