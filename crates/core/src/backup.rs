use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Component, Path, PathBuf};

use crate::database::sha256_file;
use crate::error::{LocustError, Result};
use crate::patch::GameLock;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use walkdir::WalkDir;

/// Resolve the same profile used by default restore/pack callers.
pub(crate) fn reference_profile_dir() -> PathBuf {
    #[cfg(test)]
    if let Some(profile) = tests::REFERENCE_PROFILE.with(|profile| profile.borrow().clone()) {
        return profile;
    }
    crate::config::AppConfig::config_dir()
}

/// Effective fallback store shared by restore, packing and legacy provenance.
pub fn effective_backup_root() -> PathBuf {
    std::env::var_os("LOCUST_BACKUP_ROOT")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| reference_profile_dir().join("backups"))
}

pub struct BackupManager {
    backup_root: PathBuf,
}

/// Durable reverse references, serialized with deletion across processes. The
/// lock file is never removed: replacing it would create two lock domains.
pub(crate) struct BackupReferenceIndex {
    conn: rusqlite::Connection,
    _lock: Option<fs::File>,
    root: PathBuf,
}

impl BackupReferenceIndex {
    pub(crate) fn open(root: &Path) -> Result<Self> {
        let lock = Self::lock_store(root)?;
        let mut index = Self::open_locked(root)?;
        index._lock = Some(lock);
        Ok(index)
    }

    fn lock_store(root: &Path) -> Result<fs::File> {
        let root = checked_absolute(root)?;
        fs::create_dir_all(&root)?;
        let path = checked_absolute(&root.join(".backup-references.lock"))?;
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            options.custom_flags(0x0020_0000).share_mode(0x0000_0003);
        }
        let lock = options.open(&path)?;
        reject_link(&path, &lock.metadata()?)?;
        lock.lock()?;
        Ok(lock)
    }

    // Caller retains the store lock even if SQLite cannot be opened.
    fn open_locked(root: &Path) -> Result<Self> {
        let root = checked_absolute(root)?;
        let conn = rusqlite::Connection::open(checked_absolute(
            &root.join(".backup-references.sqlite3"),
        )?)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS backup_references (
                backup_id TEXT NOT NULL,
                database_path TEXT NOT NULL,
                PRIMARY KEY (backup_id, database_path)
            );
            CREATE TABLE IF NOT EXISTS indexed_backups (backup_id TEXT PRIMARY KEY)",
        )?;
        Ok(Self {
            conn,
            _lock: None,
            root,
        })
    }

    // Only a newly created backup has complete ownership history in this index.
    // Discovering references in one project cannot establish that for old backups.
    fn mark_authoritative(&self, id: &str) -> Result<()> {
        self.conn
            .execute("INSERT OR IGNORE INTO indexed_backups VALUES (?1)", [id])?;
        Ok(())
    }

    /// Persist uncertainty outside SQLite so recovering a failed index cannot
    /// make an unindexed committed reference appear safe to delete.
    pub(crate) fn mark_unverified(root: &Path, ids: &BTreeSet<String>) {
        let manager = BackupManager::new(root.to_owned());
        for id in ids {
            let mark = || -> Result<()> {
                let dir = manager.resolve_backup_dir(id)?;
                if dir.is_dir() {
                    let path =
                        checked_absolute(&root.join(format!(".backup-reference-unverified-{id}")))?;
                    let mut marker = OpenOptions::new()
                        .write(true)
                        .create(true)
                        .truncate(true)
                        .open(path)?;
                    marker.write_all(b"index update failed")?;
                    marker.sync_all()?;
                }
                Ok(())
            };
            if let Err(error) = mark() {
                tracing::warn!(%error, backup = %id, "Cannot persist backup reference uncertainty");
            }
        }
    }

    pub(crate) fn ensure_backup_exists(&self, backup_id: &str) -> Result<()> {
        let manager = BackupManager::new(self.root.clone());
        if !manager.resolve_backup_dir(backup_id)?.is_dir() {
            return Err(failure("cannot record a reference to a missing backup"));
        }
        Ok(())
    }

    #[cfg(test)]
    fn add(&self, backup_id: &str, database: &Path) -> Result<()> {
        self.ensure_backup_exists(backup_id)?;
        self.conn.execute(
            "INSERT OR IGNORE INTO backup_references VALUES (?1, ?2)",
            rusqlite::params![backup_id, database.to_string_lossy()],
        )?;
        Ok(())
    }

    /// Update known references without changing store-wide completeness: other
    /// unopened legacy projects may still reference any of these backups.
    pub(crate) fn replace_project(&self, database: &Path, ids: &BTreeSet<String>) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM backup_references WHERE database_path = ?1",
            [database.to_string_lossy()],
        )?;
        for id in ids {
            tx.execute(
                "INSERT INTO backup_references VALUES (?1, ?2)",
                rusqlite::params![id, database.to_string_lossy()],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    fn referencing_projects(&self, backup_id: &str) -> Result<(usize, bool)> {
        let mut authoritative: bool = self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM indexed_backups WHERE backup_id = ?1)",
            [backup_id],
            |row| row.get(0),
        )?;
        if self
            .root
            .join(format!(".backup-reference-unverified-{backup_id}"))
            .try_exists()
            .unwrap_or(true)
        {
            authoritative = false;
        }
        let mut stmt = self
            .conn
            .prepare("SELECT database_path FROM backup_references WHERE backup_id = ?1")?;
        let paths = stmt
            .query_map([backup_id], |row| row.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let mut projects = BTreeMap::new();
        for path in paths.into_iter().map(PathBuf::from) {
            projects.insert(crate::database::path_identity_key(&path), path);
        }
        let mut count = 0;
        for database in projects.values() {
            let check = || -> Result<bool> {
                Ok(database.try_exists()?
                    && crate::database::Database::backup_refs_at(database)?
                        .iter()
                        .any(|backup| {
                            backup.id == backup_id
                                && crate::database::paths_identical(
                                    &backup.resolved_storage_root(),
                                    &self.root,
                                )
                        }))
            };
            match check() {
                Ok(true) => count += 1,
                Ok(false) => {
                    if let Err(error) = self.conn.execute(
                        "DELETE FROM backup_references WHERE backup_id = ?1 AND database_path = ?2",
                        rusqlite::params![backup_id, database.to_string_lossy()],
                    ) {
                        tracing::warn!(%error, "Cannot prune stale backup reference");
                        authoritative = false;
                    }
                }
                Err(error) => {
                    tracing::warn!(database = %database.display(), %error, "Cannot verify backup reference");
                    authoritative = false;
                }
            }
        }
        Ok((count, authoritative))
    }
}

/// File changes a restore would make after passing its locked preflight.
/// Paths are relative to `destination` for directory backups; single-file
/// backups report the destination file's own path.
#[derive(Debug, Serialize, Deserialize)]
pub struct RestorePreview {
    pub destination: PathBuf,
    pub replaced: Vec<PathBuf>,
    pub recreated: Vec<PathBuf>,
    pub identical: Vec<PathBuf>,
    /// Planned removals in a preview; actual removals in a restore report.
    pub removed: Vec<PathBuf>,
    pub removed_directories: Vec<PathBuf>,
    pub kept: Vec<RestoreKept>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RestoreKept {
    pub path: PathBuf,
    pub reason: String,
}

impl RestorePreview {
    fn keep(&mut self, path: PathBuf, reason: &str) {
        self.kept.push(RestoreKept {
            path,
            reason: reason.into(),
        });
    }
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

    /// Verify the entire physical original with bounded streaming memory, then
    /// rewind the SAME handle for seekable member reads. No whole-file buffer
    /// or unchecked path is exposed. Callers must keep the original stable and
    /// authenticate the member bytes they consume (as container readers do).
    /// Windows pins the file against deletion but permits core's write-capable
    /// staging ownership handles to coexist with the reader.
    pub fn open_verified(&self) -> Result<fs::File> {
        use std::io::{Seek, SeekFrom};
        let path = checked_absolute(&self.path)?;
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            options.share_mode(0x0000_0003); // FILE_SHARE_READ | FILE_SHARE_WRITE
        }
        let mut file = options.open(path)?;
        if !file.metadata()?.is_file() {
            return Err(failure("revision original is not a regular file"));
        }
        let read =
            crate::patch::stream::stream_bounded(&mut file, self.size, None).map_err(|error| {
                failure(format!(
                    "revision original changed or could not be read: {error:?}"
                ))
            })?;
        if read.actual_len != self.size || read.sha256_hex != self.sha256 {
            return Err(failure(format!(
                "revision original changed since verification: {}",
                self.path.display()
            )));
        }
        file.seek(SeekFrom::Start(0))?;
        Ok(file)
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
        String::from_utf8(self.read_bytes_from(reader)?).map_err(|error| {
            failure(format!(
                "revision original is not UTF-8 ({}): {error}",
                self.path.display()
            ))
        })
    }

    /// Read binary originals with the same immutable size/digest check as text.
    pub fn read_bytes(&self) -> Result<Vec<u8>> {
        let path = checked_absolute(&self.path)?;
        let mut file = fs::File::open(&path)?;
        if !file.metadata()?.is_file() {
            return Err(failure("revision original is not a regular file"));
        }
        self.read_bytes_from(&mut file)
    }

