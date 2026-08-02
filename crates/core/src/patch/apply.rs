//! Journaled patch apply transaction.

use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};

use chrono::Utc;
use zip::ZipArchive;

use crate::database::sha256_hex;
use crate::error::{LocustError, Result};

use super::manifest::{
    ApplyPlan, BackupBaseline, BackupManifest, Journal, JournalState, PatchManifest, Receipt,
    ReceiptAdded, ReceiptReplaced, VerificationTier,
};
use super::rollback::{rollback, RollbackOptions};
use super::store::{PatchStatus, PatchStore};
use super::verify::{
    classify_files, verify, VerificationOutcome, VerificationReport,
};
use super::zipsec::{normalize_entry_name, safe_entry_path};

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
    let store = PatchStore::new(game_root);

    // Step 1: verify (read-only).
    let report = verify(game_root, zip_path)?;
    enforce_verify_gates(&report, &opts, &store)?;

    // R3 routing when a receipt is present.
    if let Some(prior) = store.read_receipt()? {
        if let Some(ref m) = report.manifest {
            if prior.patch_id == m.patch_id
                && prior.patch_version == m.patch_version
                && opts.force
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
                    rollback(
                        game_root,
                        RollbackOptions {
                            delete_modified_added: true,
                        },
                    )?;
                    return apply_fresh(game_root, zip_path, &opts, &mut on_progress, None);
                }
                return apply_fresh(
                    game_root,
                    zip_path,
                    &opts,
                    &mut on_progress,
                    Some(prior),
                );
            }
            if prior.patch_id != m.patch_id || prior.patch_version != m.patch_version {
                // Version or id change: upgrade always; downgrade only with force.
                if matches!(
                    report.outcome,
                    VerificationOutcome::DowngradeBlocked { .. }
                ) && !opts.force
                {
                    return Err(LocustError::PatchDowngradeBlocked {
                        installed: prior.patch_version,
                        incoming: m.patch_version.clone(),
                    });
                }
                // Upgrade or forced downgrade / different patch_id → rollback then fresh.
                if store.backup_manifest_valid() {
                    rollback(
                        game_root,
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
                return apply_fresh(game_root, zip_path, &opts, &mut on_progress, None);
            }
        }
    }

    apply_fresh(game_root, zip_path, &opts, &mut on_progress, None)
}

fn enforce_verify_gates(
    report: &VerificationReport,
    opts: &ApplyOptions,
    store: &PatchStore,
) -> Result<()> {
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
        let _ = store;
        return Err(LocustError::PatchLegacyUnconfirmed(
            "structural-tier patch (no original hashes) requires --confirm-legacy or --force"
                .into(),
        ));
    }

    Ok(())
}

