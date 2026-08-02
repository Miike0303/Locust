//! Rollback to the pristine (or pre-apply) state using RULE R1.

use std::fs;
use std::path::Path;

use crate::database::sha256_hex;
use crate::error::{LocustError, Result};

use super::manifest::{BackupBaseline, JournalState};
use super::store::{PatchStatus, PatchStore};

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
    /// Added files deleted as torn-by-interrupted-apply (journal path).
    pub torn_deleted: Vec<String>,
}

/// Roll a game back using the backup manifest as the sole restore authority.
pub fn rollback(game_root: &Path, opts: RollbackOptions) -> Result<RollbackReport> {
    let store = PatchStore::new(game_root);

    match store.status()? {
        PatchStatus::NotPatched => {
            return Ok(RollbackReport {
                restored: 0,
                deleted: 0,
                baseline: None,
                messages: vec!["not patched — nothing to do".into()],
                aborted_edited: vec![],
                torn_deleted: vec![],
            });
        }
        PatchStatus::Unknown => {
            // S6a: valid backup, no receipt → restore-only with force/confirm.
            if let Some(bm) = store.read_backup_manifest()? {
                if !opts.delete_modified_added {
                    return Err(LocustError::PatchVerificationFailed(
                        "backup exists but receipt is missing — added files cannot be identified. \
                         Pass --force to restore replaced files only (patch-added files may remain)."
                            .into(),
                    ));
                }
                return restore_manifest_only(&store, &bm, vec![], true);
            }
            return Err(LocustError::PatchBackupIncomplete(
                "no backup found — factory pristine is unrecoverable".into(),
            ));
        }
        PatchStatus::Interrupted(journal) => {
            // Journal-driven path.
            let Some(bm) = store.read_backup_manifest()? else {
                return Err(LocustError::PatchBackupIncomplete(
                    "interrupted apply has no valid backup/manifest.json — refusing".into(),
                ));
            };
            // Write rolling-back journal state (best-effort marker).
            let mut j = journal.clone();
            j.state = JournalState::RollingBack;
            store.write_journal(&j)?;

            let mut torn = Vec::new();
            let mut delete_set = Vec::new();
            let manifest_paths: std::collections::HashSet<_> =
                bm.files.iter().map(|f| f.path.clone()).collect();

            for a in &journal.plan.added {
                if manifest_paths.contains(&a.path) {
                    continue; // R1 veto
                }
                let target = game_root.join(a.path.replace('/', std::path::MAIN_SEPARATOR_STR));
                if target.is_file() {
                    let h = sha256_hex(&fs::read(&target)?);
                    if h != a.patched_sha256 {
                        // Torn by interrupted apply — delete without confirmation.
                        torn.push(a.path.clone());
                    }
                    delete_set.push(a.path.clone());
                } else {
                    // Already absent — fine.
                }
            }

            for p in &delete_set {
                let target = game_root.join(p.replace('/', std::path::MAIN_SEPARATOR_STR));
                if target.is_file() {
                    fs::remove_file(&target)?;
                }
            }

            let mut restored = 0usize;
            for entry in &bm.files {
                store.restore_file(entry)?;
                restored += 1;
            }

            // Clean tmp strays.
            clean_locust_tmps(game_root);

            let baseline = bm.baseline;
            store.remove_all()?;

            return Ok(RollbackReport {
                restored,
                deleted: delete_set.len(),
                baseline: Some(baseline),
                messages: vec!["interrupted apply rolled back to backup baseline".into()],
                aborted_edited: vec![],
                torn_deleted: torn,
            });
        }
        PatchStatus::Patched(receipt) => {
            let Some(bm) = store.read_backup_manifest()? else {
                // S8: receipt present, manifest missing — never force-overridable.
                return Err(LocustError::PatchBackupIncomplete(
                    "receipt present but backup/manifest.json missing or invalid — nothing deleted. \
                     Restore the manifest from an external copy, or manually salvage backup/files/ \
                     and remove .locust/ (forfeits pristine)."
                        .into(),
                ));
            };

            // Preflight: every backup file present with matching hash.
            for entry in &bm.files {
                let src = store
                    .backup_files_dir()
                    .join(entry.path.replace('/', std::path::MAIN_SEPARATOR_STR));
                if !src.is_file() {
                    return Err(LocustError::PatchBackupIncomplete(format!(
                        "backup file missing: {}",
                        entry.path
                    )));
                }
                let h = sha256_hex(&fs::read(&src)?);
                if h != entry.sha256 {
                    return Err(LocustError::PatchBackupIncomplete(format!(
                        "backup file corrupt: {}",
                        entry.path
                    )));
                }
            }

            let manifest_paths: std::collections::HashSet<_> =
                bm.files.iter().map(|f| f.path.clone()).collect();

            // Deletion set = added[] minus manifest paths (R1).
            let mut delete_set = Vec::new();
            let mut edited = Vec::new();
            for a in &receipt.added {
                if manifest_paths.contains(&a.path) {
                    continue;
                }
                let target = game_root.join(a.path.replace('/', std::path::MAIN_SEPARATOR_STR));
                if !target.is_file() {
                    continue;
                }
                let h = sha256_hex(&fs::read(&target)?);
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

            // Journal rolling-back.
            // (receipt path has no journal usually; optional)

            for p in &delete_set {
                let target = game_root.join(p.replace('/', std::path::MAIN_SEPARATOR_STR));
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
                let p = game_root.join(d.replace('/', std::path::MAIN_SEPARATOR_STR));
                if p.is_dir() {
                    let _ = fs::remove_dir(&p); // only if empty
                }
            }

            clean_locust_tmps(game_root);
            let baseline = bm.baseline;
            store.remove_all()?;

            let msg = match baseline {
                BackupBaseline::Pristine => {
                    "restored to verified pristine".to_string()
                }
                BackupBaseline::Unverified => {
                    "restored to pre-apply state, NOT verified pristine".to_string()
                }
            };

            Ok(RollbackReport {
                restored,
                deleted: delete_set.len(),
                baseline: Some(baseline),
                messages: vec![msg],
                aborted_edited: if opts.delete_modified_added {
                    edited
                } else {
                    vec![]
                },
                torn_deleted: vec![],
            })
        }
    }
}

fn restore_manifest_only(
    store: &PatchStore,
    bm: &super::manifest::BackupManifest,
    delete_set: Vec<String>,
    note_added_may_remain: bool,
) -> Result<RollbackReport> {
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

fn clean_locust_tmps(game_root: &Path) {
    if let Ok(walk) = fs::read_dir(game_root) {
        // Only clean direct children + one level; full walk is fine for tests.
        let _ = walk;
    }
    // Walk shallow via walkdir if available — core already depends on walkdir.
    for entry in walkdir::WalkDir::new(game_root)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let name = entry.file_name().to_string_lossy();
        if name.ends_with(".locust-tmp") || name.starts_with(".locust-probe-") {
            let _ = fs::remove_file(entry.path());
        }
    }
}
