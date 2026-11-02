//! Read-only patch verification against a game tree.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use semver::Version;
use zip::ZipArchive;

use crate::database::sha256_path;
use crate::error::{LocustError, Result};

use super::manifest::{PatchFileEntry, PatchManifest, Receipt, VerificationTier};
use super::store::{PatchStatus, PatchStore};
use super::stream::{charge_declared, stream_and_hash, stream_hash_only, StagingDir};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerificationOutcome {
    Clean,
    AlreadyApplied,
    UpgradeAvailable,
    DowngradeBlocked { installed: String, incoming: String },
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
    /// Paths with conflicting content or an incompatible destination type.
    pub conflicts: Vec<String>,
    /// Warning when receipt exists but backup commit marker is missing.
    pub backup_compromised: bool,
    pub messages: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReceiptVerificationMode {
    /// Trust files whose modification time is no newer than the receipt.
    Quick,
    /// Hash every present file regardless of modification time.
    Full,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct ReceiptVerificationReport {
    /// Number of receipt entries inspected, including missing and trusted files.
    pub checked: usize,
    pub changed: Vec<String>,
    pub missing: Vec<String>,
}

/// Check installed files without acquiring GameLock or writing to the game.
/// Quick mode relies on apply publishing the receipt after writing the files;
/// full mode also detects edits that preserve or backdate modification times.
pub fn verify_receipt_files(
    store: &PatchStore,
    receipt: &Receipt,
    mode: ReceiptVerificationMode,
) -> Result<ReceiptVerificationReport> {
    let root = store.game_root().canonicalize()?;
    let receipt_modified = match mode {
        ReceiptVerificationMode::Quick => Some(store.receipt_path().metadata()?.modified()?),
        ReceiptVerificationMode::Full => None,
    };
    let files = receipt
        .replaced
        .iter()
        .map(|file| (&file.path, &file.patched_sha256))
        .chain(
            receipt
                .added
                .iter()
                .map(|file| (&file.path, &file.patched_sha256)),
        );
    let mut report = ReceiptVerificationReport::default();
    for (path, expected) in files {
        let relative = super::zipsec::safe_stored_rel(path)?;
        let target = root.join(relative);
        report.checked += 1;
        if !target.is_file() {
            report.missing.push(path.clone());
            continue;
        }
        if !target.canonicalize()?.starts_with(&root) {
            return Err(LocustError::PatchError(format!(
                "patch file resolves outside game folder: {path}"
            )));
        }
        if let Some(receipt_modified) = receipt_modified {
            if target.metadata()?.modified()? <= receipt_modified {
                continue;
            }
        }
        if sha256_path(&target)? != *expected {
            report.changed.push(path.clone());
        }
    }
    Ok(report)
}

/// Open the zip, scan security, parse optional manifest, compare to game.
pub fn verify(game_root: &Path, zip_path: &Path) -> Result<VerificationReport> {
    let game_lock = super::lock::GameLock::acquire(game_root)?;
    verify_with_lock(&game_lock, zip_path)
}

/// Verify while retaining an already acquired game lock, for example before
/// copying a game snapshot. The caller must keep this guard through its read
/// operation. Installation still performs its own fresh verification.
pub fn verify_with_lock(
    game_lock: &super::GameLock,
    zip_path: &Path,
) -> Result<VerificationReport> {
    let game_root = game_lock.root();
    super::zipsec::ensure_safe_store(game_root)?;
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

    let mut archive = open_archive(zip_path)?;

    let entries = scan_zip_entries(&mut archive, None)?;
    verify_scanned(game_root, &entries)
}

/// zip 2.x collapses exact duplicate central-directory names before exposing
/// `len`/`by_index`. Count the raw headers on the same open file, without loading
/// payloads or interpreting their compression, so hidden entries are rejected.
pub(super) fn open_archive(path: &Path) -> Result<ZipArchive<File>> {
    let mut raw = File::open(path)?;
    let archive = ZipArchive::new(raw.try_clone()?).map_err(|e| {
        LocustError::PatchError(format!("cannot open patch zip {}: {e}", path.display()))
    })?;
    let length = raw.metadata()?.len();
    let mut position = archive.central_directory_start();
    let mut count = 0usize;
    loop {
        raw.seek(SeekFrom::Start(position))?;
        let mut signature = [0; 4];
        raw.read_exact(&mut signature)?;
        if signature != [0x50, 0x4b, 0x01, 0x02] {
            break;
        }
        let mut header = [0; 42];
        raw.read_exact(&mut header)?;
        let variable = [24, 26, 28]
            .into_iter()
            .map(|i| u16::from_le_bytes([header[i], header[i + 1]]) as u64)
            .sum::<u64>();
        position = position
            .checked_add(46)
            .and_then(|p| p.checked_add(variable))
            .filter(|p| *p <= length)
            .ok_or_else(|| LocustError::PatchError("central-directory range past EOF".into()))?;
        count += 1;
        if count > archive.len() {
            return Err(LocustError::PatchUnsafeEntry(
                "duplicate or hidden central-directory entry".into(),
            ));
        }
    }
    if count != archive.len() {
        return Err(LocustError::PatchError(
            "central-directory entry count mismatch".into(),
        ));
    }
    Ok(archive)
}

/// Verify the exact archive snapshot already scanned/staged by apply.
pub(super) fn verify_scanned(
    game_root: &Path,
    entries: &[ZipEntryMeta],
) -> Result<VerificationReport> {
    super::zipsec::ensure_safe_store(game_root)?;
    let store = PatchStore::new(game_root);
    let manifest = load_manifest_from_entries(entries)?;
    let mut destinations = HashMap::new();
    for entry in entries {
        if let Some(rel) = &entry.rel {
            destinations.insert(
                entry.normalized.clone(),
                classify_destination(game_root, rel)?,
            );
        }
    }
    if let Some(manifest) = &manifest {
        for file in &manifest.files {
            let rel = super::zipsec::safe_stored_rel(&file.path)?;
            classify_destination(game_root, &rel)?;
        }
    }

    // Writability is part of verify (design step 3) — probe then clean up.
    check_writable(game_root)?;

    let receipt = store.read_receipt()?;
    let backup_ok = store.backup_manifest_valid();
    let backup_compromised = receipt.is_some() && !backup_ok && store.backup_dir().exists();

    let mut report = match manifest {
        Some(m) => verify_with_manifest(
            game_root,
            entries,
            m,
            receipt.as_ref(),
            backup_compromised,
            &destinations,
        ),
        None => verify_legacy(entries, backup_compromised, &destinations),
    }?;

    // Destination conflicts also override receipt/version shortcuts. Keep them
    // distinct from missing files and never include them in the install plan.
    let mut destination_mismatches: Vec<_> = destinations
        .values()
        .filter_map(|destination| match destination {
            Destination::Conflict(mismatch) => Some(mismatch.clone()),
            _ => None,
        })
        .collect();
    destination_mismatches.sort_by(|a, b| a.path.cmp(&b.path));
    if !destination_mismatches.is_empty() {
        report.conflicts.extend(
            destination_mismatches
                .iter()
                .map(|mismatch| mismatch.path.clone()),
        );
        match &mut report.outcome {
            VerificationOutcome::Mismatch(mismatches) => mismatches.extend(destination_mismatches),
            _ => report.outcome = VerificationOutcome::Mismatch(destination_mismatches),
        }
    }
    Ok(report)
}

enum Destination {
    File,
    Absent,
    Conflict(FileMismatch),
}

/// Inspect ancestors from the root outward so a file blocking a parent is not
/// mistaken for a missing leaf (or surfaced as an OS-specific traversal error).
fn classify_destination(game_root: &Path, rel: &Path) -> Result<Destination> {
    let mut ancestors: Vec<_> = rel
        .ancestors()
        .skip(1)
        .filter(|ancestor| !ancestor.as_os_str().is_empty())
        .collect();
    ancestors.reverse();
    for ancestor in ancestors {
        match std::fs::symlink_metadata(game_root.join(ancestor)) {
            Ok(metadata) if !metadata.is_dir() => {
                super::zipsec::ensure_no_links(game_root, ancestor)?;
                let ancestor = ancestor.to_string_lossy().replace('\\', "/");
                return Ok(Destination::Conflict(FileMismatch {
                    path: rel.to_string_lossy().replace('\\', "/"),
                    expected: format!("directory at ancestor {ancestor}"),
                    found: format!("ancestor {ancestor} exists but is not a directory"),
                }));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                super::zipsec::ensure_no_links(game_root, ancestor)?;
                return Ok(Destination::Absent);
            }
            Err(error) => return Err(error.into()),
        }
    }
    // Preserve the existing rejection of links/reparse points, including
    // dangling links. Installation retains its independent link/type checks.
    super::zipsec::ensure_no_links(game_root, rel)?;
    match std::fs::symlink_metadata(game_root.join(rel)) {
        Ok(metadata) if metadata.is_file() => {
            if metadata.permissions().readonly() {
                return Err(LocustError::GameDirNotWritable(format!(
                    "read-only file: {}",
                    game_root.join(rel).display()
                )));
            }
            Ok(Destination::File)
        }
        Ok(_) => Ok(Destination::Conflict(FileMismatch {
            path: rel.to_string_lossy().replace('\\', "/"),
            expected: "regular file or absent".into(),
            found: "exists but is not a regular file".into(),
        })),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Destination::Absent),
        Err(error) => Err(error.into()),
    }
}

/// Force can replace conflicting content, but cannot override path types or
/// read-only permissions. Check the whole plan before publishing recovery state
/// or mutating games, including before rollback transitions.
pub(super) fn ensure_file_destinations<'a>(
    game_root: &Path,
    paths: impl IntoIterator<Item = &'a str>,
) -> Result<()> {
    for path in paths {
        let rel = super::zipsec::safe_stored_rel(path)?;
        if let Destination::Conflict(mismatch) = classify_destination(game_root, &rel)? {
            return Err(LocustError::PatchVerificationFailed(format!(
                "{}: expected {}, found {}",
                mismatch.path, mismatch.expected, mismatch.found
            )));
        }
    }
    Ok(())
}

