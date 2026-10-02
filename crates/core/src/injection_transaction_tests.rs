use super::*;
use crate::{
    backup::BackupManager,
    database::Database,
    extraction::{inject_direct, FormatPlugin, FormatRegistry},
    models::StringEntry,
};
use std::sync::atomic::{AtomicBool, Ordering};

fn fixture() -> tempfile::TempDir {
    let outer = tempfile::tempdir().unwrap();
    let root = outer.path().join("game");
    fs::create_dir_all(root.join("data")).unwrap();
    fs::create_dir(root.join(".locust")).unwrap();
    fs::write(root.join(".locust/sentinel"), b"patch metadata").unwrap();
    fs::write(root.join("data/a.bin"), b"original payload").unwrap();
    fs::write(root.join("data/header.bin"), b"original header").unwrap();
    fs::write(root.join("keep.bin"), b"unrelated original").unwrap();
    outer
}

fn snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    walkdir::WalkDir::new(root)
        .into_iter()
        .filter_entry(|e| e.file_name() != STORE_DIR)
        .map(|e| e.unwrap())
        .filter(|e| e.file_type().is_file())
        .map(|e| {
            (
                e.path().strip_prefix(root).unwrap().to_owned(),
                fs::read(e.path()).unwrap(),
            )
        })
        .collect()
}

fn plugin(work: &Path, _: &Path) -> Result<InjectionReport> {
    // A plugin that holds its own non-reentrant lock must work in the copy.
    let _lock = GameLock::acquire(work)?;
    fs::write(work.join("data/a.bin"), b"translated payload")?;
    fs::write(work.join("data/header.bin"), b"translated header")?;
    fs::create_dir(work.join("new"))?;
    fs::create_dir(work.join("empty"))?;
    fs::write(work.join("new/added.bin"), b"new translation")?;
    Ok(InjectionReport {
        skip_reasons: BTreeMap::new(),
        files_modified: 99,
        strings_written: 2,
        strings_skipped: 0,
        warnings: vec![],
        files_written: vec![],
    })
}

fn interrupted(root: &Path, step: Step) {
    let result = run_with_hook(
        root,
        "fixture",
        Some("es"),
        || Ok(()),
        plugin,
        |_| Ok(()),
        &mut |current| {
            if current == step {
                Err(error("injected interruption"))
            } else {
                Ok(())
            }
        },
    );
    assert!(result.is_err());
}

#[test]
fn noop_retention_rejects_live_changes_hidden_by_an_empty_plan() {
    let outer = fixture();
    let root = outer.path().join("game");
    let manager = BackupManager::new(outer.path().join("backups"));
    let recorded = AtomicBool::new(false);
    let result = run_with_hook(
        &root,
        "fixture",
        Some("es"),
        || manager.create_backup(&root),
        |_, _| {
            Ok(InjectionReport {
                skip_reasons: Default::default(),
                files_modified: 0,
                strings_written: 0,
                strings_skipped: 0,
                warnings: vec![],
                files_written: vec![],
            })
        },
        |_| {
            recorded.store(true, Ordering::SeqCst);
            Ok(())
        },
        &mut |step| {
            if step == Step::OutputsPrepared {
                fs::write(root.join("keep.bin"), "External edit")?;
            }
            Ok(())
        },
    );
    assert!(
        result.is_err(),
        "changed live content must not become a successful no-op"
    );
    assert!(!recorded.load(Ordering::SeqCst));
    let backup = manager.list_backups().unwrap().pop().unwrap();
    assert_eq!(
        fs::read(backup.path.join("payload/keep.bin")).unwrap(),
        b"unrelated original"
    );
    assert_eq!(fs::read(root.join("keep.bin")).unwrap(), b"External edit");
}

fn empty_report() -> InjectionReport {
    InjectionReport {
        skip_reasons: Default::default(),
        files_modified: 0,
        strings_written: 0,
        strings_skipped: 0,
        warnings: vec![],
        files_written: vec![],
    }
}

#[test]
fn noop_retention_finalizer_runs_only_after_completion_under_game_lock() {
    let outer = fixture();
    let root = outer.path().join("game");
    let finalized = AtomicBool::new(false);
    run_with_backup(
        &root,
        "fixture",
        None,
        || Ok(()),
        |_, _, _| Ok(empty_report()),
        |_, _| Ok("recorded"),
        |_, _, recorded| {
            assert_eq!(*recorded, "recorded");
            assert!(GameLock::acquire(&root).is_err());
            assert!(matches!(
                load(&root.canonicalize()?)?.unwrap().phase,
                Phase::Completed
            ));
            finalized.store(true, Ordering::SeqCst);
            Ok(())
        },
    )
    .unwrap();
    assert!(finalized.load(Ordering::SeqCst));
    assert!(GameLock::acquire(&root).is_ok());
}

