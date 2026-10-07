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

/// Resolve explicit identities and optionally detect a DLsite code from the path.
/// The second value is present only when detection supplied the identity.
pub fn resolve_patch_identity(
    rj: Option<&str>,
    store_ids: &[String],
    game_version: Option<&str>,
    detect_id: bool,
    path: &Path,
) -> anyhow::Result<(super::GameIdentity, Option<String>)> {
    use super::GameIdentity;
    let mut game = GameIdentity::default();
    if let Some(code) = rj {
        game.store_ids
            .insert("dlsite".into(), normalize_dlsite_code(code)?);
    }
    for value in store_ids {
        let (store, id) = value
            .split_once('=')
            .ok_or_else(|| anyhow::anyhow!("invalid --store-id {value:?}: expected store=id"))?;
        let store = store.trim().to_lowercase();
        let id = id.trim();
        if store.is_empty() || id.is_empty() {
            anyhow::bail!("invalid --store-id {value:?}: store and id must not be empty");
        }
        if game.store_ids.contains_key(&store) {
            anyhow::bail!("duplicate store key {store:?}; supply each store only once");
        }
        let id = if store == "dlsite" {
            normalize_dlsite_code(id)?
        } else {
            id.to_string()
        };
        game.store_ids.insert(store, id);
    }
    if let Some(version) = game_version {
        let version = version.trim();
        if version.is_empty() {
            anyhow::bail!("game version must not be empty (--game-version)");
        }
        game.game_version = Some(version.to_string());
    }
    let mut detected = None;
    if detect_id && !game.store_ids.contains_key("dlsite") {
        if let Some(code) = detect_dlsite_code(path) {
            detected = Some(code.clone());
            game.store_ids.insert("dlsite".into(), code);
        }
    }
    Ok((game, detected))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_resolution_normalizes_and_reports_only_automatic_detection() {
        let path = Path::new("D:/RJ123456/[RE01234567] Game");
        let (game, detected) =
            resolve_patch_identity(None, &[], Some(" 1.2 "), true, path).unwrap();
        assert_eq!(game.store_ids["dlsite"], "RE01234567");
        assert_eq!(game.game_version.as_deref(), Some("1.2"));
        assert_eq!(detected.as_deref(), Some("RE01234567"));
        let (game, detected) = resolve_patch_identity(
            Some("vj123456"),
            &[" Steam = 123 ".into()],
            None,
            true,
            path,
        )
        .unwrap();
        assert_eq!(game.store_ids["dlsite"], "VJ123456");
        assert_eq!(game.store_ids["steam"], "123");
        assert_eq!(detected, None);
        assert!(resolve_patch_identity(None, &[], None, false, path)
            .unwrap()
            .0
            .store_ids
            .is_empty());
        let (game, detected) =
            resolve_patch_identity(None, &["DLSITE=bj123456".into()], None, true, path).unwrap();
        assert_eq!(game.store_ids["dlsite"], "BJ123456");
        assert_eq!(detected, None);
    }

    #[test]
    fn identity_resolution_preserves_validation_messages() {
        for (rj, ids, version, expected) in [
            (Some("RJ12345"), vec![], None, "invalid DLsite code"),
            (
                None,
                vec!["steam"],
                None,
                "invalid --store-id \"steam\": expected store=id",
            ),
            (None, vec!["=123"], None, "store and id must not be empty"),
            (
                None,
                vec!["steam= "],
                None,
                "store and id must not be empty",
            ),
            (
                None,
                vec!["Steam=1", " steam =2"],
                None,
                "duplicate store key \"steam\"; supply each store only once",
            ),
            (
                Some("RJ123456"),
                vec!["dlsite=RJ123456"],
                None,
                "duplicate store key \"dlsite\"",
            ),
            (None, vec!["dlsite=nope"], None, "invalid DLsite code"),
            (
                None,
                vec![],
                Some("  "),
                "game version must not be empty (--game-version)",
            ),
        ] {
            let ids = ids.into_iter().map(String::from).collect::<Vec<_>>();
            let error = resolve_patch_identity(rj, &ids, version, true, Path::new("RJ123456"))
                .unwrap_err()
                .to_string();
            assert!(error.contains(expected), "{error}");
        }
    }
}
