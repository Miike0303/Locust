use super::*;

fn direct_archive_revisions(root: &Path, archive: &Path, format: &str) {
    use locust_core::{backup::BackupManager, database::Database, extraction::inject_direct};
    let original = fs::read(archive).unwrap();
    let registry = crate::default_registry();
    let plugin = registry.get(format).unwrap();
    let db = Database::open_in_memory().unwrap();
    let backups = tempfile::tempdir().unwrap();
    let manager = BackupManager::new(backups.path().to_owned());
    let mut pristine = None;
    for prefix in ["TL ", "R2 "] {
        // Tyrano's archive writer source-checks the current payload. Refresh
        // locators/guards from that payload before testing another generation.
        let mut entries = plugin.extract(root).unwrap();
        assert!(!entries.is_empty());
        for entry in &mut entries {
            entry.translation = Some(format!("{prefix}{}", entry.source));
        }
        db.save_entries(&entries).unwrap();
        let report = inject_direct(&registry, &db, &manager, root, format, &["en".into()])
            .expect("archive Direct injection must recognize its retained staging backup");
        assert_eq!(report.strings_written, entries.len(), "{report:?}");
        assert_eq!(report.strings_skipped, 0, "{report:?}");
        assert_eq!(report.files_modified, 1);
        assert!(report.reports["en"].skip_reasons.is_empty());
        let extracted = plugin.extract(root).unwrap();
        assert_eq!(extracted.len(), entries.len());
        for entry in &entries {
            assert!(extracted
                .iter()
                .any(|out| Some(&out.source) == entry.translation.as_ref()));
        }
        assert!(
            walkdir::WalkDir::new(root).into_iter().all(|entry| {
                !entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".locust-stage-")
            }),
            "private staging must not survive a successful Direct injection"
        );
        let id = report.pristine_backup_id.unwrap();
        if let Some(first) = &pristine {
            assert_eq!(&id, first);
        } else {
            pristine = Some(id);
        }
    }
    manager.restore(&pristine.unwrap()).unwrap();
    assert_eq!(fs::read(archive).unwrap(), original);
}

#[test]
fn c121_asar_direct_revisions_exclude_private_backups() {
    let root = tempfile::tempdir().unwrap();
    let archive = root.path().join("resources").join("app.asar");
    fs::create_dir(archive.parent().unwrap()).unwrap();
    fs::write(
        &archive,
        crate::tyrano_asar::write_asar(&[(
            "data/scenario/main.ks".into(),
            b"Hello original.[p]\n".to_vec(),
        )])
        .unwrap(),
    )
    .unwrap();
    direct_archive_revisions(root.path(), &archive, "tyrano");
}

#[test]
fn c121_nw_direct_revisions_exclude_private_backups() {
    let root = tempfile::tempdir().unwrap();
    let archive = root.path().join("package.nw");
    let mut writer = zip::ZipWriter::new(File::create(&archive).unwrap());
    writer
        .start_file(
            "data/scenario/main.ks",
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
    writer.write_all(b"Hello original.[p]\n").unwrap();
    writer.finish().unwrap();
    direct_archive_revisions(root.path(), &archive, "tyrano");
}

#[test]
fn partial_write_failure_preserves_exact_original_and_recursive_sentinels() {
    let root = tempfile::tempdir().unwrap();
    let lock = GameLock::acquire(root.path()).unwrap();
    let file = root.path().join("app.asar");
    fs::write(&file, b"original bytes").unwrap();
    fs::write(root.path().join("app.asar.locust-old"), b"unrelated").unwrap();
    let unowned = root.path().join(".locust-stage-other");
    fs::create_dir(&unowned).unwrap();
    fs::write(unowned.join("sentinel"), b"recursive").unwrap();
    assert!(Prepared::create(&lock, &file, |f| {
        f.write_all(b"partial")?;
        Err(std::io::Error::other("late write failure").into())
    })
    .is_err());
    assert_eq!(fs::read(file).unwrap(), b"original bytes");
    assert_eq!(
        fs::read(root.path().join("app.asar.locust-old")).unwrap(),
        b"unrelated"
    );
    assert_eq!(fs::read(unowned.join("sentinel")).unwrap(), b"recursive");
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 3);
}