#[test]
fn noop_retention_keeps_backup_for_created_directory_only_plan() {
    let outer = fixture();
    let root = outer.path().join("game");
    let manager = BackupManager::new(outer.path().join("backups"));
    let finalized = AtomicBool::new(false);
    let (backup, report, _) = run_with_backup(
        &root,
        "fixture",
        None,
        || manager.create_backup(&root),
        |work, _, _| {
            fs::create_dir(work.join("new-empty"))?;
            Ok(empty_report())
        },
        |_, _| Ok(()),
        |_, _, _| {
            finalized.store(true, Ordering::SeqCst);
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(report.files_modified, 0);
    assert!(root.join("new-empty").is_dir());
    assert!(!finalized.load(Ordering::SeqCst));
    assert!(backup.path.join("manifest.json").is_file());
}

#[test]
fn noop_retention_skips_finalizer_on_record_failure_or_late_live_change() {
    for late_change in [false, true] {
        let outer = fixture();
        let root = outer.path().join("game");
        let manager = BackupManager::new(outer.path().join("backups"));
        let finalized = AtomicBool::new(false);
        let result = run_with_backup_hook(
            &root,
            "fixture",
            None,
            || manager.create_backup(&root),
            |_, _, _| Ok(empty_report()),
            (
                |_, _| {
                    if late_change {
                        Ok(())
                    } else {
                        Err(error("record failed"))
                    }
                },
                |_, _, _| {
                    finalized.store(true, Ordering::SeqCst);
                    Ok(())
                },
            ),
            &mut |step| {
                if late_change && step == Step::Recorded {
                    fs::write(root.join("keep.bin"), "Late external edit")?;
                }
                Ok(())
            },
        );
        assert!(result.is_err());
        assert!(!finalized.load(Ordering::SeqCst));
        let backup = manager.list_backups().unwrap().pop().unwrap();
        assert_eq!(
            fs::read(backup.path.join("payload/keep.bin")).unwrap(),
            b"unrelated original"
        );
    }
}

#[test]
fn reviewed_recovery_cannot_mutate_a_different_or_finished_operation() {
    let outer = fixture();
    let root = outer.path().join("game");
    let original = snapshot(&root);
    interrupted(&root, Step::FilesCommitted);
    let id = status(&root).unwrap().pending.unwrap().transaction_id;
    let before = snapshot(&root);
    let op = load(&root.canonicalize().unwrap()).unwrap().unwrap();
    let phase = fs::read(op.directory.join("phase.json")).unwrap();
    for force in [false, true] {
        let error = recover(
            &root,
            InjectionRecoveryOptions {
                force,
                expected_transaction_id: Some("stale-operation".into()),
            },
        )
        .unwrap_err();
        assert!(error.to_string().contains("operation changed"));
        assert_eq!(snapshot(&root), before);
        assert_eq!(fs::read(op.directory.join("phase.json")).unwrap(), phase);
    }
    recover(
        &root,
        InjectionRecoveryOptions {
            expected_transaction_id: Some(id.clone()),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(snapshot(&root), original);
    assert!(recover(
        &root,
        InjectionRecoveryOptions {
            expected_transaction_id: Some(id),
            ..Default::default()
        }
    )
    .is_err());
    // Existing CLI callers without a reviewed ID remain idempotent.
    assert_eq!(recover(&root, Default::default()).unwrap().restored, 0);
}

#[test]
fn complete_inventory_installs_unreported_modified_added_and_empty_directories() {
    let outer = fixture();
    let root = outer.path().join("game");
    let (_, report, _) = run(
        &root,
        "fixture",
        Some("es"),
        || Ok(()),
        plugin,
        |report| {
            assert!(GameLock::acquire(&root).is_err());
            assert_eq!(report.files_written.len(), 3);
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(report.files_modified, 3);
    assert_eq!(
        fs::read(root.join("data/a.bin")).unwrap(),
        b"translated payload"
    );
    assert_eq!(
        fs::read(root.join("new/added.bin")).unwrap(),
        b"new translation"
    );
    assert!(root.join("empty").is_dir());
    assert_eq!(
        fs::read(root.join(".locust/sentinel")).unwrap(),
        b"patch metadata"
    );
    assert!(status(&root).unwrap().pending.is_none());
    let operation = load(&root.canonicalize().unwrap()).unwrap().unwrap();
    assert!(!operation.directory.join("work").exists());
    assert!(!operation.directory.join("results").exists());
    assert_eq!(
        fs::read(operation.directory.join("originals/0")).unwrap(),
        b"original payload"
    );
}

#[test]
fn every_interrupted_phase_recovers_exactly_and_repeated_recovery_is_idempotent() {
    for step in [
        Step::Preparing,
        Step::PlanPublished,
        Step::Installed(0),
        Step::Installed(2),
        Step::FilesCommitted,
        Step::Recorded,
    ] {
        let outer = fixture();
        let root = outer.path().join("game");
        let before = snapshot(&root);
        interrupted(&root, step);
        assert!(status(&root).unwrap().pending.is_some(), "{step:?}");
        assert!(ensure_no_pending(&root).is_err());
        let backup_called = AtomicBool::new(false);
        assert!(run(
            &root,
            "fixture",
            None,
            || {
                backup_called.store(true, Ordering::SeqCst);
                Ok(())
            },
            plugin,
            |_| Ok(())
        )
        .is_err());
        assert!(!backup_called.load(Ordering::SeqCst));
        recover(&root, Default::default()).unwrap();
        assert_eq!(snapshot(&root), before, "{step:?}");
        assert!(status(&root).unwrap().pending.is_none());
        assert_eq!(recover(&root, Default::default()).unwrap().restored, 0);
    }
}

#[test]
fn corruption_and_user_edits_veto_all_game_and_marker_changes_before_restore() {
    for kind in [
        "backup",
        "edited-added",
        "edited-replaced",
        "invalid-path",
        "wrong-root",
        "duplicate",
        "invalid-size",
    ] {
        let outer = fixture();
        let root = outer.path().join("game");
        interrupted(&root, Step::FilesCommitted);
        let op = load(&root.canonicalize().unwrap()).unwrap().unwrap();
        match kind {
            "backup" => fs::write(op.directory.join("originals/1"), b"bad").unwrap(),
            "edited-added" => fs::write(root.join("new/added.bin"), b"user translation").unwrap(),
            "edited-replaced" => fs::write(root.join("data/header.bin"), b"user header").unwrap(),
            _ => {
                let mut plan: Plan = read(&op.directory.join("plan.json")).unwrap();
                match kind {
                    "invalid-path" => plan.files[0].path = "../outside".into(),
                    "wrong-root" => plan.game_root = outer.path().to_owned(),
                    "duplicate" => plan.files.push(plan.files[0].clone()),
                    _ => plan.files[1].original.as_mut().unwrap().size += 1,
                }
                publish(&root, &op.directory.join("plan.json"), &plan).unwrap();
            }
        }
        let before = snapshot(&root);
        let phase = fs::read(op.directory.join("phase.json")).unwrap();
        assert!(recover(&root, Default::default()).is_err(), "{kind}");
        assert_eq!(snapshot(&root), before, "{kind}");
        assert_eq!(
            fs::read(op.directory.join("phase.json")).unwrap(),
            phase,
            "{kind}"
        );
    }
}

#[test]
fn force_preserves_all_conflicting_bytes_before_recovery_and_keeps_unrelated_files() {
    let outer = fixture();
    let root = outer.path().join("game");
    let original = snapshot(&root);
    interrupted(&root, Step::FilesCommitted);
    fs::write(root.join("data/header.bin"), b"edited header").unwrap();
    fs::write(root.join("new/added.bin"), b"edited translation").unwrap();
    let pending = status(&root).unwrap().pending.unwrap();
    assert_eq!(pending.conflicts.len(), 2);
    let report = recover(
        &root,
        InjectionRecoveryOptions {
            force: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(report.preserved_conflicts.len(), 2);
    let bytes: BTreeSet<_> = report
        .preserved_conflicts
        .iter()
        .map(|p| fs::read(p).unwrap())
        .collect();
    assert!(bytes.contains(b"edited header".as_slice()));
    assert!(bytes.contains(b"edited translation".as_slice()));
    assert_eq!(snapshot(&root), original);
}

#[test]
fn interrupted_recovery_and_missing_replaced_targets_are_safe_to_retry() {
    let outer = fixture();
    let root = outer.path().join("game");
    let before = snapshot(&root);
    interrupted(&root, Step::FilesCommitted);
    fs::remove_file(root.join("data/a.bin")).unwrap();
    assert!(recover_with_hook(
        &root,
        Default::default(),
        &mut |step| if step == Step::Restored(0) {
            Err(error("recovery interrupted"))
        } else {
            Ok(())
        }
    )
    .is_err());
    assert_eq!(
        status(&root).unwrap().pending.unwrap().phase,
        InjectionPhase::RollingBack
    );
    recover(&root, Default::default()).unwrap();
    assert_eq!(snapshot(&root), before);
}

#[test]
fn staging_errors_deletions_and_original_races_never_install_partial_output() {
    for case in [
        "plugin-error",
        "deleted-file",
        "raced-original",
        "raced-directory",
    ] {
        let outer = fixture();
        let root = outer.path().join("game");
        let mut before = snapshot(&root);
        let result = run(
            &root,
            "fixture",
            None,
            || Ok(()),
            |work, selected| {
                let report = plugin(work, selected)?;
                match case {
                    "plugin-error" => return Err(error("plugin failed after writes")),
                    "deleted-file" => fs::remove_file(work.join("keep.bin"))?,
                    "raced-original" => {
                        fs::write(root.join("data/header.bin"), b"external update")?
                    }
                    _ => fs::create_dir(root.join("new"))?,
                }
                Ok(report)
            },
            |_| Ok(()),
        );
        assert!(result.is_err(), "{case}");
        if case == "raced-original" {
            before.insert(
                PathBuf::from("data/header.bin"),
                b"external update".to_vec(),
            );
        }
        assert_eq!(snapshot(&root), before, "{case}");
        assert!(status(&root).unwrap().pending.is_none(), "{case}");
    }
}

#[test]
fn database_recording_failure_leaves_recoverable_committed_state() {
    let outer = fixture();
    let root = outer.path().join("game");
    let before = snapshot(&root);
    let db = Database::open(&outer.path().join("project.db")).unwrap();
    let mut entry = StringEntry::new("line", "source", root.join("data/a.bin"));
    entry.translation = Some("translation".into());
    entry.status = crate::models::StringStatus::Translated;
    db.save_entries(&[entry]).unwrap();
    // Simulate termination/error after a real SQLite recording commit, before
    // the transaction's terminal marker can acknowledge it.
    let result = run(
        &root,
        "fixture",
        Some("es"),
        || Ok(()),
        plugin,
        |report| {
            db.record_injection(Some("es"), &root, &report.files_written)?;
            Err::<(), _>(error("simulated failure after SQLite commit"))
        },
    );
    assert!(result.is_err());
    assert!(db.get_injection(Some("es")).unwrap().is_some());
    assert_eq!(
        status(&root).unwrap().pending.unwrap().phase,
        InjectionPhase::CommittedUnrecorded
    );
    recover(&root, Default::default()).unwrap();
    assert_eq!(snapshot(&root), before);
    // Recovery does not mutate SQLite or trust a database path from the journal.
    assert!(db.get_injection(Some("es")).unwrap().is_some());
    let packed = crate::patch::pack_injection_recording(
        &db,
        crate::patch::PackOptions {
            game_path: root,
            lang: Some("es".into()),
            output: outer.path().join("stale.zip"),
            pristine: None,
            engine: Some("fixture".into()),
            project: outer.path().join("project.db"),
            require_pristine: false,
        },
    )
    .unwrap_err();
    assert!(
        packed.to_string().contains("changed")
            || packed.to_string().contains("modified")
            || packed.to_string().contains("missing"),
        "{packed}"
    );
}

#[test]
fn new_private_plugin_scratch_is_not_installed_and_legacy_lookalikes_survive() {
    let outer = fixture();
    let root = outer.path().join("game");
    let old = format!(".locust-stage-{}", uuid::Uuid::new_v4());
    fs::create_dir(root.join(&old)).unwrap();
    fs::write(root.join(&old).join("sentinel"), b"legacy").unwrap();
    let mut fresh = PathBuf::new();
    run(
        &root,
        "fixture",
        None,
        || Ok(()),
        |work, selected| {
            let report = plugin(work, selected)?;
            use std::io::Write;
            let mut stage = crate::patch::stream::StagingDir::create_prepared(work)?;
            stage.create_file("previous")?.write_all(b"private")?;
            fresh = PathBuf::from(stage.path().file_name().unwrap());
            stage.disarm();
            Ok(report)
        },
        |_| Ok(()),
    )
    .unwrap();
    assert_eq!(
        fs::read(root.join(&old).join("sentinel")).unwrap(),
        b"legacy"
    );
    assert!(!root.join(&fresh).exists());
}

#[test]
fn unknown_store_and_game_lock_refuse_before_backup_or_plugin_writes() {
    let outer = fixture();
    let root = outer.path().join("game");
    let lock = GameLock::acquire(&root).unwrap();
    assert!(run(
        &root,
        "fixture",
        None,
        || -> Result<()> { panic!("backup under competing lock") },
        plugin,
        |_| Ok(())
    )
    .is_err());
    drop(lock);
    fs::create_dir(root.join(STORE_DIR)).unwrap();
    fs::write(root.join(STORE_DIR).join("sentinel"), b"user directory").unwrap();
    assert!(run(
        &root,
        "fixture",
        None,
        || -> Result<()> { panic!("backup over unknown metadata") },
        plugin,
        |_| Ok(())
    )
    .is_err());
    assert_eq!(
        fs::read(root.join(STORE_DIR).join("sentinel")).unwrap(),
        b"user directory"
    );
}

struct VirtualPlugin;
impl FormatPlugin for VirtualPlugin {
    fn id(&self) -> &str {
        "virtual-fixture"
    }
    fn name(&self) -> &str {
        self.id()
    }
    fn supported_extensions(&self) -> &[&str] {
        &["bundle"]
    }
    fn extract(&self, _: &Path) -> Result<Vec<StringEntry>> {
        unreachable!()
    }
    fn inject(&self, path: &Path, entries: &[StringEntry]) -> Result<InjectionReport> {
        let physical = path.join("game.bundle");
        assert_eq!(entries[0].file_path, physical.join("CAB-node"));
        assert_eq!(entries[0].metadata["resource"], "virtual/unchanged");
        fs::write(&physical, entries[0].translation.as_ref().unwrap())?;
        Ok(InjectionReport {
            skip_reasons: BTreeMap::new(),
            files_modified: 1,
            strings_written: 1,
            strings_skipped: 0,
            warnings: vec![],
            files_written: vec![physical],
        })
    }
    fn inject_add(
        &self,
        path: &Path,
        language: &str,
        entries: &[StringEntry],
    ) -> Result<InjectionReport> {
        let _lock = GameLock::acquire(path)?;
        assert_eq!(entries[0].file_path, path.join("game.bundle/CAB-node"));
        let dir = path.join("tl").join(language);
        fs::create_dir_all(&dir)?;
        let output = dir.join("translation.txt");
        fs::write(&output, entries[0].translation.as_ref().unwrap())?;
        Ok(InjectionReport {
            skip_reasons: BTreeMap::new(),
            files_modified: 1,
            strings_written: 1,
            strings_skipped: 0,
            warnings: vec![],
            files_written: vec![output],
        })
    }
}

#[test]
fn direct_integration_remaps_virtual_suffix_and_records_real_game_paths() {
    let outer = tempfile::tempdir().unwrap();
    let root = outer.path().join("game");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("game.bundle"), "日本語").unwrap();
    let db = Database::open(&outer.path().join("project.db")).unwrap();
    let mut entry = StringEntry::new("virtual", "日本語", root.join("game.bundle/CAB-node"));
    entry.translation = Some("Translation".into());
    entry
        .metadata
        .insert("resource".into(), serde_json::json!("virtual/unchanged"));
    db.save_entries(&[entry]).unwrap();
    let mut registry = FormatRegistry::new();
    registry.register(Box::new(VirtualPlugin));
    let report = inject_direct(
        &registry,
        &db,
        &BackupManager::new(outer.path().join("backups")),
        &root,
        "virtual-fixture",
        &["es".into()],
    )
    .unwrap();
    assert_eq!(
        report.files_written,
        vec![display_path(
            &root.canonicalize().unwrap().join("game.bundle")
        )]
    );
    assert!(db.get_injection(Some("es")).unwrap().is_some());
    assert_eq!(fs::read(root.join("game.bundle")).unwrap(), b"Translation");
}

#[test]
fn selected_single_file_uses_parent_lock_and_does_not_clone_or_overwrite_siblings() {
    let outer = fixture();
    let root = outer.path().join("game");
    let selected = root.join("keep.bin");
    run(
        &selected,
        "single",
        None,
        || Ok(()),
        |work, copy| {
            assert_eq!(copy, work.join("keep.bin"));
            assert!(!work.join("data").exists());
            fs::write(copy, b"translated file")?;
            Ok(InjectionReport {
                skip_reasons: BTreeMap::new(),
                files_modified: 1,
                strings_written: 1,
                strings_skipped: 0,
                warnings: vec![],
                files_written: vec![copy.to_owned()],
            })
        },
        |_| Ok(()),
    )
    .unwrap();
    assert_eq!(
        status(&selected).unwrap().game_root,
        root.canonicalize().unwrap()
    );
    assert!(ensure_no_pending(&selected).is_ok());
    assert_eq!(
        fs::read(root.join("data/a.bin")).unwrap(),
        b"original payload"
    );
}

#[test]
fn real_process_termination_at_transaction_boundaries_recovers_from_disk_only() {
    use std::{
        process::{Child, Command, Stdio},
        time::{Duration, Instant},
    };
    struct Worker(Child);
    impl Drop for Worker {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    for phase in [
        "store",
        "preparing",
        "planned",
        "installed0",
        "installed2",
        "committed",
        "recorded",
        "recover0",
    ] {
        let outer = fixture();
        let root = outer.path().join("game");
        let before = snapshot(&root);
        let ready = outer.path().join("ready");
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "injection_transaction::tests::crash_child",
                "--ignored",
            ])
            .env("LOCUST_TX_TEST_ROOT", &root)
            .env("LOCUST_TX_TEST_PHASE", phase)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000);
        }
        let mut worker = Worker(command.spawn().unwrap());
        let deadline = Instant::now() + Duration::from_secs(15);
        while !ready.exists() {
            assert!(
                worker.0.try_wait().unwrap().is_none(),
                "child ended before failpoint: {phase}"
            );
            assert!(
                Instant::now() < deadline,
                "child never reached failpoint: {phase}"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(worker.0.try_wait().unwrap().is_none());
        worker.0.kill().unwrap();
        assert!(!worker.0.wait().unwrap().success());
        if phase == "store" {
            assert!(!root.join(STORE_DIR).exists());
            for (relative, bytes) in &before {
                assert_eq!(&fs::read(root.join(relative)).unwrap(), bytes);
            }
            assert!(status(&root).unwrap().pending.is_none());
            assert!(ensure_no_pending(&root).is_ok());
            run(&root, "fixture", None, || Ok(()), plugin, |_| Ok(())).unwrap();
            continue;
        }
        assert!(status(&root).unwrap().pending.is_some(), "{phase}");
        recover(&root, Default::default()).unwrap();
        assert_eq!(snapshot(&root), before, "{phase}");
        assert!(status(&root).unwrap().pending.is_none());
        assert_eq!(recover(&root, Default::default()).unwrap().restored, 0);
        if phase == "recorded" {
            let db = Database::open(&outer.path().join("recording.db")).unwrap();
            assert!(db.get_injection(Some("es")).unwrap().is_some());
        }
    }
}

#[test]
#[ignore = "hidden subprocess entrypoint; parent kills it without running Rust destructors"]
fn crash_child() {
    let root = PathBuf::from(std::env::var_os("LOCUST_TX_TEST_ROOT").unwrap());
    let phase = std::env::var("LOCUST_TX_TEST_PHASE").unwrap();
    let pause = || -> Result<()> {
        fs::write(root.parent().unwrap().join("ready"), b"ready")?;
        loop {
            std::thread::park();
        }
    };
    if phase == "recover0" {
        interrupted(&root, Step::FilesCommitted);
        recover_with_hook(&root, Default::default(), &mut |step| {
            if step == Step::Restored(0) {
                pause()
            } else {
                Ok(())
            }
        })
        .unwrap();
    } else {
        let expected = match phase.as_str() {
            "store" => Step::StorePrepared,
            "preparing" => Step::Preparing,
            "planned" => Step::PlanPublished,
            "installed0" => Step::Installed(0),
            "installed2" => Step::Installed(2),
            "committed" => Step::FilesCommitted,
            "recorded" => Step::Recorded,
            _ => panic!("invalid child phase"),
        };
        let db = Database::open(&root.parent().unwrap().join("recording.db")).unwrap();
        run_with_hook(
            &root,
            "fixture",
            Some("es"),
            || Ok(()),
            plugin,
            |report| db.record_injection(Some("es"), &root, &report.files_written),
            &mut |step| if step == expected { pause() } else { Ok(()) },
        )
        .unwrap();
    }
}

#[test]
fn store_publication_failure_does_not_create_an_unrecoverable_namespace() {
    let outer = fixture();
    let root = outer.path().join("game");
    let before = snapshot(&root);
    assert!(run_with_hook(
        &root,
        "fixture",
        None,
        || Ok(()),
        plugin,
        |_| Ok(()),
        &mut |step| if step == Step::StorePrepared {
            Err(error("before store publication"))
        } else {
            Ok(())
        }
    )
    .is_err());
    assert!(!root.join(STORE_DIR).exists());
    assert_eq!(snapshot(&root), before);
    assert!(status(&root).unwrap().pending.is_none());
    run(&root, "fixture", None, || Ok(()), plugin, |_| Ok(())).unwrap();
}

#[test]
fn unknown_new_staging_data_is_diagnosed_and_preserved_before_game_mutation() {
    let outer = fixture();
    let root = outer.path().join("game");
    let before = snapshot(&root);
    let fresh = format!(".locust-stage-{}", uuid::Uuid::new_v4());
    let result = run(
        &root,
        "fixture",
        None,
        || Ok(()),
        |work, selected| {
            let report = plugin(work, selected)?;
            fs::create_dir(work.join(&fresh))?;
            fs::write(
                work.join(&fresh).join("unexpected"),
                b"do not silently omit",
            )?;
            Ok(report)
        },
        |_| Ok(()),
    );
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("untracked staging"));
    assert_eq!(snapshot(&root), before);
    let operation = load(&root.canonicalize().unwrap()).unwrap().unwrap();
    assert_eq!(
        fs::read(
            operation
                .directory
                .join("work")
                .join(fresh)
                .join("unexpected")
        )
        .unwrap(),
        b"do not silently omit"
    );
}

#[test]
fn staging_tracking_has_nested_error_unwind_and_identity_contracts() {
    use crate::patch::stream::{track_prepared_staging, StagingDir};
    let outer = tempfile::tempdir().unwrap();
    let (child, all) = track_prepared_staging(|| {
        let (paths, child) = track_prepared_staging(|| {
            let mut stage = StagingDir::create_prepared(outer.path()).unwrap();
            let mut file = stage.create_file("previous").unwrap();
            use std::io::Write;
            file.write_all(b"owned").unwrap();
            let paths = (stage.path().to_owned(), stage.child("previous"));
            stage.disarm();
            paths
        });
        assert!(child.owns_directory(&paths.0));
        assert!(child.owns_file(&paths.1));
        (paths, child)
    });
    let ((directory, file), child) = child;
    assert!(all.owns_directory(&directory));
    assert!(all.owns_file(&file));
    fs::rename(&file, directory.join("saved")).unwrap();
    fs::write(&file, b"owned").unwrap();
    assert!(!child.owns_file(&file));
    assert!(!all.owns_file(&file));
    let (_, error_scope) = track_prepared_staging(|| Err::<(), _>("ordinary error"));
    assert!(!error_scope.owns_file(&file));
    let panic = std::panic::catch_unwind(|| track_prepared_staging(|| panic!("tracked unwind")));
    assert!(panic.is_err());
    let (live_path, clean_scope) = track_prepared_staging(|| {
        let stage = StagingDir::create_prepared(outer.path()).unwrap();
        let path = stage.path().to_owned();
        stage.create_file("temporary").unwrap();
        path
    });
    assert!(
        !live_path.exists(),
        "tracking handles must not prevent normal guard cleanup"
    );
    assert!(!clean_scope.owns_file(&file));
}

#[test]
fn invalid_translation_controls_reject_direct_before_backup_or_dispatch() {
    let outer = tempfile::tempdir().unwrap();
    let root = outer.path().join("game");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("game.bundle"), b"original").unwrap();
    let db = Database::open_in_memory().unwrap();
    let mut entry = StringEntry::new(
        "invalid-control",
        "Hello {name}",
        root.join("game.bundle/CAB-node"),
    );
    entry.translation = Some("Hola".into());
    db.save_entries(&[entry]).unwrap();
    let mut registry = FormatRegistry::new();
    registry.register(Box::new(VirtualPlugin));
    let backup = outer.path().join("backups");
    let error = inject_direct(
        &registry,
        &db,
        &BackupManager::new(backup.clone()),
        &root,
        "virtual-fixture",
        &["es".into()],
    )
    .unwrap_err();
    assert!(
        error.to_string().contains("control") || error.to_string().contains("placeholder"),
        "{error}"
    );
    assert!(!backup.exists());
    assert!(!root.join(STORE_DIR).exists());
    assert_eq!(fs::read(root.join("game.bundle")).unwrap(), b"original");
}

#[tokio::test]
async fn add_integration_records_inside_transaction_and_sqlite_rejection_is_recoverable() {
    for reject_recording in [false, true] {
        let outer = tempfile::tempdir().unwrap();
        let root = outer.path().join("game");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("game.bundle"), b"original bundle").unwrap();
        let db_path = outer.path().join("project.db");
        let db = std::sync::Arc::new(Database::open(&db_path).unwrap());
        let mut entry =
            StringEntry::new("add-entry", "original", root.join("game.bundle/CAB-node"));
        entry.translation = Some("traducción".into());
        db.save_entries(&[entry]).unwrap();
        if reject_recording {
            rusqlite::Connection::open(&db_path).unwrap().execute_batch("CREATE TRIGGER reject_record BEFORE INSERT ON injected_files BEGIN SELECT RAISE(ABORT, 'test recording rejection'); END;").unwrap();
        }
        let mut registry = FormatRegistry::new();
        registry.register(Box::new(VirtualPlugin));
        let injector = crate::extraction::MultiLangInjector::new(
            std::sync::Arc::new(registry),
            db.clone(),
            std::sync::Arc::new(BackupManager::new(outer.path().join("backups"))),
        );
        let (sender, _receiver) = tokio::sync::mpsc::channel(20);
        let report = injector
            .inject(
                &root,
                "virtual-fixture",
                crate::models::OutputMode::Add,
                vec!["es".into()],
                None,
                sender,
            )
            .await
            .unwrap();
        assert_eq!(
            fs::read(root.join("game.bundle")).unwrap(),
            b"original bundle"
        );
        if reject_recording {
            assert_eq!(report.languages_failed.len(), 1);
            assert!(report.languages_processed.is_empty());
            assert!(db.get_injection(Some("es")).unwrap().is_none());
            assert_eq!(
                status(&root).unwrap().pending.unwrap().phase,
                InjectionPhase::CommittedUnrecorded
            );
            recover(&root, Default::default()).unwrap();
            assert!(!root.join("tl/es/translation.txt").exists());
        } else {
            assert_eq!(report.languages_processed, vec!["es"]);
            assert!(
                db.get_injection(Some("es")).unwrap().is_some(),
                "Add must record before returning, without relying on an external companion call"
            );
            assert_eq!(
                fs::read_to_string(root.join("tl/es/translation.txt")).unwrap(),
                "traducción"
            );
            assert!(status(&root).unwrap().pending.is_none());
            let recorded_before = db.get_injection(Some("es")).unwrap().unwrap();
            fs::write(
                root.join("tl/es/translation.txt"),
                b"external edit after transaction",
            )
            .unwrap();
            let outcomes = crate::extraction::record_multilang_injection(
                &db,
                &report,
                &["es".into()],
                &|_| "unused".into(),
            )
            .unwrap();
            assert_eq!(
                outcomes,
                vec![(
                    "es".into(),
                    crate::extraction::RecordOutcome::Recorded { files: 1 }
                )]
            );
            let recorded_after = db.get_injection(Some("es")).unwrap().unwrap();
            assert_eq!(
                serde_json::to_value(recorded_after).unwrap(),
                serde_json::to_value(recorded_before).unwrap(),
                "the Add companion must never rehash or rewrite a completed recording"
            );
        }
    }
}

#[test]
fn plugin_cannot_create_patch_metadata_or_overwrite_its_original_receipt() {
    let outer = fixture();
    let root = outer.path().join("game");
    fs::write(
        root.join(".locust/receipt.json"),
        serde_json::to_vec(&crate::patch::manifest::Receipt {
            schema_version: 1,
            patch_id: "installed".into(),
            patch_version: "1.0".into(),
            generator_version: "test".into(),
            language: "es".into(),
            engine: "fixture".into(),
            applied_at: "test".into(),
            verification: crate::patch::manifest::VerificationTier::Strict,
            forced: false,
            baseline: crate::patch::manifest::BackupBaseline::Pristine,
            created_dirs: vec![],
            replaced: vec![],
            added: vec![],
        })
        .unwrap(),
    )
    .unwrap();
    let before = snapshot(&root);
    let result = run(
        &root,
        "fixture",
        None,
        || Ok(()),
        |work, selected| {
            let report = plugin(work, selected)?;
            fs::create_dir(work.join(".locust"))?;
            fs::write(work.join(".locust/receipt.json"), b"forged")?;
            Ok(report)
        },
        |_| Ok(()),
    );
    assert!(result.is_err());
    assert_eq!(snapshot(&root), before);
}

#[test]
fn linked_recovery_destination_and_untracked_worker_thread_staging_fail_safely() {
    let outer = fixture();
    let root = outer.path().join("game");
    let before = snapshot(&root);
    let result = run(
        &root,
        "fixture",
        None,
        || Ok(()),
        |work, selected| {
            let report = plugin(work, selected)?;
            let path = work.to_owned();
            std::thread::spawn(move || {
                use std::io::Write;
                let mut stage = crate::patch::stream::StagingDir::create_prepared(&path).unwrap();
                stage
                    .create_file("previous")
                    .unwrap()
                    .write_all(b"untracked worker")
                    .unwrap();
                stage.disarm();
            })
            .join()
            .unwrap();
            Ok(report)
        },
        |_| Ok(()),
    );
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("untracked staging"));
    assert_eq!(snapshot(&root), before);
    interrupted(&root, Step::FilesCommitted);
    let foreign = outer.path().join("foreign");
    fs::write(&foreign, b"foreign").unwrap();
    let changed = root.join("data/header.bin");
    fs::remove_file(&changed).unwrap();
    #[cfg(windows)]
    let linked = std::os::windows::fs::symlink_file(&foreign, &changed);
    #[cfg(unix)]
    let linked = std::os::unix::fs::symlink(&foreign, &changed);
    #[cfg(not(any(windows, unix)))]
    let linked: std::io::Result<()> = Err(std::io::ErrorKind::PermissionDenied.into());
    if let Err(error) = linked {
        assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
        return;
    }
    assert!(recover(
        &root,
        InjectionRecoveryOptions {
            force: true,
            ..Default::default()
        }
    )
    .is_err());
    assert_eq!(fs::read(foreign).unwrap(), b"foreign");
    assert_eq!(
        fs::read(root.join("data/a.bin")).unwrap(),
        b"translated payload"
    );
}

#[test]
fn recovery_race_after_snapshot_never_adopts_unrecognized_bytes() {
    for force in [false, true] {
        let outer = fixture();
        let root = outer.path().join("game");
        interrupted(&root, Step::FilesCommitted);
        let operation = load(&root.canonicalize().unwrap()).unwrap().unwrap();
        let phase_before = fs::read(operation.directory.join("phase.json")).unwrap();
        let error = recover_with_hook(
            &root,
            InjectionRecoveryOptions {
                force,
                ..Default::default()
            },
            &mut |step| {
                if step == Step::RecoverySnapshot {
                    fs::write(root.join("new/added.bin"), b"late external edit")?;
                }
                Ok(())
            },
        )
        .unwrap_err();
        assert!(error
            .to_string()
            .contains("changed during recovery preflight"));
        assert_eq!(
            fs::read(root.join("new/added.bin")).unwrap(),
            b"late external edit"
        );
        assert_eq!(
            fs::read(root.join("data/a.bin")).unwrap(),
            b"translated payload"
        );
        assert_eq!(
            fs::read(operation.directory.join("phase.json")).unwrap(),
            phase_before
        );
    }
}

#[test]
fn later_readonly_target_vetoes_install_and_restore_before_earlier_files() {
    let outer = fixture();
    let root = outer.path().join("game");
    let later = root.join("data/header.bin");
    let original_permissions = fs::metadata(&later).unwrap().permissions();
    let mut readonly = original_permissions.clone();
    readonly.set_readonly(true);
    assert!(run_with_hook(
        &root,
        "fixture",
        None,
        || Ok(()),
        plugin,
        |_| Ok(()),
        &mut |step| {
            if step == Step::OutputsPrepared {
                fs::set_permissions(&later, readonly.clone())?;
            }
            Ok(())
        }
    )
    .is_err());
    assert_eq!(
        fs::read(root.join("data/a.bin")).unwrap(),
        b"original payload"
    );
    fs::set_permissions(&later, original_permissions.clone()).unwrap();
    interrupted(&root, Step::FilesCommitted);
    fs::set_permissions(&later, readonly).unwrap();
    let operation = load(&root.canonicalize().unwrap()).unwrap().unwrap();
    let phase = fs::read(operation.directory.join("phase.json")).unwrap();
    assert!(recover(
        &root,
        InjectionRecoveryOptions {
            force: true,
            ..Default::default()
        }
    )
    .is_err());
    assert_eq!(
        fs::read(root.join("data/a.bin")).unwrap(),
        b"translated payload"
    );
    assert_eq!(
        fs::read(operation.directory.join("phase.json")).unwrap(),
        phase
    );
    fs::set_permissions(&later, original_permissions).unwrap();
    recover(&root, Default::default()).unwrap();
}

#[test]
fn display_path_strips_only_verbatim_prefixes() {
    #[cfg(windows)]
    {
        assert_eq!(display_path(Path::new(r"\\?\C:\x")), PathBuf::from(r"C:\x"));
        assert_eq!(
            display_path(Path::new(r"\\?\UNC\server\share\x")),
            PathBuf::from(r"\\server\share\x")
        );
        assert_eq!(display_path(Path::new(r"C:\x")), PathBuf::from(r"C:\x"));
    }
    assert_eq!(display_path(Path::new("rel/a")), PathBuf::from("rel/a"));
}

#[cfg(windows)]
#[test]
fn injection_report_omits_verbatim_windows_prefixes() {
    let outer = fixture();
    let root = outer.path().join("game");
    let (_, report, _) = run(&root, "fixture", Some("es"), || Ok(()), plugin, |_| Ok(())).unwrap();
    assert!(
        !report.files_written.is_empty(),
        "fixture plugin must report written files"
    );
    for path in &report.files_written {
        let text = path.to_string_lossy();
        assert!(
            !text.starts_with(r"\\?\"),
            "files_written must not be a verbatim path: {text}"
        );
    }
    for warning in &report.warnings {
        assert!(
            !warning.starts_with(r"\\?\"),
            "warning must not be a verbatim path: {warning}"
        );
        // The completion notice puts the originals directory after "retained at ",
        // so the warning string itself does not start with the prefix.
        assert!(
            !warning.contains(r"\\?\"),
            "warning must not embed a verbatim path: {warning}"
        );
    }
}

#[cfg(unix)]
#[test]
fn executable_permission_bits_survive_install_and_recovery() {
    use std::os::unix::fs::PermissionsExt;
    let outer = fixture();
    let root = outer.path().join("game");
    let target = root.join("data/a.bin");
    fs::set_permissions(&target, fs::Permissions::from_mode(0o755)).unwrap();
    interrupted(&root, Step::FilesCommitted);
    assert_eq!(
        fs::metadata(&target).unwrap().permissions().mode() & 0o7777,
        0o755
    );
    recover(&root, Default::default()).unwrap();
    assert_eq!(
        fs::metadata(&target).unwrap().permissions().mode() & 0o7777,
        0o755
    );
}
