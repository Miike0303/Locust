//! Journaled patch apply transaction.

use std::fs;
use std::path::{Path, PathBuf};

use chrono::Utc;

use crate::database::sha256_path;
use crate::error::{LocustError, Result};

use super::manifest::{
    ApplyPlan, BackupBaseline, BackupManifest, Journal, JournalState, PatchManifest, Receipt,
    ReceiptAdded, ReceiptReplaced, VerificationTier,
};
use super::rollback::{rollback_under_lock, RollbackOptions};
use super::store::{PatchStatus, PatchStore};
use super::stream::StagingDir;
use super::verify::{
    classify_files_with_hashes, open_archive, scan_zip_entries, verify_scanned,
    VerificationOutcome, VerificationReport, ZipEntryMeta,
};

/// Staged ZIP content in an operation-owned `.locust-stage-*/` directory.
struct StagedContent {
    path: PathBuf,
    sha256: String,
}

/// Immutable staged archive content and the canonical manifest authorized by
/// verification. Kept alive across patch switches; never reopened from a path.
struct PreparedPatch {
    entries: Vec<ZipEntryMeta>,
    manifest: Option<PatchManifest>,
    files: std::collections::HashMap<String, StagedContent>,
}

#[derive(Debug, Clone, Default)]
pub struct ApplyOptions {
    pub force: bool,
    pub confirm_legacy: bool,
    pub dry_run: bool,
}

#[derive(Debug, Clone)]
pub struct PatchProgress {
    pub current: usize,
    pub total: usize,
    pub path: String,
    pub phase: &'static str,
}

#[derive(Debug, Clone)]
pub struct ApplyReport {
    pub patch_id: String,
    pub patch_version: String,
    pub replaced: usize,
    pub added: usize,
    pub forced: bool,
    pub baseline: BackupBaseline,
    pub dry_run: bool,
    /// Prior-receipt added files whose on-disk hash ≠ receipt patched hash
    /// (user edits that will be overwritten under forced reapply).
    pub user_edits_overwritten: Vec<String>,
    pub messages: Vec<String>,
}

/// Apply a patch zip to `game_root` under the design's step order 1–7.
pub fn apply<F>(
    game_root: &Path,
    zip_path: &Path,
    opts: ApplyOptions,
    mut on_progress: F,
) -> Result<ApplyReport>
where
    F: FnMut(PatchProgress),
{
    apply_cancellable(game_root, zip_path, opts, |progress| {
        on_progress(progress);
        Ok(())
    })
}

