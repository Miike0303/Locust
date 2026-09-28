//! Cross-process exclusion for a canonical game directory. The persistent lock
//! file contains no state; kernel handle lifetime owns the lock, including crash
//! recovery. Never unlink it: replacing the inode would create two lock domains.
//!
//! A stable per-user OS data directory deliberately ignores Locust profiles and
//! temporary-directory overrides. All cooperating patch clients use this layer.
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};

use crate::error::{LocustError, Result};

/// Cross-process guard for a canonical game directory. This guard is not
/// reentrant: do not call patch apply, verify, or rollback for the same root
/// while holding it. Hold it across source reads and all resulting writes.
pub struct GameLock {
    // Kept open until the entire operation (including recovery) has returned.
    _file: File,
    _directory: File,
    root: PathBuf,
}

impl GameLock {
    /// Acquire exclusive access; Unix briefly retries before reporting a busy game.
    pub fn acquire(game_root: &Path) -> Result<Self> {
        let root = game_root.canonicalize()?;
        if !root.is_dir() {
            return Err(lock_error("game root is not a directory"));
        }
        let base = dirs::data_local_dir()
            .ok_or_else(|| lock_error("cannot locate the user's local data directory"))?;
        let base = resolve_existing_parent(&base)?;
        // OS-selected base may itself have been relocated by the user. Resolve
        // it once, then refuse links/reparse points in our own lock namespace.
        let directory = base.join("locust-patch-locks-v1");
        if directory.starts_with(&root) {
            return Err(lock_error(
                "lock storage must be outside the selected game directory",
            ));
        }
        fs::create_dir_all(&base)?;
        let builder = fs::DirBuilder::new();
        #[cfg(unix)]
        let builder = {
            use std::os::unix::fs::DirBuilderExt;
            let mut builder = builder;
            builder.mode(0o700);
            builder
        };
        match builder.create(&directory) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
        ensure_plain(&directory, true)?;
        Self::acquire_in(root, &directory)
    }

    fn acquire_in(root: PathBuf, directory: &Path) -> Result<Self> {
        ensure_plain(directory, true)?;
        let mut directory_options = OpenOptions::new();
        directory_options.read(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            // BACKUP_SEMANTICS opens a directory; pin the namespace against
            // rename/deletion while any operation owns a game lock.
            directory_options
                .custom_flags(0x0220_0000)
                .share_mode(0x0000_0003);
        }
        let directory_file = directory_options.open(directory)?;
        ensure_metadata(&directory_file.metadata()?, true)?;
        let path = directory.join(format!("{}.lock", root_key(&root)));
        match fs::symlink_metadata(&path) {
            Ok(_) => ensure_plain(&path, false)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            // OPEN_REPARSE_POINT opens the link itself instead of its target.
            // No FILE_SHARE_DELETE: a live lock cannot be unlinked/replaced.
            options.custom_flags(0x0020_0000).share_mode(0x0000_0003);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(&path)?;
        ensure_metadata(&file.metadata()?, false)?;
        ensure_plain(&path, false)?;
        #[cfg(unix)]
        let lock_result = try_lock_with_retry(|| file.try_lock());
        #[cfg(not(unix))]
        let lock_result = file.try_lock();
        match lock_result {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) => {
                return Err(LocustError::PatchError(format!(
                    "game busy: another patch operation is using {}; wait for it to finish and retry",
                    root.display()
                )));
            }
            Err(std::fs::TryLockError::Error(error)) => return Err(error.into()),
        }
        // Reject a swapped namespace after opening/locking; no game mutation
        // has occurred. Private per-user storage is not a hostile-user sandbox.
        ensure_plain(directory, true)?;
        ensure_plain(&path, false)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let opened = file.metadata()?;
            let named = fs::symlink_metadata(&path)?;
            if opened.dev() != named.dev() || opened.ino() != named.ino() {
                return Err(lock_error("lock file was replaced while being opened"));
            }
        }
        Ok(Self {
            _file: file,
            _directory: directory_file,
            root,
        })
    }

    /// Canonical directory used as the shared lock identity.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Check that a directory selection (or a file's parent) belongs to this
    /// guard before delegating work. This never acquires a second lock.
    pub fn validate_selection(&self, selection: &Path) -> Result<()> {
        let selected = selection.canonicalize()?;
        let root = if selected.is_dir() {
            selected.as_path()
        } else if selected.is_file() {
            selected
                .parent()
                .ok_or_else(|| lock_error("selection has no parent"))?
        } else {
            return Err(lock_error("selection is not a regular file or directory"));
        };
        if root_key(root) != root_key(self.root()) {
            return Err(lock_error("held guard belongs to a different game root"));
        }
        Ok(())
    }
}

