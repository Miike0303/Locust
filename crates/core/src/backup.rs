use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Component, Path, PathBuf};

use crate::database::sha256_file;
use crate::error::{LocustError, Result};
use crate::patch::GameLock;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use walkdir::WalkDir;

pub struct BackupManager {
    backup_root: PathBuf,
}

/// An original file with an immutable expected digest and size. Readers get
/// only bytes checked against that snapshot, never an unchecked backup path.
#[derive(Clone, Debug)]
pub struct RevisionOriginal {
    path: PathBuf,
    sha256: String,
    size: u64,
}

impl RevisionOriginal {
    /// Capture a standalone original. Direct injection instead uses the
    /// already-verified backup manifest, without a second snapshot scan.
    pub fn capture(path: &Path) -> Result<Self> {
        let path = checked_absolute(path)?;
        let (sha256, size) = sha256_file(&path)?;
        Ok(Self { path, sha256, size })
    }

    pub fn read_text(&self) -> Result<String> {
        let path = checked_absolute(&self.path)?;
        let mut file = fs::File::open(&path)?;
        if !file.metadata()?.is_file() {
            return Err(failure("revision original is not a regular file"));
        }
        self.read_text_from(&mut file)
    }

    fn read_text_from(&self, reader: &mut dyn std::io::Read) -> Result<String> {
        let mut bytes = Vec::new();
        let read = crate::patch::stream::stream_bounded(reader, self.size, Some(&mut bytes))
            .map_err(|error| {
                failure(format!(
                    "revision original changed or could not be read ({}): {error:?}",
                    self.path.display()
                ))
            })?;
        if read.actual_len != self.size || read.sha256_hex != self.sha256 {
            return Err(failure(format!(
                "revision original changed since verification: {}",
                self.path.display()
            )));
        }
        String::from_utf8(bytes).map_err(|error| {
            failure(format!(
                "revision original is not UTF-8 ({}): {error}",
                self.path.display()
            ))
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupEntry {
    pub id: String,
    pub path: PathBuf,
    pub created_at: DateTime<Utc>,
    pub source_path: PathBuf,
    pub file_count: usize,
    pub size_bytes: u64,
}

#[derive(Serialize, Deserialize)]
pub struct BackupManifest {
    pub source_path: PathBuf,
    pub created_at: DateTime<Utc>,
    pub file_count: usize,
    pub size_bytes: u64,
}

/// V2 keeps metadata at manifest.json and all game content under payload/.
/// The manifest is the completion marker and is committed only after validation.
#[derive(Serialize, Deserialize)]
struct ManifestV2 {
    version: u32,
    #[serde(flatten)]
    summary: BackupManifest,
    source_kind: SourceKind,
    inventory: BTreeMap<PathBuf, InventoryEntry>,
}

#[derive(Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum SourceKind {
    Directory,
    File,
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum InventoryEntry {
    Directory,
    File { size: u64, sha256: String },
}

/// A verified inventory retained while a consumer reads the pristine tree.
/// Paths can still change externally; consumers must compare the hashes they
/// use with this snapshot before publishing results based on original files.
pub(crate) struct VerifiedPristine<'a> {
    root: &'a Path,
    inventory: &'a BTreeMap<PathBuf, InventoryEntry>,
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    aliases: std::cell::OnceCell<BTreeMap<String, Option<&'a InventoryEntry>>>,
}

impl<'a> VerifiedPristine<'a> {
    fn new(root: &'a Path, inventory: &'a BTreeMap<PathBuf, InventoryEntry>) -> Self {
        Self {
            root,
            inventory,
            #[cfg(any(target_os = "windows", target_os = "macos"))]
            aliases: std::cell::OnceCell::new(),
        }
    }

    pub(crate) fn root(&self) -> &Path {
        self.root
    }

    pub(crate) fn original_sha256(&self, relative: &Path) -> Result<Option<&str>> {
        Ok(self.original_metadata(relative)?.map(|(sha256, _)| sha256))
    }

    pub(crate) fn revision_original(&self, relative: &Path) -> Result<Option<RevisionOriginal>> {
        Ok(self
            .original_metadata(relative)?
            .map(|(sha256, size)| RevisionOriginal {
                path: self.root.join(relative),
                sha256: sha256.to_owned(),
                size,
            }))
    }

    fn original_metadata(&self, relative: &Path) -> Result<Option<(&str, u64)>> {
        plain_relative(relative)?;
        let entry = self.inventory.get(relative);
        #[cfg(any(target_os = "windows", target_os = "macos"))]
        let entry = match entry {
            Some(entry) => Some(entry),
            None => {
                // Use only the saved inventory, never canonicalize a live path
                // here: a now-missing case alias must still name its original.
                // Build once on demand; generated outputs must not each scan
                // every original file in a large game backup.
                let key = |p: &Path| {
                    crate::database::fold_path_case(&p.to_string_lossy().replace('\\', "/"))
                };
                let aliases = self.aliases.get_or_init(|| {
                    let mut aliases = BTreeMap::new();
                    for (path, entry) in self.inventory {
                        aliases
                            .entry(key(path))
                            .and_modify(|value| *value = None)
                            .or_insert(Some(entry));
                    }
                    aliases
                });
                match aliases.get(&key(relative)) {
                    Some(Some(entry)) => Some(*entry),
                    None => None,
                    Some(None) => {
                        return Err(failure(format!(
                            "ambiguous original backup path: {}",
                            relative.display()
                        )))
                    }
                }
            }
        };
        match entry {
            Some(InventoryEntry::File { sha256, size }) => Ok(Some((sha256, *size))),
            None => Ok(None),
            Some(InventoryEntry::Directory) => Err(failure(format!(
                "original backup path is a directory: {}",
                relative.display()
            ))),
        }
    }
}

fn failure(message: impl Into<String>) -> LocustError {
    LocustError::BackupError(message.into())
}

fn reject_link(path: &Path, metadata: &fs::Metadata) -> Result<()> {
    #[cfg(windows)]
    let reparse = {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    };
    #[cfg(not(windows))]
    let reparse = false;
    let linked = metadata.file_type().is_symlink() || reparse;
    if linked || (!metadata.is_file() && !metadata.is_dir()) {
        return Err(failure(format!(
            "unsupported link/reparse/special path: {}",
            path.display()
        )));
    }
    Ok(())
}

/// Check every existing ancestor, including dangling links. Never canonicalize
/// first: that would conceal a link supplied by a caller or edited manifest.
fn checked_absolute(path: &Path) -> Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut current = PathBuf::new();
    for component in absolute.components() {
        if let Component::Normal(name) = component {
            plain_relative(Path::new(name))?;
        }
        if matches!(component, Component::ParentDir) {
            return Err(failure("parent traversal is not supported in backup paths"));
        }
        current.push(component);
        if matches!(component, Component::Prefix(_)) {
            continue;
        }
        match fs::symlink_metadata(&current) {
            Ok(meta) => reject_link(&current, &meta)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(absolute)
}

fn plain_relative(path: &Path) -> Result<()> {
    if path.as_os_str().is_empty() || !path.components().all(|c| matches!(c, Component::Normal(_)))
    {
        return Err(failure("invalid relative path in backup inventory"));
    }
    // Reject aliases/streams when reading a backup made on another platform.
    for component in path.components() {
        let name = component
            .as_os_str()
            .to_str()
            .ok_or_else(|| failure("non-UTF8 backup path"))?;
        if name.contains([':', '\\']) || name.ends_with(['.', ' ']) {
            return Err(failure("unsupported backup path alias"));
        }
    }
    Ok(())
}

fn scan(root: &Path, legacy: bool) -> Result<BTreeMap<PathBuf, InventoryEntry>> {
    scan_inner(root, legacy, false)
}

fn scan_source(root: &Path) -> Result<BTreeMap<PathBuf, InventoryEntry>> {
    let exclude_store = crate::injection_transaction::validate_store(root)?;
    scan_inner(root, false, exclude_store)
}

fn scan_inner(
    root: &Path,
    legacy: bool,
    exclude_store: bool,
) -> Result<BTreeMap<PathBuf, InventoryEntry>> {
    checked_absolute(root)?;
    if !root.is_dir() {
        return Err(failure("backup payload is not a directory"));
    }
    let mut inventory = BTreeMap::new();
    for entry in WalkDir::new(root)
        .follow_links(false)
        .min_depth(1)
        .into_iter()
        .filter_entry(|entry| {
            !(exclude_store
                && entry.depth() == 1
                && entry
                    .file_name()
                    .to_string_lossy()
                    .eq_ignore_ascii_case(crate::injection_transaction::STORE_DIR))
        })
    {
        let entry = entry.map_err(|e| failure(e.to_string()))?;
        let rel = entry
            .path()
            .strip_prefix(root)
            .map_err(|e| failure(e.to_string()))?
            .to_owned();
        reject_link(entry.path(), &fs::symlink_metadata(entry.path())?)?;
        if legacy && rel == Path::new("manifest.json") {
            continue;
        }
        plain_relative(&rel)?;
        let value = if entry.file_type().is_dir() {
            InventoryEntry::Directory
        } else {
            let (sha256, size) = sha256_file(entry.path())?;
            InventoryEntry::File { size, sha256 }
        };
        inventory.insert(rel, value);
    }
    Ok(inventory)
}

fn totals(inventory: &BTreeMap<PathBuf, InventoryEntry>) -> Result<(usize, u64)> {
    let mut count = 0;
    let mut total = 0u64;
    for entry in inventory.values() {
        if let InventoryEntry::File { size, .. } = entry {
            count += 1;
            total = total
                .checked_add(*size)
                .ok_or_else(|| failure("backup size overflow"))?;
        }
    }
    Ok((count, total))
}

impl BackupManager {
    /// Storage tree protected from patch output publication.
    pub fn root(&self) -> &Path {
        &self.backup_root
    }

    pub fn new(backup_root: PathBuf) -> Self {
        Self { backup_root }
    }

    fn claim_backup_dir(&self, now: DateTime<Utc>) -> Result<(String, PathBuf)> {
        checked_absolute(&self.backup_root)?;
        fs::create_dir_all(&self.backup_root)?;
        let base = now.format("%Y%m%d_%H%M%S").to_string();
        // Retention may hold an old listing while another process deletes an
        // entry. Never reuse an id after deletion: a stale retention plan must
        // not remove a new writer's directory with the same timestamp (ABA).
        for _ in 0..1000 {
            let id = format!("{base}_{}", uuid::Uuid::new_v4());
            let dir = self.backup_root.join(&id);
            match fs::create_dir(&dir) {
                Ok(()) => return Ok((id, dir)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e.into()),
            }
        }
        Err(failure("could not reserve a backup directory"))
    }

    pub fn create_backup(&self, game_path: &Path) -> Result<BackupEntry> {
        let source = checked_absolute(game_path)?.canonicalize()?;
        let kind = if source.is_dir() {
            SourceKind::Directory
        } else if source.is_file() {
            SourceKind::File
        } else {
            return Err(failure("backup source must be a regular file or directory"));
        };
        let backup_root = checked_absolute(&self.backup_root)?;
        // Resolve the nearest existing ancestor so spelling/case aliases cannot
        // hide storage nested under the source before create_dir_all runs.
        let mut ancestor = backup_root.as_path();
        let mut tail = Vec::new();
        while !ancestor.exists() {
            tail.push(
                ancestor
                    .file_name()
                    .ok_or_else(|| failure("invalid backup root"))?
                    .to_owned(),
            );
            ancestor = ancestor
                .parent()
                .ok_or_else(|| failure("invalid backup root"))?;
        }
        let mut resolved_root = ancestor.canonicalize()?;
        for part in tail.iter().rev() {
            resolved_root.push(part);
        }
        if resolved_root.starts_with(&source) || source.starts_with(&resolved_root) {
            return Err(failure("backup storage and source must not overlap"));
        }
        let inventory = if kind == SourceKind::Directory {
            scan_source(&source)?
        } else {
            let (sha256, size) = sha256_file(&source)?;
            BTreeMap::from([(PathBuf::from("file"), InventoryEntry::File { size, sha256 })])
        };
        let (file_count, size_bytes) = totals(&inventory)?;
        let now = Utc::now();
        let (id, path) = self.claim_backup_dir(now)?;
        let payload = path.join("payload");
        fs::create_dir(&payload)?;
        for (rel, entry) in &inventory {
            let dest = payload.join(rel);
            match entry {
                InventoryEntry::Directory => fs::create_dir_all(&dest)?,
                InventoryEntry::File { .. } => {
                    if let Some(parent) = dest.parent() {
                        fs::create_dir_all(parent)?;
                    }
                    let src = if kind == SourceKind::File {
                        source.clone()
                    } else {
                        source.join(rel)
                    };
                    checked_absolute(&src)?;
                    let mut input = fs::File::open(src)?;
                    let permissions = input.metadata()?.permissions();
                    let mut output = OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(&dest)?;
                    std::io::copy(&mut input, &mut output)?;
                    output.sync_all()?;
                    drop(output);
                    fs::set_permissions(&dest, permissions)?;
                }
            }
        }
        let source_after = if kind == SourceKind::Directory {
            scan_source(&source)?
        } else {
            let (sha256, size) = sha256_file(&source)?;
            BTreeMap::from([(PathBuf::from("file"), InventoryEntry::File { size, sha256 })])
        };
        if source_after != inventory || scan(&payload, false)? != inventory {
            return Err(failure(
                "backup payload changed during copying; no complete manifest committed",
            ));
        }
        let manifest = ManifestV2 {
            version: 2,
            source_kind: kind,
            inventory,
            summary: BackupManifest {
                source_path: source.clone(),
                created_at: now,
                file_count,
                size_bytes,
            },
        };
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path.join("manifest.pending"))?;
        file.write_all(&serde_json::to_vec_pretty(&manifest)?)?;
        file.sync_all()?;
        drop(file);
        fs::rename(path.join("manifest.pending"), path.join("manifest.json"))?;
        Ok(BackupEntry {
            id,
            path,
            created_at: now,
            source_path: source,
            file_count,
            size_bytes,
        })
    }

    fn resolve_backup_dir(&self, backup_id: &str) -> Result<PathBuf> {
        let path = Path::new(backup_id);
        plain_relative(path)?;
        if path.components().count() != 1 {
            return Err(failure(format!("invalid backup id: {backup_id}")));
        }
        checked_absolute(&self.backup_root.join(backup_id))
    }

    fn load_manifest(
        backup_id: &str,
        backup_dir: &Path,
    ) -> Result<(BackupManifest, Option<ManifestV2>)> {
        checked_absolute(&backup_dir.join("manifest.json"))?;
        let load = || -> Result<_> {
            let value: serde_json::Value =
                serde_json::from_slice(&fs::read(backup_dir.join("manifest.json"))?)?;
            let summary: BackupManifest = serde_json::from_value(value.clone())?;
            let v2 = if value.get("version").is_some() {
                let v2: ManifestV2 = serde_json::from_value(value)?;
                if v2.version != 2 {
                    return Err(failure("unsupported backup version"));
                }
                Some(v2)
            } else {
                if value.get("inventory").is_some() || value.get("source_kind").is_some() {
                    return Err(failure("backup manifest has no version"));
                }
                None
            };
            Ok((summary, v2))
        };
        load().map_err(|e| {
            failure(format!(
                "backup {backup_id} has no readable manifest.json: {e}"
            ))
        })
    }

    /// Use the exact injection backup as a pristine tree, after verifying its
    /// origin and complete recorded inventory. Never restore or mutate the game.
    pub fn with_pristine_tree<T>(
        &self,
        backup_id: &str,
        game_path: &Path,
        use_tree: impl FnOnce(&Path) -> Result<T>,
    ) -> Result<T> {
        self.with_verified_pristine_tree(backup_id, game_path, |tree| use_tree(tree.root()))
    }

    pub(crate) fn with_verified_pristine_tree<T>(
        &self,
        backup_id: &str,
        game_path: &Path,
        use_tree: impl FnOnce(VerifiedPristine<'_>) -> Result<T>,
    ) -> Result<T> {
        let backup_dir = self.resolve_backup_dir(backup_id)?;
        let (summary, manifest) = Self::load_manifest(backup_id, &backup_dir)?;
        let manifest = manifest.ok_or_else(|| {
            failure("this backup has no recorded hashes; select a separate pristine game copy")
        })?;
        if !summary.source_path.is_absolute() {
            return Err(failure("injection backup origin must be absolute"));
        }
        let source = checked_absolute(&summary.source_path)?.canonicalize()?;
        let game = checked_absolute(game_path)?.canonicalize()?;
        if source != game {
            return Err(failure("injection backup belongs to a different game"));
        }
        let backup_canonical = backup_dir.canonicalize()?;
        if backup_canonical.starts_with(&source) || source.starts_with(&backup_canonical) {
            return Err(failure("injection backup and game overlap"));
        }
        let payload = backup_dir.join("payload");
        let inventory = scan(&payload, false)?;
        if inventory != manifest.inventory
            || totals(&inventory)? != (summary.file_count, summary.size_bytes)
        {
            return Err(failure(
                "injection backup inventory/hash mismatch; packing refused",
            ));
        }
        if manifest.source_kind == SourceKind::Directory {
            if !source.is_dir() {
                return Err(failure("injection backup source type changed"));
            }
            return use_tree(VerifiedPristine::new(&payload, &inventory));
        }
        // Single-file backups store their bytes as payload/file. Normalize that
        // into an owned one-file tree with the original name expected by pack.
        if !source.is_file() || inventory.len() != 1 || !inventory.contains_key(Path::new("file")) {
            return Err(failure("invalid single-file injection backup"));
        }
        let stage = crate::patch::stream::StagingDir::create_prepared(
            backup_dir
                .parent()
                .ok_or_else(|| failure("backup has no parent"))?,
        )?;
        let name = source
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| failure("backup source filename is not valid Unicode"))?;
        let target = stage.child(name);
        let mut output = stage.create_file(name)?;
        std::io::copy(&mut fs::File::open(payload.join("file"))?, &mut output)?;
        output.sync_all()?;
        drop(output);
        let (sha256, size) = sha256_file(&target)?;
        let copied = InventoryEntry::File { sha256, size };
        if inventory.get(Path::new("file")) != Some(&copied) {
            return Err(failure(
                "injection backup changed while preparing pristine tree",
            ));
        }
        let mut normalized = BTreeMap::new();
        normalized.insert(PathBuf::from(name), copied);
        use_tree(VerifiedPristine::new(
            target
                .parent()
                .ok_or_else(|| failure("pristine tree has no parent"))?,
            &normalized,
        ))
    }

    /// Restore only to the recorded origin. All payload bytes and destinations
    /// are checked before the first write. Legacy backups have no historical
    /// hashes: only their count/size and current readable tree can be verified.
    /// Per-file replacement is atomic; this is not a multi-file transaction and
    /// does not delete files added after backup or merge later user edits.
    pub fn restore(&self, backup_id: &str) -> Result<()> {
        self.restore_inner(backup_id, false)
    }

    /// Check the same backup, game, and destination preconditions as restore
    /// without replacing any game files.
    pub fn verify_restore(&self, backup_id: &str) -> Result<()> {
        self.restore_inner(backup_id, true)
    }

    fn restore_inner(&self, backup_id: &str, dry_run: bool) -> Result<()> {
        let backup_dir = self.resolve_backup_dir(backup_id)?;
        let (summary, v2) = Self::load_manifest(backup_id, &backup_dir)?;
        if !summary.source_path.is_absolute() {
            return Err(failure("backup origin is relative and cannot be safely resolved after a working-directory change"));
        }
        let target = checked_absolute(&summary.source_path)?;
        let kind = v2.as_ref().map_or(SourceKind::Directory, |m| m.source_kind);
        if !(if kind == SourceKind::Directory {
            target.is_dir()
        } else {
            target.is_file()
        }) {
            return Err(failure(format!(
                "backup {backup_id} source no longer exists or has wrong type ({})",
                target.display()
            )));
        }
        let target = target.canonicalize()?;
        let backup_canonical = backup_dir.canonicalize()?;
        if backup_canonical.starts_with(&target) || target.starts_with(&backup_canonical) {
            return Err(failure("backup and restore destination overlap"));
        }
        let lock_root = if kind == SourceKind::Directory {
            target.as_path()
        } else {
            target
                .parent()
                .ok_or_else(|| failure("file source has no parent"))?
        };
        let game_lock = GameLock::acquire(lock_root)?;
        crate::injection_transaction::ensure_no_pending_under_lock(&game_lock)?;
        let payload = if v2.is_some() {
            backup_dir.join("payload")
        } else {
            backup_dir.clone()
        };
        let inventory = scan(&payload, v2.is_none())?;
        // Older backups must not resurrect an insertion journal from another
        // point in time. Current backups exclude only recognized recovery data.
        if kind == SourceKind::Directory
            && inventory.keys().any(|path| {
                path.components().next().is_some_and(|component| {
                    component
                        .as_os_str()
                        .to_string_lossy()
                        .eq_ignore_ascii_case(crate::injection_transaction::STORE_DIR)
                })
            })
        {
            return Err(failure("backup contains insertion recovery metadata; restore refused to preserve the current recovery state"));
        }
        if totals(&inventory)? != (summary.file_count, summary.size_bytes) {
            return Err(failure("backup file count/size mismatch; restore refused"));
        }
        if let Some(manifest) = &v2 {
            for rel in manifest.inventory.keys() {
                plain_relative(rel)?;
            }
            if inventory != manifest.inventory {
                return Err(failure("backup inventory/hash mismatch; restore refused"));
            }
            if kind == SourceKind::File
                && (inventory.len() != 1
                    || !matches!(
                        inventory.get(Path::new("file")),
                        Some(InventoryEntry::File { .. })
                    ))
            {
                return Err(failure("invalid single-file backup inventory"));
            }
        }
        let destination = |rel: &Path| {
            if kind == SourceKind::File {
                target.clone()
            } else {
                target.join(rel)
            }
        };
        for (rel, entry) in &inventory {
            let dest = destination(rel);
            checked_absolute(&dest)?;
            if let Ok(meta) = fs::symlink_metadata(&dest) {
                let expect_dir = matches!(entry, InventoryEntry::Directory);
                if meta.is_dir() != expect_dir || (!expect_dir && meta.permissions().readonly()) {
                    return Err(failure(format!(
                        "invalid restore destination: {}",
                        dest.display()
                    )));
                }
            }
        }
        if dry_run {
            return Ok(());
        }
        for (rel, entry) in &inventory {
            let dest = destination(rel);
            checked_absolute(&dest)?;
            match entry {
                InventoryEntry::Directory => fs::create_dir_all(&dest)?,
                InventoryEntry::File { size, sha256 } => {
                    let parent = dest
                        .parent()
                        .ok_or_else(|| failure("destination has no parent"))?;
                    fs::create_dir_all(parent)?;
                    let staged =
                        parent.join(format!(".locust-restore-{}.tmp", uuid::Uuid::new_v4()));
                    let result = (|| -> Result<()> {
                        let mut output = OpenOptions::new()
                            .write(true)
                            .create_new(true)
                            .open(&staged)?;
                        let mut input = fs::File::open(payload.join(rel))?;
                        let permissions = input.metadata()?.permissions();
                        std::io::copy(&mut input, &mut output)?;
                        output.sync_all()?;
                        drop(output);
                        if sha256_file(&staged)? != (sha256.clone(), *size) {
                            return Err(failure("backup payload changed during restore"));
                        }
                        checked_absolute(&dest)?;
                        fs::set_permissions(&staged, permissions)?;
                        fs::rename(&staged, &dest)?;
                        Ok(())
                    })();
                    if result.is_err() {
                        let _ = fs::remove_file(&staged);
                    }
                    result?;
                }
            }
        }
        Ok(())
    }

    pub fn list_backups(&self) -> Result<Vec<BackupEntry>> {
        checked_absolute(&self.backup_root)?;
        if !self.backup_root.exists() {
            return Ok(Vec::new());
        }
        let mut entries = Vec::new();
        for dir in fs::read_dir(&self.backup_root)? {
            let dir = dir?;
            reject_link(&dir.path(), &fs::symlink_metadata(dir.path())?)?;
            if !dir.file_type()?.is_dir() || !dir.path().join("manifest.json").exists() {
                continue;
            }
            let id = dir.file_name().to_string_lossy().to_string();
            let (m, _) = Self::load_manifest(&id, &dir.path())?;
            entries.push(BackupEntry {
                id,
                path: dir.path(),
                source_path: m.source_path,
                created_at: m.created_at,
                file_count: m.file_count,
                size_bytes: m.size_bytes,
            });
        }
        entries.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        Ok(entries)
    }

    /// Prove that this invocation's exact complete backup is redundant with
    /// the current source. The caller must retain GameLock through deletion.
    /// Includes .locust receipts; only our validated injection journal is skipped.
    pub(crate) fn ensure_redundant_backup(&self, expected: &BackupEntry) -> Result<()> {
        let dir = self.resolve_backup_dir(&expected.id)?;
        if !crate::database::paths_identical(&dir, &checked_absolute(&expected.path)?) {
            return Err(failure("backup location changed before no-op cleanup"));
        }
        let (summary, manifest) = Self::load_manifest(&expected.id, &dir)?;
        let manifest =
            manifest.ok_or_else(|| failure("no-op cleanup requires a verified backup"))?;
        if summary.created_at != expected.created_at
            || summary.file_count != expected.file_count
            || summary.size_bytes != expected.size_bytes
            || summary.source_path != expected.source_path
        {
            return Err(failure("backup generation changed before no-op cleanup"));
        }
        let inventory = scan(&dir.join("payload"), false)?;
        if inventory != manifest.inventory
            || totals(&inventory)? != (summary.file_count, summary.size_bytes)
        {
            return Err(failure("backup changed before no-op cleanup"));
        }
        let source = checked_absolute(&expected.source_path)?;
        let current = match manifest.source_kind {
            SourceKind::Directory if source.is_dir() => scan_source(&source)?,
            SourceKind::File if source.is_file() => {
                let (sha256, size) = sha256_file(&source)?;
                BTreeMap::from([(PathBuf::from("file"), InventoryEntry::File { sha256, size })])
            }
            _ => return Err(failure("game source type changed before no-op cleanup")),
        };
        if current != inventory {
            return Err(failure(
                "game changed since its backup; no-op cleanup refused",
            ));
        }
        Ok(())
    }

    pub fn delete_backup(&self, backup_id: &str) -> Result<()> {
        let dir = self.resolve_backup_dir(backup_id)?;
        if dir.exists() {
            // Validate descendants before recursive removal; do not follow links.
            for entry in WalkDir::new(&dir).follow_links(false) {
                let entry = entry.map_err(|e| failure(e.to_string()))?;
                reject_link(entry.path(), &fs::symlink_metadata(entry.path())?)?;
            }
            fs::remove_dir_all(dir)?;
        }
        Ok(())
    }

    pub fn delete_old_backups(&self, keep_last: usize) -> Result<usize> {
        let backups = self.list_backups()?;
        let mut deleted = 0;
        for backup in backups.iter().skip(keep_last) {
            self.delete_backup(&backup.id)?;
            deleted += 1;
        }
        Ok(deleted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn revision_reader_checks_consumed_bytes_not_a_later_path_hash() {
        use std::io::{Cursor, Read};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("script.rpy");
        fs::write(&path, "original text").unwrap();
        let original = RevisionOriginal::capture(&path).unwrap();
        // Simulate a reader opened on substituted bytes whose path is restored
        // before a final rehash could notice. The consumed bytes must fail.
        struct RestoringReader {
            path: PathBuf,
            bytes: Cursor<Vec<u8>>,
        }
        impl Read for RestoringReader {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                fs::write(&self.path, "original text")?;
                self.bytes.read(buf)
            }
        }
        fs::write(&path, "tampered text").unwrap();
        let mut reader = RestoringReader {
            path: path.clone(),
            bytes: Cursor::new(b"tampered text".to_vec()),
        };
        assert!(original
            .read_text_from(&mut reader)
            .unwrap_err()
            .to_string()
            .contains("changed since verification"));
        assert_eq!(original.read_text().unwrap(), "original text");
        // Even an unbounded changed input cannot extend the retained buffer.
        assert!(original.read_text_from(&mut std::io::repeat(b'x')).is_err());
        assert!(original.read_text_from(&mut Cursor::new(b"short")).is_err());
    }

    #[test]
    fn revision_reader_rejects_deleted_changed_and_non_utf8_originals() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("story.html");
        fs::write(&path, "original").unwrap();
        let original = RevisionOriginal::capture(&path).unwrap();
        for changed in [b"changed!".as_slice(), b"longer changed data", b"short"] {
            fs::write(&path, changed).unwrap();
            assert!(original.read_text().is_err());
        }
        fs::remove_file(&path).unwrap();
        assert!(original.read_text().is_err());
        fs::create_dir(&path).unwrap();
        assert!(original.read_text().is_err());
        fs::remove_dir(&path).unwrap();
        fs::write(&path, [0xff, 0xfe]).unwrap();
        let binary = RevisionOriginal::capture(&path).unwrap();
        assert!(binary
            .read_text()
            .unwrap_err()
            .to_string()
            .contains("not UTF-8"));
    }

    #[test]
    fn noop_retention_requires_exact_backup_and_includes_patch_receipts() {
        for change in ["receipt", "generation", "payload", "single-file"] {
            let base = tempfile::tempdir().unwrap();
            let game = base.path().join("game");
            fs::create_dir_all(game.join(".locust")).unwrap();
            fs::write(game.join(".locust/receipt"), "Original receipt").unwrap();
            let file = game.join("story.txt");
            fs::write(&file, "Original bytes").unwrap();
            let selected = if change == "single-file" {
                &file
            } else {
                &game
            };
            let manager = BackupManager::new(base.path().join("backups"));
            let backup = manager.create_backup(selected).unwrap();
            manager.ensure_redundant_backup(&backup).unwrap();
            match change {
                "receipt" => fs::write(game.join(".locust/receipt"), "Changed receipt").unwrap(),
                "generation" => {
                    let path = backup.path.join("manifest.json");
                    let mut manifest: serde_json::Value =
                        serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
                    manifest["created_at"] = serde_json::json!("2000-01-01T00:00:00Z");
                    fs::write(path, serde_json::to_vec(&manifest).unwrap()).unwrap();
                }
                "payload" => {
                    fs::write(backup.path.join("payload/story.txt"), "Corrupted backup").unwrap()
                }
                "single-file" => fs::write(&file, "External edit").unwrap(),
                _ => unreachable!(),
            }
            assert!(
                manager.ensure_redundant_backup(&backup).is_err(),
                "{change}"
            );
            assert!(backup.path.join("manifest.json").is_file());
        }
    }

    #[test]
    fn pristine_tree_checks_exact_origin_and_inventory_before_callback() {
        let directory = tempfile::tempdir().unwrap();
        let game = directory.path().join("game");
        fs::create_dir(&game).unwrap();
        fs::write(game.join("story.html"), "original").unwrap();
        let manager = BackupManager::new(directory.path().join("backups"));
        let backup = manager.create_backup(&game).unwrap();
        fs::write(game.join("story.html"), "translated").unwrap();
        manager
            .with_pristine_tree(&backup.id, &game, |tree| {
                assert_eq!(
                    fs::read_to_string(tree.join("story.html")).unwrap(),
                    "original"
                );
                Ok(())
            })
            .unwrap();
        let other = directory.path().join("other");
        fs::create_dir(&other).unwrap();
        assert!(manager
            .with_pristine_tree::<()>(&backup.id, &other, |_| panic!("wrong origin accepted"))
            .is_err());
        fs::write(backup.path.join("payload/story.html"), "tampered").unwrap();
        assert!(manager
            .with_pristine_tree::<()>(&backup.id, &game, |_| panic!("bad hash accepted"))
            .is_err());
        assert_eq!(
            fs::read_to_string(game.join("story.html")).unwrap(),
            "translated"
        );
    }

    #[test]
    fn pristine_tree_normalizes_single_file_and_removes_owned_stage() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("日本語.html");
        fs::write(&file, "original").unwrap();
        let manager = BackupManager::new(directory.path().join("backups"));
        let backup = manager.create_backup(&file).unwrap();
        fs::write(&file, "translated").unwrap();
        let stage = manager
            .with_pristine_tree(&backup.id, &file, |tree| {
                assert_eq!(
                    fs::read_to_string(tree.join("日本語.html")).unwrap(),
                    "original"
                );
                assert_eq!(fs::read_dir(tree).unwrap().count(), 1);
                Ok(tree.to_path_buf())
            })
            .unwrap();
        assert!(!stage.exists());
        assert_eq!(fs::read_to_string(&file).unwrap(), "translated");
        assert_eq!(
            fs::read_to_string(backup.path.join("payload/file")).unwrap(),
            "original"
        );
    }

    #[test]
    fn verified_pristine_retains_normalized_file_hash_and_cleans_stage_on_error() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("日本語.html");
        fs::write(&file, "original").unwrap();
        let manager = BackupManager::new(directory.path().join("backups"));
        let backup = manager.create_backup(&file).unwrap();
        let mut stage = None;
        let error = manager.with_verified_pristine_tree::<()>(&backup.id, &file, |tree| {
            stage = Some(tree.root().to_path_buf());
            assert_eq!(
                tree.original_sha256(Path::new("日本語.html"))?,
                Some(crate::database::sha256_hex(b"original").as_str())
            );
            assert_eq!(tree.original_sha256(Path::new("file"))?, None);
            assert_eq!(tree.original_sha256(Path::new("added.txt"))?, None);
            assert!(tree.original_sha256(Path::new("../escape")).is_err());
            // The immutable hash remains available even if the consumer's
            // path changes; pack is responsible for comparing before publish.
            fs::remove_file(tree.root().join("日本語.html"))?;
            assert!(tree.original_sha256(Path::new("日本語.html"))?.is_some());
            Err(failure("test consumer failed"))
        });
        assert!(error
            .unwrap_err()
            .to_string()
            .contains("test consumer failed"));
        assert!(!stage.unwrap().exists());
        assert_eq!(fs::read_to_string(file).unwrap(), "original");
        assert_eq!(
            fs::read_to_string(backup.path.join("payload/file")).unwrap(),
            "original"
        );
    }

    #[test]
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    fn verified_pristine_keeps_hash_for_case_alias_after_original_disappears() {
        let directory = tempfile::tempdir().unwrap();
        let game = directory.path().join("game");
        fs::create_dir(&game).unwrap();
        fs::write(game.join("Story.TXT"), "original").unwrap();
        let manager = BackupManager::new(directory.path().join("backups"));
        let backup = manager.create_backup(&game).unwrap();
        manager
            .with_verified_pristine_tree(&backup.id, &game, |tree| {
                let expected = crate::database::sha256_hex(b"original");
                assert_eq!(
                    tree.original_sha256(Path::new("story.txt"))?,
                    Some(expected.as_str())
                );
                fs::remove_file(tree.root().join("Story.TXT"))?;
                assert_eq!(
                    tree.original_sha256(Path::new("story.txt"))?,
                    Some(expected.as_str())
                );
                Ok(())
            })
            .unwrap();
    }

    fn tempdir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("locust_bak_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn create_game_dir() -> PathBuf {
        let dir = tempdir();
        fs::write(dir.join("data.json"), r#"{"hp": 100}"#).unwrap();
        fs::write(dir.join("strings.txt"), "Hello\nWorld").unwrap();
        fs::create_dir_all(dir.join("sub")).unwrap();
        fs::write(dir.join("sub").join("nested.txt"), "nested content").unwrap();
        dir
    }

    #[test]
    fn test_create_backup_copies_files() {
        let game_dir = create_game_dir();
        let backup_root = tempdir();
        let mgr = BackupManager::new(backup_root);
        let entry = mgr.create_backup(&game_dir).unwrap();

        assert!(entry.path.join("payload").join("data.json").exists());
        assert!(entry.path.join("payload").join("strings.txt").exists());
        assert!(entry
            .path
            .join("payload")
            .join("sub")
            .join("nested.txt")
            .exists());
        assert_eq!(entry.file_count, 3);
    }

    #[test]
    fn test_create_backup_writes_manifest() {
        let game_dir = create_game_dir();
        let backup_root = tempdir();
        let mgr = BackupManager::new(backup_root);
        let entry = mgr.create_backup(&game_dir).unwrap();

        let manifest_path = entry.path.join("manifest.json");
        assert!(manifest_path.exists());
        let manifest_str = fs::read_to_string(&manifest_path).unwrap();
        let manifest: BackupManifest = serde_json::from_str(&manifest_str).unwrap();
        assert_eq!(manifest.file_count, 3);
    }

    #[test]
    fn test_list_backups_sorted() {
        let game_dir = create_game_dir();
        let backup_root = tempdir();
        let mgr = BackupManager::new(backup_root);
        let _b1 = mgr.create_backup(&game_dir).unwrap();
        let _b2 = mgr.create_backup(&game_dir).unwrap();

        let list = mgr.list_backups().unwrap();
        assert_eq!(list.len(), 2);
        assert!(list[0].created_at >= list[1].created_at);
    }

    #[test]
    fn test_backup_id_traversal_is_rejected() {
        // Ids arrive from HTTP (`POST /api/backups/:id/restore` and delete),
        // so a traversal must not reach outside the backup root — delete in
        // particular would remove the directory tree it lands on.
        let backup_root = tempdir();
        let outsider = backup_root
            .parent()
            .unwrap()
            .join(format!("locust_outsider_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&outsider).unwrap();
        fs::write(outsider.join("keep.txt"), "important").unwrap();

        let mgr = BackupManager::new(backup_root);
        let escape = format!("../{}", outsider.file_name().unwrap().to_string_lossy());

        for bad in [
            escape.as_str(),
            "..",
            ".",
            "",
            "sub/dir",
            "sub\\dir",
            "/etc",
        ] {
            assert!(mgr.restore(bad).is_err(), "restore accepted {bad:?}");
            assert!(mgr.delete_backup(bad).is_err(), "delete accepted {bad:?}");
        }

        assert!(
            outsider.join("keep.txt").exists(),
            "traversal deleted a directory outside the backup root"
        );

        // A real id still works.
        let game_dir = create_game_dir();
        let entry = mgr.create_backup(&game_dir).unwrap();
        mgr.restore(&entry.id).unwrap();
        mgr.delete_backup(&entry.id).unwrap();
    }

    #[test]
    fn test_same_second_backups_do_not_share_a_directory() {
        // Backup ids are second-resolution timestamps. Two injects within the
        // same second must not land in one directory, or each overwrites the
        // other's manifest and restore hands back a mix of both games.
        let backup_root = tempdir();
        let mgr = BackupManager::new(backup_root);

        let game_a = create_game_dir();
        let game_b = tempdir();
        fs::write(game_b.join("only_in_b.txt"), "b").unwrap();

        let a = mgr.create_backup(&game_a).unwrap();
        let b = mgr.create_backup(&game_b).unwrap();

        assert_ne!(a.id, b.id, "same-second backups must get distinct ids");
        assert_ne!(a.path, b.path);

        // Each backup holds its own tree, not the other's.
        assert!(a.path.join("payload").join("data.json").exists());
        assert!(!a.path.join("payload").join("only_in_b.txt").exists());
        assert!(b.path.join("payload").join("only_in_b.txt").exists());
        assert!(!b.path.join("payload").join("data.json").exists());
        assert_eq!(a.file_count, 3);
        assert_eq!(b.file_count, 1);

        // Both remain independently listable and restorable.
        assert_eq!(mgr.list_backups().unwrap().len(), 2);
        fs::write(game_b.join("only_in_b.txt"), "mutated").unwrap();
        mgr.restore(&b.id).unwrap();
        assert_eq!(
            fs::read_to_string(game_b.join("only_in_b.txt")).unwrap(),
            "b"
        );
        assert!(!game_b.join("data.json").exists());
    }

    #[test]
    fn test_restore_overwrites_target() {
        let game_dir = create_game_dir();
        let backup_root = tempdir();
        let mgr = BackupManager::new(backup_root);
        let entry = mgr.create_backup(&game_dir).unwrap();

        // Modify original file
        fs::write(game_dir.join("data.json"), "MODIFIED").unwrap();
        assert_eq!(
            fs::read_to_string(game_dir.join("data.json")).unwrap(),
            "MODIFIED"
        );

        // Restore
        mgr.restore(&entry.id).unwrap();
        assert_eq!(
            fs::read_to_string(game_dir.join("data.json")).unwrap(),
            r#"{"hp": 100}"#
        );
    }

    #[test]
    fn test_restore_writes_to_backup_origin_not_another_game() {
        // Inject game A (a backup is taken), then another game exists as if it
        // were the open project. Restore must land in A; B must stay untouched.
        let game_a = create_game_dir();
        let game_b = tempdir();
        fs::write(game_b.join("b_only.txt"), "keep-b").unwrap();

        let backup_root = tempdir();
        let mgr = BackupManager::new(backup_root);
        let entry = mgr.create_backup(&game_a).unwrap();

        fs::write(game_a.join("data.json"), "A-MUTATED").unwrap();

        mgr.restore(&entry.id).unwrap();

        assert_eq!(
            fs::read_to_string(game_a.join("data.json")).unwrap(),
            r#"{"hp": 100}"#
        );
        assert_eq!(
            fs::read_to_string(game_b.join("b_only.txt")).unwrap(),
            "keep-b"
        );
        assert!(
            !game_b.join("data.json").exists(),
            "game B must not receive game A's restored files"
        );
    }

    #[test]
    fn test_restore_refuses_deleted_source_directory() {
        let game_dir = create_game_dir();
        let backup_root = tempdir();
        let mgr = BackupManager::new(backup_root);
        let entry = mgr.create_backup(&game_dir).unwrap();

        fs::remove_dir_all(&game_dir).unwrap();

        let err = mgr.restore(&entry.id).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains(&entry.id),
            "error must name the backup id: {msg}"
        );
        assert!(
            msg.contains("no longer exists"),
            "error must say the source directory is gone: {msg}"
        );
        assert!(
            !game_dir.exists(),
            "restore must not recreate a deleted game directory"
        );
    }

    #[test]
    fn test_restore_refuses_missing_or_corrupt_manifest() {
        let game_dir = create_game_dir();
        let backup_root = tempdir();
        let mgr = BackupManager::new(backup_root);

        let missing = mgr.create_backup(&game_dir).unwrap();
        fs::write(game_dir.join("data.json"), "AFTER-MISSING-BACKUP").unwrap();
        fs::remove_file(missing.path.join("manifest.json")).unwrap();
        let err = mgr.restore(&missing.id).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains(&missing.id),
            "error must name the backup id: {msg}"
        );
        assert!(
            msg.contains("manifest.json"),
            "error must name the missing manifest: {msg}"
        );
        assert_eq!(
            fs::read_to_string(game_dir.join("data.json")).unwrap(),
            "AFTER-MISSING-BACKUP",
            "a backup without a manifest must not write anything"
        );

        let corrupt = mgr.create_backup(&game_dir).unwrap();
        fs::write(game_dir.join("data.json"), "AFTER-CORRUPT-BACKUP").unwrap();
        fs::write(corrupt.path.join("manifest.json"), "not-json{").unwrap();
        let err = mgr.restore(&corrupt.id).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains(&corrupt.id),
            "error must name the backup id: {msg}"
        );
        assert!(
            msg.contains("manifest.json"),
            "error must name the unreadable manifest: {msg}"
        );
        assert_eq!(
            fs::read_to_string(game_dir.join("data.json")).unwrap(),
            "AFTER-CORRUPT-BACKUP",
            "a backup with a corrupt manifest must not write anything"
        );
    }

    #[test]
    fn test_delete_backup() {
        let game_dir = create_game_dir();
        let backup_root = tempdir();
        let mgr = BackupManager::new(backup_root);
        let entry = mgr.create_backup(&game_dir).unwrap();
        mgr.delete_backup(&entry.id).unwrap();
        let list = mgr.list_backups().unwrap();
        assert!(list.is_empty());
    }

    #[test]
    fn test_delete_old_keeps_recent() {
        let game_dir = create_game_dir();
        let backup_root = tempdir();
        let mgr = BackupManager::new(backup_root);
        for _ in 0..5 {
            mgr.create_backup(&game_dir).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(1100));
        }
        let deleted = mgr.delete_old_backups(2).unwrap();
        assert_eq!(deleted, 3);
        let remaining = mgr.list_backups().unwrap();
        assert_eq!(remaining.len(), 2);
    }
    #[test]
    fn v2_preserves_game_manifest_and_empty_directories() {
        let game = tempfile::tempdir().unwrap();
        let backups = tempfile::tempdir().unwrap();
        fs::write(game.path().join("manifest.json"), b"actual game data").unwrap();
        fs::create_dir(game.path().join("empty")).unwrap();
        let mgr = BackupManager::new(backups.path().to_owned());
        let b = mgr.create_backup(game.path()).unwrap();
        assert_eq!(
            fs::read(b.path.join("payload/manifest.json")).unwrap(),
            b"actual game data"
        );
        fs::write(game.path().join("manifest.json"), b"changed").unwrap();
        fs::remove_dir(game.path().join("empty")).unwrap();
        mgr.restore(&b.id).unwrap();
        assert_eq!(
            fs::read(game.path().join("manifest.json")).unwrap(),
            b"actual game data"
        );
        assert!(game.path().join("empty").is_dir());
    }

