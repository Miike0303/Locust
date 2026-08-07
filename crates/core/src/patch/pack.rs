//! Pack a Locust patch zip from a recorded injection (shared by CLI + HTTP).
//!
//! Packs **exclusively** from the injection recording (root + rel + hash per
//! language key). Same rules as the former CLI-only `cmd_patch` orchestration.

use std::io::Write;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::database::{paths_identical, sha256_hex, Database};
use crate::error::{LocustError, Result};
use crate::models::StringStatus;
use crate::patch::manifest::{PatchFileEntry, PatchManifest};
use crate::patch::store::PatchStore;

/// Options for [`pack_injection_recording`].
#[derive(Debug, Clone)]
pub struct PackOptions {
    /// Game tree the patch is for (must match the recording root).
    pub game_path: PathBuf,
    /// Language key for the recording (`None` = auto when exactly one exists).
    pub lang: Option<String>,
    /// Destination zip path (parent dirs created as needed).
    pub output: PathBuf,
    /// Optional pristine game root for `original_sha256` (strict-tier verify).
    /// When `None`, a valid `.locust/backup/` under `game_path` is used if present.
    pub pristine: Option<PathBuf>,
    /// Engine id for the patch manifest (e.g. `"renpy"`). Defaults to `"unknown"`.
    pub engine: Option<String>,
    /// Project database path — used only to render exact, runnable commands in
    /// error messages (`locust inject "<game>" -P "<project>" …`).
    pub project: PathBuf,
    /// Require pristine hashes: error if neither `pristine` nor a valid backup exist.
    pub require_pristine: bool,
}

/// Summary returned after a successful pack (JSON-friendly for the HTTP API).
#[derive(Debug, Clone, Serialize)]
pub struct PackReport {
    pub output_path: String,
    pub recording_lang: Option<String>,
    pub recorded_root: String,
    pub files_packed: usize,
    pub translated_strings: usize,
    pub size_bytes: u64,
    pub patch_id: String,
    pub patch_version: String,
    pub engine: String,
    pub language: String,
    /// `"strict"` when original hashes were embedded; otherwise `"structural"`.
    pub tier: String,
    pub messages: Vec<String>,
}

fn key_label(k: &Option<String>) -> String {
    k.clone().unwrap_or_else(|| "(unspecified)".to_string())
}

fn pack_err(msg: impl Into<String>) -> LocustError {
    LocustError::PatchError(msg.into())
}

/// Engines whose `inject` mutates the ORIGINAL game tree (entry-tree writers
/// plus Ren'Py, whose loose scripts are rewritten in place). Mirrors the CLI's
/// `mutates_original_tree`, keyed on the engine id the caller detected.
fn engine_mutates_original_tree(id: &str) -> bool {
    matches!(id, "unity" | "unreal" | "wolf-rpg" | "renpy")
}

/// Appended to advice that names a `--direct` re-run, for engines that mutate
/// the original tree: a legacy database on an already-injected game would loop
/// on the identical error forever without it.
fn maybe_mutated_note(engine: Option<&str>) -> &'static str {
    if engine.is_some_and(engine_mutates_original_tree) {
        "\nThis engine writes translations into the ORIGINAL game files: if this \
         game was already injected (for example through an older Locust that kept \
         no recording), that command will report 0 files written and record \
         nothing — restore the original game files from a backup or a clean copy \
         first, then re-run it."
    } else {
        ""
    }
}

