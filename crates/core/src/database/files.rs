use std::path::Path;

use sha2::{Digest, Sha256};

use crate::error::Result;

/// Lowercase hex SHA-256 of `bytes` — the hash stored in injection recordings.
/// SHA-256 over BLAKE3 because `sha2` is already in the dependency tree; the
/// column is TEXT, so nothing else depends on the choice.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

/// 1 MiB — same chunk as patch apply/verify streaming. Pack and injection
/// recording MUST use this path for multi-GB game files; `fs::read` peak RAM
/// equals file size (2× with `--pristine`).
const FILE_HASH_CHUNK: usize = 1024 * 1024;

/// SHA-256 a file without loading it entirely into RAM.
/// Returns `(lowercase_hex, byte_len)`.
pub fn sha256_file(path: &Path) -> Result<(String, u64)> {
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; FILE_HASH_CHUNK];
    let mut total = 0u64;
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        total += n as u64;
        hasher.update(&buf[..n]);
    }
    Ok((hex::encode(hasher.finalize()), total))
}

#[cfg(test)]
thread_local! {
    pub(crate) static SHA256_PATH_CALLS: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
}

/// Digest of a file on disk — same hex as [`sha256_hex`] of its bytes.
pub fn sha256_path(path: &Path) -> Result<String> {
    #[cfg(test)]
    SHA256_PATH_CALLS.with(|calls| {
        if let Some(n) = calls.get() {
            calls.set(Some(n + 1));
        }
    });
    Ok(sha256_file(path)?.0)
}

/// Stream `path` into `out` in fixed-size chunks (no full-file buffer).
pub fn copy_path_chunked(path: &Path, out: &mut dyn std::io::Write) -> Result<u64> {
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut buf = vec![0u8; FILE_HASH_CHUNK];
    let mut total = 0u64;
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        out.write_all(&buf[..n])?;
        total += n as u64;
    }
    Ok(total)
}

/// Case-fold a path fragment for comparison — ONLY where the filesystem
/// itself folds case (NTFS, APFS). On ext4 two case spellings are two
/// different files, and folding would invent a match between them.
pub fn fold_path_case(s: &str) -> String {
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    {
        s.to_lowercase()
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        s.to_string()
    }
}

/// Decompose a path into comparable (folded key, raw component) pairs.
///
/// Both sides of every recording comparison go through this, because the two
/// spellings of one path routinely diverge: relative vs absolute, drive or
/// directory case, or a `\\?\` verbatim prefix left behind by `canonicalize`.
/// A literal `strip_prefix` silently fails on all of those.
///
/// `canonicalize` is preferred (it resolves symlinks and on-disk casing); a
/// path that does not exist on disk cannot be canonicalized, so it falls back
/// to lexical absolutization, which still repairs relative-vs-absolute
/// divergence.
///
/// ponytail: duplicated in spirit with `normalize_path_for_compare` in the
/// Ren'Py plugin; unify once core grows a path-identity module. Ceiling: the
/// two implementations could drift on a platform-specific edge.
fn resolved_parts(p: &Path) -> Vec<(String, std::ffi::OsString)> {
    use std::path::{Component, Prefix};
    let resolved = p
        .canonicalize()
        .or_else(|_| std::path::absolute(p))
        .unwrap_or_else(|_| p.to_path_buf());
    let mut out = Vec::new();
    for c in resolved.components() {
        let key = match c {
            // `\\?\C:\` (verbatim, from canonicalize) and `C:\` (plain) name
            // the same drive and must compare equal.
            Component::Prefix(pr) => match pr.kind() {
                Prefix::VerbatimDisk(d) | Prefix::Disk(d) => {
                    format!("{}:", (d as char))
                }
                Prefix::VerbatimUNC(server, share) | Prefix::UNC(server, share) => format!(
                    r"\\{}\{}",
                    server.to_string_lossy(),
                    share.to_string_lossy()
                ),
                _ => pr.as_os_str().to_string_lossy().into_owned(),
            },
            // Both sides are absolute after resolution, so the root marker
            // carries no information.
            Component::RootDir | Component::CurDir => continue,
            Component::ParentDir => "..".to_string(),
            Component::Normal(s) => s.to_string_lossy().into_owned(),
        };
        out.push((fold_path_case(&key), c.as_os_str().to_os_string()));
    }
    out
}

/// True when two spellings name the same on-disk location. This is the
/// identity check `locust patch` runs between its game-path argument and a
/// recording's root: packing from a different tree would ship that tree's
/// files, not the ones injection wrote.
pub fn paths_identical(a: &Path, b: &Path) -> bool {
    let ka: Vec<String> = resolved_parts(a).into_iter().map(|(k, _)| k).collect();
    let kb: Vec<String> = resolved_parts(b).into_iter().map(|(k, _)| k).collect();
    !ka.is_empty() && ka == kb
}

