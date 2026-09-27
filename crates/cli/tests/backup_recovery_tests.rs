use locust_core::backup::BackupManager;

#[test]
fn cli_can_list_and_restore_an_injection_backup() {
    let root = tempfile::tempdir().unwrap();
    let game = root.path().join("game");
    let backup_root = root.path().join("backups");
    std::fs::create_dir(&game).unwrap();
    let script = game.join("script.rpy");
    std::fs::write(&script, b"original").unwrap();

    let backup = BackupManager::new(backup_root.clone())
        .create_backup(&game)
        .unwrap();
    std::fs::write(&script, b"translated").unwrap();

    let listed = assert_cmd::Command::cargo_bin("locust")
        .unwrap()
        .env("LOCUST_DATA_DIR", root.path().join("profile"))
        .env("LOCUST_BACKUP_ROOT", &backup_root)
        .arg("backups")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let listed = String::from_utf8(listed).unwrap();
    assert!(listed.contains(&backup.id), "{listed}");
    assert!(listed.contains(&game.display().to_string()), "{listed}");

    assert_cmd::Command::cargo_bin("locust")
        .unwrap()
        .env("LOCUST_DATA_DIR", root.path().join("profile"))
        .env("LOCUST_BACKUP_ROOT", &backup_root)
        .args(["restore-backup", &backup.id])
        .assert()
        .success();
    assert_eq!(std::fs::read(script).unwrap(), b"original");
}

#[test]
fn restore_backup_dry_run_checks_payload_without_writing_game() {
    let root = tempfile::tempdir().unwrap();
    let game = root.path().join("game");
    let backup_root = root.path().join("backups");
    std::fs::create_dir(&game).unwrap();
    let script = game.join("script.rpy");
    std::fs::write(&script, b"original").unwrap();
    let backup = BackupManager::new(backup_root.clone())
        .create_backup(&game)
        .unwrap();
    std::fs::write(&script, b"translated").unwrap();

    let run = || {
        let mut command = assert_cmd::Command::cargo_bin("locust").unwrap();
        command
            .env("LOCUST_DATA_DIR", root.path().join("profile"))
            .env("LOCUST_BACKUP_ROOT", &backup_root)
            .args(["restore-backup", &backup.id, "--dry-run"]);
        command
    };
    run().assert().success();
    assert_eq!(std::fs::read(&script).unwrap(), b"translated");

    std::fs::write(backup.path.join("payload/script.rpy"), b"corrupted").unwrap();
    run().assert().failure();
    assert_eq!(std::fs::read(&script).unwrap(), b"translated");
}