    fn read_bytes_from(&self, reader: &mut dyn std::io::Read) -> Result<Vec<u8>> {
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
        Ok(bytes)
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

/// Readable backups and the IDs and errors of backups whose manifests cannot be read.
#[derive(Debug, Default)]
pub struct BackupListing {
    pub entries: Vec<BackupEntry>,
    pub unreadable: Vec<(String, String)>,
}

/// Payload content bytes in readable, completed backups. On-disk bytes count
/// each physical file once; filesystem allocation/metadata overhead is excluded.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct BackupStorageUsage {
    pub logical_bytes: u64,
    pub on_disk_bytes: u64,
}

type PayloadIndex = BTreeMap<(u64, String), Vec<PathBuf>>;

fn try_link_payload(
    candidates: &[PathBuf],
    destination: &Path,
    size: u64,
    sha256: &str,
    permissions: &fs::Permissions,
    link_file: &mut impl FnMut(&Path, &Path) -> std::io::Result<()>,
) -> Result<bool> {
    let matches = |path: &Path| -> Result<bool> {
        checked_absolute(path)?;
        let meta = fs::metadata(path)?;
        if !meta.is_file() || meta.len() != size || meta.permissions() != *permissions {
            return Ok(false);
        }
        let (digest, actual_size) = sha256_file(path)?;
        Ok(actual_size == size && digest == sha256)
    };
    for candidate in candidates {
        // Manifests are only an index: missing, damaged, or unsafe payloads are
        // never trusted. A different permission set also needs its own copy.
        if !matches(candidate).unwrap_or(false) || link_file(candidate, destination).is_err() {
            continue;
        }
        // Recheck the linked name in case the candidate changed between hashing
        // and linking. Unlink before copying; never truncate a shared inode.
        if matches(destination).unwrap_or(false) {
            return Ok(true);
        }
        fs::remove_file(destination)?;
    }
    Ok(false)
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

/// Large file workloads benefit from concurrent I/O; keep tiny trees serial.
/// Metadata chooses scheduling only, never whether content must be hashed.
pub(crate) fn file_worker_count(files: usize, bytes: u64) -> usize {
    if bytes >= 8 * 1024 * 1024 {
        files.clamp(1, 4)
    } else {
        1
    }
}

/// Owned results in input order. Join every worker, including on failure, so
/// callers retain their source/destination locks until all writes have stopped.
pub(crate) fn bounded_file_work<T: Sync, U: Send>(
    items: &[T],
    workers: usize,
    perform: impl Fn(&T) -> Result<U> + Sync,
) -> Result<Vec<U>> {
    let workers = workers.clamp(1, 4).min(items.len().max(1));
    if workers == 1 {
        return items.iter().map(perform).collect();
    }
    std::thread::scope(|scope| {
        let mut handles = Vec::new();
        for worker in 0..workers {
            let perform = &perform;
            handles.push(scope.spawn(move || {
                items
                    .iter()
                    .enumerate()
                    .skip(worker)
                    .step_by(workers)
                    .map(|(index, item)| perform(item).map(|value| (index, value)))
                    .collect::<Result<Vec<_>>>()
            }));
        }
        // Collect joins before inspecting errors: no worker escapes this scope.
        let joined: Vec<_> = handles.into_iter().map(|handle| handle.join()).collect();
        let mut results = Vec::with_capacity(items.len());
        for entries in joined {
            results.extend(entries.map_err(|_| failure("file worker panicked"))??);
        }
        results.sort_unstable_by_key(|(index, _)| *index);
        Ok(results.into_iter().map(|(_, value)| value).collect())
    })
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
    let mut files = Vec::new();
    let mut bytes = 0u64;
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
        let metadata = fs::symlink_metadata(entry.path())?;
        reject_link(entry.path(), &metadata)?;
        if legacy && rel == Path::new("manifest.json") {
            continue;
        }
        plain_relative(&rel)?;
        if entry.file_type().is_dir() {
            inventory.insert(rel, InventoryEntry::Directory);
        } else {
            bytes = bytes.saturating_add(metadata.len());
            files.push(rel);
        }
    }
    let entries = bounded_file_work(&files, file_worker_count(files.len(), bytes), |rel| {
        let path = checked_absolute(&root.join(rel))?;
        let (sha256, size) = sha256_file(&path)?;
        Ok((rel.clone(), InventoryEntry::File { size, sha256 }))
    })?;
    for (rel, entry) in entries {
        inventory.insert(rel, entry);
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

/// Complete the deletion preflight before restoring even the first original.
/// Only journal-owned files absent from the backup can become candidates.
fn preview_created_outputs(
    target: &Path,
    inventory: &BTreeMap<PathBuf, InventoryEntry>,
    outputs: crate::injection_transaction::CreatedOutputs,
    preview: &mut RestorePreview,
) -> Result<BTreeMap<PathBuf, (String, u64)>> {
    use crate::injection_transaction::{output_key, STORE_DIR};
    let originals: BTreeSet<_> = inventory.keys().map(|p| output_key(p)).collect();
    let mut removals = BTreeMap::new();
    let mut directories = Vec::new();
    let mut walk = WalkDir::new(target)
        .follow_links(false)
        .min_depth(1)
        .into_iter();
    while let Some(entry) = walk.next() {
        let entry = entry.map_err(|e| failure(e.to_string()))?;
        if entry.depth() == 1
            && entry
                .file_name()
                .to_string_lossy()
                .eq_ignore_ascii_case(STORE_DIR)
        {
            // Already validated by created_outputs_since; retain recovery history.
            walk.skip_current_dir();
            continue;
        }
        let relative = entry
            .path()
            .strip_prefix(target)
            .map_err(|e| failure(e.to_string()))?
            .to_owned();
        let key = output_key(&relative);
        let meta = fs::symlink_metadata(entry.path())?;
        if reject_link(entry.path(), &meta).is_err() {
            // WalkDir does not descend symlinks with follow_links(false).
            // Popping its stack for a file/link would skip its siblings too.
            if entry.file_type().is_dir() {
                walk.skip_current_dir();
            }
            preview.keep(relative, "link/reparse/special path is not removed");
            continue;
        }
        if originals.contains(&key) {
            continue;
        }
        if meta.is_dir() {
            if outputs.directories.contains(&key) {
                if meta.permissions().readonly() {
                    preview.keep(relative, "created directory is read-only");
                } else {
                    directories.push(relative);
                }
            } else if outputs.files.contains(&key) {
                preview.keep(relative, "created file is now a directory");
            }
            continue;
        }
        if !outputs.files.contains(&key) {
            preview.keep(relative, "no recorded Locust creation after this backup");
            continue;
        }
        checked_absolute(entry.path())?;
        let current = sha256_file(entry.path())?;
        if !outputs
            .hashes
            .get(&key)
            .is_some_and(|hashes| hashes.contains(&current))
        {
            preview.keep(relative, "SHA256 differs from recorded Locust output");
        } else if meta.permissions().readonly() {
            preview.keep(relative, "file is read-only");
        } else {
            removals.insert(relative, current);
        }
    }
    preview.removed = removals.keys().cloned().collect();
    let mut removable: BTreeSet<_> = removals.keys().map(|p| output_key(p)).collect();
    directories.sort_by_key(|p| (std::cmp::Reverse(p.components().count()), p.clone()));
    for relative in directories {
        checked_absolute(&target.join(&relative))?;
        let mut empty_after_removal = true;
        for child in fs::read_dir(target.join(&relative))? {
            let child = child?;
            if !removable.contains(&output_key(&relative.join(child.file_name()))) {
                empty_after_removal = false;
            }
        }
        if empty_after_removal {
            removable.insert(output_key(&relative));
            preview.removed_directories.push(relative);
        } else {
            preview.keep(
                relative,
                "created directory is not empty after planned removals",
            );
        }
    }
    preview.kept.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(removals)
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
        self.create_backup_with_linker(game_path, |from, to| fs::hard_link(from, to))
    }

    fn create_backup_with_linker(
        &self,
        game_path: &Path,
        mut link_file: impl FnMut(&Path, &Path) -> std::io::Result<()>,
    ) -> Result<BackupEntry> {
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
        // Build once, before claiming the new backup. Only other completed V2
        // inventories participate; legacy backups have no trusted saved hashes.
        let candidates = self.payload_index();
        let now = Utc::now();
        let (id, path) = self.claim_backup_dir(now)?;
        let payload = path.join("payload");
        fs::create_dir(&payload)?;
        for (rel, entry) in &inventory {
            let dest = payload.join(rel);
            match entry {
                InventoryEntry::Directory => fs::create_dir_all(&dest)?,
                InventoryEntry::File { size, sha256 } => {
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
                    if let Some(paths) = candidates.get(&(*size, sha256.clone())) {
                        if try_link_payload(
                            paths,
                            &dest,
                            *size,
                            sha256,
                            &permissions,
                            &mut link_file,
                        )? {
                            continue;
                        }
                    }
                    // Payloads are immutable once published. In particular,
                    // create_new must never open an existing hard link to write.
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
        let indexed = BackupReferenceIndex::open(&self.backup_root)
            .and_then(|index| index.mark_authoritative(&id));
        if let Err(error) = indexed {
            tracing::warn!(%error, backup = %id, "Backup created without authoritative reference index");
            BackupReferenceIndex::mark_unverified(&self.backup_root, &BTreeSet::from([id.clone()]));
        }
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

    fn payload_index(&self) -> PayloadIndex {
        let mut index = PayloadIndex::new();
        let Ok(directories) = fs::read_dir(&self.backup_root) else {
            return index;
        };
        for directory in directories.flatten() {
            let path = directory.path();
            if checked_absolute(&path).is_err() || !path.is_dir() {
                continue;
            }
            let id = directory.file_name().to_string_lossy().into_owned();
            let Ok((summary, Some(manifest))) = Self::load_manifest(&id, &path) else {
                continue;
            };
            if manifest
                .inventory
                .keys()
                .any(|rel| plain_relative(rel).is_err())
                || totals(&manifest.inventory).ok()
                    != Some((summary.file_count, summary.size_bytes))
            {
                continue;
            }
            for (relative, entry) in manifest.inventory {
                if let InventoryEntry::File { size, sha256 } = entry {
                    index
                        .entry((size, sha256))
                        .or_default()
                        .push(path.join("payload").join(relative));
                }
            }
        }
        index
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
    /// removes only unchanged outputs recorded as created by completed Locust
    /// injections after this backup. Other additions are kept with reasons.
    pub fn restore(&self, backup_id: &str) -> Result<()> {
        self.restore_with_report(backup_id).map(|_| ())
    }

    /// Restore and report actual removals and preserved additions under one lock.
    pub fn restore_with_report(&self, backup_id: &str) -> Result<RestorePreview> {
        self.restore_inner(backup_id, false)
    }

    /// Check the same backup, game, and destination preconditions as restore
    /// without replacing any game files.
    pub fn verify_restore(&self, backup_id: &str) -> Result<()> {
        self.preview_restore(backup_id).map(|_| ())
    }

    /// Check restore preconditions and compare inventoried files with their
    /// destinations while holding the game lock, without changing game files.
    pub fn preview_restore(&self, id: &str) -> Result<RestorePreview> {
        self.restore_inner(id, true)
    }

    fn restore_inner(&self, backup_id: &str, dry_run: bool) -> Result<RestorePreview> {
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
        let mut preview = RestorePreview {
            destination: target.clone(),
            replaced: Vec::new(),
            recreated: Vec::new(),
            identical: Vec::new(),
            removed: Vec::new(),
            removed_directories: Vec::new(),
            kept: Vec::new(),
        };
        for (rel, entry) in &inventory {
            let InventoryEntry::File { size, sha256 } = entry else {
                continue;
            };
            let dest = destination(rel);
            let path = if kind == SourceKind::File {
                dest.clone()
            } else {
                rel.clone()
            };
            match fs::symlink_metadata(&dest) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    preview.recreated.push(path);
                }
                Err(error) => return Err(error.into()),
                Ok(meta) if meta.len() != *size => preview.replaced.push(path),
                Ok(_) => {
                    let (digest, actual_size) = sha256_file(&dest)?;
                    if actual_size == *size && digest == *sha256 {
                        preview.identical.push(path);
                    } else {
                        preview.replaced.push(path);
                    }
                }
            }
        }
        preview.replaced.sort();
        preview.recreated.sort();
        preview.identical.sort();
        let removals = if kind == SourceKind::Directory {
            let outputs = crate::injection_transaction::created_outputs_since(
                &game_lock,
                summary.created_at,
            )?;
            preview_created_outputs(&target, &inventory, outputs, &mut preview)?
        } else {
            BTreeMap::new()
        };
        crate::injection_transaction::game_status(game_lock.root())?;
        if dry_run {
            return Ok(preview);
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
                        // Always copy OUT to a fresh inode. A game edit must
                        // never mutate this or another backup's shared payload.
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
        preview.removed.clear();
        for (relative, expected) in removals {
            let dest = target.join(&relative);
            checked_absolute(&dest)?;
            // Recheck against edits by non-cooperating writers after preflight.
            // An edit is preserved, never made deletable by an earlier preview.
            if !dest.is_file() {
                preview.keep(
                    relative,
                    "created file disappeared or changed type during restore",
                );
            } else if sha256_file(&dest)? != expected {
                preview.keep(relative, "SHA256 changed during restore");
            } else if fs::metadata(&dest)?.permissions().readonly() {
                preview.keep(relative, "file became read-only during restore");
            } else {
                fs::remove_file(dest)?;
                preview.removed.push(relative);
            }
        }
        let directories = std::mem::take(&mut preview.removed_directories);
        for relative in directories {
            let dest = target.join(&relative);
            checked_absolute(&dest)?;
            match fs::remove_dir(&dest) {
                Ok(()) => preview.removed_directories.push(relative),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => preview.keep(relative, &format!("created directory kept: {error}")),
            }
        }
        // These fingerprints come from the verified backup inventory, not a
        // guess about game contents. A later backup can reinstate a translation
        // that an earlier restore had removed from the applied history.
        let mut restored_files: BTreeMap<_, _> = inventory
            .iter()
            .filter_map(|(rel, entry)| {
                let InventoryEntry::File { sha256, size } = entry else {
                    return None;
                };
                let dest = destination(rel);
                let relative = dest
                    .strip_prefix(game_lock.root())
                    .expect("locked restore destination");
                Some((
                    crate::injection_transaction::output_key(relative),
                    Some((sha256.clone(), *size)),
                ))
            })
            .collect();
        for path in &preview.removed {
            restored_files.insert(crate::injection_transaction::output_key(path), None);
        }
        crate::injection_transaction::record_backup_restore(
            &game_lock,
            summary.created_at,
            &restored_files,
        )?;
        preview.kept.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(preview)
    }

    pub fn list_backups(&self) -> Result<Vec<BackupEntry>> {
        let report = self.list_backups_report()?;
        if report.entries.is_empty() && !report.unreadable.is_empty() {
            let errors = report
                .unreadable
                .iter()
                .map(|(id, error)| format!("{id}: {error}"))
                .collect::<Vec<_>>()
                .join("; ");
            return Err(failure(format!("no readable backups: {errors}")));
        }
        Ok(report.entries)
    }

    /// Count logical inventory bytes and unique physical payload content bytes
    /// without reading file contents. This is a snapshot; concurrent deletion
    /// can produce an I/O error. Unreadable manifests are skipped as in listing.
    pub fn storage_usage(&self) -> Result<BackupStorageUsage> {
        let mut usage = BackupStorageUsage::default();
        let mut seen = BTreeSet::new();
        for backup in self.list_backups_report()?.entries {
            let (summary, manifest) = Self::load_manifest(&backup.id, &backup.path)?;
            let logical = match &manifest {
                Some(manifest) => totals(&manifest.inventory)?.1,
                None => summary.size_bytes,
            };
            usage.logical_bytes = usage
                .logical_bytes
                .checked_add(logical)
                .ok_or_else(|| failure("backup size overflow"))?;
            let payload = if manifest.is_some() {
                backup.path.join("payload")
            } else {
                backup.path.clone()
            };
            checked_absolute(&payload)?;
            for entry in WalkDir::new(&payload).follow_links(false).min_depth(1) {
                let entry = entry.map_err(|e| failure(e.to_string()))?;
                reject_link(entry.path(), &fs::symlink_metadata(entry.path())?)?;
                if !entry.file_type().is_file()
                    || (manifest.is_none() && entry.path() == backup.path.join("manifest.json"))
                {
                    continue;
                }
                let file = fs::File::open(entry.path())?;
                if seen.insert(crate::file_identity::identity(&file)?) {
                    usage.on_disk_bytes = usage
                        .on_disk_bytes
                        .checked_add(file.metadata()?.len())
                        .ok_or_else(|| failure("backup size overflow"))?;
                }
            }
        }
        Ok(usage)
    }

    /// Report manifest failures individually while keeping filesystem and link-safety errors fatal.
    pub fn list_backups_report(&self) -> Result<BackupListing> {
        checked_absolute(&self.backup_root)?;
        let mut report = BackupListing::default();
        if !self.backup_root.exists() {
            return Ok(report);
        }
        for dir in fs::read_dir(&self.backup_root)? {
            let dir = dir?;
            reject_link(&dir.path(), &fs::symlink_metadata(dir.path())?)?;
            if !dir.file_type()?.is_dir() || !dir.path().join("manifest.json").exists() {
                continue;
            }
            let id = dir.file_name().to_string_lossy().to_string();
            // Preserve load_manifest's path-safety failures as fatal listing errors.
            checked_absolute(&dir.path().join("manifest.json"))?;
            let (m, _) = match Self::load_manifest(&id, &dir.path()) {
                Ok(manifest) => manifest,
                Err(error) => {
                    report.unreadable.push((id, error.to_string()));
                    continue;
                }
            };
            report.entries.push(BackupEntry {
                id,
                path: dir.path(),
                source_path: m.source_path,
                created_at: m.created_at,
                file_count: m.file_count,
                size_bytes: m.size_bytes,
            });
        }
        report
            .entries
            .sort_by(|a, b| b.created_at.cmp(&a.created_at));
        Ok(report)
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
        self.delete_backup_with_confirmation(backup_id, false)
    }

    /// Confirmation applies only to unverified backups, never live references.
    pub fn delete_backup_with_confirmation(
        &self,
        backup_id: &str,
        confirm_unverified: bool,
    ) -> Result<()> {
        let dir = self.resolve_backup_dir(backup_id)?;
        if !dir.try_exists()? {
            return Ok(());
        }
        let unverified =
            || failure("Locust cannot confirm that no project still needs this backup");
        // Retain cross-process exclusion through removal, including corrupt-index overrides.
        let _lock = BackupReferenceIndex::lock_store(&self.backup_root).map_err(|error| {
            tracing::warn!(%error, "Backup reference store is unavailable");
            unverified()
        })?;
        let status = BackupReferenceIndex::open_locked(&self.backup_root)
            .and_then(|index| index.referencing_projects(backup_id));
        let (projects, authoritative) = status.unwrap_or_else(|error| {
            tracing::warn!(%error, "Backup reference index is unavailable");
            (0, false)
        });
        if projects != 0 {
            return Err(failure(format!(
                "backup is referenced by {projects} project(s); release their references before deleting it"
            )));
        }
        if !authoritative && !confirm_unverified {
            return Err(unverified());
        }
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

    thread_local! {
        pub(super) static REFERENCE_PROFILE: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
    }

    struct ReferenceProfile {
        _directory: tempfile::TempDir,
    }

    impl ReferenceProfile {
        fn isolated() -> Self {
            let directory = tempfile::tempdir().unwrap();
            REFERENCE_PROFILE
                .with(|profile| *profile.borrow_mut() = Some(directory.path().to_owned()));
            Self {
                _directory: directory,
            }
        }
    }

    impl Drop for ReferenceProfile {
        fn drop(&mut self) {
            REFERENCE_PROFILE.with(|profile| *profile.borrow_mut() = None);
        }
    }

    fn referenced_backup_fixture() -> (
        tempfile::TempDir,
        BackupManager,
        BackupEntry,
        crate::database::Database,
    ) {
        let temp = tempfile::tempdir().unwrap();
        let game = temp.path().join("game-a");
        fs::create_dir(&game).unwrap();
        let file = game.join("story.txt");
        fs::write(&file, b"original").unwrap();
        let manager = BackupManager::new(temp.path().join("backups"));
        let entry = manager.create_backup(&game).unwrap();
        let db = crate::database::Database::open(&temp.path().join("a.db")).unwrap();
        let backup = crate::database::RecordedBackup {
            id: entry.id.clone(),
            source_path: game.clone(),
            storage_root: Some(manager.root().to_owned()),
        };
        db.record_injection_with_backup(Some("es"), &game, &[file], Some(&backup))
            .unwrap();
        (temp, manager, entry, db)
    }

    fn pre_index_project(
        path: &Path,
        entry: &BackupEntry,
        store: Option<&Path>,
        registration: bool,
    ) {
        let db = crate::database::Database::open(path).unwrap();
        let (root, file) = if entry.source_path.is_file() {
            (
                entry.source_path.parent().unwrap(),
                entry.source_path.clone(),
            )
        } else {
            (
                entry.source_path.as_path(),
                entry.source_path.join("story.txt"),
            )
        };
        db.record_injection(Some("es"), root, &[file]).unwrap();
        drop(db);
        let reference = crate::database::RecordedBackup {
            id: entry.id.clone(),
            source_path: entry.source_path.clone(),
            storage_root: store.map(Path::to_path_buf),
        };
        let conn = rusqlite::Connection::open(path).unwrap();
        let column = if registration {
            "registration_backup"
        } else {
            "pristine_backup"
        };
        conn.execute(
            &format!("UPDATE injected_files SET {column} = ?1"),
            [serde_json::to_string(&reference).unwrap()],
        )
        .unwrap();
    }

    #[test]
    fn backup_custom_location_pre_upgrade_is_unverified() {
        let (temp, manager, entry, db) = referenced_backup_fixture();
        let path = temp.path().join("arbitrary-output/deep/custom.db");
        pre_index_project(&path, &entry, Some(manager.root()), false);
        drop(db);
        fs::remove_file(manager.root().join(".backup-references.sqlite3")).unwrap();
        let before = fs::read(&path).unwrap();
        let error = manager.delete_backup(&entry.id).unwrap_err().to_string();
        assert!(
            error.contains("Locust cannot confirm that no project still needs this backup"),
            "{error}"
        );
        assert_eq!(fs::read(path).unwrap(), before);
        assert!(entry.path.exists());
        manager
            .delete_backup_with_confirmation(&entry.id, true)
            .unwrap();
        assert!(!entry.path.exists());
    }

    #[test]
    fn backup_corrupt_index_does_not_block_record_or_release() {
        for release in [false, true] {
            let (_temp, manager, entry, db) = referenced_backup_fixture();
            let recording = db.get_injection(Some("es")).unwrap().unwrap();
            fs::write(
                manager.root().join(".backup-references.sqlite3"),
                b"invalid index",
            )
            .unwrap();
            if release {
                db.release_backup_reference(recording.pristine_backup.as_ref().unwrap())
                    .unwrap();
            } else {
                db.record_injection(
                    Some("es"),
                    &recording.root,
                    &[recording.root.join("story.txt")],
                )
                .unwrap();
            }
            assert!(db
                .get_injection(Some("es"))
                .unwrap()
                .unwrap()
                .pristine_backup
                .is_none());
            let error = manager.delete_backup(&entry.id).unwrap_err().to_string();
            assert!(
                error.contains("Locust cannot confirm that no project still needs this backup"),
                "{error}"
            );
            assert!(entry.path.exists());
            // A recovered empty SQLite index must retain the uncertainty marker.
            fs::remove_file(manager.root().join(".backup-references.sqlite3")).unwrap();
            BackupReferenceIndex::open(manager.root())
                .unwrap()
                .mark_authoritative(&entry.id)
                .unwrap();
            assert!(manager.delete_backup(&entry.id).is_err());
            manager
                .delete_backup_with_confirmation(&entry.id, true)
                .unwrap();
        }
    }

    #[test]
    fn backup_legacy_override_store_is_protected() {
        if std::env::var_os("LOCUST_CORE8_OVERRIDE_CHILD").is_none() {
            let temp = tempfile::tempdir().unwrap();
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "backup::tests::backup_legacy_override_store_is_protected",
                    "--nocapture",
                ])
                .env("LOCUST_CORE8_OVERRIDE_CHILD", "1")
                .env("LOCUST_DATA_DIR", temp.path().join("profile"))
                .env("LOCUST_BACKUP_ROOT", temp.path().join("override"))
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let game = temp.path().join("game");
        fs::create_dir(&game).unwrap();
        let file = game.join("story.txt");
        fs::write(&file, b"original").unwrap();
        let manager = BackupManager::new(std::env::var_os("LOCUST_BACKUP_ROOT").unwrap().into());
        let entry = manager.create_backup(&game).unwrap();
        let db = crate::database::Database::open(&temp.path().join("custom.db")).unwrap();
        let reference = crate::database::RecordedBackup {
            id: entry.id.clone(),
            source_path: game.clone(),
            storage_root: None,
        };
        db.record_injection_with_backup(Some("es"), &game, &[file], Some(&reference))
            .unwrap();
        let error = manager.delete_backup(&entry.id).unwrap_err().to_string();
        assert!(error.contains("referenced by 1 project(s)"), "{error}");
    }

    #[test]
    fn backup_delete_checks_unopened_pre_index_projects() {
        let _profile = ReferenceProfile::isolated();
        for location in ["adjacent", "current-profile", "origin-profile"] {
            for registration in [false, true] {
                let temp = tempfile::tempdir().unwrap();
                let game = temp.path().join("game-a");
                fs::create_dir(&game).unwrap();
                fs::write(game.join("story.txt"), b"original").unwrap();
                let manager = BackupManager::new(temp.path().join("backups"));
                let entry = manager.create_backup(&game).unwrap();
                let path = if location == "adjacent" {
                    temp.path().join("game-a.locust.db")
                } else {
                    let hash = crate::database::sha256_hex(game.to_string_lossy().as_bytes());
                    let profile = if location == "origin-profile" {
                        temp.path().to_owned()
                    } else {
                        reference_profile_dir()
                    };
                    profile
                        .join("projects")
                        .join(format!("game-a-{}.locust.db", &hash[..8]))
                };
                pre_index_project(&path, &entry, Some(manager.root()), registration);
                let before = fs::read(&path).unwrap();
                fs::remove_file(manager.root().join(".backup-references.sqlite3")).unwrap();
                let error = manager.delete_backup(&entry.id).unwrap_err().to_string();
                assert!(
                    error.contains("Locust cannot confirm that no project still needs this backup"),
                    "{error}"
                );
                assert!(entry.path.is_dir());
                assert_eq!(fs::read(&path).unwrap(), before);
                fs::remove_file(path).unwrap();
            }
        }
    }

    #[test]
    fn backup_delete_checks_unopened_single_file_origin_and_folder_projects() {
        for folder_project in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let game = temp.path().join("game-a");
            fs::create_dir(&game).unwrap();
            let file = game.join("story.txt");
            fs::write(&file, b"original").unwrap();
            let manager = BackupManager::new(temp.path().join("backups"));
            let entry = manager.create_backup(&file).unwrap();
            let path = if folder_project {
                temp.path().join("game-a.locust.db")
            } else {
                game.join("story.txt.locust.db")
            };
            pre_index_project(&path, &entry, Some(manager.root()), false);
            fs::remove_file(manager.root().join(".backup-references.sqlite3")).unwrap();
            let error = manager.delete_backup(&entry.id).unwrap_err().to_string();
            assert!(
                error.contains("Locust cannot confirm that no project still needs this backup"),
                "{error}"
            );
            assert!(entry.path.is_dir());
        }
    }

    #[test]
    fn backup_delete_does_not_scan_unrelated_unreadable_project() {
        let (temp, manager, entry, db) = referenced_backup_fixture();
        db.release_backup_reference(
            &db.get_injection(Some("es"))
                .unwrap()
                .unwrap()
                .pristine_backup
                .unwrap(),
        )
        .unwrap();
        fs::write(temp.path().join("game-a.locust.db"), b"corrupt project").unwrap();
        manager.delete_backup(&entry.id).unwrap();
        assert!(!entry.path.exists());
    }

    #[test]
    fn backup_delete_uses_listed_projects_without_origin_discovery() {
        let (temp, manager, entry, db) = referenced_backup_fixture();
        pre_index_project(
            &temp.path().join("game-a.locust.db"),
            &entry,
            Some(manager.root()),
            false,
        );
        drop(db);
        let error = manager.delete_backup(&entry.id).unwrap_err().to_string();
        assert!(error.contains("referenced by 1 project(s)"), "{error}");
        assert!(entry.path.is_dir());
    }

    #[test]
    fn backup_authoritative_delete_does_not_depend_on_origin_manifest() {
        for damage in ["missing", "corrupt", "relative"] {
            let temp = tempfile::tempdir().unwrap();
            let game = temp.path().join("game");
            fs::create_dir(&game).unwrap();
            let manager = BackupManager::new(temp.path().join("backups"));
            let entry = manager.create_backup(&game).unwrap();
            let manifest = entry.path.join("manifest.json");
            match damage {
                "missing" => fs::remove_file(&manifest).unwrap(),
                "corrupt" => fs::write(&manifest, b"invalid").unwrap(),
                _ => {
                    let mut value: serde_json::Value =
                        serde_json::from_slice(&fs::read(&manifest).unwrap()).unwrap();
                    value["source_path"] = serde_json::json!("relative-game");
                    fs::write(&manifest, serde_json::to_vec(&value).unwrap()).unwrap();
                }
            }
            manager.delete_backup(&entry.id).unwrap();
            assert!(!entry.path.exists());
        }
    }

    #[test]
    fn backup_open_healthy_project_with_unavailable_index() {
        let (_temp, manager, entry, db) = referenced_backup_fixture();
        let path = db.path();
        let recording = db.get_injection(Some("es")).unwrap();
        drop(db);
        fs::write(
            manager.root().join(".backup-references.sqlite3"),
            b"invalid index",
        )
        .unwrap();
        let reopened = crate::database::Database::open(&path)
            .expect("a broken index must not block a healthy project");
        assert_eq!(reopened.get_injection(Some("es")).unwrap(), recording);
        reopened
            .set_project_metadata("still-usable", &serde_json::json!(true))
            .unwrap();
        assert_eq!(
            reopened.get_project_metadata("still-usable").unwrap(),
            Some(serde_json::json!(true))
        );
        assert!(manager.delete_backup(&entry.id).is_err());
        assert!(entry.path.is_dir());
    }

    #[test]
    fn backup_legacy_default_store_is_indexed_on_record_and_reopen() {
        let _profile = ReferenceProfile::isolated();
        let temp = tempfile::tempdir().unwrap();
        let game = temp.path().join("legacy-game");
        fs::create_dir(&game).unwrap();
        let file = game.join("story.txt");
        fs::write(&file, b"original").unwrap();
        let manager = BackupManager::new(reference_profile_dir().join("backups"));
        let entry = manager.create_backup(&game).unwrap();
        let path = temp.path().join("custom.db");
        let db = crate::database::Database::open(&path).unwrap();
        let reference = crate::database::RecordedBackup {
            id: entry.id.clone(),
            source_path: entry.source_path.clone(),
            storage_root: None,
        };
        db.record_injection_with_backup(Some("es"), &game, &[file], Some(&reference))
            .unwrap();
        for reopen in [false, true] {
            if reopen {
                let index = BackupReferenceIndex::open(manager.root()).unwrap();
                index
                    .conn
                    .execute(
                        "DELETE FROM backup_references WHERE backup_id = ?1",
                        [&entry.id],
                    )
                    .unwrap();
                drop(index);
                drop(crate::database::Database::open(&path).unwrap());
            }
            let index = BackupReferenceIndex::open(manager.root()).unwrap();
            let count: usize = index
                .conn
                .query_row(
                    "SELECT COUNT(*) FROM backup_references WHERE backup_id = ?1",
                    [&entry.id],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(
                count, 1,
                "legacy provenance must be indexed; reopen={reopen}"
            );
            drop(index);
            assert!(manager.delete_backup(&entry.id).is_err());
        }
        let resolved = crate::database::RecordedBackup {
            storage_root: Some(manager.root().to_owned()),
            ..reference
        };
        db.release_backup_reference(&resolved).unwrap();
        assert!(db
            .get_injection(Some("es"))
            .unwrap()
            .unwrap()
            .pristine_backup
            .is_none());
        manager.delete_backup(&entry.id).unwrap();
    }

    #[test]
    fn backup_delete_checks_unopened_legacy_default_store_project() {
        let _profile = ReferenceProfile::isolated();
        let temp = tempfile::tempdir().unwrap();
        let game = temp.path().join("game-a");
        fs::create_dir(&game).unwrap();
        fs::write(game.join("story.txt"), b"original").unwrap();
        let manager = BackupManager::new(reference_profile_dir().join("backups"));
        let entry = manager.create_backup(&game).unwrap();
        let path = temp.path().join("game-a.locust.db");
        pre_index_project(&path, &entry, None, false);
        fs::remove_file(manager.root().join(".backup-references.sqlite3")).unwrap();
        let error = manager.delete_backup(&entry.id).unwrap_err().to_string();
        assert!(
            error.contains("Locust cannot confirm that no project still needs this backup"),
            "{error}"
        );
        assert!(entry.path.is_dir());
        fs::remove_file(path).unwrap();
        manager
            .delete_backup_with_confirmation(&entry.id, true)
            .unwrap();
    }

    #[test]
    fn backup_delete_checks_persisted_project_references_after_restart() {
        let (_temp, manager, entry, db) = referenced_backup_fixture();
        drop(db);
        let restarted = BackupManager::new(manager.root().to_owned());
        let error = restarted.delete_backup(&entry.id).unwrap_err().to_string();
        assert!(error.contains("referenced by 1 project(s)"), "{error}");
        assert!(entry.path.exists());
    }

    #[test]
    fn backup_open_existing_project_backfills_reference_index() {
        let (_temp, manager, entry, db) = referenced_backup_fixture();
        let path = db.path();
        drop(db);
        // Simulate provenance written by a release predating the reverse index.
        fs::remove_file(manager.root().join(".backup-references.sqlite3")).unwrap();
        let reopened = crate::database::Database::open(&path).unwrap();
        drop(reopened);
        let restarted = BackupManager::new(manager.root().to_owned());
        assert!(restarted.delete_backup(&entry.id).is_err());
        assert!(entry.path.exists());
    }

    #[test]
    fn backup_delete_cleans_deleted_project_reference() {
        let (_temp, manager, entry, db) = referenced_backup_fixture();
        let path = db.path();
        drop(db);
        fs::remove_file(&path).unwrap();
        manager.delete_backup(&entry.id).unwrap();
        assert!(!entry.path.exists());
        assert!(
            !path.exists(),
            "read-only checks must not recreate a database"
        );
        let index = BackupReferenceIndex::open(manager.root()).unwrap();
        assert_eq!(
            index
                .conn
                .query_row("SELECT COUNT(*) FROM backup_references", [], |row| row
                    .get::<_, usize>(0))
                .unwrap(),
            0
        );
    }

    #[test]
    fn backup_delete_rechecks_released_reference_read_only() {
        let (_temp, manager, entry, db) = referenced_backup_fixture();
        let path = db.path();
        drop(db);
        // Simulate a project release outside this process, leaving a stale index.
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute("DELETE FROM injected_files", []).unwrap();
        drop(conn);
        let before = fs::read(&path).unwrap();
        manager.delete_backup(&entry.id).unwrap();
        assert_eq!(fs::read(path).unwrap(), before);
        assert!(!entry.path.exists());
    }

    #[test]
    fn backup_delete_counts_projects_once_across_languages() {
        let (temp, manager, entry, db) = referenced_backup_fixture();
        let reference = db
            .get_injection(Some("es"))
            .unwrap()
            .unwrap()
            .pristine_backup
            .unwrap();
        let file = reference.source_path.join("story.txt");
        db.record_injection_with_backup(
            Some("fr"),
            &reference.source_path,
            std::slice::from_ref(&file),
            Some(&reference),
        )
        .unwrap();
        let second = crate::database::Database::open(&temp.path().join("b.db")).unwrap();
        second
            .record_injection_with_backup(None, &reference.source_path, &[file], Some(&reference))
            .unwrap();
        let error = manager.delete_backup(&entry.id).unwrap_err().to_string();
        assert!(error.contains("referenced by 2 project(s)"), "{error}");
    }

    #[test]
    fn backup_confirmation_never_overrides_live_reference_with_unreadable_peer() {
        let (temp, manager, entry, _db) = referenced_backup_fixture();
        let corrupt = temp.path().join("corrupt.db");
        fs::write(&corrupt, b"invalid database").unwrap();
        BackupReferenceIndex::open(manager.root())
            .unwrap()
            .add(&entry.id, &corrupt)
            .unwrap();
        let error = manager
            .delete_backup_with_confirmation(&entry.id, true)
            .unwrap_err()
            .to_string();
        assert!(error.contains("referenced by 1 project(s)"), "{error}");
        assert!(entry.path.exists());
    }

    #[test]
    fn backup_uncertainty_marker_never_becomes_legacy_game_content() {
        let temp = tempfile::tempdir().unwrap();
        let game = temp.path().join("game");
        fs::create_dir(&game).unwrap();
        let manager = BackupManager::new(temp.path().join("backups"));
        let dir = manager.root().join("legacy");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("story.txt"), b"original").unwrap();
        let manifest = BackupManifest {
            source_path: game.clone(),
            created_at: Utc::now(),
            file_count: 1,
            size_bytes: 8,
        };
        fs::write(
            dir.join("manifest.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        BackupReferenceIndex::mark_unverified(manager.root(), &BTreeSet::from(["legacy".into()]));
        manager.restore("legacy").unwrap();
        assert_eq!(fs::read(game.join("story.txt")).unwrap(), b"original");
        assert_eq!(
            fs::read_dir(&game).unwrap().count(),
            1,
            "index bookkeeping must never be restored as game content"
        );
    }

    #[test]
    fn backup_failed_recording_does_not_authorize_unverified_backup() {
        let temp = tempfile::tempdir().unwrap();
        let game = temp.path().join("game");
        fs::create_dir(&game).unwrap();
        let file = game.join("story.txt");
        fs::write(&file, b"original").unwrap();
        let manager = BackupManager::new(temp.path().join("backups"));
        let entry = manager.create_backup(&game).unwrap();
        fs::remove_file(manager.root().join(".backup-references.sqlite3")).unwrap();
        let path = temp.path().join("project.db");
        let db = crate::database::Database::open(&path).unwrap();
        let conn = rusqlite::Connection::open(path).unwrap();
        conn.execute_batch("CREATE TRIGGER fail_recording BEFORE INSERT ON injected_files BEGIN SELECT RAISE(FAIL, 'project write failed'); END;").unwrap();
        let reference = crate::database::RecordedBackup {
            id: entry.id.clone(),
            source_path: game.clone(),
            storage_root: Some(manager.root().to_owned()),
        };
        assert!(db
            .record_injection_with_backup(Some("es"), &game, &[file], Some(&reference))
            .is_err());
        assert!(db.get_injection(Some("es")).unwrap().is_none());
        let error = manager.delete_backup(&entry.id).unwrap_err().to_string();
        assert!(error.contains("Locust cannot confirm"), "{error}");
    }

    #[test]
    fn backup_successful_record_and_release_cannot_authorize_legacy_ownership() {
        let (_temp, manager, entry, db) = referenced_backup_fixture();
        let recording = db.get_injection(Some("es")).unwrap().unwrap();
        let reference = recording.pristine_backup.as_ref().unwrap();
        // A successful new recording against an old backup is still not proof
        // that every other project's pre-index provenance has been discovered.
        fs::remove_file(manager.root().join(".backup-references.sqlite3")).unwrap();
        db.record_injection_with_backup(
            Some("fr"),
            &recording.root,
            &[recording.root.join("story.txt")],
            Some(reference),
        )
        .unwrap();
        let error = manager
            .delete_backup_with_confirmation(&entry.id, true)
            .unwrap_err()
            .to_string();
        assert!(error.contains("referenced by 1 project(s)"), "{error}");
        db.release_backup_reference(reference).unwrap();
        let error = manager.delete_backup(&entry.id).unwrap_err().to_string();
        assert!(error.contains("Locust cannot confirm"), "{error}");
        assert_eq!(
            fs::read(entry.path.join("payload/story.txt")).unwrap(),
            b"original"
        );
        manager
            .delete_backup_with_confirmation(&entry.id, true)
            .unwrap();
        assert!(!entry.path.exists());
    }

    #[test]
    fn backup_recording_still_refuses_missing_modern_backup() {
        let (_temp, manager, entry, db) = referenced_backup_fixture();
        let recording = db.get_injection(Some("es")).unwrap().unwrap();
        db.release_backup_reference(recording.pristine_backup.as_ref().unwrap())
            .unwrap();
        manager.delete_backup(&entry.id).unwrap();
        let error = db
            .record_injection_with_backup(
                Some("fr"),
                &recording.root,
                &[recording.root.join("story.txt")],
                recording.pristine_backup.as_ref(),
            )
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("cannot record a reference to a missing backup"),
            "{error}"
        );
        assert!(db.get_injection(Some("fr")).unwrap().is_none());
    }

    #[test]
    fn backup_index_write_failure_does_not_block_recording_or_release() {
        let (_temp, manager, entry, db) = referenced_backup_fixture();
        let recording = db.get_injection(Some("es")).unwrap().unwrap();
        {
            let index = BackupReferenceIndex::open(manager.root()).unwrap();
            index.conn.execute_batch("CREATE TRIGGER fail_reference_update BEFORE INSERT ON backup_references BEGIN SELECT RAISE(FAIL, 'index write failure'); END;").unwrap();
        }
        db.record_injection_with_backup(
            Some("fr"),
            &recording.root,
            &[recording.root.join("story.txt")],
            recording.pristine_backup.as_ref(),
        )
        .unwrap();
        db.release_backup_reference(recording.pristine_backup.as_ref().unwrap())
            .unwrap();
        let error = manager.delete_backup(&entry.id).unwrap_err().to_string();
        assert!(error.contains("Locust cannot confirm"), "{error}");
        manager
            .delete_backup_with_confirmation(&entry.id, true)
            .unwrap();
    }

    #[test]
    fn backup_delete_preserves_reference_when_project_is_unreadable() {
        let (_temp, manager, entry, db) = referenced_backup_fixture();
        let path = db.path();
        drop(db);
        fs::write(path, b"corrupt sqlite").unwrap();
        assert!(manager.delete_backup(&entry.id).is_err());
        assert!(entry.path.exists());
    }

    #[test]
    fn backup_delete_tracks_registration_originals_and_replacement() {
        let (_temp, manager, entry, db) = referenced_backup_fixture();
        let recording = db.get_injection(Some("es")).unwrap().unwrap();
        let registration = manager.create_backup(&recording.root).unwrap();
        let reference = crate::database::RecordedBackup {
            id: registration.id.clone(),
            source_path: recording.root.clone(),
            storage_root: Some(manager.root().to_owned()),
        };
        let menu = recording.root.join("menu.txt");
        fs::write(&menu, b"menu").unwrap();
        db.extend_injection_with_registration_backup(
            &recording,
            std::slice::from_ref(&menu),
            &reference,
        )
        .unwrap();
        assert!(manager.delete_backup(&registration.id).is_err());
        // An ordinary replacement releases both original associations.
        db.record_injection(Some("es"), &recording.root, &[menu])
            .unwrap();
        manager.delete_backup(&registration.id).unwrap();
        manager.delete_backup(&entry.id).unwrap();
    }

    #[test]
    fn backup_retention_cannot_delete_referenced_backup() {
        let (_temp, manager, entry, _db) = referenced_backup_fixture();
        assert!(manager.delete_old_backups(0).is_err());
        assert!(entry.path.exists());
    }

    #[test]
    fn backup_explicit_release_updates_index_and_preserves_outputs() {
        let (_temp, manager, entry, db) = referenced_backup_fixture();
        let recording = db.get_injection(Some("es")).unwrap().unwrap();
        let reference = recording.pristine_backup.as_ref().unwrap();
        db.release_backup_reference(reference).unwrap();
        let released = db.get_injection(Some("es")).unwrap().unwrap();
        assert!(released.pristine_backup.is_none());
        assert_eq!(released.files, recording.files);
        {
            let index = BackupReferenceIndex::open(manager.root()).unwrap();
            assert_eq!(
                index
                    .conn
                    .query_row("SELECT COUNT(*) FROM backup_references", [], |row| row
                        .get::<_, usize>(0))
                    .unwrap(),
                0
            );
        }
        manager.delete_backup(&entry.id).unwrap();
        assert!(!entry.path.exists());
    }

    #[test]
    fn backup_release_keeps_other_languages_and_other_store_references() {
        let (_temp, manager, entry, db) = referenced_backup_fixture();
        let es = db.get_injection(Some("es")).unwrap().unwrap();
        let reference = es.pristine_backup.as_ref().unwrap();
        let menu = es.root.join("menu.txt");
        fs::write(&menu, b"menu").unwrap();
        let other = manager.create_backup(&es.root).unwrap();
        let other_reference = crate::database::RecordedBackup {
            id: other.id.clone(),
            ..reference.clone()
        };
        db.record_injection_with_backup(
            Some("fr"),
            &es.root,
            std::slice::from_ref(&menu),
            Some(&other_reference),
        )
        .unwrap();
        db.extend_injection_with_registration_backup(&es, &[menu], reference)
            .unwrap();
        let wrong_store = crate::database::RecordedBackup {
            storage_root: Some(manager.root().join("other-store")),
            ..reference.clone()
        };
        db.release_backup_reference(&wrong_store).unwrap();
        assert!(manager.delete_backup(&entry.id).is_err());
        db.release_backup_reference(reference).unwrap();
        assert!(db.registration_backups(Some("es")).unwrap().is_empty());
        assert_eq!(
            db.get_injection(Some("fr"))
                .unwrap()
                .unwrap()
                .pristine_backup,
            Some(other_reference)
        );
        manager.delete_backup(&entry.id).unwrap();
        assert!(manager.delete_backup(&other.id).is_err());
    }

    fn c127_inject(manager: &BackupManager, game: &Path, outputs: &[(&str, &str)]) -> BackupEntry {
        crate::injection_transaction::run(
            game,
            "fixture",
            Some("en"),
            || manager.create_backup(game),
            |work, _| {
                for (path, bytes) in outputs {
                    let dest = work.join(path);
                    fs::create_dir_all(dest.parent().unwrap())?;
                    fs::write(dest, bytes)?;
                }
                Ok(crate::extraction::InjectionReport {
                    files_modified: outputs.len(),
                    strings_written: outputs.len(),
                    strings_skipped: 0,
                    skip_reasons: Default::default(),
                    warnings: vec![],
                    files_written: vec![],
                })
            },
            |_| Ok(()),
        )
        .unwrap()
        .0
    }

    #[test]
    fn c127_restore_removes_only_unchanged_created_outputs() {
        let temp = tempfile::tempdir().unwrap();
        let game = temp.path().join("game");
        fs::create_dir(&game).unwrap();
        fs::write(game.join("patch.xp3"), "original").unwrap();
        let manager = BackupManager::new(temp.path().join("backups"));
        let backup = c127_inject(
            &manager,
            &game,
            &[
                ("patch.xp3", "replacement"),
                ("patch3.xp3", "translation"),
                ("tl/en/edited.rpy", "translation"),
                ("tl/en/clean.rpy", "translation"),
            ],
        );
        fs::write(game.join("tl/en/edited.rpy"), "translatioN").unwrap();
        fs::write(game.join("save.dat"), "save").unwrap();
        let preview = manager.preview_restore(&backup.id).unwrap();
        assert_eq!(
            preview.removed,
            [
                PathBuf::from("patch3.xp3"),
                PathBuf::from("tl/en/clean.rpy")
            ]
        );
        assert!(preview
            .kept
            .iter()
            .any(|k| k.path == Path::new("tl/en/edited.rpy") && k.reason.contains("SHA256")));
        assert!(preview
            .kept
            .iter()
            .any(|k| k.path == Path::new("save.dat") && k.reason.contains("no recorded")));
        assert!(game.join("patch3.xp3").exists(), "preview must not delete");
        let report = manager.restore_with_report(&backup.id).unwrap();
        assert_eq!(report.removed, preview.removed);
        assert_eq!(
            fs::read_to_string(game.join("patch.xp3")).unwrap(),
            "original"
        );
        assert_eq!(
            fs::read_to_string(game.join("tl/en/edited.rpy")).unwrap(),
            "translatioN"
        );
        assert_eq!(fs::read_to_string(game.join("save.dat")).unwrap(), "save");
        assert!(
            !game.join("patch3.xp3").exists(),
            "restore left a Locust-created patch"
        );
        assert!(!game.join("tl/en/clean.rpy").exists());
        assert!(game.join("tl/en").is_dir(), "non-empty directory must stay");
    }

    #[test]
    fn c127_restore_tracks_multiple_injections_and_backup_boundary() {
        let temp = tempfile::tempdir().unwrap();
        let game = temp.path().join("game");
        fs::create_dir(&game).unwrap();
        fs::write(game.join("original"), "original").unwrap();
        let manager = BackupManager::new(temp.path().join("backups"));
        let first = c127_inject(&manager, &game, &[("tl/en/script.rpy", "v1")]);
        let second = c127_inject(
            &manager,
            &game,
            &[("tl/en/script.rpy", "v2"), ("patch.xp3", "new")],
        );
        manager.restore(&second.id).unwrap();
        assert_eq!(
            fs::read_to_string(game.join("tl/en/script.rpy")).unwrap(),
            "v1"
        );
        assert!(!game.join("patch.xp3").exists());
        // A new completed injection puts the latest recorded bytes back.
        c127_inject(&manager, &game, &[("tl/en/script.rpy", "v3")]);
        manager.restore(&first.id).unwrap();
        assert!(
            !game.join("tl").exists(),
            "empty created directories must be removed"
        );
        assert!(game.join("original").exists());
    }

    #[test]
    fn c127_bad_creation_evidence_fails_before_any_restore_write() {
        let temp = tempfile::tempdir().unwrap();
        let game = temp.path().join("game");
        fs::create_dir(&game).unwrap();
        fs::write(game.join("original"), "original").unwrap();
        let manager = BackupManager::new(temp.path().join("backups"));
        let backup = c127_inject(
            &manager,
            &game,
            &[("original", "changed"), ("patch.xp3", "new")],
        );
        let operations = game.join(".locust-injections/operations");
        let operation = fs::read_dir(operations)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        fs::write(operation.join("plan.json"), "invalid").unwrap();
        assert!(manager.preview_restore(&backup.id).is_err());
        assert!(manager.restore(&backup.id).is_err());
        assert_eq!(
            fs::read_to_string(game.join("original")).unwrap(),
            "changed"
        );
        assert_eq!(fs::read_to_string(game.join("patch.xp3")).unwrap(), "new");
    }

    #[test]
    fn c127_undated_earlier_and_incomplete_operations_never_authorize_deletion() {
        for variant in [
            "undated",
            "earlier",
            "aborted",
            "rolled_back",
            "not_created",
        ] {
            let temp = tempfile::tempdir().unwrap();
            let game = temp.path().join("game");
            fs::create_dir(&game).unwrap();
            let manager = BackupManager::new(temp.path().join("backups"));
            let backup = c127_inject(&manager, &game, &[("patch.xp3", "translation")]);
            let operation = fs::read_dir(game.join(".locust-injections/operations"))
                .unwrap()
                .next()
                .unwrap()
                .unwrap()
                .path();
            let path = operation.join("plan.json");
            let mut plan: serde_json::Value =
                serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            match variant {
                "undated" => {
                    plan.as_object_mut().unwrap().remove("prepared_at");
                }
                "earlier" => {
                    plan["prepared_at"] =
                        serde_json::to_value(backup.created_at - chrono::Duration::seconds(1))
                            .unwrap();
                }
                "not_created" => {
                    plan["files"][0]["original"] = plan["files"][0]["result"].clone();
                }
                _ => fs::write(
                    operation.join("phase.json"),
                    serde_json::to_vec(variant).unwrap(),
                )
                .unwrap(),
            }
            fs::write(path, serde_json::to_vec(&plan).unwrap()).unwrap();
            let report = manager.restore_with_report(&backup.id).unwrap();
            assert!(report.removed.is_empty(), "{variant}");
            assert!(
                report
                    .kept
                    .iter()
                    .any(|k| k.path == Path::new("patch.xp3") && k.reason.contains("no recorded")),
                "{variant}"
            );
            assert_eq!(
                fs::read_to_string(game.join("patch.xp3")).unwrap(),
                "translation"
            );
        }
    }

    #[test]
    fn c127_foreign_unsafe_or_bad_hash_evidence_is_rejected_before_writes() {
        for variant in ["root", "path", "hash", "id", "duplicate"] {
            let temp = tempfile::tempdir().unwrap();
            let game = temp.path().join("game");
            fs::create_dir(&game).unwrap();
            fs::write(game.join("original"), "original").unwrap();
            let manager = BackupManager::new(temp.path().join("backups"));
            let backup = c127_inject(
                &manager,
                &game,
                &[("original", "changed"), ("patch.xp3", "new")],
            );
            let operation = fs::read_dir(game.join(".locust-injections/operations"))
                .unwrap()
                .next()
                .unwrap()
                .unwrap()
                .path();
            let path = operation.join("plan.json");
            let mut plan: serde_json::Value =
                serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            match variant {
                "root" => plan["game_root"] = serde_json::json!(temp.path()),
                "path" => plan["files"][0]["path"] = serde_json::json!("../outside"),
                "hash" => plan["files"][0]["result"]["sha256"] = serde_json::json!("invalid"),
                "id" => {
                    plan["transaction_id"] = serde_json::json!(uuid::Uuid::new_v4().to_string())
                }
                _ => {
                    let file = plan["files"][0].clone();
                    plan["files"].as_array_mut().unwrap().push(file);
                }
            }
            fs::write(path, serde_json::to_vec(&plan).unwrap()).unwrap();
            assert!(manager.preview_restore(&backup.id).is_err(), "{variant}");
            assert!(manager.restore(&backup.id).is_err(), "{variant}");
            assert_eq!(
                fs::read_to_string(game.join("original")).unwrap(),
                "changed"
            );
            assert!(game.join("patch.xp3").exists());
        }
    }

    #[test]
    fn c127_removal_obeys_game_lock_and_preserves_readonly_and_changed_types() {
        let temp = tempfile::tempdir().unwrap();
        let game = temp.path().join("game");
        fs::create_dir(&game).unwrap();
        let manager = BackupManager::new(temp.path().join("backups"));
        let backup = c127_inject(
            &manager,
            &game,
            &[("patch.xp3", "translation"), ("other", "new")],
        );
        let lock = GameLock::acquire(&game).unwrap();
        assert!(manager.restore(&backup.id).is_err());
        assert!(manager.preview_restore(&backup.id).is_err());
        assert!(game.join("patch.xp3").exists());
        drop(lock);
        let original_permissions = fs::metadata(game.join("patch.xp3")).unwrap().permissions();
        let mut readonly = original_permissions.clone();
        readonly.set_readonly(true);
        fs::set_permissions(game.join("patch.xp3"), readonly).unwrap();
        fs::remove_file(game.join("other")).unwrap();
        fs::create_dir(game.join("other")).unwrap();
        let report = manager.restore_with_report(&backup.id).unwrap();
        fs::set_permissions(game.join("patch.xp3"), original_permissions).unwrap();
        assert!(report.removed.is_empty());
        assert!(report.kept.iter().any(|k| k.reason.contains("read-only")));
        assert!(report
            .kept
            .iter()
            .any(|k| k.reason.contains("now a directory")));
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn c127_kept_links_do_not_hide_other_created_files() {
        for directory in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let game = temp.path().join("game");
            fs::create_dir(&game).unwrap();
            let manager = BackupManager::new(temp.path().join("backups"));
            let backup = c127_inject(
                &manager,
                &game,
                &[("a", "translation"), ("z", "translation")],
            );
            let outside = temp.path().join("outside");
            if directory {
                fs::create_dir(&outside).unwrap();
            } else {
                fs::write(&outside, "external").unwrap();
            }
            fs::remove_file(game.join("a")).unwrap();
            #[cfg(windows)]
            if directory {
                std::os::windows::fs::symlink_dir(&outside, game.join("a")).unwrap();
            } else {
                std::os::windows::fs::symlink_file(&outside, game.join("a")).unwrap();
            }
            #[cfg(unix)]
            std::os::unix::fs::symlink(&outside, game.join("a")).unwrap();
            let report = manager.restore_with_report(&backup.id).unwrap();
            assert_eq!(report.removed, [PathBuf::from("z")]);
            assert!(report
                .kept
                .iter()
                .any(|k| k.path == Path::new("a") && k.reason.contains("link")));
            assert!(outside.exists());
            assert!(fs::symlink_metadata(game.join("a"))
                .unwrap()
                .file_type()
                .is_symlink());
        }
    }

    #[test]
    fn revision_seekable_reader_verifies_and_rewinds_without_buffering() {
        use std::io::{Read, Seek, SeekFrom};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("original.pck");
        let bytes = vec![0xa5; 2 * 1024 * 1024 + 17];
        fs::write(&path, &bytes).unwrap();
        let original = RevisionOriginal::capture(&path).unwrap();
        {
            let mut file = original.open_verified().unwrap();
            assert_eq!(file.stream_position().unwrap(), 0);
            file.seek(SeekFrom::End(-17)).unwrap();
            let mut tail = [0; 17];
            file.read_exact(&mut tail).unwrap();
            assert_eq!(tail, [0xa5; 17]);
            #[cfg(windows)]
            assert!(fs::remove_file(&path).is_err());
        }
        for changed in [
            vec![0xa6; bytes.len()],
            vec![0; bytes.len() + 1],
            vec![0; 9],
        ] {
            fs::write(&path, changed).unwrap();
            assert!(original.open_verified().is_err());
        }
        fs::remove_file(&path).unwrap();
        assert!(original.open_verified().is_err());
        fs::create_dir(&path).unwrap();
        assert!(original.open_verified().is_err());
    }

    #[test]
    fn revision_seekable_reader_accepts_core_pristine_staging_ownership_handles() {
        let dir = tempfile::tempdir().unwrap();
        let stage = crate::patch::stream::StagingDir::create_prepared(dir.path()).unwrap();
        let mut output = stage.create_file("original.pck").unwrap();
        output.write_all(b"original bytes").unwrap();
        drop(output);
        // StagingDir still owns a cloned write-capable identity handle, just
        // as with_verified_pristine_tree does for single-file backups.
        let original = RevisionOriginal::capture(&stage.child("original.pck")).unwrap();
        let mut verified = original.open_verified().unwrap();
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(&mut verified, &mut bytes).unwrap();
        assert_eq!(bytes, b"original bytes");
    }

    #[test]
    fn c122_revision_binary_reader_verifies_exact_consumed_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app.asar");
        let bytes = [0, 255, 128, 42];
        fs::write(&path, bytes).unwrap();
        let original = RevisionOriginal::capture(&path).unwrap();
        assert_eq!(original.read_bytes().unwrap(), bytes);
        assert!(original
            .read_text()
            .unwrap_err()
            .to_string()
            .contains("not UTF-8"));
        for changed in [vec![0, 255, 128, 43], vec![0], vec![0; 5]] {
            fs::write(&path, changed).unwrap();
            assert!(original.read_bytes().is_err());
        }
        fs::remove_file(&path).unwrap();
        assert!(original.read_bytes().is_err());
        fs::create_dir(&path).unwrap();
        assert!(original.read_bytes().is_err());
    }

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

    fn tempdir() -> tempfile::TempDir {
        tempfile::Builder::new()
            .prefix("locust_bak_")
            .tempdir()
            .unwrap()
    }

    fn create_game_dir() -> tempfile::TempDir {
        let dir_guard = tempdir();
        let dir = dir_guard.path().to_path_buf();
        fs::write(dir.join("data.json"), r#"{"hp": 100}"#).unwrap();
        fs::write(dir.join("strings.txt"), "Hello\nWorld").unwrap();
        fs::create_dir_all(dir.join("sub")).unwrap();
        fs::write(dir.join("sub").join("nested.txt"), "nested content").unwrap();
        dir_guard
    }

    #[test]
    fn backup_dedupe_unchanged_tree_shares_every_payload_file() {
        let game = create_game_dir();
        let root = tempdir();
        let manager = BackupManager::new(root.path().to_owned());
        let first = manager.create_backup(game.path()).unwrap();
        let second = manager.create_backup(game.path()).unwrap();
        let expected = scan_source(game.path()).unwrap();
        for (rel, entry) in &expected {
            if matches!(entry, InventoryEntry::File { .. }) {
                assert_payload_sharing(&first, &second, rel, true);
            }
        }
        assert_eq!(
            manager.storage_usage().unwrap(),
            BackupStorageUsage {
                logical_bytes: first.size_bytes * 2,
                on_disk_bytes: first.size_bytes,
            }
        );
        for backup in [&first, &second] {
            assert_eq!(scan(&backup.path.join("payload"), false).unwrap(), expected);
            manager
                .with_pristine_tree(&backup.id, game.path(), |_| Ok(()))
                .unwrap();
        }
    }

    #[test]
    fn backup_dedupe_both_backups_restore_and_verify() {
        let game = create_game_dir();
        let root = tempdir();
        let manager = BackupManager::new(root.path().to_owned());
        let first = manager.create_backup(game.path()).unwrap();
        let second = manager.create_backup(game.path()).unwrap();
        assert_payload_sharing(&first, &second, Path::new("strings.txt"), true);
        let expected = scan_source(game.path()).unwrap();
        for backup in [&first, &second] {
            fs::write(game.path().join("data.json"), "changed").unwrap();
            manager.restore(&backup.id).unwrap();
            manager.verify_restore(&backup.id).unwrap();
            assert_eq!(scan_source(game.path()).unwrap(), expected);
        }
    }

    fn assert_payload_sharing(first: &BackupEntry, second: &BackupEntry, rel: &Path, shared: bool) {
        let a = fs::File::open(first.path.join("payload").join(rel)).unwrap();
        let b = fs::File::open(second.path.join("payload").join(rel)).unwrap();
        assert_eq!(crate::file_identity::same_file(&a, &b), shared, "{rel:?}");
        #[cfg(any(unix, windows))]
        if shared {
            assert_eq!(crate::file_identity::link_count(&a).unwrap(), 2);
            assert_eq!(crate::file_identity::link_count(&b).unwrap(), 2);
        }
    }

    #[test]
    fn backup_dedupe_revision_copies_only_changed_file() {
        let game = create_game_dir();
        let root = tempdir();
        let manager = BackupManager::new(root.path().to_owned());
        let first = manager.create_backup(game.path()).unwrap();
        fs::write(game.path().join("data.json"), "one-line revision").unwrap();
        let second = manager.create_backup(game.path()).unwrap();
        assert_payload_sharing(&first, &second, Path::new("data.json"), false);
        assert_payload_sharing(&first, &second, Path::new("strings.txt"), true);
        assert_payload_sharing(&first, &second, Path::new("sub/nested.txt"), true);
        assert_eq!(
            manager.storage_usage().unwrap(),
            BackupStorageUsage {
                logical_bytes: first.size_bytes + second.size_bytes,
                on_disk_bytes: first.size_bytes + b"one-line revision".len() as u64,
            }
        );
        manager
            .with_pristine_tree(&second.id, game.path(), |_| Ok(()))
            .unwrap();
    }

    #[test]
    fn backup_dedupe_corrupt_candidate_is_copied_even_at_same_size() {
        for corrupt in [b"corrupt!".as_slice(), b"short", b"longer corrupted file"] {
            let game = tempdir();
            let root = tempdir();
            fs::write(game.path().join("a"), b"original").unwrap();
            let manager = BackupManager::new(root.path().to_owned());
            let first = manager.create_backup(game.path()).unwrap();
            fs::write(first.path.join("payload/a"), corrupt).unwrap();
            let second = manager.create_backup(game.path()).unwrap();
            assert_payload_sharing(&first, &second, Path::new("a"), false);
            assert_eq!(
                fs::read(second.path.join("payload/a")).unwrap(),
                b"original"
            );
            manager
                .with_pristine_tree(&second.id, game.path(), |_| Ok(()))
                .unwrap();
        }
    }

    #[test]
    fn backup_dedupe_delete_and_retention_preserve_shared_payload() {
        for retention in [false, true] {
            let game = create_game_dir();
            let root = tempdir();
            let manager = BackupManager::new(root.path().to_owned());
            let first = manager.create_backup(game.path()).unwrap();
            let second = manager.create_backup(game.path()).unwrap();
            assert_payload_sharing(&first, &second, Path::new("strings.txt"), true);
            let expected = scan_source(game.path()).unwrap();
            if retention {
                assert_eq!(manager.delete_old_backups(1).unwrap(), 1);
            } else {
                manager.delete_backup(&first.id).unwrap();
            }
            assert!(!first.path.exists());
            assert_eq!(scan(&second.path.join("payload"), false).unwrap(), expected);
            manager
                .with_pristine_tree(&second.id, game.path(), |_| Ok(()))
                .unwrap();
            assert_eq!(
                manager.storage_usage().unwrap(),
                BackupStorageUsage {
                    logical_bytes: second.size_bytes,
                    on_disk_bytes: second.size_bytes,
                }
            );
            #[cfg(any(unix, windows))]
            assert_eq!(
                crate::file_identity::link_count(
                    &fs::File::open(second.path.join("payload/strings.txt")).unwrap()
                )
                .unwrap(),
                1
            );
        }
    }

    #[test]
    fn backup_dedupe_deleted_older_backup_newer_restores_exactly() {
        let game = create_game_dir();
        let root = tempdir();
        let manager = BackupManager::new(root.path().to_owned());
        let first = manager.create_backup(game.path()).unwrap();
        let second = manager.create_backup(game.path()).unwrap();
        let expected = scan_source(game.path()).unwrap();
        manager.delete_backup(&first.id).unwrap();
        fs::write(game.path().join("strings.txt"), "modified").unwrap();
        manager.restore(&second.id).unwrap();
        manager.verify_restore(&second.id).unwrap();
        assert_eq!(scan_source(game.path()).unwrap(), expected);
    }

    #[test]
    fn backup_dedupe_restore_then_modify_does_not_mutate_shared_payloads() {
        let game = create_game_dir();
        let root = tempdir();
        let manager = BackupManager::new(root.path().to_owned());
        let first = manager.create_backup(game.path()).unwrap();
        let second = manager.create_backup(game.path()).unwrap();
        assert_payload_sharing(&first, &second, Path::new("strings.txt"), true);
        let expected = scan_source(game.path()).unwrap();
        // Exercise both replacement and recreation during restore.
        fs::write(game.path().join("data.json"), "changed").unwrap();
        fs::remove_file(game.path().join("strings.txt")).unwrap();
        manager.restore(&second.id).unwrap();
        for backup in [&first, &second] {
            assert!(!crate::file_identity::same_file(
                &fs::File::open(game.path().join("strings.txt")).unwrap(),
                &fs::File::open(backup.path.join("payload/strings.txt")).unwrap(),
            ));
        }
        fs::write(game.path().join("strings.txt"), "user edit after restore").unwrap();
        fs::write(game.path().join("data.json"), "another edit").unwrap();
        for backup in [&first, &second] {
            assert_eq!(scan(&backup.path.join("payload"), false).unwrap(), expected);
            manager
                .with_pristine_tree(&backup.id, game.path(), |_| Ok(()))
                .unwrap();
        }
    }

    #[test]
    fn bounded_file_work_preserves_order_and_caps_workers() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let active = AtomicUsize::new(0);
        let peak = AtomicUsize::new(0);
        let barrier = std::sync::Barrier::new(4);
        let items: Vec<_> = (0..16).collect();
        let output = bounded_file_work(&items, 99, |index| {
            let count = active.fetch_add(1, Ordering::SeqCst) + 1;
            peak.fetch_max(count, Ordering::SeqCst);
            if *index < 4 {
                barrier.wait();
            }
            active.fetch_sub(1, Ordering::SeqCst);
            Ok(index * 2)
        })
        .unwrap();
        assert_eq!(
            output,
            items.iter().map(|index| index * 2).collect::<Vec<_>>()
        );
        assert_eq!(peak.load(Ordering::SeqCst), 4);
        assert_eq!(active.load(Ordering::SeqCst), 0);
        assert_eq!(file_worker_count(20, 7 * 1024 * 1024), 1);
        assert_eq!(file_worker_count(20, 8 * 1024 * 1024), 4);
    }

    #[test]
    fn bounded_file_work_joins_all_workers_before_returning_error() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let finished = AtomicUsize::new(0);
        let barrier = std::sync::Barrier::new(4);
        let result = bounded_file_work(&[0, 1, 2, 3], 4, |index| {
            barrier.wait();
            if *index == 0 {
                return Err(failure("injected worker failure"));
            }
            finished.fetch_add(1, Ordering::SeqCst);
            Ok(())
        });
        assert!(result.is_err());
        assert_eq!(finished.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn backup_source_after_copy_rejects_same_size_restored_timestamp_drift() {
        let game = tempdir();
        let root = tempdir();
        let source = game.path().join("asset.bin");
        fs::write(&source, b"original").unwrap();
        // Cross the parallel-hashing threshold without depending on hard links.
        for name in ["large-a.bin", "large-b.bin"] {
            fs::write(game.path().join(name), vec![0x5a; 4 * 1024 * 1024]).unwrap();
        }
        let manager = BackupManager::new(root.path().to_owned());
        let first = manager.create_backup(game.path()).unwrap();
        let before = fs::metadata(&source).unwrap();
        let mut attempts = 0;
        let result = manager.create_backup_with_linker(game.path(), |_, _| {
            attempts += 1;
            fs::write(&source, b"modified")?;
            fs::File::options()
                .write(true)
                .open(&source)?
                .set_times(fs::FileTimes::new().set_modified(before.modified()?))?;
            Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "force independent copy after source drift",
            ))
        });
        assert!(attempts >= 1);
        assert!(result.is_err());
        assert_eq!(fs::metadata(&source).unwrap().len(), before.len());
        assert_eq!(
            fs::metadata(&source).unwrap().modified().unwrap(),
            before.modified().unwrap()
        );
        assert_eq!(manager.list_backups().unwrap().len(), 1);
        assert_eq!(
            fs::read(first.path.join("payload/asset.bin")).unwrap(),
            b"original"
        );
    }

    #[test]
    fn backup_dedupe_link_failure_falls_back_to_copy() {
        let game = create_game_dir();
        let root = tempdir();
        let manager = BackupManager::new(root.path().to_owned());
        let first = manager.create_backup(game.path()).unwrap();
        let mut attempts = 0;
        let second = manager
            .create_backup_with_linker(game.path(), |_, _| {
                attempts += 1;
                Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "injected hard-link failure",
                ))
            })
            .unwrap();
        assert_eq!(attempts, first.file_count);
        for (rel, entry) in scan_source(game.path()).unwrap() {
            if matches!(entry, InventoryEntry::File { .. }) {
                assert_payload_sharing(&first, &second, &rel, false);
            }
        }
        assert_eq!(
            manager.storage_usage().unwrap(),
            BackupStorageUsage {
                logical_bytes: first.size_bytes * 2,
                on_disk_bytes: first.size_bytes * 2,
            }
        );
        manager
            .with_pristine_tree(&second.id, game.path(), |_| Ok(()))
            .unwrap();
    }

    #[test]
    fn backup_dedupe_changed_candidate_after_check_is_unlinked_before_copy() {
        let game = tempdir();
        let root = tempdir();
        fs::write(game.path().join("a"), b"original").unwrap();
        let manager = BackupManager::new(root.path().to_owned());
        let first = manager.create_backup(game.path()).unwrap();
        let mut attempts = 0;
        let second = manager
            .create_backup_with_linker(game.path(), |from, to| {
                attempts += 1;
                fs::write(from, b"corrupt!")?;
                fs::hard_link(from, to)
            })
            .unwrap();
        assert_eq!(attempts, 1);
        assert_payload_sharing(&first, &second, Path::new("a"), false);
        assert_eq!(fs::read(first.path.join("payload/a")).unwrap(), b"corrupt!");
        assert_eq!(
            fs::read(second.path.join("payload/a")).unwrap(),
            b"original"
        );
        manager
            .with_pristine_tree(&second.id, game.path(), |_| Ok(()))
            .unwrap();
    }

    #[test]
    fn backup_dedupe_skips_damaged_manifests_and_missing_candidates() {
        for damage in ["json", "version", "totals", "traversal", "missing"] {
            let game = create_game_dir();
            let root = tempdir();
            let manager = BackupManager::new(root.path().to_owned());
            let first = manager.create_backup(game.path()).unwrap();
            let bad = root.path().join("damaged");
            fs::create_dir(&bad).unwrap();
            let mut manifest: serde_json::Value =
                serde_json::from_slice(&fs::read(first.path.join("manifest.json")).unwrap())
                    .unwrap();
            match damage {
                "version" => manifest["version"] = serde_json::json!(999),
                "totals" => manifest["size_bytes"] = serde_json::json!(999),
                "traversal" => {
                    let file = manifest["inventory"]["data.json"].clone();
                    manifest["inventory"]["../escape"] = file;
                }
                _ => {}
            }
            fs::write(
                bad.join("manifest.json"),
                if damage == "json" {
                    b"{".to_vec()
                } else {
                    serde_json::to_vec(&manifest).unwrap()
                },
            )
            .unwrap();
            let second = manager.create_backup(game.path()).unwrap();
            assert_payload_sharing(&first, &second, Path::new("data.json"), true);
            manager
                .with_pristine_tree(&second.id, game.path(), |_| Ok(()))
                .unwrap();
            assert!(!root.path().join("escape").exists());
        }
    }

    #[test]
    fn backup_dedupe_matches_content_across_names_and_single_file_backups() {
        let game = tempdir();
        let root = tempdir();
        let file = game.path().join("original-name");
        fs::write(&file, b"original").unwrap();
        let manager = BackupManager::new(root.path().to_owned());
        let first = manager.create_backup(&file).unwrap();
        fs::rename(&file, game.path().join("new-name")).unwrap();
        let second = manager.create_backup(game.path()).unwrap();
        assert!(crate::file_identity::same_file(
            &fs::File::open(first.path.join("payload/file")).unwrap(),
            &fs::File::open(second.path.join("payload/new-name")).unwrap(),
        ));
        manager
            .with_pristine_tree(&second.id, game.path(), |_| Ok(()))
            .unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn backup_dedupe_permission_difference_uses_independent_copy() {
        use std::os::unix::fs::PermissionsExt;
        let game = tempdir();
        let root = tempdir();
        let file = game.path().join("runner");
        fs::write(&file, b"original").unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).unwrap();
        let manager = BackupManager::new(root.path().to_owned());
        let first = manager.create_backup(game.path()).unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o755)).unwrap();
        let second = manager.create_backup(game.path()).unwrap();
        assert_payload_sharing(&first, &second, Path::new("runner"), false);
        assert_eq!(
            fs::metadata(first.path.join("payload/runner"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o644
        );
        fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
        manager.restore(&second.id).unwrap();
        assert_eq!(
            fs::metadata(file).unwrap().permissions().mode() & 0o777,
            0o755
        );
    }

    #[test]
    fn test_create_backup_copies_files() {
        let game_dir_guard = create_game_dir();
        let game_dir = game_dir_guard.path().to_path_buf();
        let backup_root_guard = tempdir();
        let backup_root = backup_root_guard.path().to_path_buf();
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
        let game_dir_guard = create_game_dir();
        let game_dir = game_dir_guard.path().to_path_buf();
        let backup_root_guard = tempdir();
        let backup_root = backup_root_guard.path().to_path_buf();
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
        let game_dir_guard = create_game_dir();
        let game_dir = game_dir_guard.path().to_path_buf();
        let backup_root_guard = tempdir();
        let backup_root = backup_root_guard.path().to_path_buf();
        let mgr = BackupManager::new(backup_root);
        let _b1 = mgr.create_backup(&game_dir).unwrap();
        let _b2 = mgr.create_backup(&game_dir).unwrap();

        let list = mgr.list_backups().unwrap();
        assert_eq!(list.len(), 2);
        assert!(list[0].created_at >= list[1].created_at);
    }

    fn damaged_backup(root: &Path, id: &str) -> PathBuf {
        let dir = root.join(id);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("manifest.json"), b"{").unwrap();
        dir
    }

    #[test]
    fn listing_keeps_readable_backups_and_reports_damaged_manifests() {
        let game = tempfile::tempdir().unwrap();
        let backups = tempfile::tempdir().unwrap();
        fs::write(game.path().join("story.txt"), b"original").unwrap();
        let mgr = BackupManager::new(backups.path().to_owned());
        let valid = mgr.create_backup(game.path()).unwrap();
        damaged_backup(backups.path(), "damaged");

        let entries = mgr.list_backups().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, valid.id);
        let report = mgr.list_backups_report().unwrap();
        assert_eq!(report.entries.len(), 1);
        assert_eq!(report.entries[0].id, valid.id);
        assert_eq!(report.unreadable.len(), 1);
        assert_eq!(report.unreadable[0].0, "damaged");
        assert!(report.unreadable[0].1.contains("manifest.json"));
        assert!(report.unreadable[0].1.contains("EOF"));
    }

