//! Streaming zip-entry extraction for multi‑GB patch apply/verify.
//!
//! Entries are never fully buffered in RAM. Content is hashed (and for apply,
//! written to a same-volume staging file) in fixed-size chunks.

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::error::{LocustError, Result};
use crate::file_identity::same_file;

use super::zipsec::{check_entry_budget, max_zip_total_bytes};

/// Read chunk size for streaming (1 MiB — balances syscall count vs peak RAM).
const STREAM_CHUNK: usize = 1024 * 1024;

/// Result of streaming one zip entry.
#[derive(Debug, Clone)]
pub struct StreamedBytes {
    pub sha256_hex: String,
    pub actual_len: u64,
}

/// Keep I/O failures distinct from a source exceeding its recorded size.
/// Extraction and packing use the same loop with different recovery messages.
#[derive(Debug)]
pub(crate) enum StreamError {
    Read(std::io::Error),
    Write(std::io::Error),
    TooLong,
}

/// Hash the exact chunks passed to the writer without ever writing more than
/// `limit` bytes. Callers must discard partial output on any error.
pub(crate) fn stream_bounded(
    reader: &mut dyn Read,
    limit: u64,
    mut out: Option<&mut dyn Write>,
) -> std::result::Result<StreamedBytes, StreamError> {
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; STREAM_CHUNK];
    let mut total = 0u64;
    loop {
        let n = reader.read(&mut buf).map_err(StreamError::Read)?;
        if n == 0 {
            break;
        }
        let n64 = n as u64;
        if n64 > limit - total {
            return Err(StreamError::TooLong);
        }
        total += n64;
        hasher.update(&buf[..n]);
        if let Some(ref mut writer) = out {
            writer.write_all(&buf[..n]).map_err(StreamError::Write)?;
        }
    }
    Ok(StreamedBytes {
        sha256_hex: hex::encode(hasher.finalize()),
        actual_len: total,
    })
}

/// Stream `reader` up to `declared_uncompressed` bytes, hashing as we go.
///
/// - If `actual` would exceed `declared_uncompressed`, aborts immediately
///   (zip-bomb / lying local header). Partial `out` data is not truncated by
///   this function — callers should discard the destination file on error.
/// - When `out` is `Some`, every accepted byte is written there.
pub fn stream_and_hash(
    reader: &mut dyn Read,
    declared_uncompressed: u64,
    entry_name: &str,
    out: Option<&mut dyn Write>,
) -> Result<StreamedBytes> {
    stream_bounded(reader, declared_uncompressed, out).map_err(|error| {
        LocustError::PatchError(match error {
            StreamError::Read(e) => format!("read zip entry \"{entry_name}\": {e}"),
            StreamError::Write(e) => format!("write staged zip entry \"{entry_name}\": {e}"),
            StreamError::TooLong => format!(
                "zip entry \"{entry_name}\" expanded past its declared uncompressed size \
                 ({declared_uncompressed} bytes) — aborting (possible zip bomb)"
            ),
        })
    })
}

/// Hash-only stream (verify path) — no temp file.
pub fn stream_hash_only(
    reader: &mut dyn Read,
    declared_uncompressed: u64,
    entry_name: &str,
) -> Result<StreamedBytes> {
    stream_and_hash(reader, declared_uncompressed, entry_name, None)
}

/// Stream entry to `dest_path` (created/truncated), fsync, return hash + length.
pub fn stream_to_file(
    reader: &mut dyn Read,
    declared_uncompressed: u64,
    entry_name: &str,
    dest_path: &Path,
) -> Result<StreamedBytes> {
    if let Some(parent) = dest_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = File::create(dest_path).map_err(|e| {
        LocustError::PatchError(format!(
            "create staging file {} for \"{entry_name}\": {e}",
            dest_path.display()
        ))
    })?;
    let result = match stream_and_hash(
        reader,
        declared_uncompressed,
        entry_name,
        Some(&mut file as &mut dyn Write),
    ) {
        Ok(r) => r,
        Err(e) => {
            drop(file);
            let _ = fs::remove_file(dest_path);
            return Err(e);
        }
    };
    file.sync_all().map_err(|e| {
        LocustError::PatchError(format!(
            "fsync staging file {} for \"{entry_name}\": {e}",
            dest_path.display()
        ))
    })?;
    Ok(result)
}

