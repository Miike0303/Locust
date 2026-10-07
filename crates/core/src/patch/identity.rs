//! Pure game identity helpers shared by patch authors and player-facing clients.

use std::path::Path;

use crate::error::{LocustError, Result};

/// Validate a complete DLsite product code and normalize its prefix to uppercase.
pub fn normalize_dlsite_code(code: &str) -> Result<String> {
    let normalized = code.to_ascii_uppercase();
    let bytes = normalized.as_bytes();
    if matches!(bytes.len(), 8 | 10)
        && matches!(&bytes[..2], b"RJ" | b"RE" | b"VJ" | b"BJ" | b"RG")
        && bytes[2..].iter().all(u8::is_ascii_digit)
    {
        Ok(normalized)
    } else {
        Err(LocustError::PatchError(format!(
            "invalid DLsite code {code:?}: expected RJ, RE, VJ, BJ, or RG followed by 6 or 8 digits"
        )))
    }
}

/// Find the first whole alphanumeric token that is a DLsite code, searching the
/// game folder before its ancestors. Both path separators work on every platform.
pub fn detect_dlsite_code(path: &Path) -> Option<String> {
    let path = path.to_string_lossy();
    path.rsplit(['/', '\\']).find_map(|component| {
        component
            .split(|c: char| !c.is_alphanumeric())
            .find_map(|token| normalize_dlsite_code(token).ok())
    })
}