fn prepare(lock: &GameLock, file: &Path, bytes: &[u8]) -> Prepared {
    Prepared::create(lock, file, |f| {
        f.write_all(bytes)?;
        Ok(())
    })
    .unwrap()
}

#[test]
fn later_install_failure_restores_all_earlier_files_exactly() {
    let root = tempfile::tempdir().unwrap();
    let lock = GameLock::acquire(root.path()).unwrap();
    let payload = root.path().join("unpacked.ks");
    let archive = root.path().join("app.asar");
    fs::write(&payload, b"old payload").unwrap();
    fs::write(&archive, b"old header").unwrap();
    let ready = vec![
        prepare(&lock, &payload, b"long new payload"),
        prepare(&lock, &archive, b"new header"),
    ];
    assert!(commit(&lock, ready, |i, _| if i == 1 {
        Err(std::io::Error::other("second install failed").into())
    } else {
        Ok(())
    })
    .is_err());
    assert_eq!(fs::read(payload).unwrap(), b"old payload");
    assert_eq!(fs::read(archive).unwrap(), b"old header");
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 2);
}

#[test]
fn later_preparation_failure_never_mutates_first_destination() {
    let root = tempfile::tempdir().unwrap();
    let lock = GameLock::acquire(root.path()).unwrap();
    let first = root.path().join("first");
    let invalid = root.path().join("directory");
    fs::write(&first, b"original").unwrap();
    fs::create_dir(&invalid).unwrap();
    fs::write(invalid.join("sentinel"), b"recursive").unwrap();
    assert!(replace_files(
        &lock,
        &[
            (first.clone(), b"replacement".to_vec()),
            (invalid.clone(), b"bad".to_vec())
        ]
    )
    .is_err());
    assert_eq!(fs::read(first).unwrap(), b"original");
    assert_eq!(fs::read(invalid.join("sentinel")).unwrap(), b"recursive");
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 2);
}

#[test]
fn repeated_generations_preserve_distinct_extensionless_backups() {
    let root = tempfile::tempdir().unwrap();
    let lock = GameLock::acquire(root.path()).unwrap();
    let file = root.path().join("package.nw");
    fs::write(&file, b"zero").unwrap();
    let first = replace_files(&lock, &[(file.clone(), b"one".to_vec())]).unwrap();
    let second = replace_files(&lock, &[(file.clone(), b"two".to_vec())]).unwrap();
    assert_ne!(first, second);
    assert_eq!(fs::read(&first[0]).unwrap(), b"zero");
    assert_eq!(fs::read(&second[0]).unwrap(), b"one");
    assert!(first[0].extension().is_none());
    assert_eq!(fs::read(file).unwrap(), b"two");
}

#[test]
fn duplicate_target_rejection_does_not_write() {
    let root = tempfile::tempdir().unwrap();
    let lock = GameLock::acquire(root.path()).unwrap();
    let file = root.path().join("app.asar");
    fs::write(&file, b"original").unwrap();
    assert!(replace_files(
        &lock,
        &[
            (file.clone(), b"one".to_vec()),
            (file.clone(), b"two".to_vec())
        ]
    )
    .is_err());
    assert_eq!(fs::read(file).unwrap(), b"original");
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
}

#[test]
fn failed_real_install_restores_current_and_earlier_destinations() {
    let root = tempfile::tempdir().unwrap();
    let lock = GameLock::acquire(root.path()).unwrap();
    let first = root.path().join("unpacked.ks");
    let second = root.path().join("app.asar");
    fs::write(&first, b"old script").unwrap();
    fs::write(&second, b"old header").unwrap();
    let prepared = vec![
        prepare(&lock, &first, b"new script"),
        prepare(&lock, &second, b"new header"),
    ];
    let missing = prepared[1].stage.child("replacement");
    assert!(commit(&lock, prepared, |i, _| {
        if i == 1 {
            fs::remove_file(&missing)?;
        }
        Ok(())
    })
    .is_err());
    assert_eq!(fs::read(first).unwrap(), b"old script");
    assert_eq!(fs::read(second).unwrap(), b"old header");
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 2);
}