/// Charge `declared` against the running total ceiling **before** streaming.
pub fn charge_declared(entry_name: &str, declared: u64, total_so_far: u64) -> Result<u64> {
    check_entry_budget(entry_name, declared, total_so_far)
}

/// RAII staging directory under the game's `.locust/` (same volume as renames).
pub struct StagingDir {
    path: PathBuf,
    disarm: bool,
    prepared_handle: Option<File>,
    owned_children: std::cell::RefCell<Vec<OwnedChild>>,
}

struct OwnedChild {
    path: PathBuf,
    handle: File,
}

/// File identities allocated by StagingDir within a synchronous operation.
/// Used by injection to distinguish retained private scratch from game output.
/// Names alone never establish ownership. Guards on another thread are not
/// captured and their leftover data must be diagnosed by the caller.
#[derive(Default)]
pub struct TrackedStaging {
    files: Vec<OwnedChild>,
    directories: Vec<OwnedChild>,
}

thread_local! {
    static STAGING_TRACKERS: std::cell::RefCell<Vec<TrackedStaging>> = const { std::cell::RefCell::new(Vec::new()) };
}

fn open_identity(path: &Path, directory: bool) -> std::io::Result<File> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        let mut options = fs::OpenOptions::new();
        options.read(true).share_mode(1 | 2 | 4);
        if directory {
            options.custom_flags(0x0200_0000);
        }
        options.open(path)
    }
    #[cfg(not(windows))]
    {
        let _ = directory;
        File::open(path)
    }
}

impl TrackedStaging {
    pub fn owns_file(&self, path: &Path) -> bool {
        self.files.iter().any(|owned| {
            owned.path == path
                && open_identity(path, false)
                    .is_ok_and(|current| same_file(&owned.handle, &current))
        })
    }

    pub fn owns_directory(&self, path: &Path) -> bool {
        self.directories.iter().any(|owned| {
            owned.path == path
                && open_identity(path, true).is_ok_and(|current| same_file(&owned.handle, &current))
        })
    }
}

/// Capture every enclosing scope, including nested scopes. Panic unwinding or
/// ordinary Result errors restore the previous thread-local scope. Tracking
/// handles share delete access and do not prevent normal StagingDir cleanup.
pub fn track_prepared_staging<T>(operation: impl FnOnce() -> T) -> (T, TrackedStaging) {
    struct Scope(bool);
    impl Drop for Scope {
        fn drop(&mut self) {
            if self.0 {
                STAGING_TRACKERS.with(|stack| {
                    stack.borrow_mut().pop();
                });
            }
        }
    }
    STAGING_TRACKERS.with(|stack| stack.borrow_mut().push(TrackedStaging::default()));
    let mut scope = Scope(true);
    let result = operation();
    let owned = STAGING_TRACKERS.with(|stack| stack.borrow_mut().pop().unwrap());
    scope.0 = false;
    (result, owned)
}

fn track_directory(path: &Path) -> Result<()> {
    STAGING_TRACKERS.with(|stack| {
        for tracker in stack.borrow_mut().iter_mut() {
            tracker.directories.push(OwnedChild {
                path: path.to_owned(),
                handle: open_identity(path, true)?,
            });
        }
        Ok(())
    })
}

fn track_child(path: &Path, file: &File) -> Result<()> {
    STAGING_TRACKERS.with(|stack| {
        for tracker in stack.borrow_mut().iter_mut() {
            tracker.files.push(OwnedChild {
                path: path.to_owned(),
                handle: file.try_clone()?,
            });
        }
        Ok(())
    })
}