/// Pack a patch zip from the injection recording stored in `db`.
pub fn pack_injection_recording(db: &Database, opts: PackOptions) -> Result<PackReport> {
    let game_path = opts.game_path;
    let project = opts.project;
    let engine_id = opts.engine.clone();
    let lang = opts.lang;
    let mut messages = Vec::new();

    // Friendly pre-check: no translations → nothing to pack.
    let entries = db.get_entries(&crate::database::EntryFilter::default())?;
    let translated = entries
        .iter()
        .filter(|e| {
            e.translation.as_deref().is_some_and(|t| !t.trim().is_empty())
                && matches!(
                    e.status,
                    StringStatus::Translated | StringStatus::Reviewed | StringStatus::Approved
                )
        })
        .count();
    if translated == 0 {
        return Err(pack_err(
            "no translated, reviewed, or approved strings — nothing to pack yet. Run translate first.",
        ));
    }

    let lang_flag = lang
        .as_deref()
        .map(|l| format!(" -l {l}"))
        .unwrap_or_default();

    let keys = db.list_recorded_langs()?;
    if keys.is_empty() {
        return Err(pack_err(format!(
            "no injection has been recorded in \"{}\". `locust patch` packs exactly \
             the files a recorded injection wrote — never a list guessed from the \
             database. Run `locust inject \"{}\" -P \"{}\" --direct{}` first, then \
             re-run patch.{}",
            project.display(),
            game_path.display(),
            project.display(),
            lang_flag,
            maybe_mutated_note(engine_id.as_deref())
        )));
    }

    let recording = match lang.as_deref() {
        Some(l) => match db.get_injection(Some(l))? {
            Some(rec) => rec,
            None => {
                let mut alternatives = String::new();
                for k in &keys {
                    let Some(rec) = db.get_injection(k.as_deref())? else {
                        continue;
                    };
                    if !paths_identical(&game_path, &rec.root) {
                        continue;
                    }
                    match k {
                        Some(kk) => {
                            alternatives.push_str(&format!(", or re-run patch with -l {kk}"))
                        }
                        None if keys.len() == 1 => {
                            alternatives.push_str(", or re-run patch without -l")
                        }
                        None => {}
                    }
                }
                let listed = keys
                    .iter()
                    .map(|k| format!("\"{}\"", key_label(k)))
                    .collect::<Vec<_>>()
                    .join(", ");
                return Err(pack_err(format!(
                    "no injection recorded for language \"{l}\"; recorded: [{listed}]. \
                     Run `locust inject \"{}\" -P \"{}\" --direct -l {l}` to record \
                     it{alternatives}.{}",
                    game_path.display(),
                    project.display(),
                    maybe_mutated_note(engine_id.as_deref())
                )));
            }
        },
        None => {
            if keys.len() == 1 {
                db.get_injection(keys[0].as_deref())?
                    .expect("a listed key must resolve to its recording")
            } else {
                // Several recordings exist: packing their union produced
                // mixed-language zips and cross-copy collisions, so the key
                // must be the user's explicit choice.
                let mut listed = String::new();
                let mut example: Option<String> = None;
                let mut fallback_example: Option<String> = None;
                for k in &keys {
                    let Some(rec) = db.get_injection(k.as_deref())? else {
                        continue;
                    };
                    listed.push_str(&format!("\n  {} → {}", key_label(k), rec.root.display()));
                    if let Some(kk) = k {
                        if example.is_none() && paths_identical(&game_path, &rec.root) {
                            example = Some(format!(
                                "locust patch \"{}\" -P \"{}\" -l {kk}",
                                game_path.display(),
                                project.display()
                            ));
                        }
                        if fallback_example.is_none() {
                            fallback_example = Some(format!(
                                "locust patch \"{}\" -P \"{}\" -l {kk}",
                                rec.root.display(),
                                project.display()
                            ));
                        }
                    }
                }
                let example = example.or(fallback_example).unwrap_or_default();
                // "Pass -l <lang>" alone cannot reach the (unspecified)
                // recording — no -l value names the NULL key; say how.
                let unspecified_note = match keys.iter().find(|k| k.is_none()) {
                    Some(_) => {
                        let root = db
                            .get_injection(None)?
                            .map(|rec| rec.root.display().to_string())
                            .unwrap_or_else(|| game_path.display().to_string());
                        format!(
                            "\nNo -l value can name the \"(unspecified)\" recording; \
                             to pack it, re-record it under a named language first: \
                             locust inject \"{root}\" -P \"{}\" --direct -l <lang>, \
                             then re-run patch with that -l.",
                            project.display()
                        )
                    }
                    None => String::new(),
                };
                return Err(pack_err(format!(
                    "multiple injection recordings exist in \"{}\", so `patch` without \
                     -l is ambiguous and refused:{listed}\nPass -l <lang> to choose \
                     one. Example: {example}{unspecified_note}",
                    project.display()
                )));
            }
        }
    };

    if !paths_identical(&game_path, &recording.root) {
        return Err(pack_err(format!(
            "the recorded injection for {} wrote into \"{}\", not \"{}\". \
             Packing from a different tree is refused. Point game_path at the recorded root.",
            key_label(&recording.lang),
            recording.root.display(),
            game_path.display()
        )));
    }

    let out = opts.output;
    if let Some(parent) = out.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }

    let tmp = out.with_file_name(format!(
        "{}.tmp-{}",
        out.file_name().unwrap_or_default().to_string_lossy(),
        std::process::id()
    ));
    let zip_file = std::fs::File::create(&tmp)?;
    let mut tmp_guard = TempFileGuard::new(&tmp);
    let mut zip = zip::ZipWriter::new(zip_file);
    let zip_opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);

    let pristine_root: Option<PathBuf> = if let Some(p) = opts.pristine {
        if !p.is_dir() {
            return Err(pack_err(format!(
                "pristine path is not a directory: {}",
                p.display()
            )));
        }
        Some(p)
    } else {
        let store = PatchStore::new(&game_path);
        if store.backup_manifest_valid() {
            Some(store.backup_files_dir())
        } else {
            None
        }
    };

    if opts.require_pristine && pristine_root.is_none() {
        return Err(pack_err(
            "pristine hashes required but no --pristine path and no valid .locust/backup found",
        ));
    }
    if pristine_root.is_none() {
        messages.push(
            "packing without original hashes (no pristine path, no .locust/backup); \
             apply will use structural verification"
                .into(),
        );
    }

    let mut added = 0usize;
    let mut missing: Vec<PathBuf> = Vec::new();
    let mut changed: Vec<String> = Vec::new();
    let mut manifest_files: Vec<PatchFileEntry> = Vec::new();

    for f in &recording.files {
        let rel = Path::new(&f.rel);
        if rel
            .components()
            .any(|c| !matches!(c, std::path::Component::Normal(_)))
        {
            return Err(pack_err(format!(
                "recorded path \"{}\" escapes the game root — refusing to pack it",
                f.rel
            )));
        }
        let src = recording.root.join(rel.components().collect::<PathBuf>());
        let bytes = match std::fs::read(&src) {
            Ok(b) => b,
            Err(_) => {
                missing.push(src);
                continue;
            }
        };
        if bytes.len() as u64 != f.size || sha256_hex(&bytes) != f.hash {
            changed.push(f.rel.clone());
            continue;
        }
        let original_sha256 = pristine_root.as_ref().and_then(|root| {
            let p = root.join(rel.components().collect::<PathBuf>());
            std::fs::read(&p).ok().map(|b| sha256_hex(&b))
        });
        manifest_files.push(PatchFileEntry {
            path: f.rel.clone(),
            patched_sha256: f.hash.clone(),
            size: f.size,
            original_sha256,
        });
        // ZIP64 for entries at/over 4 GiB (multi-GB Unreal base paks).
        let entry_opts = zip_opts.large_file(bytes.len() as u64 >= 0xFFFF_FFFF);
        zip.start_file(f.rel.clone(), entry_opts)
            .map_err(|e| pack_err(format!("zip start_file {}: {e}", f.rel)))?;
        zip.write_all(&bytes)?;
        added += 1;
    }

    if !missing.is_empty() || !changed.is_empty() {
        drop(zip);
        let mut detail = String::new();
        if !changed.is_empty() {
            detail.push_str("\n  changed on disk since injection recorded them:");
            for rel in changed.iter().take(5) {
                detail.push_str(&format!("\n    {rel}"));
            }
            if changed.len() > 5 {
                detail.push_str(&format!("\n    ... and {} more", changed.len() - 5));
            }
        }
        if !missing.is_empty() {
            detail.push_str("\n  missing from disk:");
            for p in missing.iter().take(5) {
                detail.push_str(&format!("\n    {}", p.display()));
            }
            if missing.len() > 5 {
                detail.push_str(&format!("\n    ... and {} more", missing.len() - 5));
            }
        }
        return Err(pack_err(format!(
            "{} of {} recorded file(s) no longer match what injection wrote:{detail}\n\
             Re-run inject --direct{lang_flag} to refresh the recording, then re-run pack.",
            missing.len() + changed.len(),
            recording.files.len(),
        )));
    }

    let engine = opts
        .engine
        .unwrap_or_else(|| "unknown".into());
    let language = lang
        .clone()
        .or(recording.lang.clone())
        .unwrap_or_else(|| "unknown".into());
    let patch_id = uuid::Uuid::new_v4().to_string();
    let patch_version = "1.0.0".to_string();

    let patch_manifest = PatchManifest {
        schema_version: PatchManifest::SCHEMA_VERSION,
        patch_id: patch_id.clone(),
        game_name: game_path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "game".into()),
        engine: engine.clone(),
        language: language.clone(),
        patch_version: patch_version.clone(),
        generator_version: env!("CARGO_PKG_VERSION").into(),
        created_at: chrono::Utc::now().to_rfc3339(),
        files: manifest_files,
    };
    let tier = if patch_manifest.supports_strict_tier() {
        "strict"
    } else {
        "structural"
    };

    zip.start_file(PatchManifest::FILENAME, zip_opts)
        .map_err(|e| pack_err(format!("zip manifest: {e}")))?;
    zip.write_all(serde_json::to_string_pretty(&patch_manifest)?.as_bytes())?;

    let readme = "rule95 / Locust translation patch\n\n\
        Preferred apply:  locust apply <game> <this.zip>\n\
        Manual apply:     extract over your game folder, replacing files.\n\
        Back up your game folder first (locust apply does this for you).\n\n\
        This patch contains translated text only. Get the game itself from the\n\
        original creator.\n";
    zip.start_file("README.txt", zip_opts)
        .map_err(|e| pack_err(format!("zip readme: {e}")))?;
    zip.write_all(readme.as_bytes())?;
    zip.finish()
        .map_err(|e| pack_err(format!("zip finish: {e}")))?;

    if let Err(e) = std::fs::rename(&tmp, &out) {
        return Err(e.into());
    }
    tmp_guard.disarm();

    let size = std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0);

    Ok(PackReport {
        output_path: out.display().to_string(),
        recording_lang: recording.lang.clone(),
        recorded_root: recording.root.display().to_string(),
        files_packed: added,
        translated_strings: translated,
        size_bytes: size,
        patch_id,
        patch_version,
        engine,
        language,
        tier: tier.into(),
        messages,
    })
}

