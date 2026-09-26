//! Conservative font patch: replace one explicitly selected existing loose font.
//! Generation never changes game files. Existing patch apply/rollback owns backup
//! and receipts. Engine configuration, SDF atlases and layout are not rewritten.

use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::database::sha256_hex;
use crate::error::{LocustError, Result};
use crate::font_validation::{FontCoverageReport, FontValidator, FONT_COVERAGE_LIMITATION};
use crate::patch::zipsec::{ensure_no_links, normalize_entry_name, safe_entry_path};
use crate::patch::{PatchFileEntry, PatchManifest};

#[derive(Debug, Serialize)]
pub struct FontPatchReport {
    pub manifest: PatchManifest,
    pub coverage: FontCoverageReport,
    pub limitations: &'static str,
}

fn invalid(message: impl Into<String>) -> LocustError {
    LocustError::PatchError(message.into())
}

fn read_font(path: &Path) -> Result<Vec<u8>> {
    let data = crate::font_validation::read_font_data(path)?;
    if ttf_parser::fonts_in_collection(&data).is_some() {
        return Err(invalid(
            "font patching requires a single-face TTF/OTF, not a TTC/OTC collection",
        ));
    }
    ttf_parser::Face::parse(&data, 0)
        .map_err(|error| invalid(format!("invalid TTF/OTF font: {error}")))?;
    Ok(data)
}

/// Produces a standard strict-tier Locust patch from a user-supplied font.
/// `target` is the exact game-relative asset already referenced by the game.
/// Callers must write the returned zip without overwriting existing input files.
pub fn build_font_patch(
    game_path: &Path,
    source_font: &Path,
    target: &str,
    engine: &str,
    language: &str,
    translations: &[&str],
) -> Result<(Vec<u8>, FontPatchReport)> {
    let mut engine = engine.replace('_', "-");
    if engine == "html" {
        engine = "html-game".into();
    }
    if !["rpgmaker-mv", "rpgmaker-mz", "renpy", "html-game"].contains(&engine.as_str()) {
        return Err(invalid(format!(
            "unsupported font patch engine: {engine}. {FONT_COVERAGE_LIMITATION}"
        )));
    }
    let mut language_parts = language.split(['-', '_']);
    let primary = language_parts.next().unwrap_or_default();
    if language.len() > 64
        || primary.is_empty()
        || primary.len() > 8
        || !primary.bytes().all(|b| b.is_ascii_alphabetic())
        || language_parts.any(|part| {
            part.is_empty() || part.len() > 8 || !part.bytes().all(|b| b.is_ascii_alphanumeric())
        })
    {
        return Err(invalid("language must have an alphabetic primary code and nonempty alphanumeric subtags (1-8 characters each, separated by - or _)"));
    }
    let root = game_path.canonicalize()?;
    if !root.is_dir() {
        return Err(invalid("game path is not a directory"));
    }
    let normalized = normalize_entry_name(target);
    let rel = safe_entry_path(&normalized, target)?;
    ensure_no_links(&root, &rel)?;
    let destination = root.join(&rel);
    let extension = |path: &Path| {
        path.extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or("")
            .to_ascii_lowercase()
    };
    let target_ext = extension(&destination);
    if !["ttf", "otf"].contains(&target_ext.as_str()) || target_ext != extension(source_font) {
        return Err(invalid("source and existing target must have the same .ttf or .otf extension; web fonts, collections and Unity SDF assets cannot be replaced"));
    }
    // Read and validate BOTH assets before producing any output. An explicit
    // destination is mandatory: guessing font usage from its filename is unsafe.
    let original = read_font(&destination)?;
    let replacement = read_font(source_font)?;
    // Match actual sfnt flavor as well as extension; renamed CFF/TrueType files
    // otherwise change renderer requirements despite a matching filename.
    if original[..4] != replacement[..4] {
        return Err(invalid(
            "source and target have different sfnt formats (TrueType versus CFF/OpenType)",
        ));
    }
    if original == replacement {
        return Err(invalid(
            "source font is identical to target; nothing to patch",
        ));
    }
    let coverage = FontValidator::coverage_from_data(source_font, &replacement, translations)?;
    if coverage.total_unique_chars == 0 {
        return Err(invalid("target text contains no relevant characters; supply the actual translated text before generating a font patch"));
    }
    if !coverage.has_full_coverage {
        return Err(invalid(format!(
            "replacement lacks {} required character(s): {:?}",
            coverage.missing_count, coverage.missing_chars
        )));
    }
    let archive_path = rel.to_string_lossy().replace('\\', "/");
    let manifest = PatchManifest {
        schema_version: PatchManifest::SCHEMA_VERSION,
        patch_id: uuid::Uuid::new_v4().to_string(),
        game_name: root
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
        engine: engine.clone(),
        language: language.into(),
        patch_version: "1.0.0".into(),
        generator_version: env!("CARGO_PKG_VERSION").into(),
        created_at: chrono::Utc::now().to_rfc3339(),
        files: vec![PatchFileEntry {
            path: archive_path.clone(),
            patched_sha256: sha256_hex(&replacement),
            size: replacement.len() as u64,
            original_sha256: Some(sha256_hex(&original)),
        }],
    };
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for (name, data) in [
        (archive_path.as_str(), replacement),
        (PatchManifest::FILENAME, serde_json::to_vec_pretty(&manifest)?),
        ("README.txt", format!("Locust user-supplied loose font patch\n\nApply: locust apply <game> <patch.zip>\nRollback: locust patch-rollback <game>\n\nApply verifies original hashes, backs up the replaced font, and records a receipt.\nThis patch contains the font supplied by its creator; its original license and redistribution terms still apply. No font is downloaded by Locust.\n\n{FONT_COVERAGE_LIMITATION}\nExisting font references, language direction, and layout are unchanged. The patch is for the exact target font asset, not all game text.\n\nLocust tracks one applied patch per game. Do not force this standalone font patch over an installed translation patch: switching patch IDs rolls back the prior patch. Combine assets into one patch or prepare a separate game copy.\n").into_bytes()),
    ] {
        zip.start_file(name, options).map_err(|error| invalid(error.to_string()))?;
        zip.write_all(&data)?;
    }
    let bytes = zip
        .finish()
        .map_err(|error| invalid(error.to_string()))?
        .into_inner();
    Ok((
        bytes,
        FontPatchReport {
            manifest,
            coverage,
            limitations: FONT_COVERAGE_LIMITATION,
        },
    ))
}