impl StagingDir {
    /// Create `game_root/.locust/staging-<uuid>/`.
    pub fn create(game_root: &Path) -> Result<Self> {
        let path = game_root
            .join(super::store::LOCUST_DIR)
            .join(format!("staging-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&path)?;
        Ok(Self {
            path,
            disarm: false,
            prepared_handle: None,
            owned_children: Default::default(),
        })
    }

    /// Staging that survives rollback of a previously installed patch. Created
    /// exclusively beside `.locust`, and owned by this operation alone. A crash
    /// can leave this directory; recovery never scans/deletes similarly named
    /// directories because their ownership cannot be established later.
    /// Callers must hold exclusive access to the selected game while preparing
    /// and installing files. Cleanup removes only children created by this guard.
    pub fn create_prepared(game_root: &Path) -> Result<Self> {
        let path = game_root.join(format!(".locust-stage-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&path)?;
        let opened = {
            #[cfg(windows)]
            {
                use std::os::windows::fs::OpenOptionsExt;
                fs::OpenOptions::new()
                    .read(true)
                    .custom_flags(0x0200_0000) // FILE_FLAG_BACKUP_SEMANTICS
                    .share_mode(1 | 2) // no FILE_SHARE_DELETE: hold directory identity
                    .open(&path)
            }
            #[cfg(not(windows))]
            {
                File::open(&path)
            }
        };
        let handle = match opened {
            Ok(handle) => handle,
            Err(error) => {
                let _ = fs::remove_dir(&path);
                return Err(error.into());
            }
        };
        let stage = Self {
            path,
            disarm: false,
            prepared_handle: Some(handle),
            owned_children: Default::default(),
        };
        track_directory(stage.path())?;
        Ok(stage)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn child(&self, name: &str) -> PathBuf {
        self.path.join(name)
    }

    /// Claim a file only after exclusive creation. A colliding name remains
    /// unowned and must survive this guard's cleanup.
    /// `name` must be a single ordinary filename; no existing file is overwritten.
    pub fn create_file(&self, name: &str) -> Result<File> {
        if name.is_empty()
            || name.contains(['/', '\\', ':'])
            || name.chars().any(char::is_control)
            || name.ends_with(['.', ' '])
            || !matches!(
                Path::new(name).components().next(),
                Some(std::path::Component::Normal(_))
            )
        {
            return Err(LocustError::PatchError(
                "invalid owned scratch filename".into(),
            ));
        }
        super::zipsec::safe_entry_path(name, name)?;
        let path = self.child(name);
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)?;
        self.register_file(&path, &file)?;
        Ok(file)
    }

    /// Register a file successfully renamed into this exclusively owned folder.
    pub(crate) fn register_file(&self, path: &Path, file: &File) -> Result<()> {
        if path.parent() != Some(self.path()) {
            return Err(LocustError::PatchError(
                "scratch file escaped its owned directory".into(),
            ));
        }
        self.owned_children.borrow_mut().push(OwnedChild {
            path: path.to_owned(),
            handle: file.try_clone()?,
        });
        track_child(path, file)?;
        Ok(())
    }

    /// Keep the directory (normally unused — files are renamed out).
    pub fn disarm(&mut self) {
        self.disarm = true;
    }
}

impl Drop for StagingDir {
    fn drop(&mut self) {
        if !self.disarm {
            if let Some(handle) = self.prepared_handle.take() {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::MetadataExt;
                    let identity_matches = handle
                        .metadata()
                        .ok()
                        .zip(fs::symlink_metadata(&self.path).ok())
                        .is_some_and(|(owned, current)| {
                            owned.dev() == current.dev() && owned.ino() == current.ino()
                        });
                    if !identity_matches {
                        return;
                    }
                }
                // Delete only paths allocated by this operation. Never recurse
                // into an unrelated child inserted after staging was created.
                for child in self.owned_children.get_mut() {
                    let regular = fs::symlink_metadata(&child.path)
                        .is_ok_and(|m| m.is_file() && !m.file_type().is_symlink());
                    if regular
                        && File::open(&child.path)
                            .is_ok_and(|current| same_file(&child.handle, &current))
                    {
                        let _ = fs::remove_file(&child.path);
                    }
                }
                self.owned_children.get_mut().clear();
                drop(handle);
                let _ = fs::remove_dir(&self.path); // empty directories only
            } else {
                let _ = fs::remove_dir_all(&self.path);
            }
        }
    }
}

/// Re-export for callers that need the configured ceiling.
pub fn total_ceiling() -> u64 {
    max_zip_total_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn owned_stage_does_not_claim_creation_collisions() {
        let root = tempfile::tempdir().unwrap();
        let staging = StagingDir::create_prepared(root.path()).unwrap();
        let path = staging.child("collision");
        fs::write(&path, b"unrelated original").unwrap();
        assert!(staging.create_file("collision").is_err());
        drop(staging);
        assert_eq!(fs::read(path).unwrap(), b"unrelated original");
    }

    #[test]
    fn owned_stage_keeps_replacement_at_former_owned_name() {
        let root = tempfile::tempdir().unwrap();
        let staging = StagingDir::create_prepared(root.path()).unwrap();
        let path = staging.child("payload");
        let mut file = staging.create_file("payload").unwrap();
        file.write_all(b"owned payload").unwrap();
        drop(file);
        let moved = root.path().join("installed");
        fs::rename(&path, &moved).unwrap();
        fs::write(&path, b"unrelated replacement").unwrap();
        drop(staging);
        assert_eq!(fs::read(path).unwrap(), b"unrelated replacement");
        assert_eq!(fs::read(moved).unwrap(), b"owned payload");
    }

    #[test]
    fn hash_only_matches_known() {
        let data = b"hello patch stream";
        let mut cur = Cursor::new(data.as_slice());
        let r = stream_hash_only(&mut cur, data.len() as u64, "t").unwrap();
        assert_eq!(r.actual_len, data.len() as u64);
        assert_eq!(r.sha256_hex, crate::database::sha256_hex(data));
    }

    #[test]
    fn bounded_stream_hashes_exact_bytes_with_short_writes_and_empty_input() {
        #[derive(Default)]
        struct ShortWriter(Vec<u8>);
        impl Write for ShortWriter {
            fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
                let n = data.len().min(8191);
                self.0.extend_from_slice(&data[..n]);
                Ok(n)
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        for data in [
            Vec::new(),
            (0..STREAM_CHUNK + 37).map(|i| (i % 251) as u8).collect(),
        ] {
            let mut writer = ShortWriter::default();
            let streamed = stream_bounded(
                &mut Cursor::new(&data),
                data.len() as u64,
                Some(&mut writer),
            )
            .unwrap();
            assert_eq!(writer.0, data);
            assert_eq!(streamed.actual_len, data.len() as u64);
            assert_eq!(streamed.sha256_hex, crate::database::sha256_hex(&data));
        }
    }

    #[test]
    fn bounded_stream_distinguishes_write_failure_from_source_growth() {
        struct FailedWriter;
        impl Write for FailedWriter {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("simulated full disk"))
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let failure =
            stream_bounded(&mut Cursor::new(b"data"), 4, Some(&mut FailedWriter)).unwrap_err();
        assert!(matches!(failure, StreamError::Write(_)));
        let message = stream_and_hash(
            &mut Cursor::new(b"data"),
            4,
            "story.bin",
            Some(&mut FailedWriter),
        )
        .unwrap_err()
        .to_string();
        assert!(
            message.contains("write staged zip entry") && message.contains("simulated full disk")
        );
        assert!(!message.contains("zip bomb"));
    }

    #[test]
    fn bounded_stream_distinguishes_read_failure_from_source_growth() {
        struct FailedReader;
        impl Read for FailedReader {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("simulated unreadable source"))
            }
        }
        assert!(matches!(
            stream_bounded(&mut FailedReader, 4, None),
            Err(StreamError::Read(_))
        ));
        let message = stream_and_hash(&mut FailedReader, 4, "story.bin", None)
            .unwrap_err()
            .to_string();
        assert!(
            message.contains("read zip entry") && message.contains("simulated unreadable source")
        );
        assert!(!message.contains("zip bomb"));
    }

    #[test]
    fn aborts_when_actual_exceeds_declared() {
        let data = b"0123456789abcdef"; // 16 bytes
        let mut cur = Cursor::new(data.as_slice());
        let err = stream_hash_only(&mut cur, 8, "bomb.bin").unwrap_err();
        let s = err.to_string();
        assert!(s.contains("declared") || s.contains("bomb"), "{s}");
    }

    #[test]
    fn accepts_actual_shorter_than_declared() {
        let data = b"short";
        let mut cur = Cursor::new(data.as_slice());
        // Header claimed more than the stream provides — allowed (deflate/tooling).
        let r = stream_hash_only(&mut cur, 1000, "short.bin").unwrap();
        assert_eq!(r.actual_len, 5);
    }

    #[test]
    fn stream_to_file_roundtrip() {
        let dir = std::env::temp_dir().join(format!("locust_stream_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let dest = dir.join("out.bin");
        let data = vec![0xABu8; 100_000];
        let mut cur = Cursor::new(data.as_slice());
        let r = stream_to_file(&mut cur, data.len() as u64, "out.bin", &dest).unwrap();
        assert_eq!(fs::read(&dest).unwrap(), data);
        assert_eq!(r.sha256_hex, crate::database::sha256_hex(&data));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn charge_declared_rejects_over_ceiling() {
        let max = max_zip_total_bytes();
        let err = charge_declared("huge.bin", max + 1, 0).unwrap_err();
        assert!(err.to_string().contains("limit") || err.to_string().contains("expand"));
    }
}