pub(super) struct ZipEntryMeta {
    /// Original name in the archive (for error messages / unsafe-entry reports).
    #[allow(dead_code)]
    original: String,
    /// Normalized name.
    normalized: String,
    /// Safe relative path under game root, if this is a content file.
    pub(super) rel: Option<PathBuf>,
    /// SHA-256 of entry payload (streamed; not kept in RAM for content files).
    pub(super) content_sha256: String,
    pub(super) actual_len: u64,
    pub(super) staged_path: Option<PathBuf>,
    /// Small meta payloads only (manifest / readme). Empty for content files.
    meta_data: Vec<u8>,
    is_dir: bool,
}

/// Meta entries (manifest, readme) stay in RAM but still stream through the
/// same bomb checks. Cap so a malicious "manifest" cannot fill memory.
const MAX_META_BUFFER_BYTES: u64 = 16 * 1024 * 1024;

pub(super) fn scan_zip_entries(
    archive: &mut ZipArchive<File>,
    staging: Option<&StagingDir>,
) -> Result<Vec<ZipEntryMeta>> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let mut total_bytes = 0u64;
    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| LocustError::PatchError(format!("zip entry {i}: {e}")))?;
        let original = entry.name().to_string();
        if entry.is_symlink() {
            return Err(LocustError::PatchUnsafeEntry(original));
        }
        let normalized = super::zipsec::canonical_archive_name(&original)?;
        let key = super::zipsec::case_fold_key(Path::new(&normalized));
        // Metadata and directory entries share the same collision registry as
        // payloads. Never let verify and apply select different manifests.
        if !seen.insert(key.clone()) {
            return Err(LocustError::PatchUnsafeEntry(format!(
                "case-insensitive duplicate path (including metadata): {original}"
            )));
        }
        let is_dir = entry.is_dir();
        let is_meta = key == PatchManifest::FILENAME || key == "readme.txt";
        if is_dir {
            if is_meta {
                return Err(LocustError::PatchUnsafeEntry(format!(
                    "metadata entry is a directory: {original}"
                )));
            }
            out.push(ZipEntryMeta {
                original,
                normalized,
                rel: None,
                content_sha256: String::new(),
                actual_len: 0,
                staged_path: None,
                meta_data: vec![],
                is_dir: true,
            });
            continue;
        }
        let declared = entry.size();
        total_bytes = charge_declared(&original, declared, total_bytes)?;
        let mut meta_data = Vec::new();
        let mut staged_path = None;
        let streamed = if is_meta {
            if declared > MAX_META_BUFFER_BYTES {
                return Err(LocustError::PatchError(format!(
                    "zip meta entry {original} exceeds {MAX_META_BUFFER_BYTES} byte limit"
                )));
            }
            stream_and_hash(&mut entry, declared, &original, Some(&mut meta_data))?
        } else if let Some(staging) = staging {
            let path = staging.child(&format!("e{i:08}"));
            let mut file = staging.create_file(&format!("e{i:08}"))?;
            let streamed = stream_and_hash(&mut entry, declared, &original, Some(&mut file))?;
            file.sync_all()?;
            staged_path = Some(path);
            streamed
        } else {
            stream_hash_only(&mut entry, declared, &original)?
        };
        if streamed.actual_len != declared {
            return Err(LocustError::PatchError(format!(
                "zip entry actual length differs from declared size for {original}"
            )));
        }
        let rel = if is_meta {
            None
        } else {
            Some(PathBuf::from(&normalized))
        };
        out.push(ZipEntryMeta {
            original,
            normalized: if is_meta { key } else { normalized },
            rel,
            content_sha256: streamed.sha256_hex,
            actual_len: streamed.actual_len,
            staged_path,
            meta_data,
            is_dir: false,
        });
    }
    // A file cannot also be a directory required by another planned path.
    // Index the complete namespace first so archive order cannot affect safety.
    let planned_files: HashMap<_, _> = out
        .iter()
        .filter(|entry| entry.rel.is_some())
        .map(|entry| {
            (
                super::zipsec::case_fold_key(Path::new(&entry.normalized)),
                entry.normalized.as_str(),
            )
        })
        .collect();
    for entry in &out {
        for ancestor in Path::new(&entry.normalized).ancestors().skip(1) {
            if let Some(file) = planned_files.get(&super::zipsec::case_fold_key(ancestor)) {
                return Err(LocustError::PatchUnsafeEntry(format!(
                    "planned file {file} is an ancestor of planned path {}",
                    entry.normalized
                )));
            }
        }
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
    let parsed: PatchManifest = serde_json::from_slice(&m.meta_data)
        .map_err(|e| LocustError::PatchError(format!("corrupt locust-patch.json: {e}")))?;
    Ok(Some(parsed))
}

