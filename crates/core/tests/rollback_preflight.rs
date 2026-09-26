use locust_core::{
    database::sha256_hex,
    patch::{manifest::*, rollback, PatchStore, RollbackOptions},
};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

fn setup(interrupted: bool) -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir_all(root.path().join(".locust/backup/files/data")).unwrap();
    fs::create_dir(root.path().join("data")).unwrap();
    fs::write(root.path().join("data/a.bin"), b"patched first").unwrap();
    fs::write(root.path().join("data/z.bin"), b"patched second").unwrap();
    fs::write(root.path().join("data/added.bin"), b"torn or edited added").unwrap();
    let mut files = Vec::new();
    for (path, bytes) in [
        ("data/a.bin", b"original first".as_slice()),
        ("data/z.bin", b"original second".as_slice()),
    ] {
        fs::write(root.path().join(".locust/backup/files").join(path), bytes).unwrap();
        files.push(BackupFileEntry {
            path: path.into(),
            sha256: sha256_hex(bytes),
            size: bytes.len() as u64,
        });
    }
    let manifest = BackupManifest {
        schema_version: 1,
        created_at: "fixture".into(),
        baseline: BackupBaseline::Pristine,
        files,
    };
    fs::write(
        root.path().join(".locust/backup/manifest.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
    if interrupted {
        let journal = Journal {
            schema_version: 1,
            state: JournalState::Applying,
            patch_id: "neutral".into(),
            plan: ApplyPlan {
                patch_version: "1.0".into(),
                language: "es".into(),
                engine: "unreal".into(),
                generator_version: "test".into(),
                verification: VerificationTier::Strict,
                forced: false,
                baseline: BackupBaseline::Pristine,
                replaced: vec![],
                added: vec![ReceiptAdded {
                    path: "data/added.bin".into(),
                    patched_sha256: sha256_hex(b"complete added"),
                }],
                created_dirs: vec![],
            },
        };
        fs::write(
            root.path().join(".locust/journal.json"),
            serde_json::to_vec(&journal).unwrap(),
        )
        .unwrap();
    }
    root
}

fn snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    walkdir::WalkDir::new(root)
        .into_iter()
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

#[test]
fn interrupted_corrupt_later_backup_rejects_before_added_delete_restore_or_journal_write() {
    let root = setup(true);
    fs::write(
        root.path().join(".locust/backup/files/data/z.bin"),
        b"corrupt second",
    )
    .unwrap();
    let before = snapshot(root.path());
    assert!(rollback(root.path(), RollbackOptions::default()).is_err());
    assert_eq!(snapshot(root.path()), before);
}

#[test]
fn interrupted_missing_later_backup_rejects_before_any_mutation() {
    let root = setup(true);
    fs::remove_file(root.path().join(".locust/backup/files/data/z.bin")).unwrap();
    let before = snapshot(root.path());
    assert!(rollback(
        root.path(),
        RollbackOptions {
            delete_modified_added: true
        }
    )
    .is_err());
    assert_eq!(snapshot(root.path()), before);
}

#[test]
fn unknown_corrupt_later_backup_rejects_before_first_restore() {
    let root = setup(false);
    fs::write(
        root.path().join(".locust/backup/files/data/z.bin"),
        b"corrupt second",
    )
    .unwrap();
    let before = snapshot(root.path());
    assert!(rollback(
        root.path(),
        RollbackOptions {
            delete_modified_added: true
        }
    )
    .is_err());
    assert_eq!(snapshot(root.path()), before);
}

#[test]
fn unknown_missing_later_backup_rejects_before_first_restore() {
    let root = setup(false);
    fs::remove_file(root.path().join(".locust/backup/files/data/z.bin")).unwrap();
    let before = snapshot(root.path());
    assert!(rollback(
        root.path(),
        RollbackOptions {
            delete_modified_added: true
        }
    )
    .is_err());
    assert_eq!(snapshot(root.path()), before);
}

#[test]
fn unknown_without_force_keeps_everything_and_valid_force_restores_replaced_only() {
    let root = setup(false);
    let before = snapshot(root.path());
    assert!(rollback(root.path(), RollbackOptions::default()).is_err());
    assert_eq!(snapshot(root.path()), before);
    let report = rollback(
        root.path(),
        RollbackOptions {
            delete_modified_added: true,
        },
    )
    .unwrap();
    assert_eq!(report.restored, 2);
    assert_eq!(report.deleted, 0);
    assert_eq!(
        fs::read(root.path().join("data/added.bin")).unwrap(),
        b"torn or edited added"
    );
    assert_eq!(
        fs::read(root.path().join("data/a.bin")).unwrap(),
        b"original first"
    );
    assert!(!PatchStore::new(root.path()).locust_dir().exists());
}

fn patched(root: &Path) {
    let receipt = Receipt {
        schema_version: 1,
        patch_id: "neutral".into(),
        patch_version: "1.0".into(),
        generator_version: "test".into(),
        language: "es".into(),
        engine: "unreal".into(),
        applied_at: "fixture".into(),
        verification: VerificationTier::Strict,
        forced: false,
        baseline: BackupBaseline::Pristine,
        created_dirs: vec![],
        replaced: vec![],
        added: vec![ReceiptAdded {
            path: "data/added.bin".into(),
            patched_sha256: sha256_hex(b"complete added"),
        }],
    };
    fs::write(
        root.join(".locust/receipt.json"),
        serde_json::to_vec(&receipt).unwrap(),
    )
    .unwrap();
}

#[test]
fn hash_size_and_destination_validation_precedes_mutation_in_all_states() {
    for state in ["interrupted", "unknown", "patched"] {
        for invalid in ["hash", "size", "directory"] {
            let root = setup(state == "interrupted");
            if state == "patched" {
                patched(root.path());
            }
            let store = PatchStore::new(root.path());
            match invalid {
                "hash" => fs::write(
                    root.path().join(".locust/backup/files/data/z.bin"),
                    b"same size wrong",
                )
                .unwrap(),
                "size" => {
                    let mut bm = store.read_backup_manifest().unwrap().unwrap();
                    bm.files[1].size += 1;
                    fs::write(
                        store.backup_manifest_path(),
                        serde_json::to_vec(&bm).unwrap(),
                    )
                    .unwrap();
                }
                _ => {
                    fs::remove_file(root.path().join("data/z.bin")).unwrap();
                    fs::create_dir(root.path().join("data/z.bin")).unwrap();
                    fs::write(
                        root.path().join("data/z.bin/sentinel"),
                        b"unrelated directory",
                    )
                    .unwrap();
                }
            }
            let before = snapshot(root.path());
            let error = rollback(
                root.path(),
                RollbackOptions {
                    delete_modified_added: true,
                },
            )
            .unwrap_err();
            assert_eq!(snapshot(root.path()), before, "{state}/{invalid}: {error}");
        }
    }
}

#[test]
fn interrupted_manifest_veto_preserves_replaced_authority_and_force_deletes_edited_added() {
    let root = setup(true);
    let store = PatchStore::new(root.path());
    let mut journal = store.read_journal().unwrap().unwrap();
    journal.plan.added.push(ReceiptAdded {
        path: "data/a.bin".into(),
        patched_sha256: sha256_hex(b"misclassified"),
    });
    fs::write(store.journal_path(), serde_json::to_vec(&journal).unwrap()).unwrap();
    let report = rollback(
        root.path(),
        RollbackOptions {
            delete_modified_added: true,
        },
    )
    .unwrap();
    assert_eq!(report.restored, 2);
    assert_eq!(report.deleted, 1);
    assert_eq!(report.torn_deleted, vec!["data/added.bin"]);
    assert_eq!(
        fs::read(root.path().join("data/a.bin")).unwrap(),
        b"original first"
    );
    assert_eq!(
        fs::read(root.path().join("data/z.bin")).unwrap(),
        b"original second"
    );
    assert!(!root.path().join("data/added.bin").exists());
    assert!(!store.locust_dir().exists());
}

#[test]
fn interrupted_edited_added_requires_force_before_any_mutation_and_retry_is_safe() {
    for state in [JournalState::Applying, JournalState::RollingBack] {
        let root = setup(true);
        let store = PatchStore::new(root.path());
        let mut journal = store.read_journal().unwrap().unwrap();
        journal.state = state;
        fs::write(store.journal_path(), serde_json::to_vec(&journal).unwrap()).unwrap();
        let before = snapshot(root.path());
        for _ in 0..2 {
            let report = rollback(root.path(), RollbackOptions::default()).unwrap();
            assert_eq!(report.aborted_edited, vec!["data/added.bin"]);
            assert_eq!(report.restored, 0);
            assert_eq!(report.deleted, 0);
            assert!(report.torn_deleted.is_empty());
            assert_eq!(snapshot(root.path()), before);
        }
        let report = rollback(
            root.path(),
            RollbackOptions {
                delete_modified_added: true,
            },
        )
        .unwrap();
        assert_eq!(report.restored, 2);
        assert_eq!(report.deleted, 1);
        assert!(!root.path().join("data/added.bin").exists());
        assert_eq!(
            fs::read(root.path().join("data/a.bin")).unwrap(),
            b"original first"
        );
        assert_eq!(
            rollback(root.path(), RollbackOptions::default())
                .unwrap()
                .restored,
            0
        );
    }
}

#[test]
fn interrupted_exact_added_and_already_absent_added_recover_without_force() {
    for exists in [true, false] {
        let root = setup(true);
        let path = root.path().join("data/added.bin");
        if exists {
            fs::write(&path, b"complete added").unwrap();
        } else {
            fs::remove_file(&path).unwrap();
        }
        let report = rollback(root.path(), RollbackOptions::default()).unwrap();
        assert_eq!(report.restored, 2);
        assert_eq!(report.deleted, usize::from(exists));
        assert!(report.aborted_edited.is_empty());
        assert!(report.torn_deleted.is_empty());
        assert!(!path.exists());
    }
}

#[test]
fn patched_edited_added_still_requires_force_and_replaced_edits_are_restored() {
    let root = setup(false);
    patched(root.path());
    let before = snapshot(root.path());
    let report = rollback(root.path(), RollbackOptions::default()).unwrap();
    assert_eq!(report.aborted_edited, vec!["data/added.bin"]);
    assert_eq!(report.restored, 0);
    assert_eq!(snapshot(root.path()), before);
    fs::write(root.path().join("data/a.bin"), b"user edited replaced").unwrap();
    let report = rollback(
        root.path(),
        RollbackOptions {
            delete_modified_added: true,
        },
    )
    .unwrap();
    assert_eq!(report.restored, 2);
    assert_eq!(report.deleted, 1);
    assert_eq!(
        fs::read(root.path().join("data/a.bin")).unwrap(),
        b"original first"
    );
}

#[cfg(windows)]
#[test]
fn owned_staging_install_failure_keeps_backup_and_unrelated_sentinels() {
    use std::os::windows::fs::OpenOptionsExt;
    let root = setup(true);
    let store = PatchStore::new(root.path());
    let bm = store.read_backup_manifest().unwrap().unwrap();
    let unowned = root.path().join(".locust-stage-unrelated");
    fs::create_dir(&unowned).unwrap();
    fs::write(unowned.join("sentinel"), b"unowned recursive bytes").unwrap();
    fs::write(
        root.path().join("data/a.bin.locust-tmp"),
        b"unowned sibling",
    )
    .unwrap();
    let before = snapshot(root.path());
    // Permit source reads but deny a move-aside of the destination. This fails
    // after the owned restore copy was fully written and synced.
    let held = fs::OpenOptions::new()
        .read(true)
        .share_mode(1 | 2)
        .open(root.path().join("data/a.bin"))
        .unwrap();
    assert!(store.restore_file(&bm.files[0]).is_err());
    assert_eq!(snapshot(root.path()), before);
    assert_eq!(
        fs::read_dir(root.path())
            .unwrap()
            .filter(|entry| entry
                .as_ref()
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".locust-stage-"))
            .count(),
        1
    );
    drop(held);
    store.restore_file(&bm.files[0]).unwrap();
    assert_eq!(
        fs::read(root.path().join("data/a.bin")).unwrap(),
        b"original first"
    );
    assert_eq!(
        fs::read(store.backup_files_dir().join("data/a.bin")).unwrap(),
        b"original first"
    );
    assert_eq!(
        fs::read(unowned.join("sentinel")).unwrap(),
        b"unowned recursive bytes"
    );
}
