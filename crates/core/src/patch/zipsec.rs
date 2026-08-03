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

/// Cap a single uncompressed zip entry (bytes). Above this we refuse to
/// buffer the entry in memory (zip-bomb / multi-GB patch DoS — review W4).
pub const MAX_ZIP_ENTRY_BYTES: u64 = 64 * 1024 * 1024;

/// Cap total uncompressed content read from one patch zip.
pub const MAX_ZIP_TOTAL_BYTES: u64 = 512 * 1024 * 1024;

/// Refuse entries whose declared uncompressed size is absurd before reading.
pub fn check_entry_budget(
    entry_name: &str,
    uncompressed_size: u64,
    total_so_far: u64,
) -> Result<u64> {
    if uncompressed_size > MAX_ZIP_ENTRY_BYTES {
        return Err(LocustError::PatchError(format!(
            "zip entry \"{entry_name}\" is {uncompressed_size} bytes uncompressed \
             (limit {} per entry) — refusing to load into memory",
            MAX_ZIP_ENTRY_BYTES
        )));
    }
    let next = total_so_far.saturating_add(uncompressed_size);
    if next > MAX_ZIP_TOTAL_BYTES {
        return Err(LocustError::PatchError(format!(
            "patch zip would expand to at least {next} bytes (limit {}) — \
             refusing to load into memory",
            MAX_ZIP_TOTAL_BYTES
        )));
    }
    Ok(next)
}

#[cfg(test)]
mod budget_tests {
    use super::*;

    #[test]
    fn rejects_oversized_entry() {
        let err = check_entry_budget("huge.bin", MAX_ZIP_ENTRY_BYTES + 1, 0).unwrap_err();
        assert!(err.to_string().contains("per entry"));
    }

    #[test]
    fn rejects_total_over_budget() {
        // Use max-per-entry chunks until the total budget trips.
        let chunk = MAX_ZIP_ENTRY_BYTES;
        let mut total = 0u64;
        let mut last_err = None;
        for i in 0..20 {
            match check_entry_budget(&format!("e{i}"), chunk, total) {
                Ok(n) => total = n,
                Err(e) => {
                    last_err = Some(e);
                    break;
                }
            }
        }
        assert!(last_err.is_some(), "must eventually hit total budget");
        assert!(last_err.unwrap().to_string().contains("limit"));
    }

    #[test]
    fn accepts_small() {
        assert_eq!(check_entry_budget("a", 100, 0).unwrap(), 100);
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