    #[test]
    fn v2_single_file_roundtrip_and_parent_lock() {
        let game = tempfile::tempdir().unwrap();
        let backups = tempfile::tempdir().unwrap();
        let file = game.path().join("neutral.bin");
        fs::write(&file, b"original").unwrap();
        fs::write(game.path().join("sibling"), b"untouched").unwrap();
        let mgr = BackupManager::new(backups.path().to_owned());
        let b = mgr.create_backup(&file).unwrap();
        assert_eq!(b.file_count, 1);
        fs::write(&file, b"changed").unwrap();
        let guard = GameLock::acquire(game.path()).unwrap();
        assert!(mgr.restore(&b.id).unwrap_err().to_string().contains("busy"));
        assert_eq!(fs::read(&file).unwrap(), b"changed");
        drop(guard);
        mgr.restore(&b.id).unwrap();
        assert_eq!(fs::read(&file).unwrap(), b"original");
        assert_eq!(fs::read(game.path().join("sibling")).unwrap(), b"untouched");
    }

    #[test]
    fn v2_corrupt_late_file_fails_before_first_destination_write() {
        let game = tempfile::tempdir().unwrap();
        let backups = tempfile::tempdir().unwrap();
        for name in ["a", "z"] {
            fs::write(game.path().join(name), b"original").unwrap();
        }
        let mgr = BackupManager::new(backups.path().to_owned());
        let b = mgr.create_backup(game.path()).unwrap();
        fs::write(game.path().join("a"), b"sentinel").unwrap();
        fs::write(b.path.join("payload/z"), b"corrupt!").unwrap(); // Same length.
        assert!(mgr
            .restore(&b.id)
            .unwrap_err()
            .to_string()
            .contains("hash mismatch"));
        assert_eq!(fs::read(game.path().join("a")).unwrap(), b"sentinel");
    }