    #[test]
    fn listing_only_damaged_backups_is_an_error_naming_each_id() {
        let backups = tempfile::tempdir().unwrap();
        let mgr = BackupManager::new(backups.path().to_owned());
        for (index, id) in ["damaged-first", "damaged-second"].iter().enumerate() {
            damaged_backup(backups.path(), id);
            let report = mgr.list_backups_report().unwrap();
            assert!(report.entries.is_empty());
            assert_eq!(report.unreadable.len(), index + 1);
            let error = mgr.list_backups().unwrap_err().to_string();
            for (unreadable_id, _) in &report.unreadable {
                assert!(error.contains(unreadable_id), "{error}");
            }
        }
    }

    #[test]
    fn pruning_keeps_damaged_backups() {
        let game = tempfile::tempdir().unwrap();
        let backups = tempfile::tempdir().unwrap();
        fs::write(game.path().join("story.txt"), b"original").unwrap();
        let mgr = BackupManager::new(backups.path().to_owned());
        let valid = mgr.create_backup(game.path()).unwrap();
        let damaged = damaged_backup(backups.path(), "damaged");
        fs::write(damaged.join("original.txt"), b"recoverable original").unwrap();

        assert_eq!(mgr.delete_old_backups(0).unwrap(), 1);
        assert!(!valid.path.exists());
        assert!(damaged.is_dir());
        assert_eq!(fs::read(damaged.join("manifest.json")).unwrap(), b"{");
        assert_eq!(
            fs::read(damaged.join("original.txt")).unwrap(),
            b"recoverable original"
        );
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn listing_rejects_linked_backup_directories_and_manifests() {
        for manifest_link in [false, true] {
            let game = tempfile::tempdir().unwrap();
            let backups = tempfile::tempdir().unwrap();
            fs::write(game.path().join("story.txt"), b"original").unwrap();
            let mgr = BackupManager::new(backups.path().to_owned());
            let valid = mgr.create_backup(game.path()).unwrap();
            let linked = backups.path().join("linked");
            let (target, link) = if manifest_link {
                fs::create_dir(&linked).unwrap();
                (
                    valid.path.join("manifest.json"),
                    linked.join("manifest.json"),
                )
            } else {
                (valid.path.clone(), linked)
            };
            #[cfg(windows)]
            let result = if manifest_link {
                std::os::windows::fs::symlink_file(&target, &link)
            } else {
                std::os::windows::fs::symlink_dir(&target, &link)
            };
            #[cfg(unix)]
            let result = std::os::unix::fs::symlink(&target, &link);
            if let Err(error) = result {
                eprintln!("skipping link-safety test: cannot create link: {error}");
                return;
            }
            let error = mgr.list_backups().unwrap_err().to_string();
            assert!(
                error.contains("unsupported link/reparse/special path"),
                "{error}"
            );
            let error = mgr.list_backups_report().unwrap_err().to_string();
            assert!(
                error.contains("unsupported link/reparse/special path"),
                "{error}"
            );
        }
    }

    #[test]
    fn test_backup_id_traversal_is_rejected() {
        // Ids arrive from HTTP (`POST /api/backups/:id/restore` and delete),
        // so a traversal must not reach outside the backup root — delete in
        // particular would remove the directory tree it lands on.
        let backup_root_guard = tempdir();
        let backup_root = backup_root_guard.path().to_path_buf();
        let outsider_guard = tempfile::Builder::new()
            .prefix("locust_outsider_")
            .tempdir_in(backup_root.parent().unwrap())
            .unwrap();
        let outsider = outsider_guard.path().to_path_buf();
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
        let game_dir_guard = create_game_dir();
        let game_dir = game_dir_guard.path().to_path_buf();
        let entry = mgr.create_backup(&game_dir).unwrap();
        mgr.restore(&entry.id).unwrap();
        mgr.delete_backup(&entry.id).unwrap();
    }

    #[test]
    fn test_same_second_backups_do_not_share_a_directory() {
        // Backup ids are second-resolution timestamps. Two injects within the
        // same second must not land in one directory, or each overwrites the
        // other's manifest and restore hands back a mix of both games.
        let backup_root_guard = tempdir();
        let backup_root = backup_root_guard.path().to_path_buf();
        let mgr = BackupManager::new(backup_root);

        let game_a_guard = create_game_dir();

        let game_a = game_a_guard.path().to_path_buf();
        let game_b_guard = tempdir();
        let game_b = game_b_guard.path().to_path_buf();
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
    fn preview_restore_classifies_changed_missing_and_identical() {
        let game = tempfile::tempdir().unwrap();
        let backups = tempfile::tempdir().unwrap();
        fs::create_dir(game.path().join("nested")).unwrap();
        fs::create_dir(game.path().join("empty")).unwrap();
        fs::write(game.path().join("same-length.rpy"), b"original").unwrap();
        fs::write(game.path().join("different-length.rpy"), b"short").unwrap();
        fs::write(game.path().join("nested/missing.rpy"), b"recreate me").unwrap();
        fs::write(game.path().join("identical.rpy"), b"unchanged").unwrap();
        let mgr = BackupManager::new(backups.path().to_owned());
        let backup = mgr.create_backup(game.path()).unwrap();

        fs::write(game.path().join("same-length.rpy"), b"modified").unwrap();
        fs::write(game.path().join("different-length.rpy"), b"longer change").unwrap();
        fs::remove_file(game.path().join("nested/missing.rpy")).unwrap();
        fs::write(game.path().join("added.rpy"), b"keep this new file").unwrap();

        let snapshot = |root: &Path| -> BTreeMap<PathBuf, Vec<u8>> {
            WalkDir::new(root)
                .into_iter()
                .map(|entry| entry.unwrap())
                .filter(|entry| entry.file_type().is_file())
                .map(|entry| {
                    (
                        entry.path().strip_prefix(root).unwrap().to_owned(),
                        fs::read(entry.path()).unwrap(),
                    )
                })
                .collect()
        };
        let game_before = snapshot(game.path());
        let backup_before = snapshot(&backup.path);
        let preview = mgr.preview_restore(&backup.id).unwrap();
        assert_eq!(preview.destination, game.path().canonicalize().unwrap());
        assert_eq!(
            preview.replaced,
            vec![
                PathBuf::from("different-length.rpy"),
                PathBuf::from("same-length.rpy"),
            ]
        );
        assert_eq!(preview.recreated, vec![PathBuf::from("nested/missing.rpy")]);
        assert_eq!(preview.identical, vec![PathBuf::from("identical.rpy")]);
        assert_eq!(snapshot(game.path()), game_before);
        assert_eq!(snapshot(&backup.path), backup_before);

        mgr.verify_restore(&backup.id).unwrap();
        assert_eq!(snapshot(game.path()), game_before);
        assert_eq!(snapshot(&backup.path), backup_before);

        let mut expected = game_before;
        for path in preview.replaced.iter().chain(&preview.recreated) {
            expected.insert(
                path.clone(),
                fs::read(backup.path.join("payload").join(path)).unwrap(),
            );
        }
        mgr.restore(&backup.id).unwrap();
        assert_eq!(snapshot(game.path()), expected);
        assert_eq!(snapshot(&backup.path), backup_before);
        assert_eq!(
            fs::read(game.path().join("added.rpy")).unwrap(),
            b"keep this new file"
        );
    }

    #[test]
    fn preview_restore_single_file_reports_original_path() {
        let game = tempfile::tempdir().unwrap();
        let backups = tempfile::tempdir().unwrap();
        let file = game.path().join("script.rpy");
        fs::write(&file, b"original").unwrap();
        let mgr = BackupManager::new(backups.path().to_owned());
        let backup = mgr.create_backup(&file).unwrap();
        let destination = file.canonicalize().unwrap();

        let preview = mgr.preview_restore(&backup.id).unwrap();
        assert_eq!(preview.destination, destination);
        assert!(preview.replaced.is_empty());
        assert!(preview.recreated.is_empty());
        assert_eq!(preview.identical, vec![destination.clone()]);

        fs::write(&file, b"modified").unwrap();
        let preview = mgr.preview_restore(&backup.id).unwrap();
        assert_eq!(preview.destination, destination);
        assert_eq!(preview.replaced, vec![destination]);
        assert!(preview.recreated.is_empty());
        assert!(preview.identical.is_empty());
        assert_eq!(fs::read(&file).unwrap(), b"modified");
        assert_eq!(
            fs::read(backup.path.join("payload/file")).unwrap(),
            b"original"
        );
    }

    #[test]
    fn test_restore_overwrites_target() {
        let game_dir_guard = create_game_dir();
        let game_dir = game_dir_guard.path().to_path_buf();
        let backup_root_guard = tempdir();
        let backup_root = backup_root_guard.path().to_path_buf();
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
        let game_a_guard = create_game_dir();
        let game_a = game_a_guard.path().to_path_buf();
        let game_b_guard = tempdir();
        let game_b = game_b_guard.path().to_path_buf();
        fs::write(game_b.join("b_only.txt"), "keep-b").unwrap();

        let backup_root_guard = tempdir();
        let backup_root = backup_root_guard.path().to_path_buf();
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
        let game_dir_guard = create_game_dir();
        let game_dir = game_dir_guard.path().to_path_buf();
        let backup_root_guard = tempdir();
        let backup_root = backup_root_guard.path().to_path_buf();
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
        let game_dir_guard = create_game_dir();
        let game_dir = game_dir_guard.path().to_path_buf();
        let backup_root_guard = tempdir();
        let backup_root = backup_root_guard.path().to_path_buf();
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
        let game_dir_guard = create_game_dir();
        let game_dir = game_dir_guard.path().to_path_buf();
        let backup_root_guard = tempdir();
        let backup_root = backup_root_guard.path().to_path_buf();
        let mgr = BackupManager::new(backup_root);
        let entry = mgr.create_backup(&game_dir).unwrap();
        mgr.delete_backup(&entry.id).unwrap();
        let list = mgr.list_backups().unwrap();
        assert!(list.is_empty());
    }

    #[test]
    fn test_delete_old_keeps_recent() {
        let game_dir_guard = create_game_dir();
        let game_dir = game_dir_guard.path().to_path_buf();
        let backup_root_guard = tempdir();
        let backup_root = backup_root_guard.path().to_path_buf();
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
    // Identity/retention fixtures must be completed backups: unknown origins
    // are deliberately protected by the deletion guard.
    fn claim_completed_backup(manager: &BackupManager, now: DateTime<Utc>) -> (String, PathBuf) {
        let (id, path) = manager.claim_backup_dir(now).unwrap();
        let summary = BackupManifest {
            source_path: std::path::absolute(manager.root().join("game-origin")).unwrap(),
            created_at: now,
            file_count: 0,
            size_bytes: 0,
        };
        fs::write(
            path.join("manifest.json"),
            serde_json::to_vec(&summary).unwrap(),
        )
        .unwrap();
        BackupReferenceIndex::open(manager.root())
            .unwrap()
            .mark_authoritative(&id)
            .unwrap();
        (id, path)
    }

    #[test]
    fn stale_retention_cannot_delete_a_reused_backup_id() {
        let backups = tempfile::tempdir().unwrap();
        let mgr = BackupManager::new(backups.path().to_owned());
        let now = Utc::now();
        // Retention A captured old_id. Retention B deletes it meanwhile.
        let (old_id, _) = claim_completed_backup(&mgr, now);
        mgr.delete_backup(&old_id).unwrap();
        // A new writer claims a directory in the exact same second.
        let (new_id, new_path) = claim_completed_backup(&mgr, now);
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
                let (old, _) = claim_completed_backup(&mgr, now);
                plan_tx.send(old.clone()).unwrap();
                mgr.delete_backup(&old).unwrap();
                let (fresh, path) = claim_completed_backup(&mgr, now);
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
