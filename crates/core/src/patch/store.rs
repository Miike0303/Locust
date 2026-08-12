//! Game-adjacent `.locust/` store: receipt, journal, backup layout.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::database::sha256_hex;
use crate::error::{LocustError, Result};

use super::manifest::{BackupFileEntry, BackupManifest, Journal, Receipt};
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

    /// Write JSON with tmp + fsync + rename durability for the marker itself.
    pub fn write_durable_json<T: serde::Serialize>(&self, path: &Path, value: &T) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("tmp");
        {
            let mut f = fs::File::create(&tmp)?;
            let data = serde_json::to_vec_pretty(value)?;
            f.write_all(&data)?;
            f.sync_all()?;
        }
        // Windows `rename` fails if the destination exists; replace atomically
        // by removing the target first (content is already fsynced on tmp).
        if path.exists() {
            fs::remove_file(path)?;
        }
        fs::rename(&tmp, path)?;
        // Best-effort parent-dir durability (Windows needs BACKUP_SEMANTICS).
        if let Some(parent) = path.parent() {
            let _ = sync_dir(parent);
        }
        Ok(())
    }

    /// Replace `dest` with `tmp`.
    ///
    /// On Unix, `rename` replaces atomically. On Windows it fails if `dest`
    /// exists, so we move the old file aside first (`*.locust-old`), then
    /// rename `tmp` into place, then delete the aside copy. That keeps a
    /// recoverable copy across the crash window (review W3) instead of
    /// delete-then-rename, which could leave the path empty.
    pub fn replace_file(tmp: &Path, dest: &Path) -> Result<()> {
        if !dest.exists() {
            return fs::rename(tmp, dest).map_err(|e| {
                LocustError::PatchError(format!(
                    "rename {} → {}: {e}",
                    tmp.display(),
                    dest.display()
                ))
            });
        }
        let aside = {
            let mut p = dest.as_os_str().to_owned();
            p.push(".locust-old");
            PathBuf::from(p)
        };
        let _ = fs::remove_file(&aside);
        fs::rename(dest, &aside).map_err(|e| {
            LocustError::PatchError(format!(
                "move aside {} → {}: {e}",
                dest.display(),
                aside.display()
            ))
        })?;
        if let Err(e) = fs::rename(tmp, dest) {
            // Best-effort restore of the previous file.
            let _ = fs::rename(&aside, dest);
            return Err(LocustError::PatchError(format!(
                "rename {} → {} after aside: {e}",
                tmp.display(),
                dest.display()
            )));
        }
        let _ = fs::remove_file(&aside);
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
        let dest = self.backup_files_dir().join(&rel_path);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).map_err(|e| {
                LocustError::PatchError(format!("mkdir {}: {e}", parent.display()))
            })?;
        }
        fs::copy(src, &dest).map_err(|e| {
            LocustError::PatchError(format!(
                "backup copy {} → {}: {e}",
                src.display(),
                dest.display()
            ))
        })?;
        let bytes = fs::read(&dest).map_err(|e| {
            LocustError::PatchError(format!("read backup {}: {e}", dest.display()))
        })?;
        let hash = sha256_hex(&bytes);
        let src_hash = sha256_hex(&fs::read(src).map_err(|e| {
            LocustError::PatchError(format!("read src {}: {e}", src.display()))
        })?);
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
                    LocustError::PatchError(format!(
                        "open backup for sync {}: {e}",
                        dest.display()
                    ))
                })?;
            f.sync_all().map_err(|e| {
                LocustError::PatchError(format!("sync backup {}: {e}", dest.display()))
            })?;
        }
        Ok(BackupFileEntry {
            path: rel.replace('\\', "/"),
            sha256: hash,
            size: bytes.len() as u64,
        })
    }

    /// Restore one backup entry to the game root (copy-tmp + rename + hash check).
    pub fn restore_file(&self, entry: &BackupFileEntry) -> Result<()> {
        let rel = safe_stored_rel(&entry.path)?;
        let src = self.backup_files_dir().join(&rel);
        if !src.is_file() {
            return Err(LocustError::PatchBackupIncomplete(format!(
                "backup file missing: {}",
                entry.path
            )));
        }
        let bytes = fs::read(&src)?;
        let hash = sha256_hex(&bytes);
        if hash != entry.sha256 {
            return Err(LocustError::PatchBackupIncomplete(format!(
                "backup file hash mismatch: {} (expected {}, got {})",
                entry.path, entry.sha256, hash
            )));
        }
        let dest = self.game_root.join(&rel);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }
        let tmp = {
            let mut t = dest.as_os_str().to_owned();
            t.push(".locust-tmp");
            PathBuf::from(t)
        };
        fs::write(&tmp, &bytes)?;
        {
            let f = fs::OpenOptions::new().read(true).write(true).open(&tmp)?;
            f.sync_all()?;
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
    use std::os::windows::ffi::OsStrExt;
    use std::process::Command;
    // Prefer attrib.exe — no extra crate; best-effort.
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let _ = wide;
    let _ = Command::new("attrib")
        .args(["+H", &path.to_string_lossy()])
        .status();
    Ok(())
}