#[cfg(any(unix, test))]
fn try_lock_with_retry(
    mut try_lock: impl FnMut() -> std::result::Result<(), std::fs::TryLockError>,
) -> std::result::Result<(), std::fs::TryLockError> {
    const ATTEMPTS: usize = 11;
    // Bridge the fork-inheritance window before a child execs/closes the fd.
    // A genuinely held lock still fails after the bounded 50 ms retry period.
    for _ in 1..ATTEMPTS {
        match try_lock() {
            Err(std::fs::TryLockError::WouldBlock) => {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            result => return result,
        }
    }
    try_lock()
}

// Resolve all existing ancestors before deciding whether storage is outside
// the game. Missing OS data directories can then be created without first
// mutating a selected game that happens to contain the configured data path.
fn resolve_existing_parent(path: &Path) -> Result<PathBuf> {
    match path.canonicalize() {
        Ok(path) => Ok(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let parent = path
                .parent()
                .ok_or_else(|| lock_error("local data directory has no existing parent"))?;
            let name = path
                .file_name()
                .ok_or_else(|| lock_error("invalid local data directory"))?;
            Ok(resolve_existing_parent(parent)?.join(name))
        }
        Err(error) => Err(error.into()),
    }
}

fn lock_error(message: &str) -> LocustError {
    LocustError::PatchError(format!("cannot lock game: {message}"))
}

fn ensure_plain(path: &Path, directory: bool) -> Result<()> {
    ensure_metadata(&fs::symlink_metadata(path)?, directory)
}

fn ensure_metadata(metadata: &fs::Metadata, directory: bool) -> Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return Err(lock_error("lock storage contains a reparse point"));
        }
    }
    if metadata.file_type().is_symlink()
        || (directory && !metadata.is_dir())
        || (!directory && !metadata.is_file())
    {
        return Err(lock_error(
            "lock storage must be a regular file inside a plain directory",
        ));
    }
    Ok(())
}

