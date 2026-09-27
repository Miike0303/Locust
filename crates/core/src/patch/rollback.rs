//! Rollback to the pristine (or pre-apply) state using RULE R1.

use std::fs;
use std::path::Path;

use crate::database::sha256_path;
use crate::error::{LocustError, Result};

use super::manifest::{BackupBaseline, JournalState};
use super::store::{PatchStatus, PatchStore};
use super::zipsec::safe_stored_rel;

#[derive(Debug, Clone, Default)]
pub struct RollbackOptions {
    /// When true, delete user-edited added files without aborting for confirmation.
    pub delete_modified_added: bool,
}

#[derive(Debug, Clone)]
pub struct RollbackReport {
    pub restored: usize,
    pub deleted: usize,
    pub baseline: Option<BackupBaseline>,
    pub messages: Vec<String>,
    /// Added files that looked user-edited (receipt path) and were kept because
    /// confirmation was required and not given.
    pub aborted_edited: Vec<String>,
    /// Changed added files explicitly authorized for deletion. The field name
    /// is retained for API compatibility;
    /// a hash mismatch alone cannot distinguish torn bytes from a user edit.
    pub torn_deleted: Vec<String>,
}

/// Roll a game back using the backup manifest as the sole restore authority.
pub fn rollback(game_root: &Path, opts: RollbackOptions) -> Result<RollbackReport> {
    let game_lock = super::lock::GameLock::acquire(game_root)?;
    rollback_under_lock(&game_lock, opts)
}

/// Run rollback's preflight and classify deletions without changing game files.
pub fn preview_rollback(game_root: &Path, opts: RollbackOptions) -> Result<RollbackReport> {
    let game_lock = super::lock::GameLock::acquire(game_root)?;
    rollback_under_lock_impl(&game_lock, opts, true)
}

/// Caller must hold GameLock for this canonical root across the complete
/// rollback-and-reapply transition. Never reenter the public locking wrapper.
pub(super) fn rollback_under_lock(
    game_lock: &super::lock::GameLock,
    opts: RollbackOptions,
) -> Result<RollbackReport> {
    rollback_under_lock_impl(game_lock, opts, false)
}

