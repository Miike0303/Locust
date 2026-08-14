//! Streaming zip-entry extraction for multi‑GB patch apply/verify.
//!
//! Entries are never fully buffered in RAM. Content is hashed (and for apply,
//! written to a same-volume staging file) in fixed-size chunks.

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::error::{LocustError, Result};

use super::zipsec::{check_entry_budget, max_zip_total_bytes};

/// Read chunk size for streaming (1 MiB — balances syscall count vs peak RAM).
const STREAM_CHUNK: usize = 1024 * 1024;

/// Result of streaming one zip entry.
#[derive(Debug, Clone)]
pub struct StreamedBytes {
    pub sha256_hex: String,
    pub actual_len: u64,
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
    mut out: Option<&mut dyn Write>,
) -> Result<StreamedBytes> {
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; STREAM_CHUNK];
    let mut total = 0u64;

    loop {
        let n = reader.read(&mut buf).map_err(|e| {
            LocustError::PatchError(format!("read zip entry \"{entry_name}\": {e}"))
        })?;
        if n == 0 {
            break;
        }
        let n64 = n as u64;
        if total.saturating_add(n64) > declared_uncompressed {
            return Err(LocustError::PatchError(format!(
                "zip entry \"{entry_name}\" expanded past its declared uncompressed size \
                 ({declared_uncompressed} bytes) — aborting (possible zip bomb)"
            )));
        }
        total += n64;
        hasher.update(&buf[..n]);
        if let Some(ref mut w) = out {
            w.write_all(&buf[..n]).map_err(|e| {
                LocustError::PatchError(format!("write staged zip entry \"{entry_name}\": {e}"))
            })?;
        }
    }

    Ok(StreamedBytes {
        sha256_hex: hex::encode(hasher.finalize()),
        actual_len: total,
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
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn child(&self, name: &str) -> PathBuf {
        self.path.join(name)
    }

    /// Keep the directory (normally unused — files are renamed out).
    pub fn disarm(&mut self) {
        self.disarm = true;
    }
}

impl Drop for StagingDir {
    fn drop(&mut self) {
        if !self.disarm {
            let _ = fs::remove_dir_all(&self.path);
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
    fn hash_only_matches_known() {
        let data = b"hello patch stream";
        let mut cur = Cursor::new(data.as_slice());
        let r = stream_hash_only(&mut cur, data.len() as u64, "t").unwrap();
        assert_eq!(r.actual_len, data.len() as u64);
        assert_eq!(r.sha256_hex, crate::database::sha256_hex(data));
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
