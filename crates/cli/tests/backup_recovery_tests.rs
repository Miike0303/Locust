use locust_core::backup::BackupManager;

#[test]
fn cli_lists_readable_backup_and_warns_about_damaged_manifest() {
    let root = tempfile::tempdir().unwrap();
    let game = root.path().join("game");
    let backup_root = root.path().join("backups");
    std::fs::create_dir(&game).unwrap();
    std::fs::write(game.join("story.txt"), b"original").unwrap();
    let valid = BackupManager::new(backup_root.clone())
        .create_backup(&game)
        .unwrap();
    let damaged = backup_root.join("damaged");
    std::fs::create_dir(&damaged).unwrap();
    std::fs::write(damaged.join("manifest.json"), b"{").unwrap();

    let output = assert_cmd::Command::cargo_bin("locust")
        .unwrap()
        .env("LOCUST_DATA_DIR", root.path().join("profile"))
        .env("LOCUST_BACKUP_ROOT", &backup_root)
        .arg("backups")
        .assert()
        .success()
        .get_output()
        .clone();
    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert_eq!(stdout.lines().count(), 1, "{stdout}");
    assert!(stdout.starts_with(&format!("{}\t", valid.id)), "{stdout}");
    assert!(
        stderr.contains("warning: backup damaged has an unreadable manifest.json:"),
        "{stderr}"
    );
    assert!(stderr.contains("EOF"), "{stderr}");
}

#[test]
fn cli_fails_and_warns_for_each_backup_when_all_manifests_are_damaged() {
    let root = tempfile::tempdir().unwrap();
    let backup_root = root.path().join("backups");
    let ids = ["damaged-first", "damaged-second"];
    for id in ids {
        let damaged = backup_root.join(id);
        std::fs::create_dir_all(&damaged).unwrap();
        std::fs::write(damaged.join("manifest.json"), b"{").unwrap();
    }

    let output = assert_cmd::Command::cargo_bin("locust")
        .unwrap()
        .env("LOCUST_DATA_DIR", root.path().join("profile"))
        .env("LOCUST_BACKUP_ROOT", &backup_root)
        .arg("backups")
        .assert()
        .failure()
        .get_output()
        .clone();
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).unwrap();
    for id in ids {
        assert!(
            stderr.contains(&format!(
                "warning: backup {id} has an unreadable manifest.json:"
            )),
            "{stderr}"
        );
    }
}

#[test]
fn cli_reports_no_backups_for_an_empty_backup_root() {
    let root = tempfile::tempdir().unwrap();
    let output = assert_cmd::Command::cargo_bin("locust")
        .unwrap()
        .env("LOCUST_DATA_DIR", root.path().join("profile"))
        .env("LOCUST_BACKUP_ROOT", root.path().join("backups"))
        .arg("backups")
        .assert()
        .success()
        .get_output()
        .clone();
    assert_eq!(output.stdout, b"No injection backups found.\n");
    assert!(output.stderr.is_empty());
}

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
    let output = run().assert().success().get_output().stdout.clone();
    let output = String::from_utf8(output).unwrap();
    let expected = format!(
        "Destination: {}\n\
         Would replace 1 file(s):\n  script.rpy\n\
         Would recreate 0 missing file(s):\n\
         Already identical: 0\n\
         Files added to the game after this backup are not removed.\n",
        game.canonicalize().unwrap().display()
    );
    assert_eq!(output, expected);
    assert_eq!(std::fs::read(&script).unwrap(), b"translated");

    std::fs::write(backup.path.join("payload/script.rpy"), b"corrupted").unwrap();
    run().assert().failure();
    assert_eq!(std::fs::read(&script).unwrap(), b"translated");
}