fn rollback_under_lock_impl(
    game_lock: &super::lock::GameLock,
    opts: RollbackOptions,
    dry_run: bool,
) -> Result<RollbackReport> {
    crate::injection_transaction::ensure_no_pending_under_lock(game_lock)?;
    let game_root = game_lock.root();
    super::zipsec::ensure_safe_store(game_root)?;
    let store = PatchStore::new(game_root);
    let status = store.status()?;
    // One immutable restore plan, validated in every state before the first
    // added-file deletion, restored destination, or rolling-back marker.
    let backup_manifest = store.read_backup_manifest()?;
    if let Some(backup) = &backup_manifest {
        store.preflight_restore(backup)?;
    }
    let (added, dirs) = match &status {
        PatchStatus::Patched(receipt) => (&receipt.added[..], &receipt.created_dirs[..]),
        PatchStatus::Interrupted(journal) => {
            (&journal.plan.added[..], &journal.plan.created_dirs[..])
        }
        _ => (&[][..], &[][..]),
    };
    for path in added.iter().map(|file| &file.path).chain(dirs.iter()) {
        super::zipsec::ensure_no_links(game_root, &safe_stored_rel(path)?)?;
    }

    match status {
        PatchStatus::NotPatched => Ok(RollbackReport {
            restored: 0,
            deleted: 0,
            baseline: None,
            messages: vec!["not patched — nothing to do".into()],
            aborted_edited: vec![],
            torn_deleted: vec![],
        }),
        PatchStatus::Unknown => {
            // S6a: valid backup, no receipt → restore-only with force/confirm.
            if let Some(bm) = &backup_manifest {
                if !opts.delete_modified_added {
                    return Err(LocustError::PatchVerificationFailed(
                        "backup exists but receipt is missing — added files cannot be identified. \
                         Pass --force to restore replaced files only (patch-added files may remain)."
                            .into(),
                    ));
                }
                return restore_manifest_only(&store, bm, vec![], true, dry_run);
            }
            Err(LocustError::PatchBackupIncomplete(
                "no backup found — factory pristine is unrecoverable".into(),
            ))
        }
        PatchStatus::Interrupted(journal) => {
            // Journal-driven path.
            let Some(bm) = &backup_manifest else {
                return Err(LocustError::PatchBackupIncomplete(
                    "interrupted apply has no valid backup/manifest.json — refusing".into(),
                ));
            };
            let mut edited = Vec::new();
            let mut delete_set = Vec::new();
            let manifest_paths: std::collections::HashSet<_> =
                bm.files.iter().map(|f| f.path.clone()).collect();

            for a in &journal.plan.added {
                if manifest_paths.contains(&a.path) {
                    continue; // R1 veto
                }
                let rel = safe_stored_rel(&a.path)?;
                let target = game_root.join(&rel);
                if target.is_file() {
                    let h = sha256_path(&target)?;
                    if h != a.patched_sha256 {
                        // Interruption cannot prove ownership of changed bytes:
                        // the user may have edited this file after the crash.
                        edited.push(a.path.clone());
                    }
                    delete_set.push(a.path.clone());
                } else {
                    // Already absent — fine.
                }
            }

            if !edited.is_empty() && !opts.delete_modified_added {
                return Ok(RollbackReport {
                    restored: 0,
                    deleted: 0,
                    baseline: Some(bm.baseline),
                    messages: vec![format!(
                        "abort: {} added file(s) differ from the interrupted patch — pass --force to delete them",
                        edited.len()
                    )],
                    aborted_edited: edited,
                    torn_deleted: vec![],
                });
            }

            if dry_run {
                return Ok(RollbackReport {
                    restored: bm.files.len(),
                    deleted: delete_set.len(),
                    baseline: Some(bm.baseline),
                    messages: vec!["dry-run: interrupted apply can be rolled back".into()],
                    aborted_edited: vec![],
                    torn_deleted: edited,
                });
            }

            // Classification and restore preflight completed without mutation.
            let mut j = journal.clone();
            j.state = JournalState::RollingBack;
            store.write_journal(&j)?;

            for p in &delete_set {
                let rel = safe_stored_rel(p)?;
                let target = game_root.join(&rel);
                if target.is_file() {
                    fs::remove_file(&target)?;
                }
            }

            let mut restored = 0usize;
            for entry in &bm.files {
                store.restore_file(entry)?;
                restored += 1;
            }

            let baseline = bm.baseline;
            store.remove_all()?;

            Ok(RollbackReport {
                restored,
                deleted: delete_set.len(),
                baseline: Some(baseline),
                messages: vec!["interrupted apply rolled back to backup baseline".into()],
                aborted_edited: vec![],
                torn_deleted: edited,
            })
        }
        PatchStatus::Patched(receipt) => {
            let Some(bm) = &backup_manifest else {
                // S8: receipt present, manifest missing — never force-overridable.
                return Err(LocustError::PatchBackupIncomplete(
                    "receipt present but backup/manifest.json missing or invalid — nothing deleted. \
                     Restore the manifest from an external copy, or manually salvage backup/files/ \
                     and remove .locust/ (forfeits pristine)."
                        .into(),
                ));
            };

            let manifest_paths: std::collections::HashSet<_> =
                bm.files.iter().map(|f| f.path.clone()).collect();

            // Deletion set = added[] minus manifest paths (R1).
            let mut delete_set = Vec::new();
            let mut edited = Vec::new();
            for a in &receipt.added {
                if manifest_paths.contains(&a.path) {
                    continue;
                }
                let rel = safe_stored_rel(&a.path)?;
                let target = game_root.join(&rel);
                if !target.is_file() {
                    continue;
                }
                let h = sha256_path(&target)?;
                if h != a.patched_sha256 {
                    edited.push(a.path.clone());
                }
                delete_set.push(a.path.clone());
            }

            if !edited.is_empty() && !opts.delete_modified_added {
                return Ok(RollbackReport {
                    restored: 0,
                    deleted: 0,
                    baseline: Some(bm.baseline),
                    messages: vec![format!(
                        "abort: {} added file(s) were edited after apply — pass --force to delete them",
                        edited.len()
                    )],
                    aborted_edited: edited,
                    torn_deleted: vec![],
                });
            }

            if dry_run {
                return Ok(RollbackReport {
                    restored: bm.files.len(),
                    deleted: delete_set.len(),
                    baseline: Some(bm.baseline),
                    messages: vec!["dry-run: patch can be rolled back".into()],
                    aborted_edited: vec![],
                    torn_deleted: edited,
                });
            }

            // Journal rolling-back.
            // (receipt path has no journal usually; optional)

            for p in &delete_set {
                let rel = safe_stored_rel(p)?;
                let target = game_root.join(&rel);
                if target.is_file() {
                    fs::remove_file(&target)?;
                }
            }

            let mut restored = 0usize;
            for entry in &bm.files {
                store.restore_file(entry)?;
                restored += 1;
            }

            // Remove created dirs if empty.
            for d in receipt.created_dirs.iter().rev() {
                let Ok(rel) = safe_stored_rel(d) else {
                    continue;
                };
                let p = game_root.join(&rel);
                if p.is_dir() {
                    let _ = fs::remove_dir(&p); // only if empty
                }
            }

            let baseline = bm.baseline;
            store.remove_all()?;

            let msg = match baseline {
                BackupBaseline::Pristine => "restored to verified pristine".to_string(),
                BackupBaseline::Unverified => {
                    "restored to pre-apply state, NOT verified pristine".to_string()
                }
            };

            Ok(RollbackReport {
                restored,
                deleted: delete_set.len(),
                baseline: Some(baseline),
                messages: vec![msg],
                aborted_edited: vec![],
                torn_deleted: edited,
            })
        }
    }
}

fn restore_manifest_only(
    store: &PatchStore,
    bm: &super::manifest::BackupManifest,
    delete_set: Vec<String>,
    note_added_may_remain: bool,
    dry_run: bool,
) -> Result<RollbackReport> {
    if dry_run {
        return Ok(RollbackReport {
            restored: bm.files.len(),
            deleted: delete_set.len(),
            baseline: Some(bm.baseline),
            messages: vec![
                "dry-run: restore is possible, but patch-added files may remain without a receipt"
                    .into(),
            ],
            aborted_edited: vec![],
            torn_deleted: vec![],
        });
    }
    for p in &delete_set {
        let target = store
            .game_root()
            .join(p.replace('/', std::path::MAIN_SEPARATOR_STR));
        if target.is_file() {
            fs::remove_file(target)?;
        }
    }
    let mut restored = 0usize;
    for entry in &bm.files {
        store.restore_file(entry)?;
        restored += 1;
    }
    let baseline = bm.baseline;
    store.remove_all()?;
    let mut messages = vec!["restored files from backup manifest".into()];
    if note_added_may_remain {
        messages.push(
            "patch-added files may remain — they could not be identified without a receipt".into(),
        );
    }
    Ok(RollbackReport {
        restored,
        deleted: delete_set.len(),
        baseline: Some(baseline),
        messages,
        aborted_edited: vec![],
        torn_deleted: vec![],
    })
}
