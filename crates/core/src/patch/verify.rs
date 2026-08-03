//! Read-only patch verification against a game tree.

use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};

use semver::Version;
use zip::ZipArchive;

use crate::database::sha256_hex;
use crate::error::{LocustError, Result};

use super::manifest::{PatchFileEntry, PatchManifest, Receipt, VerificationTier};
use super::store::{PatchStatus, PatchStore};
use super::zipsec::{
    case_fold_key, check_entry_budget, normalize_entry_name, safe_entry_path,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerificationOutcome {
    Clean,
    AlreadyApplied,
    UpgradeAvailable,
    DowngradeBlocked {
        installed: String,
        incoming: String,
    },
    Unknown,
    Interrupted,
    Mismatch(Vec<FileMismatch>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileMismatch {
    pub path: String,
    pub expected: String,
    pub found: String,
}

#[derive(Debug, Clone)]
pub struct VerificationReport {
    pub outcome: VerificationOutcome,
    pub tier: Option<VerificationTier>,
    pub manifest: Option<PatchManifest>,
    /// Paths that would be replaced (exist in pristine / current game).
    pub replaced: Vec<String>,
    /// Paths that would be newly added.
    pub added: Vec<String>,
    /// Added paths that already exist with non-patched content (conflicts).
    pub conflicts: Vec<String>,
    /// Warning when receipt exists but backup commit marker is missing.
    pub backup_compromised: bool,
    pub messages: Vec<String>,
}

/// Open the zip, scan security, parse optional manifest, compare to game.
pub fn verify(game_root: &Path, zip_path: &Path) -> Result<VerificationReport> {
    let store = PatchStore::new(game_root);

    // Interrupted always wins — force never overrides.
    if matches!(store.status()?, PatchStatus::Interrupted(_)) {
        return Ok(VerificationReport {
            outcome: VerificationOutcome::Interrupted,
            tier: None,
            manifest: None,
            replaced: vec![],
            added: vec![],
            conflicts: vec![],
            backup_compromised: false,
            messages: vec![
                "a previous apply was interrupted — run `locust patch-rollback` first".into(),
            ],
        });
    }

    let file = File::open(zip_path)?;
    let mut archive = ZipArchive::new(file).map_err(|e| {
        LocustError::PatchError(format!("cannot open patch zip {}: {e}", zip_path.display()))
    })?;

    let entries = scan_zip_entries(&mut archive)?;
    let manifest = load_manifest_from_entries(&entries)?;

    // Writability is part of verify (design step 3) — probe then clean up.
    check_writable(game_root)?;

    let receipt = store.read_receipt()?;
    let backup_ok = store.backup_manifest_valid();
    let backup_compromised = receipt.is_some() && !backup_ok && store.backup_dir().exists();

    match manifest {
        Some(m) => verify_with_manifest(game_root, &entries, m, receipt.as_ref(), backup_compromised),
        None => verify_legacy(game_root, &entries, backup_compromised),
    }
}

struct ZipEntryMeta {
    /// Original name in the archive (for error messages / unsafe-entry reports).
    #[allow(dead_code)]
    original: String,
    /// Normalized name.
    normalized: String,
    /// Safe relative path under game root, if this is a content file.
    rel: Option<PathBuf>,
    /// Raw bytes (small manifests; file content hashed for verify).
    data: Vec<u8>,
    is_dir: bool,
}

fn scan_zip_entries(archive: &mut ZipArchive<File>) -> Result<Vec<ZipEntryMeta>> {
    let mut out = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut total_bytes = 0u64;

    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| {
            LocustError::PatchError(format!("zip entry {i}: {e}"))
        })?;
        let original = entry.name().to_string();
        // Symlink entries (unix external attrs) — zip crate exposes is_symlink on ZipFile in v2.
        if entry.is_symlink() {
            return Err(LocustError::PatchUnsafeEntry(original));
        }
        let is_dir = entry.is_dir() || original.ends_with('/');
        let normalized = normalize_entry_name(&original);
        if is_dir {
            out.push(ZipEntryMeta {
                original,
                normalized,
                rel: None,
                data: vec![],
                is_dir: true,
            });
            continue;
        }
        // Skip the manifest file from the content plan later.
        let rel = if normalized == PatchManifest::FILENAME
            || normalized.eq_ignore_ascii_case("readme.txt")
        {
            None
        } else {
            Some(safe_entry_path(&normalized, &original)?)
        };

        if let Some(ref r) = rel {
            let key = case_fold_key(r);
            if !seen.insert(key) {
                return Err(LocustError::PatchUnsafeEntry(format!(
                    "case-insensitive duplicate path: {original}"
                )));
            }
        }

        // Declared uncompressed size — refuse zip-bombs before buffering (W4).
        total_bytes = check_entry_budget(&original, entry.size(), total_bytes)?;

        let mut data = Vec::new();
        entry
            .read_to_end(&mut data)
            .map_err(|e| LocustError::PatchError(format!("read {original}: {e}")))?;
        // Actual bytes can differ from the header; re-check and charge actual.
        if (data.len() as u64) > entry.size() {
            // Header understated size — re-budget with actual length from prior total.
            let prior = total_bytes.saturating_sub(entry.size());
            total_bytes = check_entry_budget(&original, data.len() as u64, prior)?;
        }

        out.push(ZipEntryMeta {
            original,
            normalized,
            rel,
            data,
            is_dir: false,
        });
    }
    Ok(out)
}

fn load_manifest_from_entries(entries: &[ZipEntryMeta]) -> Result<Option<PatchManifest>> {
    let Some(m) = entries
        .iter()
        .find(|e| e.normalized == PatchManifest::FILENAME)
    else {
        return Ok(None);
    };
    let parsed: PatchManifest = serde_json::from_slice(&m.data).map_err(|e| {
        LocustError::PatchError(format!("corrupt locust-patch.json: {e}"))
    })?;
    Ok(Some(parsed))
}

fn check_writable(game_root: &Path) -> Result<()> {
    if !game_root.is_dir() {
        return Err(LocustError::GameDirNotWritable(format!(
            "not a directory: {}",
            game_root.display()
        )));
    }
    let probe = game_root.join(format!(
        ".locust-probe-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    match fs::write(&probe, b"ok") {
        Ok(()) => {
            let _ = fs::remove_file(&probe);
            Ok(())
        }
        Err(e) => Err(LocustError::GameDirNotWritable(format!(
            "{} ({e})",
            game_root.display()
        ))),
    }
}

fn verify_with_manifest(
    game_root: &Path,
    entries: &[ZipEntryMeta],
    manifest: PatchManifest,
    receipt: Option<&Receipt>,
    backup_compromised: bool,
) -> Result<VerificationReport> {
    // Index zip content by normalized rel path for hash checks.
    let mut zip_by_path: HashMap<String, &ZipEntryMeta> = HashMap::new();
    for e in entries {
        if let Some(ref r) = e.rel {
            zip_by_path.insert(r.to_string_lossy().replace('\\', "/"), e);
        }
    }

    // Ensure every manifest path is present and safe in the zip.
    for f in &manifest.files {
        let n = normalize_entry_name(&f.path);
        let rel = safe_entry_path(&n, &f.path)?;
        let key = rel.to_string_lossy().replace('\\', "/");
        let Some(ze) = zip_by_path.get(&key) else {
            return Err(LocustError::PatchError(format!(
                "manifest lists {} but zip has no such entry",
                f.path
            )));
        };
        let hash = sha256_hex(&ze.data);
        if hash != f.patched_sha256 {
            return Err(LocustError::PatchError(format!(
                "zip content hash mismatch for {}: manifest says {}, zip has {}",
                f.path, f.patched_sha256, hash
            )));
        }
    }

    // Receipt version / id comparison (before content tier).
    if let Some(r) = receipt {
        if r.patch_id != manifest.patch_id {
            let msg = format!(
                "a different patch is installed (installed id {}, incoming id {})",
                r.patch_id, manifest.patch_id
            );
            return Ok(VerificationReport {
                outcome: VerificationOutcome::Mismatch(vec![FileMismatch {
                    path: "(patch_id)".into(),
                    expected: r.patch_id.clone(),
                    found: manifest.patch_id.clone(),
                }]),
                tier: Some(if manifest.supports_strict_tier() {
                    VerificationTier::Strict
                } else {
                    VerificationTier::Structural
                }),
                manifest: Some(manifest),
                replaced: vec![],
                added: vec![],
                conflicts: vec![],
                backup_compromised,
                messages: vec![msg],
            });
        }
        match compare_versions(&r.patch_version, &manifest.patch_version) {
            VersionOrder::Equal => {
                let msg = format!(
                    "patch {}@{} already applied",
                    r.patch_id, r.patch_version
                );
                return Ok(VerificationReport {
                    outcome: VerificationOutcome::AlreadyApplied,
                    tier: Some(if manifest.supports_strict_tier() {
                        VerificationTier::Strict
                    } else {
                        VerificationTier::Structural
                    }),
                    manifest: Some(manifest),
                    replaced: vec![],
                    added: vec![],
                    conflicts: vec![],
                    backup_compromised,
                    messages: vec![msg],
                });
            }
            VersionOrder::IncomingNewer => {
                let msg = format!(
                    "upgrade available: installed {}, incoming {}",
                    r.patch_version, manifest.patch_version
                );
                return Ok(VerificationReport {
                    outcome: VerificationOutcome::UpgradeAvailable,
                    tier: Some(if manifest.supports_strict_tier() {
                        VerificationTier::Strict
                    } else {
                        VerificationTier::Structural
                    }),
                    manifest: Some(manifest),
                    replaced: vec![],
                    added: vec![],
                    conflicts: vec![],
                    backup_compromised,
                    messages: vec![msg],
                });
            }
            VersionOrder::IncomingOlder | VersionOrder::Unorderable => {
                let msg = format!(
                    "downgrade/unorderable blocked: installed {}, incoming {}",
                    r.patch_version, manifest.patch_version
                );
                return Ok(VerificationReport {
                    outcome: VerificationOutcome::DowngradeBlocked {
                        installed: r.patch_version.clone(),
                        incoming: manifest.patch_version.clone(),
                    },
                    tier: Some(if manifest.supports_strict_tier() {
                        VerificationTier::Strict
                    } else {
                        VerificationTier::Structural
                    }),
                    manifest: Some(manifest),
                    replaced: vec![],
                    added: vec![],
                    conflicts: vec![],
                    backup_compromised,
                    messages: vec![msg],
                });
            }
        }
    }

    if manifest.supports_strict_tier() {
        verify_strict(game_root, &manifest, backup_compromised)
    } else {
        verify_structural(game_root, &manifest, backup_compromised)
    }
}

fn verify_strict(
    game_root: &Path,
    manifest: &PatchManifest,
    backup_compromised: bool,
) -> Result<VerificationReport> {
    let mut replaced = Vec::new();
    let mut added = Vec::new();
    let mut conflicts = Vec::new();
    let mut mismatches = Vec::new();
    let mut any_already = false;
    let mut any_clean = false;

    for f in &manifest.files {
        let target = game_root.join(f.path.replace('/', std::path::MAIN_SEPARATOR_STR));
        let original = f.original_sha256.as_deref().unwrap_or("");
        // Identity patch: translation equals source → original hash == patched hash.
        // A pristine game matching that hash is Clean, not AlreadyApplied/Unknown.
        let identity_patch = !original.is_empty() && original == f.patched_sha256;
        if target.is_file() {
            let hash = sha256_hex(&fs::read(&target)?);
            if hash == f.patched_sha256 {
                if identity_patch || hash == original {
                    any_clean = true;
                } else {
                    any_already = true;
                }
                // Plan as replaced/added based on original presence.
                if original.is_empty() {
                    added.push(f.path.clone());
                } else {
                    replaced.push(f.path.clone());
                }
            } else if !original.is_empty() && hash == original {
                any_clean = true;
                replaced.push(f.path.clone());
            } else if original.is_empty() {
                // Expected added, but something else is there.
                conflicts.push(f.path.clone());
            } else {
                mismatches.push(FileMismatch {
                    path: f.path.clone(),
                    expected: format!("original={original} or patched={}", f.patched_sha256),
                    found: hash,
                });
            }
        } else {
            // Absent.
            if original.is_empty() {
                any_clean = true;
                added.push(f.path.clone());
            } else {
                // Expected replaced file missing — mismatch / wrong game.
                mismatches.push(FileMismatch {
                    path: f.path.clone(),
                    expected: format!("present with original={original}"),
                    found: "absent".into(),
                });
            }
        }
    }

    let outcome = if !mismatches.is_empty() {
        VerificationOutcome::Mismatch(mismatches)
    } else if !conflicts.is_empty() {
        // Conflicts alone block by default (force reclassifies).
        VerificationOutcome::Mismatch(
            conflicts
                .iter()
                .map(|p| FileMismatch {
                    path: p.clone(),
                    expected: "absent (added path)".into(),
                    found: "present with unexpected content".into(),
                })
                .collect(),
        )
    } else if any_already && !any_clean {
        // All files look patched, none still match a distinct original.
        // Without a receipt (handled above) this is Unknown, never silent reapply.
        VerificationOutcome::Unknown
    } else if any_already && any_clean {
        // Mixed pristine + patched content without a receipt.
        VerificationOutcome::Unknown
    } else {
        VerificationOutcome::Clean
    };

    // Re-evaluate: if every existing file matches patched and none match
    // original, and we got here without a receipt, force Unknown.
    let all_look_patched = manifest.files.iter().all(|f| {
        let target = game_root.join(f.path.replace('/', std::path::MAIN_SEPARATOR_STR));
        if !target.is_file() {
            // absent added path is "clean" not patched-looking
            f.original_sha256.is_none()
        } else {
            fs::read(&target)
                .map(|b| sha256_hex(&b) == f.patched_sha256)
                .unwrap_or(false)
        }
    });
    let any_original = manifest.files.iter().any(|f| {
        let target = game_root.join(f.path.replace('/', std::path::MAIN_SEPARATOR_STR));
        let Some(orig) = f.original_sha256.as_ref() else {
            return false;
        };
        target.is_file()
            && fs::read(&target)
                .map(|b| sha256_hex(&b) == *orig)
                .unwrap_or(false)
    });
    let outcome = if all_look_patched && !any_original && !manifest.files.is_empty() {
        // If every path is an added path that is absent, that's Clean not Unknown.
        let all_absent_added = manifest.files.iter().all(|f| {
            f.original_sha256.is_none()
                && !game_root
                    .join(f.path.replace('/', std::path::MAIN_SEPARATOR_STR))
                    .is_file()
        });
        if all_absent_added {
            VerificationOutcome::Clean
        } else if any_original {
            outcome
        } else {
            // Files match patched hashes with no receipt.
            let any_present = manifest.files.iter().any(|f| {
                game_root
                    .join(f.path.replace('/', std::path::MAIN_SEPARATOR_STR))
                    .is_file()
            });
            if any_present {
                VerificationOutcome::Unknown
            } else {
                outcome
            }
        }
    } else {
        outcome
    };

    Ok(VerificationReport {
        outcome,
        tier: Some(VerificationTier::Strict),
        manifest: Some(manifest.clone()),
        replaced,
        added,
        conflicts,
        backup_compromised,
        messages: vec![],
    })
}

fn verify_structural(
    game_root: &Path,
    manifest: &PatchManifest,
    backup_compromised: bool,
) -> Result<VerificationReport> {
    let mut replaced = Vec::new();
    let mut added = Vec::new();
    let mut conflicts = Vec::new();
    let mut missing_replaced = Vec::new();

    for f in &manifest.files {
        let target = game_root.join(f.path.replace('/', std::path::MAIN_SEPARATOR_STR));
        if target.is_file() {
            let hash = sha256_hex(&fs::read(&target)?);
            if hash == f.patched_sha256 {
                // looks already applied
                replaced.push(f.path.clone());
            } else {
                // Existing non-patched file → treat as replace candidate.
                // Without original hashes we cannot detect wrong-game mods
                // beyond "exists"; conflicts for true-adds need heuristics:
                // structural treats existing as replaced unless we know it
                // was intended as add-only (no original AND we require
                // confirmation for structural overall).
                if f.original_sha256.is_none() {
                    // Structural: existing at intended-add path = conflict.
                    conflicts.push(f.path.clone());
                } else {
                    replaced.push(f.path.clone());
                }
            }
        } else {
            // Absent — if we expected a game file (majority case for RPG Maker
            // pure replacement, paths always existed), missing is a problem
            // for structural "every replaced path exists". Without originals
            // we treat all as "must exist" only when ≥1 other file exists —
            // design: every replaced path exists; added may be absent.
            // Structural without originals: classify absent as added, present
            // as replaced.
            added.push(f.path.clone());
            missing_replaced.push(f.path.clone());
        }
    }

    // Structural confirmation is required at apply time; verify reports Clean
    // when all present paths exist and no hard mismatches. If ALL files are
    // absent, this may be the wrong game → Mismatch.
    let all_absent = replaced.is_empty() && conflicts.is_empty() && !added.is_empty();
    let outcome = if all_absent && manifest.files.len() > 1 {
        VerificationOutcome::Mismatch(
            missing_replaced
                .into_iter()
                .map(|p| FileMismatch {
                    path: p,
                    expected: "present (structural tier)".into(),
                    found: "absent".into(),
                })
                .collect(),
        )
    } else if !conflicts.is_empty() {
        VerificationOutcome::Mismatch(
            conflicts
                .iter()
                .map(|p| FileMismatch {
                    path: p.clone(),
                    expected: "absent (added path)".into(),
                    found: "present".into(),
                })
                .collect(),
        )
    } else {
        VerificationOutcome::Clean
    };

    Ok(VerificationReport {
        outcome,
        tier: Some(VerificationTier::Structural),
        manifest: Some(manifest.clone()),
        replaced,
        added,
        conflicts,
        backup_compromised,
        messages: vec![
            "structural tier: no original hashes — apply requires confirmation".into(),
        ],
    })
}

fn verify_legacy(
    game_root: &Path,
    entries: &[ZipEntryMeta],
    backup_compromised: bool,
) -> Result<VerificationReport> {
    let content: Vec<_> = entries
        .iter()
        .filter(|e| !e.is_dir && e.rel.is_some())
        .collect();
    if content.is_empty() {
        return Err(LocustError::PatchError(
            "legacy patch zip contains no content files".into(),
        ));
    }
    let existing = content
        .iter()
        .filter(|e| {
            let rel = e.rel.as_ref().unwrap();
            game_root.join(rel).is_file()
        })
        .count();
    let ratio = existing as f64 / content.len() as f64;
    let mut replaced = Vec::new();
    let mut added = Vec::new();
    for e in &content {
        let rel = e.rel.as_ref().unwrap();
        let s = rel.to_string_lossy().replace('\\', "/");
        if game_root.join(rel).is_file() {
            replaced.push(s);
        } else {
            added.push(s);
        }
    }
    if ratio < 0.8 {
        return Ok(VerificationReport {
            outcome: VerificationOutcome::Mismatch(vec![FileMismatch {
                path: "(legacy heuristic)".into(),
                expected: "≥80% of entry paths exist on disk".into(),
                found: format!("{:.0}% exist", ratio * 100.0),
            }]),
            tier: Some(VerificationTier::Legacy),
            manifest: None,
            replaced,
            added,
            conflicts: vec![],
            backup_compromised,
            messages: vec![
                "legacy patch (no locust-patch.json) — apply requires --confirm-legacy".into(),
            ],
        });
    }
    Ok(VerificationReport {
        outcome: VerificationOutcome::Clean,
        tier: Some(VerificationTier::Legacy),
        manifest: None,
        replaced,
        added,
        conflicts: vec![],
        backup_compromised,
        messages: vec![
            "legacy patch (no locust-patch.json) — apply requires --confirm-legacy".into(),
        ],
    })
}

#[derive(Debug, PartialEq, Eq)]
enum VersionOrder {
    Equal,
    IncomingNewer,
    IncomingOlder,
    Unorderable,
}

fn compare_versions(installed: &str, incoming: &str) -> VersionOrder {
    if installed == incoming {
        return VersionOrder::Equal;
    }
    match (Version::parse(installed), Version::parse(incoming)) {
        (Ok(a), Ok(b)) => {
            if b > a {
                VersionOrder::IncomingNewer
            } else if b < a {
                VersionOrder::IncomingOlder
            } else {
                VersionOrder::Equal
            }
        }
        _ => VersionOrder::Unorderable,
    }
}


/// Build a synthetic plan classification from a report (used by apply).
pub fn classify_files(
    game_root: &Path,
    files: &[PatchFileEntry],
    force: bool,
) -> Result<(Vec<super::manifest::ReceiptReplaced>, Vec<super::manifest::ReceiptAdded>, Vec<String>)>
{
    use super::manifest::{ReceiptAdded, ReceiptReplaced};
    let mut replaced = Vec::new();
    let mut added = Vec::new();
    let user_edit_warnings = Vec::new();

    for f in files {
        let target = game_root.join(f.path.replace('/', std::path::MAIN_SEPARATOR_STR));
        if target.is_file() {
            let hash = sha256_hex(&fs::read(&target)?);
            if let Some(orig) = &f.original_sha256 {
                replaced.push(ReceiptReplaced {
                    path: f.path.clone(),
                    original_sha256: Some(orig.clone()),
                    patched_sha256: f.patched_sha256.clone(),
                });
                let _ = hash;
            } else if hash == f.patched_sha256 {
                // already patched added file
                added.push(ReceiptAdded {
                    path: f.path.clone(),
                    patched_sha256: f.patched_sha256.clone(),
                });
            } else if force {
                // Conflict → reclassify to replaced, backup current bytes.
                replaced.push(ReceiptReplaced {
                    path: f.path.clone(),
                    original_sha256: Some(hash),
                    patched_sha256: f.patched_sha256.clone(),
                });
            } else {
                return Err(LocustError::PatchVerificationFailed(format!(
                    "added-path conflict at {} (file exists with different content)",
                    f.path
                )));
            }
        } else if f.original_sha256.is_some() {
            // Expected to exist for replace — still plan as replaced (apply may fail later).
            replaced.push(ReceiptReplaced {
                path: f.path.clone(),
                original_sha256: f.original_sha256.clone(),
                patched_sha256: f.patched_sha256.clone(),
            });
        } else {
            added.push(ReceiptAdded {
                path: f.path.clone(),
                patched_sha256: f.patched_sha256.clone(),
            });
        }
    }
    let _ = user_edit_warnings;
    Ok((replaced, added, user_edit_warnings))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_order() {
        assert_eq!(compare_versions("1.0.0", "1.0.0"), VersionOrder::Equal);
        assert_eq!(
            compare_versions("1.0.0", "1.1.0"),
            VersionOrder::IncomingNewer
        );
        assert_eq!(
            compare_versions("2.0.0", "1.0.0"),
            VersionOrder::IncomingOlder
        );
        assert_eq!(
            compare_versions("not-semver", "also-not"),
            VersionOrder::Unorderable
        );
    }
}