fn check_writable(game_root: &Path) -> Result<()> {
    if !game_root.is_dir() {
        return Err(LocustError::GameDirNotWritable(format!(
            "not a directory: {}",
            game_root.display()
        )));
    }
    let probe = (|| -> Result<()> {
        let staging = StagingDir::create_prepared(game_root)?;
        staging.create_file("probe")?;
        Ok(())
    })();
    probe.map_err(|e| LocustError::GameDirNotWritable(format!("{} ({e})", game_root.display())))
}

fn verify_with_manifest(
    game_root: &Path,
    entries: &[ZipEntryMeta],
    mut manifest: PatchManifest,
    receipt: Option<&Receipt>,
    backup_compromised: bool,
    destinations: &HashMap<String, Destination>,
) -> Result<VerificationReport> {
    // Index zip content by normalized rel path for hash checks.
    let mut zip_by_path: HashMap<String, &ZipEntryMeta> = HashMap::new();
    for e in entries {
        if let Some(ref r) = e.rel {
            zip_by_path.insert(r.to_string_lossy().replace('\\', "/"), e);
        }
    }

    if manifest.schema_version != PatchManifest::SCHEMA_VERSION {
        return Err(LocustError::PatchError(format!(
            "unsupported patch manifest schema {}",
            manifest.schema_version
        )));
    }
    let mut listed = HashSet::new();
    // Normalize once, then keep precisely this plan for verification, apply,
    // journal and receipt. Every regular payload must be listed exactly once.
    for f in &mut manifest.files {
        let key = super::zipsec::canonical_archive_name(&f.path)?;
        if !listed.insert(key.to_lowercase()) {
            return Err(LocustError::PatchUnsafeEntry(format!(
                "duplicate manifest target: {}",
                f.path
            )));
        }
        let Some(ze) = zip_by_path.get(&key) else {
            return Err(LocustError::PatchError(format!(
                "manifest lists {} but zip has no such content entry",
                f.path
            )));
        };
        if ze.content_sha256 != f.patched_sha256 || ze.actual_len != f.size {
            return Err(LocustError::PatchError(format!(
                "zip content hash or size mismatch for {}",
                f.path
            )));
        }
        f.path = key;
    }
    if zip_by_path
        .keys()
        .any(|key| !listed.contains(&key.to_lowercase()))
    {
        return Err(LocustError::PatchError(
            "zip contains an unlisted regular payload".into(),
        ));
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
                let msg = format!("patch {}@{} already applied", r.patch_id, r.patch_version);
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
        verify_strict(game_root, &manifest, backup_compromised, destinations)
    } else {
        verify_structural(game_root, &manifest, backup_compromised, destinations)
    }
}

