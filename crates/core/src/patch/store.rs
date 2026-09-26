//! Game-adjacent `.locust/` store: receipt, journal, backup layout.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::database::{sha256_file, sha256_path};
use crate::error::{LocustError, Result};

use super::manifest::{BackupFileEntry, BackupManifest, Journal, Receipt};
use super::stream::StagingDir;
use super::zipsec::safe_stored_rel;

/// Well-known directory beside the game root.
pub const LOCUST_DIR: &str = ".locust";

#[derive(Debug, Clone)]
pub struct PatchStore {
    game_root: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PatchStatus {
    NotPatched,
    Patched(Receipt),
    Interrupted(Journal),
    /// Looks touched but evidence is incomplete (no receipt, partial backup…).
    Unknown,
}

impl PatchStore {
    pub fn new(game_root: impl Into<PathBuf>) -> Self {
        Self {
            game_root: game_root.into(),
        }
    }

    pub fn game_root(&self) -> &Path {
        &self.game_root
    }

    pub fn locust_dir(&self) -> PathBuf {
        self.game_root.join(LOCUST_DIR)
    }

    pub fn backup_dir(&self) -> PathBuf {
        self.locust_dir().join("backup")
    }

    pub fn backup_files_dir(&self) -> PathBuf {
        self.backup_dir().join("files")
    }

    pub fn backup_manifest_path(&self) -> PathBuf {
        self.backup_dir().join("manifest.json")
    }

    pub fn receipt_path(&self) -> PathBuf {
        self.locust_dir().join(Receipt::FILENAME)
    }

    pub fn journal_path(&self) -> PathBuf {
        self.locust_dir().join(Journal::FILENAME)
    }

    pub fn status(&self) -> Result<PatchStatus> {
        let journal_path = self.journal_path();
        if journal_path.is_file() {
            let j = read_json::<Journal>(&journal_path)?;
            return Ok(PatchStatus::Interrupted(j));
        }
        let receipt_path = self.receipt_path();
        if receipt_path.is_file() {
            let r = read_json::<Receipt>(&receipt_path)?;
            return Ok(PatchStatus::Patched(r));
        }
        if self.locust_dir().exists() {
            return Ok(PatchStatus::Unknown);
        }
        Ok(PatchStatus::NotPatched)
    }

    pub fn read_receipt(&self) -> Result<Option<Receipt>> {
        let p = self.receipt_path();
        if !p.is_file() {
            return Ok(None);
        }
        Ok(Some(read_json(&p)?))
    }

    pub fn read_journal(&self) -> Result<Option<Journal>> {
        let p = self.journal_path();
        if !p.is_file() {
            return Ok(None);
        }
        Ok(Some(read_json(&p)?))
    }

    pub fn read_backup_manifest(&self) -> Result<Option<BackupManifest>> {
        let p = self.backup_manifest_path();
        if !p.is_file() {
            return Ok(None);
        }
        match read_json::<BackupManifest>(&p) {
            Ok(m) => Ok(Some(m)),
            Err(e) => Err(LocustError::PatchBackupIncomplete(format!(
                "backup manifest present but invalid at {}: {e}",
                p.display()
            ))),
        }
    }

    /// Whether a valid backup commit marker exists.
    pub fn backup_manifest_valid(&self) -> bool {
        matches!(self.read_backup_manifest(), Ok(Some(_)))
    }

    pub fn ensure_locust_dir(&self) -> Result<()> {
        fs::create_dir_all(self.locust_dir())?;
        #[cfg(windows)]
        {
            // Best-effort hidden attribute; failure is non-fatal.
            let _ = hide_dir_windows(&self.locust_dir());
        }
        Ok(())
    }