fn apply_fresh<F>(
    game_root: &Path,
    zip_path: &Path,
    opts: &ApplyOptions,
    on_progress: &mut F,
    prior_receipt: Option<Receipt>,
) -> Result<ApplyReport>
where
    F: FnMut(PatchProgress),
{
    let store = PatchStore::new(game_root);
    let file = File::open(zip_path)?;
    let mut archive = ZipArchive::new(file)
        .map_err(|e| LocustError::PatchError(format!("open zip: {e}")))?;

    // Load zip bytes by relative path.
    let mut zip_files: std::collections::HashMap<String, Vec<u8>> =
        std::collections::HashMap::new();
    let mut manifest: Option<PatchManifest> = None;
    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| LocustError::PatchError(format!("zip: {e}")))?;
        if entry.is_dir() {
            continue;
        }
        let original = entry.name().to_string();
        let normalized = normalize_entry_name(&original);
        let mut data = Vec::new();
        entry
            .read_to_end(&mut data)
            .map_err(|e| LocustError::PatchError(format!("read {original}: {e}")))?;
        if normalized == PatchManifest::FILENAME {
            manifest = Some(serde_json::from_slice(&data).map_err(|e| {
                LocustError::PatchError(format!("manifest parse: {e}"))
            })?);
            continue;
        }
        if normalized.eq_ignore_ascii_case("readme.txt") {
            continue;
        }
        let rel = safe_entry_path(&normalized, &original)?;
        zip_files.insert(rel.to_string_lossy().replace('\\', "/"), data);
    }

    // Build plan.
    let (mut replaced, mut added, mut user_edits) = if let Some(ref m) = manifest {
        classify_files(game_root, &m.files, opts.force)?
    } else {
        // Legacy: every existing path replaced, absent = added.
        let mut replaced = Vec::new();
        let mut added = Vec::new();
        for (path, data) in &zip_files {
            let target = game_root.join(path.replace('/', std::path::MAIN_SEPARATOR_STR));
            let patched = sha256_hex(data);
            if target.is_file() {
                let orig = sha256_hex(&fs::read(&target)?);
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
        (replaced, added, Vec::new())
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
                    let h = sha256_hex(&fs::read(&target)?);
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
                    let target =
                        game_root.join(a.path.replace('/', std::path::MAIN_SEPARATOR_STR));
                    if target.is_file() {
                        let h = sha256_hex(&fs::read(&target)?);
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

    // Baseline: pristine only when strict and not forced-over-unknown.
    let baseline = match (&report_outcome_needs_unverified(opts, prior_receipt.is_some()), tier) {
        (false, VerificationTier::Strict) if !opts.force || prior_receipt.is_some() => {
            // Forced same-version reapply keeps pristine backup.
            if prior_receipt.is_some() {
                BackupBaseline::Pristine
            } else if opts.force {
                // force over Clean is still pristine originals.
                BackupBaseline::Pristine
            } else {
                BackupBaseline::Pristine
            }
        }
        (false, VerificationTier::Strict) => BackupBaseline::Pristine,
        _ => {
            if prior_receipt.is_some() && store.backup_manifest_valid() {
                // In-place reapply: keep existing baseline.
                store
                    .read_backup_manifest()?
                    .map(|m| m.baseline)
                    .unwrap_or(BackupBaseline::Pristine)
            } else if opts.force {
                BackupBaseline::Unverified
            } else if tier == VerificationTier::Strict {
                BackupBaseline::Pristine
            } else {
                BackupBaseline::Unverified
            }
        }
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
    store.ensure_locust_dir()?;
    prepare_backup_slot(&store, tier, opts)?;

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
    let mut current = 0usize;
    let write_paths: Vec<String> = plan
        .replaced
        .iter()
        .map(|r| r.path.clone())
        .chain(plan.added.iter().map(|a| a.path.clone()))
        .collect();
    for path in write_paths {
        current += 1;
        on_progress(PatchProgress {
            current,
            total,
            path: path.clone(),
            phase: "write",
        });
        let data = zip_files.get(&path).ok_or_else(|| {
            LocustError::PatchError(format!("zip missing path {path}"))
        })?;
        let dest = game_root.join(path.replace('/', std::path::MAIN_SEPARATOR_STR));
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }
        // Sibling temp name that cannot collide with a real extension.
        let tmp = {
            let mut t = dest.as_os_str().to_owned();
            t.push(".locust-tmp");
            PathBuf::from(t)
        };
        fs::write(&tmp, data)?;
        {
            // Windows: sync_all needs a writable handle (see store::backup_file).
            let f = fs::OpenOptions::new().read(true).write(true).open(&tmp)?;
            f.sync_all()?;
        }
        if dest.is_file() {
            let meta = fs::metadata(&dest)?;
            if meta.permissions().readonly() {
                let _ = fs::remove_file(&tmp);
                return Err(LocustError::GameDirNotWritable(format!(
                    "read-only file: {}",
                    dest.display()
                )));
            }
        }
        PatchStore::replace_file(&tmp, &dest)?;
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

fn report_outcome_needs_unverified(opts: &ApplyOptions, _has_prior: bool) -> bool {
    opts.force
}

/// RULE R2: decide whether an existing backup/ may be discarded or is incomplete.
fn prepare_backup_slot(
    store: &PatchStore,
    tier: VerificationTier,
    _opts: &ApplyOptions,
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
    // R2: discard only when strict-tier Clean. We are about to apply; "Clean"
    // is implied only for non-forced structural/legacy we already gated — but
    // structural/legacy MUST never authorize discard.
    if tier != VerificationTier::Strict {
        return Err(LocustError::PatchBackupIncomplete(
            "manifest-less backup/ cannot be discarded under structural or legacy verification \
             (only strict-tier Clean may rebuild it). Salvage backup/files/ manually."
                .into(),
        ));
    }
    // Strict tier + no receipt + no journal → safe to discard and rebuild.
    // Status may still be Unknown if files look patched — apply only reaches
    // here with force in that case; design still allows discard only for
    // verify-Clean. Check game content via store status is insufficient;
    // caller already forced Unknown. For force+Unknown, R2 says: if
    // manifest-less backup exists, R2 fires and NOTHING is discarded when
    // verify is not strict Clean. Unknown ≠ Clean → hard error.
    match store.status()? {
        PatchStatus::NotPatched => {
            fs::remove_dir_all(&backup_dir)?;
            fs::create_dir_all(store.backup_files_dir())?;
            Ok(())
        }
        _ => Err(LocustError::PatchBackupIncomplete(
            "manifest-less backup/ present and game is not verify-Clean at strict tier — \
             refusing to discard (R2). Manual recovery required."
                .into(),
        )),
    }
}
