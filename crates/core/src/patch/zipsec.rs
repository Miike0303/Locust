//! Zip entry path normalization and security checks.
//!
//! First extractor in the Locust codebase — packaging-side guards only prevent
//! writing bad paths into a zip; this module protects the game tree on extract.

use std::path::{Component, Path, PathBuf};

use crate::error::{LocustError, Result};

/// Windows reserved device names (case-insensitive), with or without extension.
const RESERVED: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6",
    "COM7", "COM8", "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6",
    "LPT7", "LPT8", "LPT9",
];

/// Normalize a zip entry name before any security or extraction work.
///
/// - Fold `\` to `/` (Linux treats `\` as a literal name character; splitting
///   only on `/` would miss `foo\..\..` traversal).
/// - Strip trailing dots and spaces from every component (Windows resolves
///   `foo.` → `foo`).
///
/// Unicode NFC/NFD normalization is explicitly out of scope for v1.
pub fn normalize_entry_name(raw: &str) -> String {
    let folded = raw.replace('\\', "/");
    let parts: Vec<String> = folded
        .split('/')
        .map(|c| {
            // Windows resolves trailing dots/spaces on real names (`foo.` →
            // `foo`), but `.` and `..` are structural — never strip them to
            // empty (that would hide traversal from the security scanner).
            if c == "." || c == ".." {
                c.to_string()
            } else {
                c.trim_end_matches(['.', ' ']).to_string()
            }
        })
        .collect();
    parts.join("/")
}

/// Return true if `name` (a single path component, already normalized) is a
/// Windows reserved device name, with or without a trailing extension.
fn is_reserved_device(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    let base = name.split('.').next().unwrap_or(name);
    RESERVED.iter().any(|r| base.eq_ignore_ascii_case(r))
}

/// Validate a normalized entry name and return a game-root-relative path.
///
/// Rejects: empty names, absolute/prefix components, `..`, drive letters /
/// NTFS ADS (`:`), reserved device names, and any resolution that would
/// escape `game_root`.
pub fn safe_entry_path(normalized: &str, original: &str) -> Result<PathBuf> {
    if normalized.is_empty() || normalized == "/" {
        return Err(LocustError::PatchUnsafeEntry(original.to_string()));
    }

    let mut components: Vec<String> = Vec::new();
    for part in normalized.split('/') {
        if part.is_empty() {
            // Leading/trailing slash remnants are fine; an empty middle
            // component is not a real path we accept.
            continue;
        }
        if part == ".." {
            return Err(LocustError::PatchUnsafeEntry(original.to_string()));
        }
        if part == "." {
            continue;
        }
        if part.contains(':') {
            // Drive letters (`C:foo`) and NTFS ADS (`file:stream`).
            return Err(LocustError::PatchUnsafeEntry(original.to_string()));
        }
        if is_reserved_device(part) {
            return Err(LocustError::PatchUnsafeEntry(original.to_string()));
        }
        // Absolute-looking component on Windows (`\foo` already folded).
        if Path::new(part).is_absolute() {
            return Err(LocustError::PatchUnsafeEntry(original.to_string()));
        }
        components.push(part.to_string());
    }

    if components.is_empty() {
        return Err(LocustError::PatchUnsafeEntry(original.to_string()));
    }

    // First component must not itself be a root/prefix marker.
    if matches!(
        Path::new(&components[0]).components().next(),
        Some(Component::RootDir | Component::Prefix(_))
    ) {
        return Err(LocustError::PatchUnsafeEntry(original.to_string()));
    }

    Ok(components.iter().collect())
}

/// Case-insensitive path key for detecting duplicate zip entries that would
/// collide on NTFS/APFS. Always folds — patches are redistributed, so the
/// producer OS cannot decide safety for the consumer (design WARNING).
pub fn case_fold_key(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/").to_lowercase()
}

/// Validate a stored relative path (receipt / backup manifest / journal)
/// before joining under the game root. Same rules as zip entries — a
/// tampered `.locust/` marker must not escape the game tree.
pub fn safe_stored_rel(rel: &str) -> Result<PathBuf> {
    let normalized = normalize_entry_name(rel);
    safe_entry_path(&normalized, rel)
}