/// Apply a patch while allowing the progress callback to stop before a file write.
pub fn apply_cancellable<F>(
    game_root: &Path,
    zip_path: &Path,
    opts: ApplyOptions,
    mut on_progress: F,
) -> Result<ApplyReport>
where
    F: FnMut(PatchProgress) -> Result<()>,
{
    let game_lock = super::lock::GameLock::acquire(game_root)?;
    crate::injection_transaction::ensure_no_pending_under_lock(&game_lock)?;
    let game_root = game_lock.root();
    let store = PatchStore::new(game_root);

    super::zipsec::ensure_safe_store(game_root)?;
    if matches!(store.status()?, PatchStatus::Interrupted(_)) {
        return Err(LocustError::PatchInterrupted(
            "run patch-rollback first".into(),
        ));
    }
    let prior_receipt = store.read_receipt()?;
    let mut archive = open_archive(zip_path)?;
    // A legacy overlay cannot preserve the installed patch's rollback inventory.
    // Check the same open archive before staging or probing game writability.
    if prior_receipt.is_some() {
        let mut has_manifest = false;
        for name in archive.file_names() {
            let normalized = super::zipsec::canonical_archive_name(name)?;
            has_manifest |= normalized.to_lowercase() == PatchManifest::FILENAME;
        }
        if !has_manifest {
            return Err(LocustError::PatchError(
                "a Locust patch is already installed; roll it back before applying a patch without a manifest"
                    .into(),
            ));
        }
    }
    // Stage and validate once BEFORE rollback or any asset/receipt mutation.
    // The operation-owned directory is outside .locust so rollback cannot
    // erase incoming content, and replacing the source ZIP cannot alter it.
    let _staging = StagingDir::create_prepared(game_root)?;
    let entries = scan_zip_entries(&mut archive, Some(&_staging))?;
    drop(archive);
    let report = verify_scanned(game_root, &entries)?;
    let files = entries
        .iter()
        .filter_map(|entry| {
            entry
                .rel
                .as_ref()
                .zip(entry.staged_path.as_ref())
                .map(|(rel, path)| {
                    (
                        rel.to_string_lossy().replace('\\', "/"),
                        StagedContent {
                            path: path.clone(),
                            sha256: entry.content_sha256.clone(),
                        },
                    )
                })
        })
        .collect();
    let prepared = PreparedPatch {
        entries,
        manifest: report.manifest.clone(),
        files,
    };
    enforce_verify_gates(&report, &opts)?;

    // R2: only a strict-tier Clean verify may authorize discarding a
    // manifest-less backup/ (design rev 4). Status alone is wrong — presence
    // of backup/ makes status Unknown even when the game content is Clean.
    let r2_allow_discard = matches!(report.outcome, VerificationOutcome::Clean)
        && matches!(report.tier, Some(VerificationTier::Strict));

    // R3 routing when a receipt is present.
    if let Some(prior) = prior_receipt {
        if let Some(ref m) = report.manifest {
            if prior.patch_id == m.patch_id && prior.patch_version == m.patch_version && opts.force
            {
                // Same id+version forced reapply — in-place if file set matches.
                let prior_set: std::collections::HashSet<_> = prior
                    .replaced
                    .iter()
                    .map(|r| r.path.clone())
                    .chain(prior.added.iter().map(|a| a.path.clone()))
                    .collect();
                let incoming_set: std::collections::HashSet<_> =
                    m.files.iter().map(|f| f.path.clone()).collect();
                if prior_set != incoming_set {
                    // File-set drift → rollback-then-fresh.
                    reject_dry_run_rollback(&opts)?;
                    require_full_rollback(
                        &game_lock,
                        RollbackOptions {
                            delete_modified_added: true,
                        },
                    )?;
                    // After a full rollback the game is pristine → allow R2 discard.
                    return apply_after_rollback(game_root, &prepared, &opts, &mut on_progress);
                }
                return apply_fresh(
                    game_root,
                    &prepared,
                    &opts,
                    &mut on_progress,
                    Some(prior),
                    r2_allow_discard,
                );
            }
            if prior.patch_id != m.patch_id || prior.patch_version != m.patch_version {
                // Version or id change: upgrade always; downgrade only with force.
                if matches!(report.outcome, VerificationOutcome::DowngradeBlocked { .. })
                    && !opts.force
                {
                    return Err(LocustError::PatchDowngradeBlocked {
                        installed: prior.patch_version,
                        incoming: m.patch_version.clone(),
                    });
                }
                check_transition_baseline(game_root, &store, m, &prior, &opts)?;
                // Upgrade or forced downgrade / different patch_id → rollback then fresh.
                reject_dry_run_rollback(&opts)?;
                if store.backup_manifest_valid() {
                    // Soft-abort on edited added files must NOT continue into
                    // apply_fresh on a still-patched tree (CRITICAL review finding).
                    require_full_rollback(
                        &game_lock,
                        RollbackOptions {
                            delete_modified_added: opts.force,
                        },
                    )?;
                } else if store.receipt_path().exists() {
                    return Err(LocustError::PatchBackupIncomplete(
                        "cannot upgrade/reinstall: backup/manifest.json missing — \
                         restore it externally or manually remove .locust/ (forfeits pristine)"
                            .into(),
                    ));
                }
                return apply_after_rollback(game_root, &prepared, &opts, &mut on_progress);
            }
        }
    }

    apply_fresh(
        game_root,
        &prepared,
        &opts,
        &mut on_progress,
        None,
        r2_allow_discard,
    )
}