    #[test]
    fn v2_missing_and_extra_payload_files_fail_preflight() {
        let game = tempfile::tempdir().unwrap();
        let backups = tempfile::tempdir().unwrap();
        fs::write(game.path().join("a"), b"original").unwrap();
        let mgr = BackupManager::new(backups.path().to_owned());
        let b = mgr.create_backup(game.path()).unwrap();
        fs::write(game.path().join("a"), b"sentinel").unwrap();
        fs::remove_file(b.path.join("payload/a")).unwrap();
        assert!(mgr.restore(&b.id).is_err());
        fs::write(b.path.join("payload/a"), b"original").unwrap();
        fs::write(b.path.join("payload/extra"), b"injected").unwrap();
        assert!(mgr.restore(&b.id).is_err());
        assert_eq!(fs::read(game.path().join("a")).unwrap(), b"sentinel");
    }

    #[test]
    fn invalid_late_destination_fails_before_first_write() {
        let game = tempfile::tempdir().unwrap();
        let backups = tempfile::tempdir().unwrap();
        fs::write(game.path().join("a"), b"original").unwrap();
        fs::create_dir(game.path().join("z")).unwrap();
        fs::write(game.path().join("z/data"), b"later").unwrap();
        let mgr = BackupManager::new(backups.path().to_owned());
        let b = mgr.create_backup(game.path()).unwrap();
        fs::write(game.path().join("a"), b"sentinel").unwrap();
        fs::remove_file(game.path().join("z/data")).unwrap();
        fs::remove_dir(game.path().join("z")).unwrap();
        fs::write(game.path().join("z"), b"obstruction").unwrap();
        assert!(mgr.restore(&b.id).is_err());
        assert_eq!(fs::read(game.path().join("a")).unwrap(), b"sentinel");
        assert_eq!(fs::read(game.path().join("z")).unwrap(), b"obstruction");
    }