fn verify_strict(
    game_root: &Path,
    manifest: &PatchManifest,
    backup_compromised: bool,
    destinations: &HashMap<String, Destination>,
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
        let present = match &destinations[&f.path] {
            Destination::File => true,
            Destination::Absent => false,
            Destination::Conflict(_) => continue,
        };
        if present {
            let hash = sha256_path(&target)?;
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
    destinations: &HashMap<String, Destination>,
) -> Result<VerificationReport> {
    let mut replaced = Vec::new();
    let mut added = Vec::new();
    let mut conflicts = Vec::new();
    let mut missing_replaced = Vec::new();

    for f in &manifest.files {
        let target = game_root.join(f.path.replace('/', std::path::MAIN_SEPARATOR_STR));
        let present = match &destinations[&f.path] {
            Destination::File => true,
            Destination::Absent => false,
            Destination::Conflict(_) => continue,
        };
        if present {
            let hash = sha256_path(&target)?;
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
            // A path the packager knew to be new is expected to be absent;
            // it never counts toward the wrong-game heuristic below.
            if !f.added {
                missing_replaced.push(f.path.clone());
            }
        }
    }

    // Structural confirmation is required at apply time; verify reports Clean
    // when all present paths exist and no hard mismatches. If ALL files are
    // absent, this may be the wrong game → Mismatch.
    let all_absent = replaced.is_empty() && conflicts.is_empty() && !missing_replaced.is_empty();
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
        messages: vec!["structural tier: no original hashes — apply requires confirmation".into()],
    })
}