/// Forward-slash path of `file` relative to `root`, or `None` when `file` is
/// not strictly under `root`. This is the containment check behind injection
/// recordings: a file that does not resolve under its language's root must
/// hard-fail at record time, never be recorded against the wrong tree.
pub fn rel_under_root(file: &Path, root: &Path) -> Option<String> {
    let f = resolved_parts(file);
    let r = resolved_parts(root);
    if f.len() <= r.len() || !f.iter().zip(&r).all(|(a, b)| a.0 == b.0) {
        return None;
    }
    Some(
        f[r.len()..]
            .iter()
            .map(|(_, raw)| raw.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/"),
    )
}

/// Resolved, case-folded identity key for one physical file — two spellings
/// of the same file compare equal, two different files never do.
pub(crate) fn path_identity_key(p: &Path) -> String {
    resolved_parts(p)
        .iter()
        .map(|(k, _)| k.as_str())
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::super::tests::{make_recorded_tree, recording_tempdir};
    use super::*;

    #[test]
    fn sha256_file_matches_in_memory_across_chunk_boundary() {
        // FILE_HASH_CHUNK is 1 MiB — force at least two reads.
        let dir_guard = recording_tempdir();
        let dir = dir_guard.path().to_path_buf();
        let path = dir.join("big.bin");
        let mut data = vec![0u8; FILE_HASH_CHUNK + 50_000];
        for (i, b) in data.iter_mut().enumerate() {
            *b = (i % 251) as u8;
        }
        std::fs::write(&path, &data).unwrap();
        let (hash, size) = sha256_file(&path).unwrap();
        assert_eq!(size, data.len() as u64);
        assert_eq!(hash, sha256_hex(&data));
        assert_eq!(sha256_path(&path).unwrap(), hash);
        // Negative: wrong length must not pass as equal to a truncated hash input.
        assert_ne!(hash, sha256_hex(&data[..data.len() - 1]));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn copy_path_chunked_roundtrip_multi_chunk() {
        let dir_guard = recording_tempdir();
        let dir = dir_guard.path().to_path_buf();
        let src = dir.join("src.bin");
        let dst = dir.join("dst.bin");
        let data = vec![0x5Au8; FILE_HASH_CHUNK + 12];
        std::fs::write(&src, &data).unwrap();
        let mut out = std::fs::File::create(&dst).unwrap();
        let n = copy_path_chunked(&src, &mut out).unwrap();
        drop(out);
        assert_eq!(n, data.len() as u64);
        assert_eq!(std::fs::read(&dst).unwrap(), data);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ─── path helpers backing the recording contract ────────────────────────

    #[test]
    fn test_rel_under_root_uses_forward_slashes() {
        let base_guard = recording_tempdir();
        let base = base_guard.path().to_path_buf();
        let (root, file, _) = make_recorded_tree(&base);
        assert_eq!(
            rel_under_root(&file, &root),
            Some("game/script.rpy".to_string())
        );
    }

    #[test]
    fn test_rel_under_root_resolves_relative_vs_absolute_spelling() {
        // The recording is absolutized at record time, but the file list a
        // plugin reports can be spelled relative when inject was invoked with
        // a relative game path. A purely lexical prefix match would fail.
        let cwd = std::env::current_dir().unwrap();
        let stored = cwd.join("mygame").join("Data").join("BasicData.wolf");
        assert_eq!(
            rel_under_root(&stored, Path::new("mygame")),
            Some("Data/BasicData.wolf".to_string())
        );
    }

    #[cfg(any(windows, target_os = "macos"))]
    #[test]
    fn test_rel_under_root_is_case_insensitive_where_the_filesystem_is() {
        assert_eq!(
            rel_under_root(
                Path::new("/games/MYGAME/Data/BasicData.wolf"),
                Path::new("/Games/MyGame")
            ),
            Some("Data/BasicData.wolf".to_string())
        );
    }

    #[cfg(windows)]
    #[test]
    fn test_rel_under_root_resolves_verbatim_prefix_spelling() {
        // `canonicalize()` yields `\\?\C:\...` verbatim paths on Windows. A
        // plainly spelled file must still match a verbatim-spelled root.
        let base_guard = recording_tempdir();
        let base = base_guard.path().to_path_buf();
        let (root, file, _) = make_recorded_tree(&base);
        let verbatim_root = root.canonicalize().unwrap();
        assert_eq!(
            rel_under_root(&file, &verbatim_root),
            Some("game/script.rpy".to_string())
        );
    }

    #[test]
    fn test_rel_under_root_is_none_outside_the_root_and_for_the_root_itself() {
        let base_guard = recording_tempdir();
        let base = base_guard.path().to_path_buf();
        let (root, _file, _) = make_recorded_tree(&base);
        assert_eq!(rel_under_root(&base.join("elsewhere.txt"), &root), None);
        assert_eq!(
            rel_under_root(&root, &root),
            None,
            "the root itself is not a file under the root"
        );
    }

    #[test]
    fn test_paths_identical_across_spellings() {
        let base_guard = recording_tempdir();
        let base = base_guard.path().to_path_buf();
        let (root, _file, _) = make_recorded_tree(&base);
        let canonical = root.canonicalize().unwrap();
        assert!(paths_identical(&root, &canonical));
        assert!(!paths_identical(&root, &base));
    }
}