/// Do not discard an installed patch merely to discover afterward that the
/// incoming strict patch targets a different pristine game or dirty loose file.
fn check_transition_baseline(
    game_root: &Path,
    store: &PatchStore,
    incoming: &PatchManifest,
    prior: &Receipt,
    opts: &ApplyOptions,
) -> Result<()> {
    if opts.force || !incoming.supports_strict_tier() {
        return Ok(());
    }
    let backup = store.read_backup_manifest()?.ok_or_else(|| {
        LocustError::PatchBackupIncomplete("transition has no backup manifest".into())
    })?;
    for file in &incoming.files {
        let future_hash = if let Some(original) = backup.files.iter().find(|b| b.path == file.path)
        {
            Some(original.sha256.clone())
        } else if prior.added.iter().any(|a| a.path == file.path) {
            None
        } else {
            let path = game_root.join(&file.path);
            if path.is_file() {
                Some(sha256_path(&path)?)
            } else {
                None
            }
        };
        let expected = file.original_sha256.as_deref().filter(|h| !h.is_empty());
        if future_hash.as_deref() != expected {
            return Err(LocustError::PatchVerificationFailed(format!("incoming patch does not match the rollback baseline at {}; installed patch was preserved",file.path)));
        }
    }
    Ok(())
}

fn apply_after_rollback<F>(
    game_root: &Path,
    prepared: &PreparedPatch,
    opts: &ApplyOptions,
    on_progress: &mut F,
) -> Result<ApplyReport>
where
    F: FnMut(PatchProgress) -> Result<()>,
{
    // Re-check the baseline restored by rollback, using the SAME staged data.
    let report = verify_scanned(game_root, &prepared.entries)?;
    enforce_verify_gates(&report, opts)?;
    let clean = report.outcome == VerificationOutcome::Clean
        && report.tier == Some(VerificationTier::Strict);
    apply_fresh(game_root, prepared, opts, on_progress, None, clean)
}

// A preview must never restore/delete the currently installed patch. Until a
// virtual-baseline planner exists, explicitly reject transitions requiring a
// rollback instead of reporting a misleading dry-run over the patched tree.
fn reject_dry_run_rollback(opts: &ApplyOptions) -> Result<()> {
    if opts.dry_run {
        return Err(LocustError::PatchVerificationFailed(
            "dry-run cannot preview a patch transition that requires rollback; \
             no files changed. Run verify to inspect the incoming version, or \
             apply without dry-run when ready to replace the installed patch."
                .into(),
        ));
    }
    Ok(())
}

/// Rollback that must fully succeed before a subsequent apply. A soft abort
/// (user-edited added files without force) is an error so callers cannot
/// treat it as "pristine enough" and continue writing.
fn require_full_rollback(game_lock: &super::lock::GameLock, opts: RollbackOptions) -> Result<()> {
    let report = rollback_under_lock(game_lock, opts)?;
    if !report.aborted_edited.is_empty() {
        return Err(LocustError::PatchVerificationFailed(format!(
            "rollback aborted before reapply — {} added file(s) were edited after the \
             previous apply and need --force (or restore them manually): {}",
            report.aborted_edited.len(),
            report.aborted_edited.join(", ")
        )));
    }
    Ok(())
}