fn verify_legacy(
    entries: &[ZipEntryMeta],
    backup_compromised: bool,
    destinations: &HashMap<String, Destination>,
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
        .filter(|e| matches!(destinations[&e.normalized], Destination::File))
        .count();
    let ratio = existing as f64 / content.len() as f64;
    let mut replaced = Vec::new();
    let mut added = Vec::new();
    for e in &content {
        let rel = e.rel.as_ref().unwrap();
        let s = rel.to_string_lossy().replace('\\', "/");
        match &destinations[&e.normalized] {
            Destination::File => replaced.push(s),
            Destination::Absent => added.push(s),
            Destination::Conflict(_) => {}
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
) -> Result<(
    Vec<super::manifest::ReceiptReplaced>,
    Vec<super::manifest::ReceiptAdded>,
    Vec<String>,
)> {
    let (replaced, added, warnings, _) = classify_files_with_hashes(game_root, files, force)?;
    Ok((replaced, added, warnings))
}

type ClassifiedFilesWithHashes = (
    Vec<super::manifest::ReceiptReplaced>,
    Vec<super::manifest::ReceiptAdded>,
    Vec<String>,
    HashMap<String, String>,
);

/// Also retain hashes read during classification for reconciliation under the apply lock.
pub(super) fn classify_files_with_hashes(
    game_root: &Path,
    files: &[PatchFileEntry],
    force: bool,
) -> Result<ClassifiedFilesWithHashes> {
    use super::manifest::{ReceiptAdded, ReceiptReplaced};
    let mut replaced = Vec::new();
    let mut added = Vec::new();
    let user_edit_warnings = Vec::new();
    let mut current_hashes = HashMap::new();

    for f in files {
        let target = game_root.join(f.path.replace('/', std::path::MAIN_SEPARATOR_STR));
        if target.is_file() {
            if let Some(orig) = &f.original_sha256 {
                replaced.push(ReceiptReplaced {
                    path: f.path.clone(),
                    original_sha256: Some(orig.clone()),
                    patched_sha256: f.patched_sha256.clone(),
                });
            } else {
                let hash = sha256_path(&target)?;
                current_hashes.insert(f.path.clone(), hash.clone());
                if hash == f.patched_sha256 {
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
    Ok((replaced, added, user_edit_warnings, current_hashes))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn receipt_fixture() -> (tempfile::TempDir, PatchStore, Receipt) {
        let game = tempfile::tempdir().unwrap();
        let store = PatchStore::new(game.path());
        std::fs::write(game.path().join("replaced.txt"), b"patched").unwrap();
        std::fs::write(game.path().join("added.txt"), b"added").unwrap();
        let receipt = serde_json::from_value(serde_json::json!({
            "schema_version": 1, "patch_id": "fixture", "patch_version": "1",
            "generator_version": "test", "language": "es", "engine": "html",
            "applied_at": "fixture", "verification": "strict", "forced": false,
            "baseline": "pristine", "created_dirs": [],
            "replaced": [{"path": "replaced.txt", "patched_sha256": crate::database::sha256_hex(b"patched")}],
            "added": [{"path": "added.txt", "patched_sha256": crate::database::sha256_hex(b"added")}]
        }))
        .unwrap();
        store.write_receipt(&receipt).unwrap();
        (game, store, receipt)
    }

    #[test]
    fn receipt_quick_trusts_old_and_equal_mtime_but_full_detects_changes() {
        let (game, store, receipt) = receipt_fixture();
        let receipt_modified = store.receipt_path().metadata().unwrap().modified().unwrap();
        let target = game.path().join("replaced.txt");
        std::fs::write(&target, b"user edit").unwrap();
        let file = File::options().write(true).open(&target).unwrap();
        for modified in [
            receipt_modified - std::time::Duration::from_secs(2),
            receipt_modified,
        ] {
            file.set_modified(modified).unwrap();
            let quick =
                verify_receipt_files(&store, &receipt, ReceiptVerificationMode::Quick).unwrap();
            assert_eq!(quick.checked, 2);
            assert!(quick.changed.is_empty(), "old edits must remain trusted");
            assert!(quick.missing.is_empty());
            let full =
                verify_receipt_files(&store, &receipt, ReceiptVerificationMode::Full).unwrap();
            assert_eq!(full.checked, 2);
            assert_eq!(full.changed, ["replaced.txt"]);
            assert!(full.missing.is_empty());
        }
        // Full verification does not depend on a receipt file on disk.
        std::fs::remove_file(store.receipt_path()).unwrap();
        assert_eq!(
            verify_receipt_files(&store, &receipt, ReceiptVerificationMode::Full)
                .unwrap()
                .changed,
            ["replaced.txt"]
        );
    }

    #[test]
    fn receipt_quick_detects_newer_changes_and_missing_files() {
        let (game, store, receipt) = receipt_fixture();
        let receipt_modified = store.receipt_path().metadata().unwrap().modified().unwrap();
        let target = game.path().join("replaced.txt");
        std::fs::write(&target, b"user edit").unwrap();
        File::options()
            .write(true)
            .open(&target)
            .unwrap()
            .set_modified(receipt_modified + std::time::Duration::from_secs(2))
            .unwrap();
        std::fs::remove_file(game.path().join("added.txt")).unwrap();
        let report =
            verify_receipt_files(&store, &receipt, ReceiptVerificationMode::Quick).unwrap();
        assert_eq!(report.checked, 2);
        assert_eq!(report.changed, ["replaced.txt"]);
        assert_eq!(report.missing, ["added.txt"]);
    }

    #[test]
    fn receipt_verification_rejects_unsafe_paths_in_both_modes() {
        let (_game, store, mut receipt) = receipt_fixture();
        receipt.replaced[0].path = "../outside.txt".into();
        for mode in [
            ReceiptVerificationMode::Quick,
            ReceiptVerificationMode::Full,
        ] {
            assert!(verify_receipt_files(&store, &receipt, mode).is_err());
        }
    }

    #[test]
    fn classify_files_hashes_only_existing_added_paths() {
        let game = tempfile::tempdir().unwrap();
        std::fs::write(game.path().join("replaced.rpy"), b"original").unwrap();
        std::fs::write(game.path().join("added.rpy"), b"patched").unwrap();
        let files = vec![
            PatchFileEntry {
                added: false,
                path: "replaced.rpy".into(),
                patched_sha256: crate::database::sha256_hex(b"translated"),
                size: 10,
                original_sha256: Some(crate::database::sha256_hex(b"original")),
            },
            PatchFileEntry {
                added: false,
                path: "added.rpy".into(),
                patched_sha256: crate::database::sha256_hex(b"patched"),
                size: 7,
                original_sha256: None,
            },
            PatchFileEntry {
                added: false,
                path: "missing-replaced.rpy".into(),
                patched_sha256: crate::database::sha256_hex(b"translated"),
                size: 10,
                original_sha256: Some(crate::database::sha256_hex(b"original")),
            },
            PatchFileEntry {
                added: false,
                path: "missing-added.rpy".into(),
                patched_sha256: crate::database::sha256_hex(b"patched"),
                size: 7,
                original_sha256: None,
            },
        ];

        crate::database::SHA256_PATH_CALLS.with(|calls| calls.set(Some(0)));
        let (replaced, added, warnings, current_hashes) =
            classify_files_with_hashes(game.path(), &files, false).unwrap();
        let hashes = crate::database::SHA256_PATH_CALLS.with(|calls| calls.replace(None));

        assert_eq!(replaced.len(), 2);
        assert_eq!(added.len(), 2);
        assert!(warnings.is_empty());
        assert_eq!(
            current_hashes,
            HashMap::from([("added.rpy".into(), crate::database::sha256_hex(b"patched"))])
        );
        assert_eq!(
            hashes,
            Some(1),
            "only the existing added path needs hashing"
        );
        assert_eq!(
            classify_files(game.path(), &files, false).unwrap(),
            (replaced, added, warnings)
        );
    }

    #[test]
    fn classify_files_with_hashes_retains_forced_conflict_hash() {
        let game = tempfile::tempdir().unwrap();
        std::fs::create_dir(game.path().join("nested")).unwrap();
        std::fs::write(game.path().join("nested/added.pak"), b"user edit").unwrap();
        let files = [PatchFileEntry {
            added: false,
            path: "nested/added.pak".into(),
            patched_sha256: crate::database::sha256_hex(b"patched"),
            size: 7,
            original_sha256: None,
        }];

        let (replaced, added, warnings, current_hashes) =
            classify_files_with_hashes(game.path(), &files, true).unwrap();
        let edited_hash = crate::database::sha256_hex(b"user edit");
        assert_eq!(
            current_hashes,
            HashMap::from([("nested/added.pak".into(), edited_hash.clone())])
        );
        assert_eq!(replaced.len(), 1);
        assert_eq!(
            replaced[0].original_sha256.as_deref(),
            Some(edited_hash.as_str())
        );
        assert!(added.is_empty());
        assert!(warnings.is_empty());
        assert_eq!(
            classify_files(game.path(), &files, true).unwrap(),
            (replaced, added, warnings)
        );
        assert!(matches!(
            classify_files(game.path(), &files, false),
            Err(LocustError::PatchVerificationFailed(_))
        ));
        assert!(matches!(
            classify_files_with_hashes(game.path(), &files, false),
            Err(LocustError::PatchVerificationFailed(_))
        ));
    }

    #[test]
    fn strict_verify_hashes_each_existing_file_once() {
        let game = tempfile::tempdir().unwrap();
        let original = b"original";
        let patched = b"patched";
        std::fs::write(game.path().join("script.rpy"), patched).unwrap();
        let manifest = PatchManifest {
            game: None,
            schema_version: PatchManifest::SCHEMA_VERSION,
            patch_id: "test".into(),
            game_name: "test".into(),
            engine: "renpy".into(),
            language: "es".into(),
            patch_version: "1.0.0".into(),
            generator_version: "test".into(),
            created_at: "test".into(),
            files: vec![PatchFileEntry {
                added: false,
                path: "script.rpy".into(),
                patched_sha256: crate::database::sha256_hex(patched),
                size: patched.len() as u64,
                original_sha256: Some(crate::database::sha256_hex(original)),
            }],
        };

        crate::database::SHA256_PATH_CALLS.with(|calls| calls.set(Some(0)));
        let destinations = HashMap::from([(
            "script.rpy".into(),
            classify_destination(game.path(), Path::new("script.rpy")).unwrap(),
        )]);
        let report = verify_strict(game.path(), &manifest, false, &destinations).unwrap();
        let hashes = crate::database::SHA256_PATH_CALLS.with(|calls| calls.replace(None));

        assert_eq!(report.outcome, VerificationOutcome::Unknown);
        assert_eq!(hashes, Some(1), "one game-file hash per strict verify");
    }

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
