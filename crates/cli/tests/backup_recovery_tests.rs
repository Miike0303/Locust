use locust_core::backup::BackupManager;

#[test]
fn c127_cli_restores_created_renpy_overlay_and_reports_kept_mods() {
    use locust_core::database::{Database, EntryFilter};
    use std::fs;
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let game = root.join("renpy");
    fs::create_dir_all(game.join("game")).unwrap();
    let archive = include_bytes!("fixtures/c127-scripts.rpa");
    fs::write(game.join("game/scripts.rpa"), archive).unwrap();
    let project = root.join("p.db");
    let run = || {
        let mut command = assert_cmd::Command::cargo_bin("locust").unwrap();
        command
            .env("LOCUST_DATA_DIR", root.join("profile"))
            .env("LOCUST_BACKUP_ROOT", root.join("backups"));
        command
    };
    run()
        .arg("extract")
        .arg(&game)
        .arg("-o")
        .arg(&project)
        .assert()
        .success();
    let db = Database::open(&project).unwrap();
    let mut entries = db.get_entries(&EntryFilter::default()).unwrap();
    assert_eq!(entries.len(), 1);
    entries[0].translation = Some("Translated archive dialogue.".into());
    entries[0].status = locust_core::models::StringStatus::Translated;
    db.save_entries(&entries).unwrap();
    run()
        .arg("inject")
        .arg(&game)
        .arg("-P")
        .arg(&project)
        .args(["-l", "en", "--direct"])
        .assert()
        .success();
    let overlay = game.join("game/scripts/story.rpy");
    assert!(fs::read_to_string(&overlay)
        .unwrap()
        .contains("Translated archive dialogue."));
    fs::write(game.join("game/mod.rpy"), "user mod").unwrap();
    let manager = BackupManager::new(root.join("backups"));
    let backup = manager.list_backups().unwrap().remove(0);
    let preview = run()
        .args(["restore-backup", &backup.id, "--dry-run"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let preview = String::from_utf8(preview).unwrap().replace('\\', "/");
    assert!(
        preview.contains("Would remove 1 file(s) created by Locust:"),
        "{preview}"
    );
    assert!(preview.contains("game/scripts/story.rpy"), "{preview}");
    assert!(
        preview.contains("game/mod.rpy: no recorded Locust creation"),
        "{preview}"
    );
    assert!(overlay.exists());
    let output = run()
        .args(["restore-backup", &backup.id])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let output = String::from_utf8(output).unwrap();
    assert!(
        output.contains("Removed 1 file(s) created by Locust:"),
        "{output}"
    );
    assert!(!overlay.exists());
    assert!(!game.join("game/scripts").exists());
    assert_eq!(fs::read(game.join("game/scripts.rpa")).unwrap(), archive);
    assert_eq!(
        fs::read_to_string(game.join("game/mod.rpy")).unwrap(),
        "user mod"
    );
    let repeated = manager.restore_with_report(&backup.id).unwrap();
    assert!(repeated.removed.is_empty());
}

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
    assert_eq!(stdout.lines().count(), 2, "{stdout}");
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
    assert_eq!(
        output.stdout,
        b"No injection backups found.\nTotal: 0 logical bytes, 0 on-disk bytes\n"
    );
    assert!(output.stderr.is_empty());
}

#[test]
fn cli_can_list_and_restore_an_injection_backup() {
    let root = tempfile::tempdir().unwrap();
    let raw_game = root.path().join("game");
    let backup_root = root.path().join("backups");
    std::fs::create_dir(&raw_game).unwrap();
    let game = raw_game.canonicalize().unwrap();
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
    eprintln!("raw game: {raw_game:?}\ncanonical game: {game:?}\nlisted: {listed:?}");
    let rows: Vec<_> = listed.lines().collect();
    assert_eq!(rows.len(), 2, "{listed}");
    let fields: Vec<_> = rows[0].split('\t').collect();
    assert_eq!(fields.len(), 4, "{listed}");
    assert!(
        fields.iter().all(|field| !field.trim().is_empty()),
        "{listed}"
    );
    assert_eq!(fields[0], backup.id, "{listed}");
    assert!(!fields[1].starts_with(r"\\?\"), "{listed}");
    // Listing paths are readable, not verbatim: compare filesystem identity.
    let listed_game = std::path::Path::new(fields[1]).canonicalize().unwrap();
    assert_eq!(listed_game, game.canonicalize().unwrap(), "{listed}");

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
    // Shown without the Windows `\\?\` verbatim prefix that canonicalize adds.
    let canonical = game.canonicalize().unwrap().display().to_string();
    let shown = canonical.strip_prefix(r"\\?\").unwrap_or(&canonical);
    let expected = format!(
        "Destination: {}\n\
         Would replace 1 file(s):\n  script.rpy\n\
         Would recreate 0 missing file(s):\n\
         Already identical: 0\n\
         Would remove 0 file(s) created by Locust:\n\
         Would remove 0 empty directory/directories created by Locust:\n\
         Kept 0 added path(s):\n",
        shown
    );
    assert_eq!(output, expected);
    assert_eq!(std::fs::read(&script).unwrap(), b"translated");

    std::fs::write(backup.path.join("payload/script.rpy"), b"corrupted").unwrap();
    run().assert().failure();
    assert_eq!(std::fs::read(&script).unwrap(), b"translated");
}

#[test]
fn backups_list_shows_paths_without_verbatim_prefix() {
    let root = tempfile::tempdir().unwrap();
    let game = root.path().join("game");
    let backup_root = root.path().join("backups");
    std::fs::create_dir(&game).unwrap();
    std::fs::write(game.join("script.rpy"), b"original").unwrap();
    BackupManager::new(backup_root.clone())
        .create_backup(&game.canonicalize().unwrap())
        .unwrap();
    let output = assert_cmd::Command::cargo_bin("locust")
        .unwrap()
        .env("LOCUST_DATA_DIR", root.path().join("profile"))
        .env("LOCUST_BACKUP_ROOT", &backup_root)
        .arg("backups")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let output = String::from_utf8(output).unwrap();
    assert!(!output.contains(r"\\?\"), "{output}");
    let canonical = game.canonicalize().unwrap().display().to_string();
    let shown = canonical.strip_prefix(r"\\?\").unwrap_or(&canonical);
    assert!(output.contains(shown), "{output}");
}

#[test]
fn cli_backups_total_counts_shared_payload_once() {
    let root = tempfile::tempdir().unwrap();
    let game = root.path().join("game");
    let backup_root = root.path().join("backups");
    std::fs::create_dir(&game).unwrap();
    std::fs::write(game.join("story.txt"), b"original").unwrap();
    let manager = BackupManager::new(backup_root.clone());
    let first = manager.create_backup(&game).unwrap();
    let second = manager.create_backup(&game).unwrap();
    let listing = || {
        let output = assert_cmd::Command::cargo_bin("locust")
            .unwrap()
            .env("LOCUST_DATA_DIR", root.path().join("profile"))
            .env("LOCUST_BACKUP_ROOT", &backup_root)
            .arg("backups")
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        String::from_utf8(output).unwrap()
    };
    let listed = listing();
    assert_eq!(listed.lines().count(), 3, "{listed}");
    assert!(listed.contains(&first.id) && listed.contains(&second.id));
    assert_eq!(
        listed.lines().last().unwrap(),
        "Total: 16 logical bytes, 8 on-disk bytes"
    );
    std::fs::write(game.join("story.txt"), b"revision").unwrap();
    manager.create_backup(&game).unwrap();
    assert_eq!(
        listing().lines().last().unwrap(),
        "Total: 24 logical bytes, 16 on-disk bytes"
    );
    manager.delete_backup(&first.id).unwrap();
    assert_eq!(
        listing().lines().last().unwrap(),
        "Total: 16 logical bytes, 16 on-disk bytes"
    );
}

#[test]
fn cli_opening_one_legacy_project_never_authorizes_deleting_an_unopened_peers_backup() {
    use locust_core::database::{Database, RecordedBackup};
    use std::fs;

    // Exercise both fallback pristine provenance and explicit-store registration
    // provenance without opening either project after its legacy rows are seeded.
    for explicit_store in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let game = temp.path().join("game");
        fs::create_dir(&game).unwrap();
        let file = game.join("story.txt");
        fs::write(&file, b"shared original").unwrap();
        let manager = BackupManager::new(temp.path().join("backups"));
        let backup = manager.create_backup(&game).unwrap();
        let reference = RecordedBackup {
            id: backup.id.clone(),
            source_path: game.clone(),
            storage_root: explicit_store.then(|| manager.root().to_owned()),
        };
        let a = temp.path().join("visited/a.db");
        let b = temp.path().join("unopened/deep/b.db");
        let column = if explicit_store {
            "registration_backup"
        } else {
            "pristine_backup"
        };
        for path in [&a, &b] {
            let db = Database::open(path).unwrap();
            db.record_injection(Some("es"), &game, std::slice::from_ref(&file))
                .unwrap();
            drop(db);
            let conn = rusqlite::Connection::open(path).unwrap();
            conn.execute(
                &format!("UPDATE injected_files SET {column} = ?1"),
                [serde_json::to_string(&reference).unwrap()],
            )
            .unwrap();
        }
        fs::remove_file(manager.root().join(".backup-references.sqlite3")).unwrap();
        let unopened_before = fs::read(&b).unwrap();
        let run = || {
            let mut cmd = assert_cmd::Command::cargo_bin("locust").unwrap();
            cmd.env("LOCUST_DATA_DIR", temp.path().join("profile"))
                .env("LOCUST_BACKUP_ROOT", manager.root());
            cmd
        };
        let assert_unverified = |output: std::process::Output| {
            assert_eq!(
                output.status.code(),
                Some(1),
                "stdout: {}\nstderr: {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(output.stdout.is_empty(), "{:?}", output.stdout);
            assert!(
                String::from_utf8_lossy(&output.stderr)
                    .contains("Locust cannot confirm that no project still needs this backup"),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert_eq!(fs::read(&b).unwrap(), unopened_before);
            assert_eq!(
                fs::read(backup.path.join("payload/story.txt")).unwrap(),
                b"shared original"
            );
        };
        assert_unverified(run().args(["delete-backup", &backup.id]).output().unwrap());

        // Public stats opens only A and lazily backfills its references.
        run().arg("stats").arg(&a).assert().success();
        run()
            .args(["delete-backup", &backup.id, "--confirm-unverified"])
            .assert()
            .code(1)
            .stdout("")
            .stderr(predicates::str::contains("referenced by 1 project(s)"));
        assert_eq!(fs::read(&b).unwrap(), unopened_before);
        assert!(backup.path.is_dir());

        // Model A releasing its provenance, leaving the index stale. B still
        // owns the same original but has never been visited by this release.
        let conn = rusqlite::Connection::open(&a).unwrap();
        conn.execute(
            "UPDATE injected_files SET pristine_backup = NULL, registration_backup = NULL",
            [],
        )
        .unwrap();
        drop(conn);
        assert_unverified(run().args(["delete-backup", &backup.id]).output().unwrap());
    }
}

#[test]
fn cli_delete_unverified_requires_confirmation_and_never_overrides_live_references() {
    use locust_core::database::{Database, RecordedBackup};
    let temp = tempfile::tempdir().unwrap();
    let game = temp.path().join("game");
    std::fs::create_dir(&game).unwrap();
    let file = game.join("story.txt");
    std::fs::write(&file, b"original").unwrap();
    let manager = BackupManager::new(temp.path().join("backups"));
    let backup = manager.create_backup(&game).unwrap();
    std::fs::remove_file(manager.root().join(".backup-references.sqlite3")).unwrap();
    let run = |confirm: bool| {
        let mut cmd = assert_cmd::Command::cargo_bin("locust").unwrap();
        cmd.env("LOCUST_DATA_DIR", temp.path().join("profile"))
            .env("LOCUST_BACKUP_ROOT", manager.root())
            .args(["delete-backup", &backup.id]);
        if confirm {
            cmd.arg("--confirm-unverified");
        }
        cmd
    };
    run(false)
        .assert()
        .failure()
        .stderr(predicates::str::contains(
            "Locust cannot confirm that no project still needs this backup",
        ));
    assert!(backup.path.exists());
    let db = Database::open(&temp.path().join("custom/project.db")).unwrap();
    db.record_injection_with_backup(
        Some("es"),
        &game,
        &[file],
        Some(&RecordedBackup {
            id: backup.id.clone(),
            source_path: game.clone(),
            storage_root: Some(manager.root().to_owned()),
        }),
    )
    .unwrap();
    run(true)
        .assert()
        .failure()
        .stderr(predicates::str::contains("referenced by 1 project(s)"));
    assert!(backup.path.exists());
    db.release_backup_reference(
        &db.get_injection(Some("es"))
            .unwrap()
            .unwrap()
            .pristine_backup
            .unwrap(),
    )
    .unwrap();
    // Make it unverified again to exercise the explicit CLI override.
    std::fs::remove_file(manager.root().join(".backup-references.sqlite3")).unwrap();
    run(true).assert().success();
    assert!(!backup.path.exists());
}