// ── Uncompressed size budgets (zip-bomb / DoS) ─────────────────────────────
//
// **History (W4, pre-streaming):** entries were fully buffered in RAM, so we
// capped each entry at 64 MiB and the whole zip at 512 MiB
// (`MAX_ZIP_ENTRY_BYTES` / old `MAX_ZIP_TOTAL_BYTES`). A real multi‑GB Unreal
// pak patch is a single entry far above those caps, so the memory model had
// to go.
//
// **Now (streaming):** entry bytes go to disk (apply) or are discarded after
// hashing (verify). Peak RAM is a small fixed chunk, not file size. Budgets:
//
// 1. **Declared total ceiling** — sum of local-header uncompressed sizes
//    must stay ≤ [`max_zip_total_bytes`] (default 32 GiB). Checked *before*
//    streaming each entry. Override with env `LOCUST_PATCH_MAX_UNCOMPRESSED`.
// 2. **Actual ≤ declared** — while streaming, if the inflater produces more
//    bytes than the header declared, abort immediately (classic zip bomb).
//
// There is **no** per-entry hard cap anymore; a single 8 GiB entry is valid
// when it fits under the total ceiling.
//
// The old 64 MiB constant is kept only as a documented historical value so
// greps of review notes still resolve.

/// Historical per-entry RAM cap (no longer enforced — see module docs above).
pub const MAX_ZIP_ENTRY_BYTES: u64 = 64 * 1024 * 1024;

/// Default total uncompressed expansion ceiling for one patch zip (32 GiB).
///
/// Not a memory limit: streaming stages to disk. This bounds disk fill and
/// CPU time for a malicious or accidental mega-archive.
pub const MAX_ZIP_TOTAL_BYTES: u64 = 32 * 1024 * 1024 * 1024;

/// Env var for [`max_zip_total_bytes`] (decimal byte count).
pub const MAX_UNCOMPRESSED_ENV: &str = "LOCUST_PATCH_MAX_UNCOMPRESSED";

/// Configured total uncompressed ceiling (env override or [`MAX_ZIP_TOTAL_BYTES`]).
pub fn max_zip_total_bytes() -> u64 {
    if let Ok(raw) = std::env::var(MAX_UNCOMPRESSED_ENV) {
        if let Ok(n) = raw.trim().parse::<u64>() {
            if n > 0 {
                return n;
            }
        }
    }
    MAX_ZIP_TOTAL_BYTES
}

/// Default ceiling for a patch zip fetched over http(s) (32 GiB).
///
/// Bounds a hostile or misconfigured host: a chunked response carries no
/// `Content-Length`, so the only real defence is counting bytes as they land.
pub const MAX_DOWNLOAD_BYTES: u64 = 32 * 1024 * 1024 * 1024;

/// Env var for [`max_download_bytes`] (decimal byte count).
pub const MAX_DOWNLOAD_ENV: &str = "LOCUST_PATCH_MAX_DOWNLOAD";

/// Configured download ceiling (env override or [`MAX_DOWNLOAD_BYTES`]).
///
/// Shared by the CLI and the server so the two transports cannot drift apart.
pub fn max_download_bytes() -> u64 {
    if let Ok(raw) = std::env::var(MAX_DOWNLOAD_ENV) {
        if let Ok(n) = raw.trim().parse::<u64>() {
            if n > 0 {
                return n;
            }
        }
    }
    MAX_DOWNLOAD_BYTES
}

/// Charge an entry's **declared** uncompressed size against the running total.
///
/// Does not read entry bytes. Per-entry size is not limited (streaming);
/// only the zip-wide ceiling applies. Actual expansion vs declared is
/// enforced by [`super::stream::stream_and_hash`].
pub fn check_entry_budget(
    entry_name: &str,
    uncompressed_size: u64,
    total_so_far: u64,
) -> Result<u64> {
    check_entry_budget_with(entry_name, uncompressed_size, total_so_far, max_zip_total_bytes())
}