/// Apply the install policy to a verification report before expensive work
/// such as copying a game. This does not authorize a later write: `apply`
/// validates the actual target and archive again under its own game lock.
pub fn enforce_verify_gates(report: &VerificationReport, opts: &ApplyOptions) -> Result<()> {
    match &report.outcome {
        VerificationOutcome::Interrupted => {
            return Err(LocustError::PatchInterrupted(
                "run patch-rollback first".into(),
            ));
        }
        VerificationOutcome::AlreadyApplied if !opts.force => {
            let id = report
                .manifest
                .as_ref()
                .map(|m| format!("{}@{}", m.patch_id, m.patch_version))
                .unwrap_or_else(|| "unknown".into());
            return Err(LocustError::PatchAlreadyApplied(id));
        }
        VerificationOutcome::DowngradeBlocked {
            installed,
            incoming,
        } if !opts.force => {
            return Err(LocustError::PatchDowngradeBlocked {
                installed: installed.clone(),
                incoming: incoming.clone(),
            });
        }
        VerificationOutcome::Mismatch(ms) if !opts.force => {
            let detail = ms
                .iter()
                .map(|m| format!("{}: expected {}, found {}", m.path, m.expected, m.found))
                .collect::<Vec<_>>()
                .join("; ");
            return Err(LocustError::PatchVerificationFailed(detail));
        }
        VerificationOutcome::Unknown if !opts.force => {
            return Err(LocustError::PatchVerificationFailed(
                "game looks patched or modified but no usable receipt — refusing silent reapply \
                 (pass --force to proceed; backup will be marked unverified)"
                    .into(),
            ));
        }
        _ => {}
    }

    if report.manifest.is_none() && !opts.confirm_legacy {
        return Err(LocustError::PatchLegacyUnconfirmed(
            "zip has no locust-patch.json — pass --confirm-legacy to apply".into(),
        ));
    }

    // Structural tier needs explicit confirmation (design); --force also accepts.
    if report.tier == Some(VerificationTier::Structural)
        && !opts.force
        && !opts.confirm_legacy
        && !matches!(report.outcome, VerificationOutcome::AlreadyApplied)
    {
        return Err(LocustError::PatchLegacyUnconfirmed(
            "structural-tier patch (no original hashes) requires --confirm-legacy or --force"
                .into(),
        ));
    }

    Ok(())
}