    /// Publish one complete JSON generation with a same-volume rename.
    /// Never move the old marker aside: it must remain discoverable until the
    /// new generation replaces it, including when this process is terminated.
    /// File fsync and best-effort directory sync do not guarantee power-loss
    /// durability on every supported filesystem.
    pub fn write_durable_json<T: serde::Serialize>(&self, path: &Path, value: &T) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let staging =
            StagingDir::create_prepared(path.parent().ok_or_else(|| {
                LocustError::PatchError("marker has no parent directory".into())
            })?)?;
        let tmp = staging.child("marker.json");
        {
            let mut f = staging.create_file("marker.json")?;
            let data = serde_json::to_vec_pretty(value)?;
            f.write_all(&data)?;
            f.sync_all()?;
        }
        let parent = path
            .parent()
            .ok_or_else(|| LocustError::PatchError("marker has no parent directory".into()))?;
        let name = path
            .file_name()
            .ok_or_else(|| LocustError::PatchError("marker has no filename".into()))?;
        super::zipsec::ensure_no_links(parent, Path::new(name))?;
        match fs::symlink_metadata(path) {
            Ok(meta) if !meta.is_file() => {
                return Err(LocustError::PatchError(format!(
                    "marker target is not a regular file: {}",
                    path.display()
                )));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        // std::fs::rename replaces existing files on Unix and Windows. The
        // staged file is under this same parent, so no cross-volume copy or
        // dest->aside fallback may turn a committed marker into an absent one.
        fs::rename(&tmp, path).map_err(|error| {
            LocustError::PatchError(format!(
                "publish marker {} -> {}: {error}",
                tmp.display(),
                path.display()
            ))
        })?;
        // Best-effort parent-dir durability (Windows needs BACKUP_SEMANTICS).
        if let Some(parent) = path.parent() {
            let _ = sync_dir(parent);
        }
        Ok(())
    }

    /// Replace `dest` with `tmp`.
    ///
    /// The old file is held inside an exclusively created, handle-owned
    /// directory. No predictable sibling is overwritten or cleaned. If both
    /// installation and restoration fail, retain the aside for manual recovery;
    /// interrupted game recovery uses the committed backup manifest and journal.
    pub fn replace_file(tmp: &Path, dest: &Path) -> Result<()> {
        let parent = dest
            .parent()
            .ok_or_else(|| LocustError::PatchError("replacement has no parent directory".into()))?;
        let name = dest
            .file_name()
            .ok_or_else(|| LocustError::PatchError("replacement has no filename".into()))?;
        super::zipsec::ensure_no_links(parent, Path::new(name))?;
        if fs::symlink_metadata(dest).is_ok_and(|meta| !meta.is_file()) {
            return Err(LocustError::PatchError(format!(
                "replacement target is not a regular file: {}",
                dest.display()
            )));
        }
        if !dest.exists() {
            return fs::rename(tmp, dest).map_err(|e| {
                LocustError::PatchError(format!(
                    "rename {} → {}: {e}",
                    tmp.display(),
                    dest.display()
                ))
            });
        }
        let mut staging = StagingDir::create_prepared(parent)?;
        let aside = staging.child("previous");
        let original_handle = fs::File::open(dest)?;
        fs::rename(dest, &aside).map_err(|e| {
            LocustError::PatchError(format!(
                "move aside {} → {}: {e}",
                dest.display(),
                aside.display()
            ))
        })?;
        staging.register_file(&aside, &original_handle)?;
        if let Err(e) = fs::rename(tmp, dest) {
            if let Err(restore_error) = fs::rename(&aside, dest) {
                staging.disarm();
                return Err(LocustError::PatchError(format!(
                    "replacement failed: {e}; restoration failed: {restore_error}; previous file retained at {}",
                    aside.display()
                )));
            }
            return Err(LocustError::PatchError(format!(
                "rename {} → {} after aside: {e}",
                tmp.display(),
                dest.display()
            )));
        }
        Ok(())
    }

    pub fn write_receipt(&self, receipt: &Receipt) -> Result<()> {
        self.write_durable_json(&self.receipt_path(), receipt)
    }

    pub fn write_journal(&self, journal: &Journal) -> Result<()> {
        self.write_durable_json(&self.journal_path(), journal)
    }

    pub fn write_backup_manifest(&self, manifest: &BackupManifest) -> Result<()> {
        self.write_durable_json(&self.backup_manifest_path(), manifest)
    }

    pub fn delete_journal(&self) -> Result<()> {
        let p = self.journal_path();
        if p.exists() {
            fs::remove_file(p)?;
        }
        Ok(())
    }

    /// Remove the entire `.locust/` tree (post successful rollback).
    pub fn remove_all(&self) -> Result<()> {
        let d = self.locust_dir();
        if d.exists() {
            fs::remove_dir_all(&d)?;
        }
        Ok(())
    }

    /// Copy `src` into the backup files tree at `rel`, hash-verify, fsync.
    pub fn backup_file(&self, src: &Path, rel: &str) -> Result<BackupFileEntry> {
        let rel_path = safe_stored_rel(rel)?;
        super::zipsec::ensure_no_links(&self.game_root, &rel_path)?;
        super::zipsec::ensure_no_links(&self.backup_files_dir(), &rel_path)?;
        let dest = self.backup_files_dir().join(&rel_path);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| LocustError::PatchError(format!("mkdir {}: {e}", parent.display())))?;
        }
        fs::copy(src, &dest).map_err(|e| {
            LocustError::PatchError(format!(
                "backup copy {} → {}: {e}",
                src.display(),
                dest.display()
            ))
        })?;
        let (hash, size) = sha256_file(&dest)
            .map_err(|e| LocustError::PatchError(format!("hash backup {}: {e}", dest.display())))?;
        let src_hash = sha256_path(src)
            .map_err(|e| LocustError::PatchError(format!("hash src {}: {e}", src.display())))?;
        if hash != src_hash {
            return Err(LocustError::PatchError(format!(
                "backup hash mismatch for {rel}"
            )));
        }
        // fsync the copy. On Windows FlushFileBuffers requires a handle
        // opened with write access — read-only open yields ERROR_ACCESS_DENIED.
        {
            let f = fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&dest)
                .map_err(|e| {
                    LocustError::PatchError(format!("open backup for sync {}: {e}", dest.display()))
                })?;
            f.sync_all().map_err(|e| {
                LocustError::PatchError(format!("sync backup {}: {e}", dest.display()))
            })?;
        }
        Ok(BackupFileEntry {
            path: rel.replace('\\', "/"),
            sha256: hash,
            size,
        })
    }

    /// Validate the complete restore plan without creating files or markers.
    /// Rollback invokes this in every state before its first mutation.
    pub(crate) fn preflight_restore(&self, manifest: &BackupManifest) -> Result<()> {
        for entry in &manifest.files {
            self.validate_restore_entry(entry)?;
        }
        Ok(())
    }

    fn validate_restore_entry(&self, entry: &BackupFileEntry) -> Result<PathBuf> {
        let rel = safe_stored_rel(&entry.path)?;
        super::zipsec::ensure_no_links(&self.game_root, &rel)?;
        super::zipsec::ensure_no_links(
            &self.game_root,
            &Path::new(".locust/backup/files").join(&rel),
        )?;
        super::zipsec::ensure_no_links(&self.backup_files_dir(), &rel)?;
        let src = self.backup_files_dir().join(&rel);
        if !src.is_file() {
            return Err(LocustError::PatchBackupIncomplete(format!(
                "backup file missing: {}",
                entry.path
            )));
        }
        let (hash, size) = sha256_file(&src)?;
        if hash != entry.sha256 || size != entry.size {
            return Err(LocustError::PatchBackupIncomplete(format!(
                "backup file hash/size mismatch: {} (expected {} / {}, got {} / {})",
                entry.path, entry.sha256, entry.size, hash, size
            )));
        }
        let dest = self.game_root.join(&rel);
        if fs::symlink_metadata(&dest).is_ok_and(|meta| !meta.is_file()) {
            return Err(LocustError::PatchError(format!(
                "restore target is not a regular file: {}",
                dest.display()
            )));
        }
        Ok(rel)
    }

    /// Restore one entry from an owned, fully hashed copy. Backups remain intact
    /// on any copy, validation, or installation failure.
    pub fn restore_file(&self, entry: &BackupFileEntry) -> Result<()> {
        let rel = self.validate_restore_entry(entry)?;
        let src = self.backup_files_dir().join(&rel);
        let dest = self.game_root.join(&rel);
        let staging = StagingDir::create_prepared(&self.game_root)?;
        let tmp = staging.child("restore");
        {
            let mut tmpf = staging.create_file("restore")?;
            let copied = super::stream::stream_and_hash(
                &mut fs::File::open(&src)?,
                entry.size,
                &entry.path,
                Some(&mut tmpf),
            )
            .map_err(|error| {
                LocustError::PatchBackupIncomplete(format!(
                    "cannot stage backup {}: {error}",
                    entry.path
                ))
            })?;
            if copied.sha256_hex != entry.sha256 || copied.actual_len != entry.size {
                return Err(LocustError::PatchBackupIncomplete(format!(
                    "backup changed while restoring: {}",
                    entry.path
                )));
            }
            tmpf.sync_all()?;
        }
        super::zipsec::ensure_no_links(&self.game_root, &rel)?;
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }
        Self::replace_file(&tmp, &dest)?;
        Ok(())
    }
}

pub fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    let data = fs::read_to_string(path)?;
    Ok(serde_json::from_str(&data)?)
}

fn sync_dir(path: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
        let f = fs::OpenOptions::new()
            .read(true)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
            .open(path)?;
        f.sync_all()
    }
    #[cfg(not(windows))]
    {
        let f = fs::File::open(path)?;
        f.sync_all()
    }
}

#[cfg(windows)]
fn hide_dir_windows(path: &Path) -> std::io::Result<()> {
    use std::os::windows::process::CommandExt;
    use std::process::Command;
    // attrib does not accept the verbatim paths returned by canonicalize.
    // Resolve the parent through CreateProcess and pass only the basename.
    let output = Command::new("attrib")
        .arg("+H")
        .arg(
            path.file_name()
                .ok_or_else(|| std::io::Error::other("missing directory name"))?,
        )
        .current_dir(
            path.parent()
                .ok_or_else(|| std::io::Error::other("missing parent"))?,
        )
        .creation_flags(0x08000000)
        .output()?;
    if output.status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other(
            "could not hide patch recovery directory",
        ))
    }
}
