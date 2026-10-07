//! Shared Rule95 entry rendering and safe companion-file publication.

use super::PatchManifest;
use std::path::Path;

/// Facts read from the finished ZIP, separate from the pure Markdown renderer.
pub struct ReleaseEntryMetadata {
    pub manifest: PatchManifest,
    pub zip_sha256: String,
    pub zip_size: u64,
}

/// Render without filesystem access. The caller supplies its detected engine
/// because format plugins depend on core, not the other way around.
pub fn render_release_entry(
    game_path: &Path,
    lang: Option<&str>,
    zip_path: &Path,
    engine_hint: &str,
    metadata: &ReleaseEntryMetadata,
) -> String {
    let ReleaseEntryMetadata {
        manifest,
        zip_sha256,
        zip_size,
    } = metadata;
    // JSON strings are also valid YAML strings, including quotes and newlines.
    let quoted = |text: &str| serde_json::to_string(text).expect("string serialization");
    let game = manifest.game.as_ref();
    let rj = game
        .and_then(|g| g.store_ids.get("dlsite"))
        .map(|code| format!("rjCode: {}\n", quoted(code)))
        .unwrap_or_default();
    let game_version = quoted(
        game.and_then(|g| g.game_version.as_deref())
            .unwrap_or("TODO"),
    );
    let file = zip_path.file_name().unwrap_or_default().to_string_lossy();
    let mut patch = format!(
        "patch:\n  id: {}\n  file: {}\n  size: {zip_size}\n  sha256: {}\n  language: {}\n",
        quoted(&manifest.patch_id),
        quoted(&file),
        quoted(zip_sha256),
        quoted(&manifest.language)
    );
    let fingerprint = game.map(|g| g.fingerprint.as_slice()).unwrap_or_default();
    if fingerprint.is_empty() {
        patch.push_str("  fingerprint: []\n");
    } else {
        patch.push_str("  fingerprint:\n");
        for entry in fingerprint {
            patch.push_str(&format!(
                "    - path: {}\n      size: {}\n      sha256: {}\n",
                quoted(&entry.path),
                entry.size,
                quoted(&entry.sha256)
            ));
        }
    }
    let declared_code = game.and_then(|g| g.store_ids.get("dlsite")).cloned();
    let code = declared_code.or_else(|| super::detect_dlsite_code(game_path));
    let title = release_title(game_path, code.as_deref());
    let target = lang.unwrap_or("es");
    let yaml_title = quoted(&title);

    format!(
        "---\n\
         gameTitle: {yaml_title}\n\
         {rj}\
         sourceLang: \"en\"        # TODO: ja | en | zh | ko | other\n\
         engine: \"{engine_hint}\"  # TODO: pick from the schema enum (rpgmaker-mv/mz/xp/vxace, ...)\n\
         tags: []\n\
         platforms: []            # F95 | DLsite | Ryuugames | Steam | Itch | Other\n\
         originalCreator:\n\
         \x20 name: \"TODO\"\n\
         \x20 links: []            # [{{ label: \"Patreon\", url: \"https://...\" }}]\n\
         storePage: \"\"           # TODO original game page\n\
         gameVersion: {game_version}\n\
         translationVersion: \"1.0\"\n\
         translationStatus: \"complete\"   # complete | in-progress\n\
         gameStatus: \"ongoing\"           # completed | ongoing\n\
         cover: \"\"               # TODO R2 URL\n\
         screenshots: []          # TODO R2 URLs\n\
         mirrors: []              # fill after uploading the patch zip to R2\n\
         dateAdded: TODO-YYYY-MM-DD\n\
         {patch}\
         ---\n\n\
         Translation into {target} of *{title}*, made with Locust.\n\n\
         **How to apply:** Download the patch ZIP, then select it in Rule95 Patcher. \
         Select the original game folder and choose Apply patch. The patcher \
         verifies the game files and creates a restorable backup. Choose \
         Undo last patch in Rule95 Patcher to restore the game.\n"
    )
}