fn apply_fresh<F>(
    game_root: &Path,
    prepared: &PreparedPatch,
    opts: &ApplyOptions,
    on_progress: &mut F,
    prior_receipt: Option<Receipt>,
    r2_allow_discard: bool,
) -> Result<ApplyReport>
where
    F: FnMut(PatchProgress) -> Result<()>,
{
    let store = PatchStore::new(game_root);
    let _empty_locust_guard = EmptyLocustGuard(game_root.to_path_buf());
    let manifest = &prepared.manifest;
    let zip_files = &prepared.files;

    // Build plan.
    let (mut replaced, mut added, mut user_edits, mut current_hashes) =
        if let Some(ref m) = manifest {
            classify_files_with_hashes(game_root, &m.files, opts.force)?
        } else {
            // Legacy: every existing path replaced, absent = added.
            let mut replaced = Vec::new();
            let mut added = Vec::new();
            for (path, staged) in zip_files {
                let target = game_root.join(path.replace('/', std::path::MAIN_SEPARATOR_STR));
                let patched = staged.sha256.clone();
                if target.is_file() {
                    let orig = sha256_path(&target)?;
                    replaced.push(ReceiptReplaced {
                        path: path.clone(),
                        original_sha256: Some(orig),
                        patched_sha256: patched,
                    });
                } else {
                    added.push(ReceiptAdded {
                        path: path.clone(),
                        patched_sha256: patched,
                    });
                }
            }
            (
                replaced,
                added,
                Vec::new(),
                std::collections::HashMap::new(),
            )
        };

    // R1 carry-forward: prior receipt classifications win per path on reapply.
    if let Some(ref prior) = prior_receipt {
        let prior_added: std::collections::HashMap<_, _> = prior
            .added
            .iter()
            .map(|a| (a.path.clone(), a.clone()))
            .collect();
        let prior_replaced: std::collections::HashMap<_, _> = prior
            .replaced
            .iter()
            .map(|r| (r.path.clone(), r.clone()))
            .collect();

        // Paths that were added stay added even if force reclassification would
        // move them — except we still overwrite content.
        let mut new_replaced = Vec::new();
        let mut new_added = Vec::new();
        for r in replaced.drain(..) {
            if let Some(pa) = prior_added.get(&r.path) {
                // Check user edit.
                let target = game_root.join(r.path.replace('/', std::path::MAIN_SEPARATOR_STR));
                if target.is_file() {
                    let h = match current_hashes.remove(&r.path) {
                        Some(hash) => hash,
                        None => sha256_path(&target)?,
                    };
                    if h != pa.patched_sha256 {
                        user_edits.push(r.path.clone());
                    }
                }
                new_added.push(ReceiptAdded {
                    path: r.path,
                    patched_sha256: r.patched_sha256,
                });
            } else if let Some(pr) = prior_replaced.get(&r.path) {
                new_replaced.push(ReceiptReplaced {
                    path: r.path,
                    original_sha256: pr.original_sha256.clone(),
                    patched_sha256: r.patched_sha256,
                });
            } else {
                new_replaced.push(r);
            }
        }
        for a in added.drain(..) {
            if let Some(pr) = prior_replaced.get(&a.path) {
                new_replaced.push(ReceiptReplaced {
                    path: a.path,
                    original_sha256: pr.original_sha256.clone(),
                    patched_sha256: a.patched_sha256,
                });
            } else {
                if let Some(pa) = prior_added.get(&a.path) {
                    let target = game_root.join(a.path.replace('/', std::path::MAIN_SEPARATOR_STR));
                    if target.is_file() {
                        let h = match current_hashes.remove(&a.path) {
                            Some(hash) => hash,
                            None => sha256_path(&target)?,
                        };
                        if h != pa.patched_sha256 {
                            user_edits.push(a.path.clone());
                        }
                    }
                }
                new_added.push(a);
            }
        }
        replaced = new_replaced;
        added = new_added;
    }

    let tier = if manifest.as_ref().is_some_and(|m| m.supports_strict_tier()) {
        VerificationTier::Strict
    } else if manifest.is_some() {
        VerificationTier::Structural
    } else {
        VerificationTier::Legacy
    };

    // Baseline: pristine when we backup verified originals under strict tier.
    // Forced same-version reapply keeps the existing committed baseline.
    // Force over Unknown/Mismatch (no prior receipt) is unverified.
    // Force alone over a clean strict first apply remains pristine.
    let baseline = if prior_receipt.is_some() && store.backup_manifest_valid() {
        store
            .read_backup_manifest()?
            .map(|m| m.baseline)
            .unwrap_or(BackupBaseline::Pristine)
    } else if tier == VerificationTier::Strict && r2_allow_discard {
        BackupBaseline::Pristine
    } else {
        BackupBaseline::Unverified
    };

    let (patch_id, patch_version, language, engine, generator_version) =
        if let Some(ref m) = manifest {
            (
                m.patch_id.clone(),
                m.patch_version.clone(),
                m.language.clone(),
                m.engine.clone(),
                m.generator_version.clone(),
            )
        } else {
            (
                uuid::Uuid::new_v4().to_string(),
                "0.0.0-legacy".into(),
                "unknown".into(),
                "unknown".into(),
                env!("CARGO_PKG_VERSION").to_string(),
            )
        };

    // created_dirs: parent dirs of added files that do not yet exist.
    let mut created_dirs = Vec::new();
    for a in &added {
        let target = game_root.join(a.path.replace('/', std::path::MAIN_SEPARATOR_STR));
        if let Some(parent) = target.parent() {
            if !parent.exists() {
                let rel = parent
                    .strip_prefix(game_root)
                    .unwrap_or(parent)
                    .to_string_lossy()
                    .replace('\\', "/");
                if !rel.is_empty() && !created_dirs.contains(&rel) {
                    created_dirs.push(rel);
                }
            }
        }
    }

    let plan = ApplyPlan {
        patch_version: patch_version.clone(),
        language: language.clone(),
        engine: engine.clone(),
        generator_version: generator_version.clone(),
        verification: tier,
        forced: opts.force,
        baseline,
        replaced: replaced.clone(),
        added: added.clone(),
        created_dirs: created_dirs.clone(),
    };

    if opts.dry_run {
        // StagingDir + EmptyLocustGuard drops clean staging / empty .locust.
        return Ok(ApplyReport {
            patch_id,
            patch_version,
            replaced: plan.replaced.len(),
            added: plan.added.len(),
            forced: opts.force,
            baseline,
            dry_run: true,
            user_edits_overwritten: user_edits,
            messages: vec!["dry-run: no files written".into()],
        });
    }

    // Step 3: ensure .locust/ and handle existing backup (R2).
    // Staging already created the parent .locust/; ensure hides + layout.
    store.ensure_locust_dir()?;
    prepare_backup_slot(&store, tier, r2_allow_discard)?;

    // Step 4: backup every not-already-backed-up replaced file.
    let mut backup_entries = if let Some(existing) = store.read_backup_manifest()? {
        existing.files
    } else {
        Vec::new()
    };
    let already: std::collections::HashSet<_> =
        backup_entries.iter().map(|e| e.path.clone()).collect();

    for r in &plan.replaced {
        if already.contains(&r.path) {
            continue;
        }
        let src = game_root.join(r.path.replace('/', std::path::MAIN_SEPARATOR_STR));
        if src.is_file() {
            let entry = store.backup_file(&src, &r.path)?;
            backup_entries.push(entry);
        }
    }

    // Write backup manifest LAST (commit marker). On in-place reapply with
    // existing valid manifest, leave it untouched (design: never overwrite).
    if !store.backup_manifest_valid() {
        let bm = BackupManifest {
            schema_version: BackupManifest::SCHEMA_VERSION,
            created_at: Utc::now().to_rfc3339(),
            baseline,
            files: backup_entries,
        };
        store.write_backup_manifest(&bm)?;
    }

    // Step 5: journal before first game mutation.
    let journal = Journal {
        schema_version: Journal::SCHEMA_VERSION,
        state: JournalState::Applying,
        patch_id: patch_id.clone(),
        plan: plan.clone(),
    };
    store.write_journal(&journal)?;

    // Step 6: write files.
    let total = plan.replaced.len() + plan.added.len();
    let write_paths: Vec<String> = plan
        .replaced
        .iter()
        .map(|r| r.path.clone())
        .chain(plan.added.iter().map(|a| a.path.clone()))
        .collect();
    for (current, path) in write_paths.into_iter().enumerate() {
        on_progress(PatchProgress {
            current: current + 1,
            total,
            path: path.clone(),
            phase: "write",
        })?;
        let staged = zip_files
            .get(&path)
            .ok_or_else(|| LocustError::PatchError(format!("zip missing path {path}")))?;
        super::zipsec::ensure_no_links(game_root, &super::zipsec::safe_stored_rel(&path)?)?;
        let dest = game_root.join(path.replace('/', std::path::MAIN_SEPARATOR_STR));
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }
        if dest.is_file() {
            let meta = fs::metadata(&dest)?;
            if meta.permissions().readonly() {
                return Err(LocustError::GameDirNotWritable(format!(
                    "read-only file: {}",
                    dest.display()
                )));
            }
        }
        // The prepared file is already durable and on the game volume. Never
        // claim a predictable sibling name that may contain user-owned data.
        PatchStore::replace_file(&staged.path, &dest)?;
    }

    // Step 7: receipt, delete journal.
    let receipt = Receipt {
        schema_version: Receipt::SCHEMA_VERSION,
        patch_id: patch_id.clone(),
        patch_version: patch_version.clone(),
        generator_version,
        language,
        engine,
        applied_at: Utc::now().to_rfc3339(),
        verification: tier,
        forced: opts.force,
        baseline,
        created_dirs,
        replaced: plan.replaced.clone(),
        added: plan.added.clone(),
    };
    store.write_receipt(&receipt)?;
    store.delete_journal()?;

    Ok(ApplyReport {
        patch_id,
        patch_version,
        replaced: plan.replaced.len(),
        added: plan.added.len(),
        forced: opts.force,
        baseline,
        dry_run: false,
        user_edits_overwritten: user_edits,
        messages: vec![],
    })
}