/// Remove a temp file on drop unless disarmed after a successful rename.
struct TempFileGuard {
    path: Option<PathBuf>,
}

impl TempFileGuard {
    fn new(path: &Path) -> Self {
        Self {
            path: Some(path.to_path_buf()),
        }
    }
    fn disarm(&mut self) {
        self.path = None;
    }
}

impl Drop for TempFileGuard {
    fn drop(&mut self) {
        if let Some(p) = self.path.take() {
            let _ = std::fs::remove_file(p);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::StringEntry;
    use std::fs;
    use std::io::Read;

    fn tempdir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("locust_pack_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn temp_file_guard_removes_on_drop_and_survives_disarm() {
        let dir = tempdir();
        // Dropped while armed: the half-written temp must not be left behind.
        // This covers every `?` between creating the temp archive and renaming.
        let armed = dir.join("armed.tmp-1");
        fs::write(&armed, b"partial").unwrap();
        {
            let _g = TempFileGuard::new(&armed);
        }
        assert!(!armed.exists(), "armed guard must remove the temp on drop");

        // Disarmed: the file has become the real destination and must survive.
        let kept = dir.join("kept.tmp-1");
        fs::write(&kept, b"complete").unwrap();
        {
            let mut g = TempFileGuard::new(&kept);
            g.disarm();
        }
        assert!(kept.exists(), "disarmed guard must leave the destination alone");
    }

    #[test]
    fn pack_from_recording_writes_zip_with_bytes() {
        let base = tempdir();
        let game = base.join("game");
        let game_sub = game.join("game");
        fs::create_dir_all(&game_sub).unwrap();
        let script = game_sub.join("script.rpy");
        let contents = "label start:\n    \"Hola\"\n";
        fs::write(&script, contents).unwrap();

        let db = Database::open_in_memory().unwrap();
        let mut entry = StringEntry::new("script.rpy#2", "Hello", script.clone());
        entry.translation = Some("Hola".into());
        entry.status = StringStatus::Translated;
        db.save_entries(&[entry]).unwrap();
        db.record_injection(Some("es"), &game, &[script]).unwrap();

        let out = base.join("out-patch.zip");
        let report = pack_injection_recording(
            &db,
            PackOptions {
                game_path: game,
                lang: Some("es".into()),
                output: out.clone(),
                pristine: None,
                engine: Some("renpy".into()),
                project: base.join("project.locust.db"),
                require_pristine: false,
            },
        )
        .unwrap();

        assert_eq!(report.files_packed, 1);
        assert_eq!(report.tier, "structural");
        assert!(out.is_file());

        let file = fs::File::open(&out).unwrap();
        let mut archive = zip::ZipArchive::new(file).unwrap();
        {
            let mut zf = archive.by_name("game/script.rpy").unwrap();
            let mut read_back = String::new();
            zf.read_to_string(&mut read_back).unwrap();
            assert_eq!(read_back, contents);
        }
        assert!(archive.by_name(PatchManifest::FILENAME).is_ok());
    }

    #[test]
    fn pack_without_recording_errors() {
        let base = tempdir();
        let game = base.join("g");
        fs::create_dir_all(&game).unwrap();
        let db = Database::open_in_memory().unwrap();
        let mut entry = StringEntry::new("e", "Hi", game.join("f.txt"));
        entry.translation = Some("Hola".into());
        entry.status = StringStatus::Translated;
        db.save_entries(&[entry]).unwrap();

        let err = pack_injection_recording(
            &db,
            PackOptions {
                game_path: game,
                lang: None,
                output: base.join("x.zip"),
                pristine: None,
                engine: None,
                project: base.join("project.locust.db"),
                require_pristine: false,
            },
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("no injection") || err.contains("recorded"), "{err}");
    }

    #[test]
    fn pack_require_pristine_without_backup_errors() {
        let base = tempdir();
        let game = base.join("game");
        let game_sub = game.join("game");
        fs::create_dir_all(&game_sub).unwrap();
        let script = game_sub.join("a.rpy");
        fs::write(&script, "x").unwrap();
        let db = Database::open_in_memory().unwrap();
        let mut entry = StringEntry::new("a", "x", script.clone());
        entry.translation = Some("y".into());
        entry.status = StringStatus::Translated;
        db.save_entries(&[entry]).unwrap();
        db.record_injection(Some("es"), &game, &[script]).unwrap();

        let err = pack_injection_recording(
            &db,
            PackOptions {
                game_path: game,
                lang: Some("es".into()),
                output: base.join("p.zip"),
                pristine: None,
                engine: None,
                project: base.join("project.locust.db"),
                require_pristine: true,
            },
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("pristine"), "{err}");
    }
}