/// Remove matching whole code tokens and their enclosing brackets, then normalize
/// whitespace. Other codes and codes embedded in words remain part of the title.
fn release_title(game_path: &Path, code: Option<&str>) -> String {
    let title = game_path.file_name().unwrap_or_default().to_string_lossy();
    let Some(code) = code else {
        return title.split_whitespace().collect::<Vec<_>>().join(" ");
    };
    let mut cleaned = String::new();
    let mut cursor = 0;
    for (start, _) in title.match_indices(|c: char| c.is_alphanumeric()) {
        if start < cursor {
            continue;
        }
        let end = title[start..]
            .find(|c: char| !c.is_alphanumeric())
            .map_or(title.len(), |offset| start + offset);
        if title[start..end].eq_ignore_ascii_case(code) {
            let mut remove_start = start;
            let mut remove_end = end;
            let before = title[..start].chars().next_back();
            let after = title[end..].chars().next();
            if matches!(
                (before, after),
                (Some('['), Some(']')) | (Some('('), Some(')')) | (Some('{'), Some('}'))
            ) {
                remove_start -= 1;
                remove_end += 1;
            }
            cleaned.push_str(&title[cursor..remove_start]);
            cursor = remove_end;
        } else {
            cleaned.push_str(&title[cursor..end]);
            cursor = end;
        }
    }
    cleaned.push_str(&title[cursor..]);
    cleaned.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Validate the optional companion file before publishing any patch output.
pub fn validate_release_entry_output(
    path: &std::path::Path,
    game_path: &std::path::Path,
    project: &std::path::Path,
    zip: &std::path::Path,
    pristine: Option<&std::path::Path>,
    backup_root: &Path,
) -> anyhow::Result<()> {
    use super::ensure_pack_output_outside;
    if path.as_os_str().is_empty() {
        anyhow::bail!("entry_path required");
    }
    if path.try_exists()? {
        anyhow::bail!(
            "Astro output already exists; choose a new file: {}",
            path.display()
        );
    }
    for protected in [game_path, project, zip, backup_root] {
        ensure_pack_output_outside(path, protected)?;
    }
    if let Some(pristine) = pristine {
        ensure_pack_output_outside(path, pristine)?;
    }
    for suffix in ["-wal", "-shm", "-journal"] {
        let mut sidecar = project.as_os_str().to_os_string();
        sidecar.push(suffix);
        ensure_pack_output_outside(path, std::path::Path::new(&sidecar))?;
    }
    Ok(())
}

/// Read the finished ZIP and publish its entry without replacing any destination.
pub fn write_release_entry(
    path: &Path,
    game_path: &Path,
    lang: Option<&str>,
    zip_path: &Path,
    engine_hint: &str,
) -> anyhow::Result<()> {
    let mut zip = zip::ZipArchive::new(std::fs::File::open(zip_path)?)?;
    let manifest = serde_json::from_reader(zip.by_name(PatchManifest::FILENAME)?)?;
    let (zip_sha256, zip_size) = crate::database::sha256_file(zip_path)?;
    let md = render_release_entry(
        game_path,
        lang,
        zip_path,
        engine_hint,
        &ReleaseEntryMetadata {
            manifest,
            zip_sha256,
            zip_size,
        },
    );
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;
    // `create_new` refuses an existing destination atomically, including one
    // created after preflight, and works on filesystems without hard links
    // (FAT/exFAT drives). A failed write removes only the file it created.
    let mut file = std::fs::File::create_new(path)?;
    let result = std::io::Write::write_all(&mut file, md.as_bytes()).and_then(|()| file.sync_all());
    drop(file);
    if result.is_err() {
        let _ = std::fs::remove_file(path);
    }
    result?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::patch::PatchManifest;

    fn metadata() -> ReleaseEntryMetadata {
        ReleaseEntryMetadata {
            manifest: serde_json::from_value(serde_json::json!({
                "schema_version":1,"patch_id":"test","game_name":"game","engine":"renpy",
                "language":"es","patch_version":"1.0.0","generator_version":"0.1.0","created_at":"t","files":[],
                "game":{"store_ids":{"dlsite":"RJ01234567"},"game_version":"1.2","fingerprint":[{"path":"game/script.rpy","size":4,"sha256":"original"}]}
            })).unwrap(),
            zip_sha256: "abc123".into(),
            zip_size: 123,
        }
    }

    #[test]
    fn release_entry_render_matches_legacy_bytes() {
        let md = render_release_entry(
            Path::new("Demo Game"),
            Some("es"),
            Path::new("patch.zip"),
            "renpy",
            &metadata(),
        );
        assert_eq!(md.as_bytes(), r#"---
gameTitle: "Demo Game"
rjCode: "RJ01234567"
sourceLang: "en"        # TODO: ja | en | zh | ko | other
engine: "renpy"  # TODO: pick from the schema enum (rpgmaker-mv/mz/xp/vxace, ...)
tags: []
platforms: []            # F95 | DLsite | Ryuugames | Steam | Itch | Other
originalCreator:
  name: "TODO"
  links: []            # [{ label: "Patreon", url: "https://..." }]
storePage: ""           # TODO original game page
gameVersion: "1.2"
translationVersion: "1.0"
translationStatus: "complete"   # complete | in-progress
gameStatus: "ongoing"           # completed | ongoing
cover: ""               # TODO R2 URL
screenshots: []          # TODO R2 URLs
mirrors: []              # fill after uploading the patch zip to R2
dateAdded: TODO-YYYY-MM-DD
patch:
  id: "test"
  file: "patch.zip"
  size: 123
  sha256: "abc123"
  language: "es"
  fingerprint:
    - path: "game/script.rpy"
      size: 4
      sha256: "original"
---

Translation into es of *Demo Game*, made with Locust.

**How to apply:** Download the patch ZIP, then select it in Rule95 Patcher. Select the original game folder and choose Apply patch. The patcher verifies the game files and creates a restorable backup. Choose Undo last patch in Rule95 Patcher to restore the game.
"#.as_bytes());
    }

    #[test]
    fn release_entry_title_removes_only_matching_code_tokens() {
        for (name, expected) in [
            ("[RJ01234567] Demo   Game", "Demo Game"),
            ("Demo (rj01234567) Game", "Demo Game"),
            ("RJ01234567 Demo\tGame", "Demo Game"),
            ("[RJ11111111] Demo Game", "[RJ11111111] Demo Game"),
            ("xRJ01234567 Demo Game", "xRJ01234567 Demo Game"),
            ("Demo Game", "Demo Game"),
        ] {
            let md = render_release_entry(
                Path::new(name),
                None,
                Path::new("patch.zip"),
                "other",
                &metadata(),
            );
            assert!(
                md.starts_with(&format!("---\ngameTitle: {expected:?}\n")),
                "{md}"
            );
        }
        let mut data = metadata();
        data.manifest.game = None;
        let md = render_release_entry(
            Path::new("[RJ01234567] Demo Game"),
            None,
            Path::new("patch.zip"),
            "other",
            &data,
        );
        assert!(md.starts_with("---\ngameTitle: \"Demo Game\"\n"));
        assert!(!md.contains("rjCode:"));
    }

    #[test]
    fn release_entry_output_validation_protects_all_inputs() {
        let dir = tempfile::tempdir().unwrap();
        let game = dir.path().join("game");
        let project = dir.path().join("project.db");
        let zip = dir.path().join("patch.zip");
        let pristine = dir.path().join("pristine");
        let backups = dir.path().join("backups");
        for path in [&game, &pristine, &backups] {
            std::fs::create_dir(path).unwrap();
        }
        let existing = dir.path().join("existing.md");
        std::fs::write(&existing, "keep").unwrap();
        for path in [
            game.join("new/entry.md"),
            pristine.join("entry.md"),
            backups.join("entry.md"),
            project.clone(),
            dir.path().join("project.db-wal"),
            dir.path().join("project.db-shm"),
            dir.path().join("project.db-journal"),
            zip.clone(),
            existing.clone(),
        ] {
            assert!(
                validate_release_entry_output(
                    &path,
                    &game,
                    &project,
                    &zip,
                    Some(&pristine),
                    &backups
                )
                .is_err(),
                "{}",
                path.display()
            );
        }
        assert!(!game.join("new").exists());
        assert_eq!(std::fs::read_to_string(existing).unwrap(), "keep");
        validate_release_entry_output(
            &dir.path().join("entry.md"),
            &game,
            &project,
            &zip,
            Some(&pristine),
            &backups,
        )
        .unwrap();
    }

    #[test]
    fn release_entry_writer_hashes_written_zip_and_never_overwrites() {
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();
        let zip_path = dir.path().join("patch.zip");
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&zip_path).unwrap());
        zip.start_file(
            PatchManifest::FILENAME,
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
        zip.write_all(&serde_json::to_vec(&metadata().manifest).unwrap())
            .unwrap();
        zip.finish().unwrap();
        let entry = dir.path().join("entry.md");
        write_release_entry(
            &entry,
            Path::new("Demo Game"),
            Some("es"),
            &zip_path,
            "renpy",
        )
        .unwrap();
        let original = std::fs::read(&entry).unwrap();
        let (hash, size) = crate::database::sha256_file(&zip_path).unwrap();
        let md = String::from_utf8(original.clone()).unwrap();
        assert!(md.contains(&format!("  size: {size}\n  sha256: {hash:?}\n")));
        assert!(
            write_release_entry(&entry, Path::new("changed"), None, &zip_path, "other").is_err()
        );
        assert_eq!(std::fs::read(&entry).unwrap(), original);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
    }
}