/// Save without clobbering an existing file (including a source or game font).
pub fn write_font_patch(output: &Path, bytes: &[u8]) -> Result<PathBuf> {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(output.to_path_buf())
}

/// Merge into a verified translation patch, keeping its identity and all its
/// original hashes. The base must verify cleanly on the supplied game tree.
/// This deliberately bounds in-memory loose-engine patches to 256 MiB.
pub fn combine_font_patch(
    game_path: &Path,
    base_patch: &Path,
    font_archive: &[u8],
    report: &mut FontPatchReport,
) -> Result<Vec<u8>> {
    use crate::patch::verify::{verify, VerificationOutcome};
    const LIMIT: u64 = 256 * 1024 * 1024;
    if std::fs::metadata(base_patch)?.len() > LIMIT {
        return Err(invalid("base font/translation patch exceeds 256 MiB limit"));
    }
    // Bound declared expansion BEFORE the generic verifier streams entries.
    let mut preflight = zip::ZipArchive::new(std::fs::File::open(base_patch)?)
        .map_err(|e| invalid(e.to_string()))?;
    if preflight.len() > 10000 {
        return Err(invalid("base patch exceeds 10000 entries"));
    }
    let mut declared_total = report.manifest.files[0].size;
    for index in 0..preflight.len() {
        let entry = preflight
            .by_index(index)
            .map_err(|e| invalid(e.to_string()))?;
        declared_total = declared_total
            .checked_add(entry.size())
            .ok_or_else(|| invalid("base patch size overflow"))?;
        if declared_total > LIMIT {
            return Err(invalid("combined patch exceeds 256 MiB uncompressed limit"));
        }
    }
    drop(preflight);
    let verification = verify(game_path, base_patch)?;
    if verification.outcome != VerificationOutcome::Clean || !verification.conflicts.is_empty() {
        return Err(invalid("base patch must verify cleanly against this game; use a clean copy or rollback the installed patch first"));
    }
    let mut base = verification
        .manifest
        .ok_or_else(|| invalid("base patch must contain locust-patch.json"))?;
    let family = |engine: &str| {
        let family = engine
            .replace('_', "-")
            .replace("rpgmaker-mz", "rpgmaker-mv");
        if family == "html" {
            "html-game".to_string()
        } else {
            family
        }
    };
    if family(&base.engine) != family(&report.manifest.engine)
        || base.language != report.manifest.language
    {
        return Err(invalid(
            "base patch engine/language must match the requested font patch",
        ));
    }
    let font_entry = report.manifest.files[0].clone();
    let target_key = crate::patch::zipsec::case_fold_key(&crate::patch::zipsec::safe_stored_rel(
        &font_entry.path,
    )?);
    for file in &base.files {
        if crate::patch::zipsec::case_fold_key(&crate::patch::zipsec::safe_stored_rel(&file.path)?)
            == target_key
        {
            return Err(invalid(
                "base patch already contains the requested font destination",
            ));
        }
    }
    let mut data = Vec::new();
    std::fs::File::open(base_patch)?
        .take(LIMIT + 1)
        .read_to_end(&mut data)?;
    if data.len() as u64 > LIMIT {
        return Err(invalid("base patch exceeds 256 MiB limit"));
    }
    let mut source = zip::ZipArchive::new(Cursor::new(data)).map_err(|e| invalid(e.to_string()))?;
    let mut output = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    let mut seen = std::collections::HashSet::new();
    let mut total = font_entry.size;
    let mut manifest_seen = false;
    for index in 0..source.len() {
        let mut entry = source.by_index(index).map_err(|e| invalid(e.to_string()))?;
        if entry.is_dir() || entry.is_symlink() {
            return Err(invalid("base patch may only contain regular files"));
        }
        let name = normalize_entry_name(entry.name());
        safe_entry_path(&name, entry.name())?;
        if !seen.insert(name.to_ascii_lowercase()) {
            return Err(invalid("duplicate base patch path"));
        }
        total = total
            .checked_add(entry.size())
            .ok_or_else(|| invalid("base patch size overflow"))?;
        if total > LIMIT {
            return Err(invalid("combined patch exceeds 256 MiB uncompressed limit"));
        }
        let mut payload = Vec::new();
        let size = entry.size();
        let streamed =
            crate::patch::stream::stream_and_hash(&mut entry, size, &name, Some(&mut payload))?;
        if streamed.actual_len != size {
            return Err(invalid("base patch size mismatch"));
        }
        if name == PatchManifest::FILENAME {
            let snapshot: PatchManifest = serde_json::from_slice(&payload)?;
            if snapshot != base {
                return Err(invalid("base patch changed during verification"));
            }
            manifest_seen = true;
            continue;
        }
        if !name.eq_ignore_ascii_case("README.txt") {
            let expected = base
                .files
                .iter()
                .find(|file| file.path == name)
                .ok_or_else(|| invalid("unlisted base patch file"))?;
            if expected.size != size || expected.patched_sha256 != streamed.sha256_hex {
                return Err(invalid("base patch content changed during verification"));
            }
        } else {
            payload.extend_from_slice(b"\nThis combined patch also replaces the selected user-supplied loose font. Font licensing/redistribution terms remain the creator's responsibility.\n");
            payload.extend_from_slice(FONT_COVERAGE_LIMITATION.as_bytes());
        }
        output
            .start_file(&name, options)
            .map_err(|e| invalid(e.to_string()))?;
        output.write_all(&payload)?;
    }
    if !manifest_seen
        || base
            .files
            .iter()
            .any(|file| !seen.contains(&file.path.to_ascii_lowercase()))
    {
        return Err(invalid(
            "base patch is missing manifest or recorded payload",
        ));
    }
    let mut font_zip =
        zip::ZipArchive::new(Cursor::new(font_archive)).map_err(|e| invalid(e.to_string()))?;
    let mut font = font_zip
        .by_name(&font_entry.path)
        .map_err(|e| invalid(e.to_string()))?;
    output
        .start_file(&font_entry.path, options)
        .map_err(|e| invalid(e.to_string()))?;
    let streamed = crate::patch::stream::stream_and_hash(
        &mut font,
        font_entry.size,
        &font_entry.path,
        Some(&mut output),
    )?;
    if streamed.actual_len != font_entry.size || streamed.sha256_hex != font_entry.patched_sha256 {
        return Err(invalid("font payload hash mismatch"));
    }
    base.files.push(font_entry);
    output
        .start_file(PatchManifest::FILENAME, options)
        .map_err(|e| invalid(e.to_string()))?;
    output.write_all(&serde_json::to_vec_pretty(&base)?)?;
    let combined = output
        .finish()
        .map_err(|e| invalid(e.to_string()))?
        .into_inner();
    report.manifest = base;
    Ok(combined)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::font_validation::build_minimal_ascii_font;
    use crate::patch::{apply, rollback, ApplyOptions, RollbackOptions};

    fn fixture() -> (tempfile::TempDir, PathBuf, Vec<u8>, Vec<u8>) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("fonts")).unwrap();
        let old = build_minimal_ascii_font();
        let mut new = old.clone();
        new.extend_from_slice(b"synthetic font test variant");
        std::fs::write(dir.path().join("fonts/game.ttf"), &old).unwrap();
        let source = dir.path().join("source.ttf");
        std::fs::write(&source, &new).unwrap();
        (dir, source, old, new)
    }

    #[test]
    fn font_patch_apply_backup_receipt_and_exact_rollback() {
        let (dir, source, old, new) = fixture();
        let target = dir.path().join("fonts/game.ttf");
        let (bytes, report) = build_font_patch(
            dir.path(),
            &source,
            "fonts/game.ttf",
            "rpgmaker_mz",
            "es",
            &["abc \n"],
        )
        .unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), old);
        assert!(report.manifest.supports_strict_tier());
        let zip = dir.path().join("font.zip");
        write_font_patch(&zip, &bytes).unwrap();
        assert!(write_font_patch(&zip, b"clobber").is_err());
        let applied = apply(dir.path(), &zip, ApplyOptions::default(), |_| {}).unwrap();
        assert_eq!(applied.replaced, 1);
        assert_eq!(std::fs::read(&target).unwrap(), new);
        let store = crate::patch::PatchStore::new(dir.path());
        assert!(store.backup_manifest_valid());
        assert_eq!(store.read_receipt().unwrap().unwrap().replaced.len(), 1);
        rollback(dir.path(), RollbackOptions::default()).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), old);
    }

    #[test]
    fn font_patch_rejects_wrong_paths_fonts_engines_and_missing_glyphs() {
        let (dir, source, old, _) = fixture();
        for target in [
            "../source.ttf",
            "C:/source.ttf",
            ".locust/font.ttf",
            "missing.ttf",
            "fonts/game.woff",
        ] {
            assert!(
                build_font_patch(dir.path(), &source, target, "html", "es", &["abc"]).is_err(),
                "{target}"
            );
        }
        assert!(build_font_patch(
            dir.path(),
            &source,
            "fonts/game.ttf",
            "unity",
            "zh",
            &["abc"]
        )
        .unwrap_err()
        .to_string()
        .contains("SDF"));
        assert!(build_font_patch(
            dir.path(),
            &source,
            "fonts/game.ttf",
            "renpy",
            "zh",
            &["一"]
        )
        .is_err());
        std::fs::write(&source, b"wOFFbroken").unwrap();
        assert!(build_font_patch(
            dir.path(),
            &source,
            "fonts/game.ttf",
            "html",
            "es",
            &["abc"]
        )
        .is_err());
        assert_eq!(
            std::fs::read(dir.path().join("fonts/game.ttf")).unwrap(),
            old
        );
    }

    fn base_zip(dir: &Path, file_path: &str, old: &[u8], new: &[u8]) -> (PathBuf, PatchManifest) {
        let path = dir.join("base.zip");
        let manifest = PatchManifest {
            schema_version: 1,
            patch_id: "base-translation".into(),
            game_name: "fixture".into(),
            engine: "rpgmaker_mz".into(),
            language: "es".into(),
            patch_version: "1.2.3".into(),
            generator_version: "0.1.0".into(),
            created_at: "2026-09-11T00:00:00Z".into(),
            files: vec![PatchFileEntry {
                path: file_path.into(),
                size: new.len() as u64,
                original_sha256: Some(sha256_hex(old)),
                patched_sha256: sha256_hex(new),
            }],
        };
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
        let options = zip::write::SimpleFileOptions::default();
        zip.start_file(file_path, options).unwrap();
        zip.write_all(new).unwrap();
        zip.start_file(PatchManifest::FILENAME, options).unwrap();
        zip.write_all(&serde_json::to_vec(&manifest).unwrap())
            .unwrap();
        zip.finish().unwrap();
        (path, manifest)
    }

    #[test]
    fn combined_text_and_font_preserves_manifest_applies_and_rolls_back() {
        let (dir, source, old, new) = fixture();
        std::fs::write(dir.path().join("dialogue.txt"), b"Hello").unwrap();
        let (base_path, base) = base_zip(dir.path(), "dialogue.txt", b"Hello", b"Hola");
        let (bytes, mut report) = build_font_patch(
            dir.path(),
            &source,
            "fonts/game.ttf",
            "rpgmaker_mz",
            "es",
            &["Hola"],
        )
        .unwrap();
        let combined = combine_font_patch(dir.path(), &base_path, &bytes, &mut report).unwrap();
        assert_eq!(report.manifest.patch_id, base.patch_id);
        assert_eq!(report.manifest.patch_version, base.patch_version);
        assert_eq!(report.manifest.files[0], base.files[0]);
        assert_eq!(report.manifest.files.len(), 2);
        let path = dir.path().join("combined.zip");
        write_font_patch(&path, &combined).unwrap();
        assert_eq!(
            apply(dir.path(), &path, ApplyOptions::default(), |_| {})
                .unwrap()
                .replaced,
            2
        );
        assert_eq!(
            std::fs::read(dir.path().join("dialogue.txt")).unwrap(),
            b"Hola"
        );
        assert_eq!(
            std::fs::read(dir.path().join("fonts/game.ttf")).unwrap(),
            new
        );
        rollback(dir.path(), RollbackOptions::default()).unwrap();
        assert_eq!(
            std::fs::read(dir.path().join("dialogue.txt")).unwrap(),
            b"Hello"
        );
        assert_eq!(
            std::fs::read(dir.path().join("fonts/game.ttf")).unwrap(),
            old
        );
    }

    #[test]
    fn combined_rejects_conflict_language_and_tampering() {
        let (dir, source, old, new) = fixture();
        let (bytes, mut report) = build_font_patch(
            dir.path(),
            &source,
            "fonts/game.ttf",
            "rpgmaker_mz",
            "es",
            &["Hola"],
        )
        .unwrap();
        let (path, _) = base_zip(dir.path(), "fonts/game.ttf", &old, &new);
        assert!(combine_font_patch(dir.path(), &path, &bytes, &mut report).is_err());
        std::fs::write(dir.path().join("dialogue.txt"), b"Hello").unwrap();
        let (path, _) = base_zip(dir.path(), "dialogue.txt", b"Hello", b"Hola");
        report.manifest.language = "fr".into();
        assert!(combine_font_patch(dir.path(), &path, &bytes, &mut report).is_err());
        report.manifest.language = "es".into();
        std::fs::write(dir.path().join("dialogue.txt"), b"user edit").unwrap();
        assert!(combine_font_patch(dir.path(), &path, &bytes, &mut report).is_err());
    }
}