/// Same as [`check_entry_budget`] with an explicit total ceiling (tests).
pub fn check_entry_budget_with(
    entry_name: &str,
    uncompressed_size: u64,
    total_so_far: u64,
    max_total: u64,
) -> Result<u64> {
    if uncompressed_size > max_total {
        return Err(LocustError::PatchError(format!(
            "zip entry \"{entry_name}\" declares {uncompressed_size} bytes uncompressed \
             (limit {max_total} for the whole patch) — refusing to extract \
             (set {MAX_UNCOMPRESSED_ENV} to raise the ceiling)"
        )));
    }
    let next = total_so_far.saturating_add(uncompressed_size);
    if next > max_total {
        return Err(LocustError::PatchError(format!(
            "patch zip would expand to at least {next} bytes (limit {max_total}) — \
             refusing to extract (set {MAX_UNCOMPRESSED_ENV} to raise the ceiling)"
        )));
    }
    Ok(next)
}

#[cfg(test)]
mod budget_tests {
    use super::*;

    #[test]
    fn accepts_entry_larger_than_historical_64mib() {
        // Streaming removed the 64 MiB per-entry RAM cap.
        let n = check_entry_budget_with("pak", MAX_ZIP_ENTRY_BYTES * 2, 0, MAX_ZIP_TOTAL_BYTES)
            .unwrap();
        assert_eq!(n, MAX_ZIP_ENTRY_BYTES * 2);
    }

    #[test]
    fn rejects_declared_over_total_ceiling() {
        let ceiling = 1024u64;
        let err = check_entry_budget_with("huge.bin", ceiling + 1, 0, ceiling).unwrap_err();
        let s = err.to_string();
        assert!(s.contains("limit") || s.contains("expand") || s.contains("declares"), "{s}");
    }

    #[test]
    fn rejects_sum_over_total_ceiling() {
        let ceiling = 1000u64;
        let t = check_entry_budget_with("a", 600, 0, ceiling).unwrap();
        let err = check_entry_budget_with("b", 600, t, ceiling).unwrap_err();
        assert!(err.to_string().contains("limit") || err.to_string().contains("expand"));
    }

    #[test]
    fn accepts_small() {
        assert_eq!(check_entry_budget_with("a", 100, 0, 512).unwrap(), 100);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_dotdot() {
        let n = normalize_entry_name("foo/../../etc/passwd");
        assert!(safe_entry_path(&n, "foo/../../etc/passwd").is_err());
    }

    #[test]
    fn rejects_backslash_traversal() {
        let n = normalize_entry_name(r"foo\..\..\etc\passwd");
        assert!(safe_entry_path(&n, r"foo\..\..\etc\passwd").is_err());
    }

    #[test]
    fn rejects_absolute_and_drive() {
        let n = normalize_entry_name("C:/Windows/system32");
        // After normalize: C:/Windows/system32 — colon rejected.
        assert!(safe_entry_path(&n, "C:/Windows/system32").is_err());
    }

    #[test]
    fn rejects_ads() {
        let n = normalize_entry_name("data/file.txt:stream");
        assert!(safe_entry_path(&n, "data/file.txt:stream").is_err());
    }

    #[test]
    fn rejects_reserved_device() {
        let n = normalize_entry_name("game/CON");
        assert!(safe_entry_path(&n, "game/CON").is_err());
        let n2 = normalize_entry_name("game/nul.txt");
        assert!(safe_entry_path(&n2, "game/nul.txt").is_err());
    }

    #[test]
    fn accepts_normal_relative() {
        let n = normalize_entry_name("game/scripts/script.rpy");
        let p = safe_entry_path(&n, "game/scripts/script.rpy").unwrap();
        assert_eq!(p, PathBuf::from("game").join("scripts").join("script.rpy"));
    }

    #[test]
    fn trailing_dot_space_normalize() {
        let n = normalize_entry_name("data/Map001.json.");
        assert_eq!(n, "data/Map001.json");
        let n2 = normalize_entry_name("data/foo /bar");
        // "foo " → "foo" after strip
        assert_eq!(n2, "data/foo/bar");
    }

    #[test]
    fn case_fold_detects_collision() {
        assert_eq!(
            case_fold_key(Path::new("Data/Map.json")),
            case_fold_key(Path::new("data/map.json"))
        );
    }
}