#[test]
fn failed_restoration_reports_and_retains_exact_original_without_deleting_foreign_directory() {
    let root = tempfile::tempdir().unwrap();
    let lock = GameLock::acquire(root.path()).unwrap();
    let first = root.path().join("unpacked.ks");
    let second = root.path().join("app.asar");
    fs::write(&first, b"recoverable original").unwrap();
    fs::write(&second, b"original header").unwrap();
    let prepared = vec![
        prepare(&lock, &first, b"replacement"),
        prepare(&lock, &second, b"new header"),
    ];
    let recovery = prepared[0].stage.child("previous");
    let error = commit(&lock, prepared, |i, _| {
        if i == 1 {
            // Simulate an uncooperative writer replacing our installed output.
            fs::remove_file(&first)?;
            fs::create_dir(&first)?;
            fs::write(first.join("sentinel"), b"foreign directory")?;
            return Err(std::io::Error::other("later install failed").into());
        }
        Ok(())
    })
    .unwrap_err()
    .to_string();
    assert!(error.contains("rollback incomplete"));
    assert!(error.contains(&recovery.display().to_string()));
    assert_eq!(fs::read(recovery).unwrap(), b"recoverable original");
    assert_eq!(
        fs::read(first.join("sentinel")).unwrap(),
        b"foreign directory"
    );
    assert_eq!(fs::read(second).unwrap(), b"original header");
}

#[test]
fn both_plugins_share_core_game_lock_across_processes() {
    use locust_core::extraction::FormatPlugin;
    let root = tempfile::tempdir().unwrap();
    let lock = GameLock::acquire(root.path()).unwrap();
    assert!(crate::tyrano::TyranoPlugin::new()
        .inject(root.path(), &[])
        .unwrap_err()
        .to_string()
        .contains("game busy"));
    assert!(crate::yuris::YurisPlugin::new()
        .inject(root.path(), &[])
        .unwrap_err()
        .to_string()
        .contains("game busy"));
    let child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "archive_replace::tests::lock_child",
            "--ignored",
            "--nocapture",
        ])
        .env("LOCUST_ARCHIVE_LOCK_TEST_ROOT", root.path())
        .output()
        .unwrap();
    assert!(
        child.status.success(),
        "{} {}",
        String::from_utf8_lossy(&child.stdout),
        String::from_utf8_lossy(&child.stderr)
    );
    drop(lock);
    assert_eq!(
        crate::tyrano::TyranoPlugin::new()
            .inject(root.path(), &[])
            .unwrap()
            .files_modified,
        0
    );
    assert_eq!(
        crate::yuris::YurisPlugin::new()
            .inject(root.path(), &[])
            .unwrap()
            .files_modified,
        0
    );
}

#[test]
#[ignore = "subprocess entrypoint executed by both_plugins_share_core_game_lock_across_processes"]
fn lock_child() {
    use locust_core::extraction::FormatPlugin;
    let root = PathBuf::from(std::env::var_os("LOCUST_ARCHIVE_LOCK_TEST_ROOT").unwrap());
    assert!(crate::tyrano::TyranoPlugin::new()
        .inject(&root, &[])
        .unwrap_err()
        .to_string()
        .contains("game busy"));
    assert!(crate::yuris::YurisPlugin::new()
        .inject(&root, &[])
        .unwrap_err()
        .to_string()
        .contains("game busy"));
}

#[cfg(any(unix, windows))]
#[test]
fn linked_destination_rejected_and_unrelated_target_preserved() {
    let root = tempfile::tempdir().unwrap();
    let lock = GameLock::acquire(root.path()).unwrap();
    let target = root.path().join("user");
    let file = root.path().join("game.ypf");
    fs::write(&target, b"user bytes").unwrap();
    #[cfg(windows)]
    let result = std::os::windows::fs::symlink_file(&target, &file);
    #[cfg(unix)]
    let result = std::os::unix::fs::symlink(&target, &file);
    if let Err(error) = result {
        assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
        eprintln!("symlink creation unavailable on this host: {error}");
        return;
    }
    assert!(replace_files(&lock, &[(file.clone(), b"new".to_vec())]).is_err());
    assert!(fs::symlink_metadata(file).unwrap().file_type().is_symlink());
    assert_eq!(fs::read(target).unwrap(), b"user bytes");
}