/// Remove `.locust/` when it exists but is empty (dry-run / failed-staging residue).
fn remove_empty_locust_dir(game_root: &Path) {
    let locust = game_root.join(super::store::LOCUST_DIR);
    if !locust.is_dir() {
        return;
    }
    let empty = fs::read_dir(&locust)
        .map(|mut d| d.next().is_none())
        .unwrap_or(false);
    if empty {
        let _ = fs::remove_dir(&locust);
    }
}

/// Drops after [`StagingDir`] so an empty `.locust/` can be removed.
struct EmptyLocustGuard(PathBuf);

impl Drop for EmptyLocustGuard {
    fn drop(&mut self) {
        remove_empty_locust_dir(&self.0);
    }
}

/// RULE R2: decide whether an existing backup/ may be discarded or is incomplete.
///
/// `r2_allow_discard` is true only when verify reported Clean at the **strict**
/// tier (content hashes). Structural/legacy and Unknown never authorize discard.
/// Do not use `store.status() == NotPatched` alone: a leftover `backup/` without
/// receipt makes status Unknown while the game itself may still be Clean.
fn prepare_backup_slot(
    store: &PatchStore,
    tier: VerificationTier,
    r2_allow_discard: bool,
) -> Result<()> {
    let backup_dir = store.backup_dir();
    if !backup_dir.exists() {
        fs::create_dir_all(store.backup_files_dir())?;
        return Ok(());
    }
    if store.backup_manifest_valid() {
        // Valid pristine backup — preserve forever.
        return Ok(());
    }
    // Manifest-less or invalid.
    let has_receipt = store.receipt_path().is_file();
    let has_journal = store.journal_path().is_file();
    if has_receipt || has_journal {
        return Err(LocustError::PatchBackupIncomplete(
            "backup/ exists without a valid manifest.json while a receipt or journal is present — \
             nothing deleted. Restore the manifest externally, or manually salvage backup/files/ \
             and remove .locust/ (forfeits pristine baseline)."
                .into(),
        ));
    }
    // R2: structural/legacy never authorize discard (even if somehow Clean).
    if tier != VerificationTier::Strict {
        return Err(LocustError::PatchBackupIncomplete(
            "manifest-less backup/ cannot be discarded under structural or legacy verification \
             (only strict-tier Clean may rebuild it). Salvage backup/files/ manually."
                .into(),
        ));
    }
    if r2_allow_discard {
        fs::remove_dir_all(&backup_dir)?;
        fs::create_dir_all(store.backup_files_dir())?;
        return Ok(());
    }
    Err(LocustError::PatchBackupIncomplete(
        "manifest-less backup/ present and game is not verify-Clean at strict tier — \
         refusing to discard (R2). Manual recovery required."
            .into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::{sha256_hex, SHA256_PATH_CALLS};
    use crate::patch::manifest::PatchFileEntry;
    use std::io::Write;

    fn write_added_patch(zip_path: &Path, manifest: &PatchManifest, content: &[u8]) {
        let mut zip = zip::ZipWriter::new(fs::File::create(zip_path).unwrap());
        let options = zip::write::SimpleFileOptions::default();
        zip.start_file(PatchManifest::FILENAME, options).unwrap();
        zip.write_all(&serde_json::to_vec(manifest).unwrap())
            .unwrap();
        zip.start_file(&manifest.files[0].path, options).unwrap();
        zip.write_all(content).unwrap();
        zip.finish().unwrap();
    }

    /// Exercise the public apply path, including its single GameLock and receipt writes.
    fn forced_reapply_hashes(edited: bool, incoming_original: bool) -> Option<usize> {
        let game = tempfile::tempdir().unwrap();
        let archive_dir = tempfile::tempdir().unwrap();
        let zip_path = archive_dir.path().join("patch.zip");
        let path = "Content/Paks/translation.pak";
        let content = b"patched pak";
        let mut manifest = PatchManifest {
            game: None,
            schema_version: PatchManifest::SCHEMA_VERSION,
            patch_id: "reapply-hash-fixture".into(),
            game_name: "fixture".into(),
            engine: "unreal".into(),
            language: "es".into(),
            patch_version: "1.0.0".into(),
            generator_version: "test".into(),
            created_at: "2026-01-01T00:00:00Z".into(),
            files: vec![PatchFileEntry {
                path: path.into(),
                patched_sha256: sha256_hex(content),
                size: content.len() as u64,
                original_sha256: None,
            }],
        };
        write_added_patch(&zip_path, &manifest, content);
        apply(
            game.path(),
            &zip_path,
            ApplyOptions {
                confirm_legacy: true,
                ..Default::default()
            },
            |_| {},
        )
        .unwrap();

        if edited {
            fs::write(game.path().join(path), b"user edit").unwrap();
        }
        if incoming_original {
            // Same version and file set, but classification now skips this hash.
            manifest.files[0].original_sha256 = Some(sha256_hex(b"original pak"));
            write_added_patch(&zip_path, &manifest, content);
        }

        SHA256_PATH_CALLS.with(|calls| calls.set(Some(0)));
        let result = apply(
            game.path(),
            &zip_path,
            ApplyOptions {
                force: true,
                ..Default::default()
            },
            |_| {},
        );
        let hashes = SHA256_PATH_CALLS.with(|calls| calls.replace(None));
        let report = result.unwrap();
        assert_eq!(report.replaced, 0);
        assert_eq!(report.added, 1);
        assert_eq!(
            report.user_edits_overwritten,
            if edited { vec![path] } else { vec![] }
        );
        assert_eq!(fs::read(game.path().join(path)).unwrap(), content);

        // Compare the entire written JSON with the pre-optimization receipt.
        // The wall-clock timestamp is the only nondeterministic field.
        let mut receipt: serde_json::Value =
            serde_json::from_slice(&fs::read(PatchStore::new(game.path()).receipt_path()).unwrap())
                .unwrap();
        chrono::DateTime::parse_from_rfc3339(receipt["applied_at"].as_str().unwrap()).unwrap();
        receipt["applied_at"] = serde_json::json!("<timestamp>");
        assert_eq!(
            receipt,
            serde_json::json!({
                "schema_version": 1,
                "patch_id": "reapply-hash-fixture",
                "patch_version": "1.0.0",
                "generator_version": "test",
                "language": "es",
                "engine": "unreal",
                "applied_at": "<timestamp>",
                "verification": if incoming_original { "strict" } else { "structural" },
                "forced": true,
                "baseline": "unverified",
                "created_dirs": [],
                "replaced": [],
                "added": [{ "path": path, "patched_sha256": sha256_hex(content) }]
            })
        );
        hashes
    }

    #[test]
    fn forced_reapply_hashes_existing_added_file_once() {
        assert_eq!(forced_reapply_hashes(false, false), Some(1));
    }

    #[test]
    fn forced_reapply_hashes_edited_added_file_once() {
        assert_eq!(forced_reapply_hashes(true, false), Some(1));
    }

    #[test]
    fn forced_reapply_hashes_prior_added_path_with_incoming_original() {
        for edited in [false, true] {
            assert_eq!(forced_reapply_hashes(edited, true), Some(1));
        }
    }
}