fn root_key(root: &Path) -> String {
    use sha2::{Digest, Sha256};
    #[cfg(windows)]
    let bytes = root
        .to_string_lossy()
        .replace('\\', "/")
        .to_lowercase()
        .into_bytes();
    #[cfg(not(windows))]
    let bytes = root.as_os_str().as_encoded_bytes();
    hex::encode(Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lock_retry_succeeds_after_transient_contention() {
        let mut attempts = 0;
        let result = try_lock_with_retry(|| {
            attempts += 1;
            if attempts <= 3 {
                Err(std::fs::TryLockError::WouldBlock)
            } else {
                Ok(())
            }
        });
        assert!(result.is_ok());
        assert_eq!(attempts, 4);
    }

    #[test]
    fn lock_retry_returns_would_block_after_bound() {
        let mut attempts = 0;
        let result = try_lock_with_retry(|| {
            attempts += 1;
            Err(std::fs::TryLockError::WouldBlock)
        });
        assert!(matches!(result, Err(std::fs::TryLockError::WouldBlock)));
        assert_eq!(attempts, 11);
    }

    #[test]
    fn lock_retry_returns_other_errors_without_retrying() {
        let mut attempts = 0;
        let result = try_lock_with_retry(|| {
            attempts += 1;
            Err(std::fs::TryLockError::Error(std::io::Error::from(
                std::io::ErrorKind::PermissionDenied,
            )))
        });
        assert!(matches!(
            result,
            Err(std::fs::TryLockError::Error(error))
                if error.kind() == std::io::ErrorKind::PermissionDenied
        ));
        assert_eq!(attempts, 1);
    }

    #[cfg(unix)]
    #[test]
    fn acquire_succeeds_when_lock_is_released_during_retry() {
        let game = tempfile::tempdir().unwrap();
        let lock = GameLock::acquire(game.path()).unwrap();
        let (started, release) = std::sync::mpsc::channel();
        let releaser = std::thread::spawn(move || {
            release.recv().unwrap();
            std::thread::sleep(std::time::Duration::from_millis(10));
            drop(lock);
        });

        started.send(()).unwrap();
        let result = GameLock::acquire(game.path());
        releaser.join().unwrap();
        assert!(result.is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn acquire_returns_game_busy_when_lock_remains_held() {
        let game = tempfile::tempdir().unwrap();
        let lock = GameLock::acquire(game.path()).unwrap();
        let error = GameLock::acquire(game.path()).err().unwrap();
        assert!(error.to_string().contains("game busy"));
        drop(lock);
    }

    #[test]
    fn delegated_selection_requires_the_same_directory_or_file_parent() {
        let root = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let file = root.path().join("game.txt");
        fs::write(&file, b"original").unwrap();
        let child = root.path().join("child");
        fs::create_dir(&child).unwrap();
        let lock = GameLock::acquire(root.path()).unwrap();
        lock.validate_selection(root.path()).unwrap();
        lock.validate_selection(&root.path().join(".")).unwrap();
        lock.validate_selection(&file).unwrap();
        assert!(lock.validate_selection(other.path()).is_err());
        assert!(lock.validate_selection(&child).is_err());
        assert!(lock
            .validate_selection(&root.path().join("missing"))
            .is_err());
        assert!(GameLock::acquire(root.path()).is_err());
        assert_eq!(fs::read(file).unwrap(), b"original");
    }

    #[test]
    fn persistent_file_preserves_existing_bytes_and_releases_on_drop() {
        let temp = tempfile::tempdir().unwrap();
        let game = tempfile::tempdir().unwrap();
        let root = game.path().canonicalize().unwrap();
        let path = temp.path().join(format!("{}.lock", root_key(&root)));
        fs::write(&path, b"foreign bytes must never be truncated").unwrap();
        let first = GameLock::acquire_in(root.clone(), temp.path()).unwrap();
        assert!(GameLock::acquire_in(root.clone(), temp.path())
            .err()
            .unwrap()
            .to_string()
            .contains("busy"));
        drop(first);
        assert_eq!(
            fs::read(&path).unwrap(),
            b"foreign bytes must never be truncated"
        );
        drop(GameLock::acquire_in(root, temp.path()).unwrap());
        assert!(
            path.exists(),
            "persistent lock domain must never be unlinked"
        );
    }

    #[test]
    fn missing_and_file_game_roots_are_rejected() {
        let temp = tempfile::tempdir().unwrap();
        assert!(GameLock::acquire(&temp.path().join("absent")).is_err());
        let file = temp.path().join("file");
        fs::write(&file, b"source").unwrap();
        assert!(GameLock::acquire(&file).is_err());
        assert_eq!(fs::read(file).unwrap(), b"source");
    }

    #[test]
    fn configured_storage_parent_is_resolved_without_creating_anything() {
        let temp = tempfile::tempdir().unwrap();
        let missing = temp.path().join("missing/a/b");
        let resolved = resolve_existing_parent(&missing).unwrap();
        assert_eq!(
            resolved,
            temp.path().canonicalize().unwrap().join("missing/a/b")
        );
        assert!(!temp.path().join("missing").exists());
    }

    #[cfg(any(windows, unix))]
    #[test]
    fn linked_lock_file_and_namespace_are_rejected_without_touching_targets() {
        let temp = tempfile::tempdir().unwrap();
        let game = tempfile::tempdir().unwrap();
        let root = game.path().canonicalize().unwrap();
        let target = temp.path().join("foreign");
        fs::write(&target, b"preserve foreign target").unwrap();
        let path = temp.path().join(format!("{}.lock", root_key(&root)));
        #[cfg(windows)]
        std::os::windows::fs::symlink_file(&target, &path).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&target, &path).unwrap();
        assert!(GameLock::acquire_in(root.clone(), temp.path()).is_err());
        assert_eq!(fs::read(&target).unwrap(), b"preserve foreign target");
        let linked_dir = temp.path().join("linked-dir");
        #[cfg(windows)]
        std::os::windows::fs::symlink_dir(game.path(), &linked_dir).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(game.path(), &linked_dir).unwrap();
        assert!(GameLock::acquire_in(root, &linked_dir).is_err());
        assert!(fs::read_dir(game.path()).unwrap().next().is_none());
    }

    #[cfg(windows)]
    #[test]
    fn windows_live_lock_pins_file_and_directory_against_replacement() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("locks");
        fs::create_dir(&directory).unwrap();
        let game = tempfile::tempdir().unwrap();
        let root = game.path().canonicalize().unwrap();
        let path = directory.join(format!("{}.lock", root_key(&root)));
        let guard = GameLock::acquire_in(root, &directory).unwrap();
        assert!(fs::remove_file(&path).is_err());
        assert!(fs::rename(&directory, temp.path().join("renamed")).is_err());
        drop(guard);
        assert!(path.exists());
        fs::rename(&directory, temp.path().join("renamed")).unwrap();
    }
}