    #[test]
    fn nested_backup_root_rejected_before_creating_storage() {
        let game = tempfile::tempdir().unwrap();
        let nested = game.path().join("backups/new");
        let mgr = BackupManager::new(nested.clone());
        assert!(mgr.create_backup(game.path()).is_err());
        assert!(!nested.exists());
    }

    #[test]
    fn incomplete_backup_is_not_listed_or_restored() {
        let game = tempfile::tempdir().unwrap();
        let backups = tempfile::tempdir().unwrap();
        fs::write(game.path().join("a"), b"sentinel").unwrap();
        fs::create_dir(backups.path().join("incomplete")).unwrap();
        fs::write(backups.path().join("incomplete/manifest.pending"), b"{}").unwrap();
        let mgr = BackupManager::new(backups.path().to_owned());
        assert!(mgr.list_backups().unwrap().is_empty());
        assert!(mgr.restore("incomplete").is_err());
        assert_eq!(fs::read(game.path().join("a")).unwrap(), b"sentinel");
    }

    #[test]
    fn legacy_restores_origin_but_refuses_incomplete_inventory() {
        let game = tempfile::tempdir().unwrap();
        let backups = tempfile::tempdir().unwrap();
        let backup = backups.path().join("legacy");
        fs::create_dir(&backup).unwrap();
        fs::write(backup.join("a"), b"old").unwrap();
        let manifest = BackupManifest {
            source_path: game.path().to_owned(),
            created_at: Utc::now(),
            file_count: 1,
            size_bytes: 3,
        };
        fs::write(
            backup.join("manifest.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        fs::write(game.path().join("a"), b"changed").unwrap();
        let mgr = BackupManager::new(backups.path().to_owned());
        mgr.restore("legacy").unwrap();
        assert_eq!(fs::read(game.path().join("a")).unwrap(), b"old");
        fs::write(game.path().join("a"), b"sentinel").unwrap();
        fs::remove_file(backup.join("a")).unwrap();
        assert!(mgr.restore("legacy").is_err());
        assert_eq!(fs::read(game.path().join("a")).unwrap(), b"sentinel");
    }

    #[test]
    fn v2_traversal_inventory_and_unknown_version_are_rejected() {
        let game = tempfile::tempdir().unwrap();
        let backups = tempfile::tempdir().unwrap();
        fs::write(game.path().join("a"), b"original").unwrap();
        let mgr = BackupManager::new(backups.path().to_owned());
        let b = mgr.create_backup(game.path()).unwrap();
        fs::write(game.path().join("a"), b"sentinel").unwrap();
        let mpath = b.path.join("manifest.json");
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(&mpath).unwrap()).unwrap();
        value["inventory"]["../escape"] = serde_json::json!({"kind":"directory"});
        fs::write(&mpath, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(mgr.restore(&b.id).is_err());
        value["version"] = 999.into();
        fs::write(&mpath, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(mgr.restore(&b.id).is_err());
        assert_eq!(fs::read(game.path().join("a")).unwrap(), b"sentinel");
    }

    #[cfg(any(unix, windows))]
    fn link_dir(target: &Path, link: &Path) {
        #[cfg(windows)]
        std::os::windows::fs::symlink_dir(target, link).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(target, link).unwrap();
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn linked_source_payload_destination_and_backup_id_are_rejected() {
        let game = tempfile::tempdir().unwrap();
        let backups = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("sentinel"), b"outside").unwrap();
        fs::write(game.path().join("a"), b"original").unwrap();
        fs::create_dir(game.path().join("z")).unwrap();
        fs::write(game.path().join("z/sentinel"), b"original").unwrap();
        let mgr = BackupManager::new(backups.path().to_owned());
        link_dir(outside.path(), &game.path().join("source-link"));
        assert!(mgr.create_backup(game.path()).is_err());
        #[cfg(windows)]
        fs::remove_dir(game.path().join("source-link")).unwrap();
        #[cfg(unix)]
        fs::remove_file(game.path().join("source-link")).unwrap();
        let b = mgr.create_backup(game.path()).unwrap();
        fs::write(game.path().join("a"), b"sentinel").unwrap();
        fs::remove_file(game.path().join("z/sentinel")).unwrap();
        fs::remove_dir(game.path().join("z")).unwrap();
        link_dir(outside.path(), &game.path().join("z"));
        assert!(mgr.restore(&b.id).is_err());
        assert_eq!(fs::read(game.path().join("a")).unwrap(), b"sentinel");
        link_dir(outside.path(), &b.path.join("payload/evil"));
        assert!(mgr.restore(&b.id).is_err());
        assert!(mgr.delete_backup(&b.id).is_err());
        link_dir(outside.path(), &backups.path().join("alias"));
        assert!(mgr.restore("alias").is_err());
        assert!(mgr.delete_backup("alias").is_err());
        assert_eq!(
            fs::read(outside.path().join("sentinel")).unwrap(),
            b"outside"
        );
    }

    #[test]
    fn replacement_does_not_mutate_external_hardlink() {
        let game = tempfile::tempdir().unwrap();
        let backups = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::write(game.path().join("a"), b"original").unwrap();
        let mgr = BackupManager::new(backups.path().to_owned());
        let b = mgr.create_backup(game.path()).unwrap();
        fs::write(game.path().join("a"), b"sentinel").unwrap();
        fs::hard_link(game.path().join("a"), outside.path().join("linked")).unwrap();
        mgr.restore(&b.id).unwrap();
        assert_eq!(fs::read(game.path().join("a")).unwrap(), b"original");
        assert_eq!(
            fs::read(outside.path().join("linked")).unwrap(),
            b"sentinel"
        );
    }
    #[cfg(unix)]
    #[test]
    fn backup_and_restore_preserve_executable_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let game = tempfile::tempdir().unwrap();
        let backups = tempfile::tempdir().unwrap();
        let script = game.path().join("runner");
        fs::write(&script, b"original").unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        let mgr = BackupManager::new(backups.path().to_owned());
        let b = mgr.create_backup(game.path()).unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o644)).unwrap();
        fs::write(&script, b"changed").unwrap();
        mgr.restore(&b.id).unwrap();
        assert_eq!(
            fs::metadata(&script).unwrap().permissions().mode() & 0o777,
            0o755
        );
    }
    #[test]
    fn legacy_relative_origin_is_refused_without_guessing_current_directory() {
        let backups = tempfile::tempdir().unwrap();
        let dir = backups.path().join("relative");
        fs::create_dir(&dir).unwrap();
        let manifest = BackupManifest {
            source_path: PathBuf::from("some-game"),
            created_at: Utc::now(),
            file_count: 0,
            size_bytes: 0,
        };
        fs::write(
            dir.join("manifest.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        let mgr = BackupManager::new(backups.path().to_owned());
        assert!(mgr
            .restore("relative")
            .unwrap_err()
            .to_string()
            .contains("origin is relative"));
    }
    #[test]
    fn stale_retention_cannot_delete_a_reused_backup_id() {
        let backups = tempfile::tempdir().unwrap();
        let mgr = BackupManager::new(backups.path().to_owned());
        let now = Utc::now();
        // Retention A captured old_id. Retention B deletes it meanwhile.
        let (old_id, _) = mgr.claim_backup_dir(now).unwrap();
        mgr.delete_backup(&old_id).unwrap();
        // A new writer claims a directory in the exact same second.
        let (new_id, new_path) = mgr.claim_backup_dir(now).unwrap();
        fs::write(new_path.join("in-progress"), b"owned by new writer").unwrap();
        // Retention A finally executes its stale plan.
        mgr.delete_backup(&old_id).unwrap();
        assert_ne!(
            new_id, old_id,
            "backup identities must never be reused after retention"
        );
        assert_eq!(
            fs::read(new_path.join("in-progress")).unwrap(),
            b"owned by new writer"
        );
    }

    #[test]
    fn concurrent_creation_and_retention_do_not_destroy_in_progress_copies() {
        use std::sync::{Arc, Barrier};
        let game = tempfile::tempdir().unwrap();
        let backups = tempfile::tempdir().unwrap();
        fs::write(game.path().join("data"), vec![b'x'; 16 * 1024]).unwrap();
        let barrier = Arc::new(Barrier::new(4));
        std::thread::scope(|scope| {
            let mut workers = Vec::new();
            for _ in 0..4 {
                let game = game.path();
                let backups = backups.path();
                let barrier = barrier.clone();
                workers.push(scope.spawn(move || {
                    let mgr = BackupManager::new(backups.to_owned());
                    barrier.wait();
                    for _ in 0..20 {
                        // An overlapping deletion may invalidate a stale listing;
                        // callers already treat best-effort retention as optional.
                        let _ = mgr.delete_old_backups(3);
                        mgr.create_backup(game)
                            .expect("retention must never delete an in-progress copy");
                    }
                }));
            }
            for worker in workers {
                worker.join().unwrap();
            }
        });
        assert_eq!(
            fs::read(game.path().join("data")).unwrap(),
            vec![b'x'; 16 * 1024]
        );
    }
    #[test]
    fn concurrent_stale_retention_plan_never_targets_a_new_generation() {
        let backups = tempfile::tempdir().unwrap();
        let mgr = BackupManager::new(backups.path().to_owned());
        let (plan_tx, plan_rx) = std::sync::mpsc::channel::<String>();
        let (go_tx, go_rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        std::thread::scope(|scope| {
            let retainer = BackupManager::new(backups.path().to_owned());
            scope.spawn(move || {
                for id in plan_rx {
                    if go_rx.recv().is_err() {
                        break;
                    }
                    done_tx.send(retainer.delete_backup(&id)).unwrap();
                }
            });
            for _ in 0..8 {
                let now = Utc::now();
                let (old, _) = mgr.claim_backup_dir(now).unwrap();
                plan_tx.send(old.clone()).unwrap();
                mgr.delete_backup(&old).unwrap();
                let (fresh, path) = mgr.claim_backup_dir(now).unwrap();
                fs::write(path.join("sentinel"), b"new generation").unwrap();
                go_tx.send(()).unwrap();
                done_rx.recv().unwrap().unwrap();
                assert!(
                    path.join("sentinel").is_file(),
                    "concurrent stale deletion removed the new generation"
                );
                mgr.delete_backup(&fresh).unwrap();
            }
            drop(plan_tx);
        });
    }
}
