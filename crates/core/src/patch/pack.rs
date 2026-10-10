//! Pack a Locust patch zip from a recorded injection (shared by CLI + HTTP).
//!
//! Packs **exclusively** from the injection recording (root + rel + hash per
//! language key). Same rules as the former CLI-only `cmd_patch` orchestration.

use std::io::Write;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::database::{paths_identical, sha256_file, Database, InjectionRecording};
use crate::error::{LocustError, Result};
use crate::patch::manifest::{FingerprintEntry, GameIdentity, PatchFileEntry, PatchManifest};
use crate::patch::store::PatchStore;
use crate::patch::stream::{stream_bounded, StreamError};

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
    /// Optional catalog metadata. Its fingerprint is filled from pristine files.
    pub game: Option<GameIdentity>,
}

/// Catalog identity actually embedded in the finished archive.
#[derive(Debug, Clone, Serialize)]
pub struct PackedGameIdentity {
    pub store_ids: std::collections::BTreeMap<String, String>,
    pub game_version: Option<String>,
    pub fingerprint_count: usize,
}

/// Summary returned after a successful pack (JSON-friendly for the HTTP API).
#[derive(Debug, Clone, Serialize)]
pub struct PackReport {
    pub game: Option<PackedGameIdentity>,
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

/// Hash a pristine original for the manifest.
///
/// `NotFound` is a legitimate added file (`None`). Any other I/O failure
/// (sharing, permission, directory/non-file) is an error with path context.
fn hash_original_file(root: &Path, rel: &Path) -> Result<Option<(String, u64)>> {
    let path = root.join(rel.components().collect::<PathBuf>());
    match sha256_file(&path) {
        Ok(original) => Ok(Some(original)),
        Err(LocustError::IoError(err)) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(pack_err(format!(
            "cannot hash original {}: {err}",
            path.display()
        ))),
    }
}

/// Appended to advice that names a `--direct` re-run. Every registered
/// format writes in place, so a legacy database on an already-injected
/// game would loop on the identical error forever without this note.
/// `None` means the caller does not know the engine — keep the old
/// silence rather than guess.
fn maybe_mutated_note(engine: Option<&str>) -> &'static str {
    if engine.is_some() {
        "\nThis engine writes translations into the ORIGINAL game files: if this \
         game was already injected (for example through an older Locust that kept \
         no recording), that command will report 0 files written and record \
         nothing — restore the original game files from a backup or a clean copy \
         first, then re-run it."
    } else {
        ""
    }
}

/// Resolve existing ancestors (including links) while allowing a new output leaf.
fn resolved_destination(path: &Path) -> Result<PathBuf> {
    let absolute = std::path::absolute(path)?;
    let mut resolved = PathBuf::new();
    let mut inaccessible_ancestor = None;
    for component in absolute.components() {
        match component {
            std::path::Component::ParentDir => {
                resolved.pop();
            }
            std::path::Component::CurDir => {}
            std::path::Component::Normal(name) => {
                resolved.push(name);
                match std::fs::canonicalize(&resolved) {
                    Ok(canonical) => {
                        resolved = canonical;
                        inaccessible_ancestor = None;
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    // A sandbox may deny an ancestor while allowing a more
                    // specific temp directory. Keep resolving until the
                    // accessible descendant confirms the full real path.
                    Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                        inaccessible_ancestor = Some(error);
                    }
                    Err(error) => return Err(error.into()),
                }
            }
            other => resolved.push(other.as_os_str()),
        }
    }
    if let Some(error) = inaccessible_ancestor {
        return Err(error.into());
    }
    Ok(resolved)
}

/// Refuse publishing an archive inside a game, pristine tree or backup store.
/// Must run before creating output directories or temporary files.
pub fn ensure_pack_output_outside(output: &Path, protected_root: &Path) -> Result<()> {
    let output = resolved_destination(output)?;
    let root = resolved_destination(protected_root)?;
    if output.starts_with(&root) {
        return Err(pack_err(format!(
            "patch output must be outside the game and backup trees: {}",
            root.display()
        )));
    }
    Ok(())
}

/// Pack a patch zip from the injection recording stored in `db`.
pub fn pack_injection_recording(db: &Database, opts: PackOptions) -> Result<PackReport> {
    pack_recording(db, opts, None)
}

#[cfg(test)]
type SelectionHook = Box<dyn FnOnce(&Database)>;

#[cfg(test)]
type PristineHook = Box<dyn FnOnce(&Path)>;

#[cfg(test)]
thread_local! {
    static AFTER_BACKUP_SELECTION: std::cell::RefCell<Option<SelectionHook>> = const {
        std::cell::RefCell::new(None)
    };
    static AFTER_PRISTINE_VALIDATION: std::cell::RefCell<Option<PristineHook>> = const {
        std::cell::RefCell::new(None)
    };
    static BEFORE_PAYLOAD_STREAM: std::cell::RefCell<Option<PristineHook>> = const {
        std::cell::RefCell::new(None)
    };
}

#[cfg(test)]
fn run_selection_hook(db: &Database) {
    let hook = AFTER_BACKUP_SELECTION.with(|slot| slot.borrow_mut().take());
    if let Some(hook) = hook {
        hook(db);
    }
}

/// Resolve the exact backup associated with the selected recording, including
/// its original store when the project moves between CLI and desktop profiles.
/// Explicit pristine trees take precedence. Missing recorded backups fail closed.
pub fn pack_with_pristine_backup(
    db: &Database,
    mut opts: PackOptions,
    fallback_store: &crate::backup::BackupManager,
    requested_backup: Option<&str>,
    use_recorded_backup: bool,
) -> Result<PackReport> {
    if requested_backup.is_some() && opts.pristine.is_some() {
        return Err(pack_err("choose either a backup ID or a pristine folder"));
    }
    let selection = opts.game_path.clone();
    if opts.game_path.is_file() {
        opts.game_path = std::path::absolute(&opts.game_path)?
            .parent()
            .ok_or_else(|| pack_err("game file has no parent"))?
            .to_path_buf();
    }
    // Select once: an absent recording must not become a new generation after
    // backup resolution, silently dropping the original hashes from its ZIP.
    let selected = select_pack_recording(db, &opts);
    #[cfg(test)]
    run_selection_hook(db);
    let (recording, translated) = selected?;
    let recorded = recording.pristine_backup.as_ref();
    if let (Some(requested), Some(recorded)) = (requested_backup, recorded) {
        if requested != recorded.id {
            return Err(pack_err("selected backup does not match the recorded injection; reopen Pack to use its original backup"));
        }
    }
    let stored_root = recorded.and_then(|backup| backup.storage_root.as_ref());
    if stored_root.is_some_and(|root| !root.is_absolute()) {
        return Err(pack_err("recorded backup store must be absolute"));
    }
    ensure_pack_output_outside(&opts.output, fallback_store.root())?;
    if let Some(root) = stored_root {
        ensure_pack_output_outside(&opts.output, root)?;
    }
    let chosen = requested_backup.or_else(|| {
        if use_recorded_backup && opts.pristine.is_none() {
            recorded.map(|backup| backup.id.as_str())
        } else {
            None
        }
    });
    let origin = recorded
        .map(|backup| backup.source_path.as_path())
        .unwrap_or(&selection);
    let original_store = stored_root.map(|root| crate::backup::BackupManager::new(root.clone()));
    let store = original_store.as_ref().unwrap_or(fallback_store);
    if let Some(id) = chosen {
        store.with_verified_pristine_tree(id, origin, |tree| {
            #[cfg(test)]
            {
                let hook = AFTER_PRISTINE_VALIDATION.with(|slot| slot.borrow_mut().take());
                if let Some(hook) = hook {
                    hook(tree.root());
                }
            }
            opts.pristine = Some(tree.root().to_path_buf());
            pack_selected_recording(db, opts, &recording, translated, Some(&tree))
        })
    } else {
        pack_selected_recording(db, opts, &recording, translated, None)
    }
}

/// Pack only the generation used to select its original backup. A concurrent
/// injection must not pair new output files with a previous generation's backup.
pub fn pack_recorded_generation(
    db: &Database,
    opts: PackOptions,
    expected: &crate::database::InjectionRecording,
) -> Result<PackReport> {
    pack_recording(db, opts, Some(expected))
}

fn pack_recording(
    db: &Database,
    opts: PackOptions,
    expected: Option<&crate::database::InjectionRecording>,
) -> Result<PackReport> {
    let (recording, translated) = select_pack_recording(db, &opts)?;
    if expected.is_some_and(|expected| expected != &recording) {
        return Err(pack_err(
            "injection recording changed while selecting its backup; reopen Pack and retry",
        ));
    }
    pack_selected_recording(db, opts, &recording, translated, None)
}

/// Apply the same language/root selection and diagnostics to every entry point.
/// The caller must retain this concrete generation through backup resolution.
fn select_pack_recording(db: &Database, opts: &PackOptions) -> Result<(InjectionRecording, usize)> {
    let game_path = &opts.game_path;
    let project = &opts.project;
    let engine_id = &opts.engine;
    let lang = &opts.lang;

    // Friendly pre-check: no translations → nothing to pack.
    let translated = db.count_translated_entries()?;
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
                    if !paths_identical(game_path, &rec.root) {
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
                        if example.is_none() && paths_identical(game_path, &rec.root) {
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

    if !paths_identical(game_path, &recording.root) {
        return Err(pack_err(format!(
            "the recorded injection for {} wrote into \"{}\", not \"{}\". \
             Packing from a different tree is refused. Point game_path at the recorded root.",
            key_label(&recording.lang),
            recording.root.display(),
            game_path.display()
        )));
    }

    Ok((recording, translated))
}

fn load_registration_originals(
    db: &Database,
    recording: &InjectionRecording,
    output: &Path,
) -> Result<std::collections::HashMap<String, (String, u64)>> {
    // Registration may follow Add after an author edits a menu/map. Its exact
    // originals live in regular verified backups, separate from Add's baseline.
    let mut registration_groups = std::collections::BTreeMap::new();
    for (rel, backup) in db.registration_backups(recording.lang.as_deref())? {
        let key = serde_json::to_string(&backup)?;
        registration_groups
            .entry(key)
            .or_insert_with(|| (backup, Vec::new()))
            .1
            .push(rel);
    }
    let mut registration_originals = std::collections::HashMap::new();
    for (_, (backup, rels)) in registration_groups {
        if !paths_identical(&backup.source_path, &recording.root) {
            return Err(pack_err("registration backup belongs to a different game"));
        }
        let storage = backup
            .storage_root
            .as_ref()
            .filter(|root| root.is_absolute())
            .ok_or_else(|| pack_err("registration backup store must be absolute"))?;
        ensure_pack_output_outside(output, storage)?;
        let manager = crate::backup::BackupManager::new(storage.clone());
        manager.with_verified_pristine_tree(&backup.id, &backup.source_path, |tree| {
            for rel in rels {
                let path = super::zipsec::safe_stored_rel(&rel)?;
                let original = hash_original_file(tree.root(), &path)?
                    .ok_or_else(|| pack_err(format!("registration original is missing: {rel}")))?;
                if tree.original_sha256(&path)? != Some(original.0.as_str()) {
                    return Err(pack_err("registration backup changed after validation"));
                }
                registration_originals.insert(rel, original);
            }
            Ok(())
        })?;
    }

    Ok(registration_originals)
}

fn pack_selected_recording(
    db: &Database,
    opts: PackOptions,
    recording: &InjectionRecording,
    translated: usize,
    verified_pristine: Option<&crate::backup::VerifiedPristine<'_>>,
) -> Result<PackReport> {
    let game_path = opts.game_path;
    let lang = opts.lang;
    let mut messages = Vec::new();

    // A recorded generation must remain stable through hashing, streaming and
    // ZIP publication while another process injects, applies or restores files.
    let _game_lock = super::GameLock::acquire(&recording.root)?;
    crate::injection_transaction::ensure_no_pending_under_lock(&_game_lock)?;
    if db.get_injection(recording.lang.as_deref())?.as_ref() != Some(recording) {
        return Err(pack_err(
            "injection recording changed while selecting its backup; reopen Pack and retry",
        ));
    }

    let registration_originals = load_registration_originals(db, recording, &opts.output)?;

    let out = opts.output;
    ensure_pack_output_outside(&out, &recording.root)?;
    if let Some(pristine) = &opts.pristine {
        ensure_pack_output_outside(&out, pristine)?;
    }
    let database = db.path();
    if database != Path::new(":memory:") {
        let resolved_output = resolved_destination(&out)?;
        for suffix in ["", "-wal", "-shm", "-journal"] {
            let mut protected = database.as_os_str().to_os_string();
            protected.push(suffix);
            if resolved_output == resolved_destination(Path::new(&protected))? {
                return Err(pack_err(
                    "patch output would overwrite the project database",
                ));
            }
        }
    }
    if let Some(parent) = out.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }

    // The scratch name must be unique per pack, not per process: the server
    // packs on a blocking task, so two requests for the same `output_path`
    // share a PID. `create_new` also refuses to truncate a name already in
    // use, so a loser fails instead of writing into the winner's zip.
    let tmp = out.with_file_name(format!(
        "{}.tmp-{}",
        out.file_name().unwrap_or_default().to_string_lossy(),
        uuid::Uuid::new_v4()
    ));
    let zip_file = std::fs::File::create_new(&tmp)?;
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

    if opts.require_pristine && pristine_root.is_none() && registration_originals.is_empty() {
        return Err(pack_err(
            "pristine hashes required but no --pristine path and no valid .locust/backup found",
        ));
    }
    if pristine_root.is_none() && registration_originals.is_empty() {
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
    let mut fingerprint = Vec::new();

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
        // Open once, then hash the exact 1 MiB chunks written to ZIP. A separate
        // hash/read pass could certify different bytes after an external edit.
        let mut input = match std::fs::File::open(&src) {
            Ok(file) => file,
            Err(_) => {
                missing.push(src);
                continue;
            }
        };
        let size = match input.metadata() {
            Ok(metadata) if metadata.is_file() => metadata.len(),
            Ok(_) | Err(_) => {
                missing.push(src);
                continue;
            }
        };
        if size != f.size {
            changed.push(f.rel.clone());
            continue;
        }
        let original = match (registration_originals.get(&f.rel), &pristine_root) {
            (Some(original), _) => Some(original.clone()),
            (None, Some(root)) => hash_original_file(root, rel)?,
            (None, None) => None,
        };
        let original_sha256 = original.as_ref().map(|(hash, _)| hash.clone());
        if let Some(pristine) = verified_pristine {
            if !registration_originals.contains_key(&f.rel)
                && original_sha256.as_deref() != pristine.original_sha256(rel)?
            {
                return Err(pack_err(format!(
                    "injection backup changed after validation at {}; packing refused",
                    rel.display()
                )));
            }
        }
        // ZIP64 for entries at/over 4 GiB (multi-GB Unreal base paks).
        let entry_opts = zip_opts.large_file(size >= 0xFFFF_FFFF);
        zip.start_file(f.rel.clone(), entry_opts)
            .map_err(|e| pack_err(format!("zip start_file {}: {e}", f.rel)))?;
        #[cfg(test)]
        {
            let hook = BEFORE_PAYLOAD_STREAM.with(|slot| slot.borrow_mut().take());
            if let Some(hook) = hook {
                hook(&src);
            }
        }
        match stream_bounded(&mut input, f.size, Some(&mut zip)) {
            Ok(streamed) if streamed.actual_len == f.size && streamed.sha256_hex == f.hash => {}
            Ok(_) | Err(StreamError::TooLong) => {
                changed.push(f.rel.clone());
                continue;
            }
            Err(StreamError::Read(_)) => {
                missing.push(src);
                continue;
            }
            Err(StreamError::Write(error)) => {
                return Err(pack_err(format!("zip write {}: {error}", f.rel)));
            }
        }
        if let Some((sha256, size)) = original {
            fingerprint.push(FingerprintEntry {
                path: f.rel.clone(),
                size,
                sha256,
            });
        }
        manifest_files.push(PatchFileEntry {
            path: f.rel.clone(),
            patched_sha256: f.hash.clone(),
            size: f.size,
            original_sha256,
        });
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
             Keep any external edits separately. Restore the last recorded injected files \
             before packing; Direct reinjection also refuses a changed recording. \
             To start over, use a separate project and a pristine game copy.",
            missing.len() + changed.len(),
            recording.files.len(),
        )));
    }

    let engine = opts.engine.unwrap_or_else(|| "unknown".into());
    let language = lang
        .clone()
        .or(recording.lang.clone())
        .unwrap_or_else(|| "unknown".into());
    let patch_id = uuid::Uuid::new_v4().to_string();
    let patch_version = "1.0.0".to_string();

    fingerprint.sort_by(|a, b| a.size.cmp(&b.size).then_with(|| a.path.cmp(&b.path)));
    fingerprint.truncate(3);
    let game = opts
        .game
        .map(|mut game| {
            game.fingerprint = fingerprint;
            game
        })
        .filter(|game| *game != GameIdentity::default());

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
        game,
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
        This patch contains modified game files, which may include complete\n\
        asset bundles. Get the base game from the original creator.\n";
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
        game: patch_manifest.game.map(|game| PackedGameIdentity {
            store_ids: game.store_ids,
            game_version: game.game_version,
            fingerprint_count: game.fingerprint.len(),
        }),
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
    #[test]
    fn registration_originals_use_verified_pre_registration_bytes() {
        use crate::database::{sha256_hex, Database, RecordedBackup};
        let temp = tempfile::tempdir().unwrap();
        let game = temp.path().join("game");
        std::fs::create_dir(&game).unwrap();
        let pack = game.join("lang_es.json");
        let menu = game.join("plugins.js");
        std::fs::write(&pack, b"Spanish pack").unwrap();
        std::fs::write(&menu, b"author menu before registration").unwrap();
        let manager = crate::backup::BackupManager::new(temp.path().join("backups"));
        let backup = manager.create_backup(&game).unwrap();
        let provenance = RecordedBackup {
            id: backup.id,
            source_path: backup.source_path,
            storage_root: Some(manager.root().to_owned()),
        };
        let db = Database::open_in_memory().unwrap();
        db.record_injection(Some("es"), &game, &[pack]).unwrap();
        let before = db.get_injection(Some("es")).unwrap().unwrap();
        std::fs::write(&menu, b"Spanish registered menu").unwrap();
        db.extend_injection_with_registration_backup(&before, &[menu], &provenance)
            .unwrap();
        let after = db.get_injection(Some("es")).unwrap().unwrap();
        let output = temp.path().join("patch.zip");
        let originals = super::load_registration_originals(&db, &after, &output).unwrap();
        assert_eq!(originals.len(), 1, "the Add pack must remain an added file");
        assert_eq!(
            originals["plugins.js"],
            (
                sha256_hex(b"author menu before registration"),
                b"author menu before registration".len() as u64
            )
        );
        std::fs::write(backup.path.join("payload/plugins.js"), b"corrupted backup").unwrap();
        assert!(super::load_registration_originals(&db, &after, &output)
            .unwrap_err()
            .to_string()
            .contains("mismatch"));
    }
    use super::*;
    use crate::models::{StringEntry, StringStatus};
    use std::fs;
    use std::io::Read;

    fn tempdir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("locust_pack_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn pack_counts_translations_without_materializing_entries() {
        let fixture = tempfile::tempdir().unwrap();
        let game = fixture.path().join("game");
        fs::create_dir(&game).unwrap();
        let script = game.join("story.txt");
        fs::write(&script, "Hola").unwrap();
        let db = Database::open_in_memory().unwrap();
        let mut entries = Vec::new();
        for status in [
            StringStatus::Pending,
            StringStatus::Translated,
            StringStatus::Reviewed,
            StringStatus::Approved,
            StringStatus::Error,
        ] {
            for translation in [
                None,
                Some(""),
                Some(" \t\r\n"),
                Some("\u{3000}"),
                Some(" \t\u{3000}\n"),
                Some("Hola"),
                Some(" \u{3000}Hola\t"),
            ] {
                let mut entry =
                    StringEntry::new(format!("line-{}", entries.len()), "Hello", script.clone());
                entry.status = status.clone();
                entry.translation = translation.map(str::to_owned);
                entries.push(entry);
            }
        }
        db.save_entries(&entries).unwrap();
        db.record_injection(Some("es"), &game, &[script]).unwrap();
        let options = PackOptions {
            game: None,
            game_path: game,
            lang: Some("es".into()),
            output: fixture.path().join("patch.zip"),
            pristine: None,
            engine: None,
            project: fixture.path().join("project.db"),
            require_pristine: false,
        };
        let expected = db
            .get_entries(&crate::database::EntryFilter::default())
            .unwrap()
            .iter()
            .filter(|entry| {
                entry
                    .translation
                    .as_deref()
                    .is_some_and(|t| !t.trim().is_empty())
                    && matches!(
                        entry.status,
                        StringStatus::Translated | StringStatus::Reviewed | StringStatus::Approved
                    )
            })
            .count();
        assert_eq!(expected, 6);

        crate::database::ENTRY_ROWS_MATERIALIZED.with(|count| count.set(Some(0)));
        let result = select_pack_recording(&db, &options);
        let materialized =
            crate::database::ENTRY_ROWS_MATERIALIZED.with(|count| count.replace(None));
        assert_eq!(result.unwrap().1, expected);
        assert_eq!(materialized, Some(0));
    }

    fn live_payload_changed_before_stream(change: &str) {
        let base = tempfile::tempdir().unwrap();
        let game = base.path().join("game");
        let pristine = base.path().join("pristine");
        fs::create_dir(&game).unwrap();
        fs::create_dir(&pristine).unwrap();
        let source = game.join("story.bin");
        fs::write(pristine.join("story.bin"), "Original Japanese").unwrap();
        let recorded: Vec<u8> = (0..(2 * 1024 * 1024 + 37))
            .map(|i| (i % 251) as u8)
            .collect();
        fs::write(&source, &recorded).unwrap();
        let db = Database::open_in_memory().unwrap();
        let mut entry = StringEntry::new("line", "Original Japanese", source.clone());
        entry.translation = Some("Texto traducido".into());
        entry.status = StringStatus::Translated;
        db.save_entries(&[entry]).unwrap();
        db.record_injection(Some("es"), &game, std::slice::from_ref(&source))
            .unwrap();
        let saved_recording = db.get_injection(Some("es")).unwrap();
        let output = base.path().join("existing.zip");
        fs::write(&output, "existing archive").unwrap();
        let options = PackOptions {
            game: None,
            game_path: game.clone(),
            lang: Some("es".into()),
            output: output.clone(),
            pristine: Some(pristine.clone()),
            engine: None,
            project: base.path().join("project.db"),
            require_pristine: true,
        };
        let mut edited = recorded.clone();
        match change {
            "same-length" => edited[1024 * 1024 + 3] ^= 0xff,
            "longer" => edited.push(42),
            "shorter" => edited.truncate(1024 * 1024 + 9),
            _ => unreachable!(),
        }
        let new_bytes = edited.clone();
        BEFORE_PAYLOAD_STREAM.with(|slot| {
            *slot.borrow_mut() = Some(Box::new(move |path| fs::write(path, new_bytes).unwrap()));
        });
        let result = pack_injection_recording(&db, options.clone());
        assert!(BEFORE_PAYLOAD_STREAM.with(|slot| slot.borrow().is_none()));
        let error = result
            .expect_err("must not publish bytes different from recording")
            .to_string();
        assert!(error.contains("no longer match"), "{error}");
        assert_eq!(fs::read_to_string(&output).unwrap(), "existing archive");
        assert_eq!(fs::read(&source).unwrap(), edited);
        assert_eq!(db.get_injection(Some("es")).unwrap(), saved_recording);
        assert!(!fs::read_dir(base.path()).unwrap().any(|p| p
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("existing.zip.tmp-")));

        // Once the recorded bytes are restored, every byte in the produced
        // ZIP must verify, apply and roll back against the actual original.
        fs::write(&source, &recorded).unwrap();
        pack_injection_recording(&db, options).unwrap();
        let verified = crate::patch::verify(&pristine, &output).unwrap();
        assert_eq!(verified.outcome, crate::patch::VerificationOutcome::Clean);
        crate::patch::apply(&pristine, &output, Default::default(), |_| {}).unwrap();
        assert_eq!(fs::read(pristine.join("story.bin")).unwrap(), recorded);
        crate::patch::rollback(&pristine, Default::default()).unwrap();
        assert_eq!(
            fs::read_to_string(pristine.join("story.bin")).unwrap(),
            "Original Japanese"
        );
    }

    #[test]
    fn pack_refuses_live_payload_rewritten_before_stream() {
        live_payload_changed_before_stream("same-length");
    }

    #[test]
    fn pack_refuses_live_payload_grown_before_stream() {
        live_payload_changed_before_stream("longer");
    }

    #[test]
    fn pack_refuses_live_payload_shrunk_before_stream() {
        live_payload_changed_before_stream("shorter");
    }

    fn pristine_changed_after_validation(change: &str) {
        use crate::backup::BackupManager;
        use crate::database::RecordedBackup;

        let base = tempfile::tempdir().unwrap();
        let game = base.path().join("game");
        fs::create_dir(&game).unwrap();
        let source = game.join("story.txt");
        let added = game.join("added.txt");
        fs::write(&source, "Original Japanese").unwrap();
        let store = BackupManager::new(base.path().join("backups"));
        let backup = store.create_backup(&game).unwrap();
        let provenance = RecordedBackup {
            id: backup.id.clone(),
            source_path: backup.source_path.clone(),
            storage_root: Some(store.root().to_path_buf()),
        };
        fs::write(&source, "Texto traducido").unwrap();
        fs::write(&added, "Generated translation overlay").unwrap();
        let db = Database::open_in_memory().unwrap();
        let mut entry = StringEntry::new("line", "Original Japanese", source.clone());
        entry.translation = Some("Texto traducido".into());
        entry.status = StringStatus::Translated;
        db.save_entries(&[entry]).unwrap();
        db.record_injection_with_backup(
            Some("es"),
            &game,
            &[source.clone(), added.clone()],
            Some(&provenance),
        )
        .unwrap();
        let recording = db.get_injection(Some("es")).unwrap();
        let output = base.path().join("existing.zip");
        fs::write(&output, "existing archive").unwrap();
        let options = PackOptions {
            game: None,
            game_path: game.clone(),
            lang: Some("es".into()),
            output: output.clone(),
            pristine: None,
            engine: None,
            project: base.path().join("project.db"),
            require_pristine: false,
        };
        let change = change.to_owned();
        AFTER_PRISTINE_VALIDATION.with(|slot| {
            *slot.borrow_mut() = Some(Box::new(move |tree| match change.as_str() {
                "changed" => fs::write(tree.join("story.txt"), "Changed original").unwrap(),
                "missing" => fs::remove_file(tree.join("story.txt")).unwrap(),
                "added" => fs::write(tree.join("added.txt"), "Not in original backup").unwrap(),
                _ => unreachable!(),
            }));
        });
        let result = pack_with_pristine_backup(&db, options.clone(), &store, None, true);
        assert!(AFTER_PRISTINE_VALIDATION.with(|slot| slot.borrow().is_none()));
        let error = result
            .expect_err("changed backup must not replace original hashes")
            .to_string();
        assert!(error.contains("backup changed"), "{error}");
        assert_eq!(fs::read_to_string(&output).unwrap(), "existing archive");
        assert_eq!(fs::read_to_string(&source).unwrap(), "Texto traducido");
        assert_eq!(
            fs::read_to_string(&added).unwrap(),
            "Generated translation overlay"
        );
        assert_eq!(db.get_injection(Some("es")).unwrap(), recording);
        assert!(!fs::read_dir(base.path()).unwrap().any(|p| p
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("existing.zip.tmp-")));

        // Restore only this test's intentionally changed backup and prove retry.
        let payload = backup.path.join("payload");
        fs::write(payload.join("story.txt"), "Original Japanese").unwrap();
        if payload.join("added.txt").exists() {
            fs::remove_file(payload.join("added.txt")).unwrap();
        }
        let report = pack_with_pristine_backup(&db, options, &store, None, true).unwrap();
        assert_eq!(report.tier, "strict");
        let original = base.path().join("original");
        fs::create_dir(&original).unwrap();
        fs::write(original.join("story.txt"), "Original Japanese").unwrap();
        let verified = crate::patch::verify(&original, &output).unwrap();
        assert_eq!(verified.outcome, crate::patch::VerificationOutcome::Clean);
    }

    #[test]
    fn pack_refuses_backup_original_changed_after_validation() {
        pristine_changed_after_validation("changed");
    }

    #[test]
    fn pack_refuses_backup_original_missing_after_validation() {
        pristine_changed_after_validation("missing");
    }

    #[test]
    fn pack_refuses_backup_counterpart_added_after_validation() {
        pristine_changed_after_validation("added");
    }

    fn recording_changed_during_backup_selection(lang: Option<&str>, initially_recorded: bool) {
        use crate::backup::BackupManager;
        use crate::database::RecordedBackup;

        let base = tempfile::tempdir().unwrap();
        let game = base.path().join("game");
        fs::create_dir(&game).unwrap();
        let source = game.join("story.txt");
        fs::write(&source, "Original Japanese").unwrap();
        let store = BackupManager::new(base.path().join("backups"));
        let backup = store.create_backup(&game).unwrap();
        let provenance = RecordedBackup {
            id: backup.id,
            source_path: backup.source_path,
            storage_root: Some(store.root().to_path_buf()),
        };
        fs::write(&source, "Texto traducido").unwrap();
        let db = Database::open_in_memory().unwrap();
        let mut entry = StringEntry::new("line", "Original Japanese", source.clone());
        entry.translation = Some("Texto traducido".into());
        entry.status = StringStatus::Translated;
        db.save_entries(&[entry]).unwrap();
        if initially_recorded {
            db.record_injection_with_backup(
                lang,
                &game,
                std::slice::from_ref(&source),
                Some(&provenance),
            )
            .unwrap();
        }
        let output = base.path().join("existing.zip");
        fs::write(&output, "existing archive").unwrap();
        let options = PackOptions {
            game: None,
            game_path: game.clone(),
            lang: lang.map(str::to_owned),
            output: output.clone(),
            pristine: None,
            engine: None,
            project: base.path().join("project.db"),
            require_pristine: false,
        };
        let record_game = game.clone();
        let record_source = source.clone();
        let record_lang = lang.map(str::to_owned);
        AFTER_BACKUP_SELECTION.with(|slot| {
            *slot.borrow_mut() = Some(Box::new(move |db| {
                fs::write(&record_source, "Nueva traduccion").unwrap();
                db.record_injection_with_backup(
                    record_lang.as_deref(),
                    &record_game,
                    &[record_source],
                    Some(&provenance),
                )
                .unwrap();
            }));
        });
        let result = pack_with_pristine_backup(&db, options.clone(), &store, None, true);
        assert!(AFTER_BACKUP_SELECTION.with(|slot| slot.borrow().is_none()));
        assert!(db.get_injection(lang).unwrap().is_some());
        let error = result
            .expect_err("must refuse the stale snapshot")
            .to_string();
        assert!(
            error.contains(if initially_recorded {
                "recording changed"
            } else {
                "no injection has been recorded"
            }),
            "{error}"
        );
        assert_eq!(fs::read_to_string(&output).unwrap(), "existing archive");
        assert_eq!(fs::read_to_string(&source).unwrap(), "Nueva traduccion");

        // A fresh attempt must deliberately select the new generation and its
        // original backup, never silently downgrade it to structural packing.
        let report = pack_with_pristine_backup(&db, options, &store, None, true).unwrap();
        assert_eq!(report.tier, "strict");
        let original = base.path().join("original");
        fs::create_dir(&original).unwrap();
        fs::write(original.join("story.txt"), "Original Japanese").unwrap();
        let verified = crate::patch::verify(&original, &output).unwrap();
        assert_eq!(verified.outcome, crate::patch::VerificationOutcome::Clean);
        assert_eq!(
            verified.manifest.unwrap().files[0].original_sha256,
            Some(crate::database::sha256_hex(b"Original Japanese"))
        );
    }

    #[test]
    fn pack_refuses_first_named_recording_appearing_during_backup_selection() {
        recording_changed_during_backup_selection(Some("es"), false);
    }

    #[test]
    fn pack_refuses_first_unspecified_recording_appearing_during_backup_selection() {
        recording_changed_during_backup_selection(None, false);
    }

    #[test]
    fn pack_refuses_named_recording_replaced_during_backup_selection() {
        recording_changed_during_backup_selection(Some("es"), true);
    }

    #[test]
    fn pack_refuses_unspecified_recording_replaced_during_backup_selection() {
        recording_changed_during_backup_selection(None, true);
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
        assert!(
            kept.exists(),
            "disarmed guard must leave the destination alone"
        );
    }

    #[test]
    fn pack_refuses_database_and_sidecar_destinations() {
        let base = tempfile::tempdir().unwrap();
        let game = base.path().join("game");
        fs::create_dir(&game).unwrap();
        let script = game.join("script.rpy");
        fs::write(&script, "Hola").unwrap();
        let project = base.path().join("project.db");
        let db = Database::open(&project).unwrap();
        let mut entry = StringEntry::new("line", "Hello", script.clone());
        entry.translation = Some("Hola".into());
        entry.status = StringStatus::Translated;
        db.save_entries(&[entry]).unwrap();
        db.record_injection(Some("es"), &game, &[script]).unwrap();
        for suffix in ["", "-wal", "-shm", "-journal"] {
            let output = base.path().join(format!("project.db{suffix}"));
            let before = fs::read(&output).ok();
            let result = pack_injection_recording(
                &db,
                PackOptions {
                    game: None,
                    game_path: game.clone(),
                    lang: Some("es".into()),
                    output: output.clone(),
                    pristine: None,
                    engine: Some("renpy".into()),
                    project: base.path().join("wrong-label.db"),
                    require_pristine: false,
                },
            );
            assert!(result.unwrap_err().to_string().contains("project database"));
            assert_eq!(fs::read(&output).ok(), before);
        }
        assert_eq!(db.get_entries(&Default::default()).unwrap().len(), 1);
    }

    #[test]
    fn pack_refuses_output_in_game_or_pristine_before_writing() {
        for protected in ["game", "pristine"] {
            let base = tempfile::tempdir().unwrap();
            let game = base.path().join("game");
            let pristine = base.path().join("pristine");
            fs::create_dir(&game).unwrap();
            fs::create_dir(&pristine).unwrap();
            let script = game.join("script.rpy");
            fs::write(&script, "Hola").unwrap();
            fs::write(pristine.join("script.rpy"), "Hello").unwrap();
            let db = Database::open_in_memory().unwrap();
            let mut entry = StringEntry::new("line", "Hello", script.clone());
            entry.translation = Some("Hola".into());
            entry.status = StringStatus::Translated;
            db.save_entries(&[entry]).unwrap();
            db.record_injection(Some("es"), &game, std::slice::from_ref(&script))
                .unwrap();
            for suffix in ["script.rpy", "new/nested/patch.zip", "absent/../script.rpy"] {
                let output = base.path().join(protected).join(suffix);
                let result = pack_injection_recording(
                    &db,
                    PackOptions {
                        game: None,
                        game_path: game.clone(),
                        lang: Some("es".into()),
                        output,
                        pristine: Some(pristine.clone()),
                        engine: Some("renpy".into()),
                        project: base.path().join("project.db"),
                        require_pristine: true,
                    },
                );
                assert!(
                    result.is_err(),
                    "must refuse output inside {protected}: {suffix}"
                );
                assert_eq!(fs::read_to_string(&script).unwrap(), "Hola");
                assert_eq!(
                    fs::read_to_string(pristine.join("script.rpy")).unwrap(),
                    "Hello"
                );
                assert!(!base.path().join(protected).join("new").exists());
                assert!(!base.path().join(protected).join("absent").exists());
            }
        }
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
                game: None,
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
    fn pack_refuses_busy_game_before_creating_output() {
        let base = tempfile::tempdir().unwrap();
        let game = base.path().join("game");
        fs::create_dir(&game).unwrap();
        let script = game.join("script.rpy");
        fs::write(&script, "Hola").unwrap();
        let db = Database::open_in_memory().unwrap();
        let mut entry = StringEntry::new("line", "Hello", script.clone());
        entry.translation = Some("Hola".into());
        entry.status = StringStatus::Translated;
        db.save_entries(&[entry]).unwrap();
        db.record_injection(Some("es"), &game, &[script]).unwrap();
        let out = base.path().join("new/output.zip");
        let options = PackOptions {
            game: None,
            game_path: game.clone(),
            lang: Some("es".into()),
            output: out.clone(),
            pristine: None,
            engine: Some("renpy".into()),
            project: base.path().join("project.db"),
            require_pristine: false,
        };
        let guard = super::super::GameLock::acquire(&game).unwrap();
        assert!(pack_injection_recording(&db, options.clone())
            .unwrap_err()
            .to_string()
            .contains("game busy"));
        assert!(!out.parent().unwrap().exists());
        drop(guard);
        assert!(pack_injection_recording(&db, options).is_ok());
        assert!(out.exists());
    }

    #[test]
    fn concurrent_packs_to_one_output_do_not_share_a_scratch_file() {
        // The scratch name used to be PID-derived, so two packs inside one
        // process (the server packs on a blocking task) targeted the same file
        // and clobbered each other's zip mid-write.
        let base = tempdir();
        let out = base.join("shared-out.zip");

        let handles: Vec<_> = (0..2)
            .map(|i| {
                let base = base.clone();
                let out = out.clone();
                std::thread::spawn(move || {
                    let game = base.join(format!("game{i}"));
                    let game_sub = game.join("game");
                    fs::create_dir_all(&game_sub).unwrap();
                    let script = game_sub.join("script.rpy");
                    fs::write(&script, format!("label start:\n    \"Hola {i}\"\n")).unwrap();

                    let db = Database::open_in_memory().unwrap();
                    let mut entry = StringEntry::new("script.rpy#2", "Hello", script.clone());
                    entry.translation = Some("Hola".into());
                    entry.status = StringStatus::Translated;
                    db.save_entries(&[entry]).unwrap();
                    db.record_injection(Some("es"), &game, &[script]).unwrap();

                    pack_injection_recording(
                        &db,
                        PackOptions {
                            game: None,
                            game_path: game,
                            lang: Some("es".into()),
                            output: out,
                            pristine: None,
                            engine: Some("renpy".into()),
                            project: base.join(format!("project{i}.locust.db")),
                            require_pristine: false,
                        },
                    )
                    .map(|r| r.files_packed)
                })
            })
            .collect();

        let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        assert!(
            results.iter().any(|r| r.is_ok()),
            "at least one pack must succeed: {results:?}"
        );

        // Whoever won, the zip on disk must be a complete archive, not a
        // half-written file two writers took turns on.
        let file = fs::File::open(&out).unwrap();
        let mut archive = zip::ZipArchive::new(file).expect("output must be a valid zip");
        assert!(archive.by_name(PatchManifest::FILENAME).is_ok());

        let leftovers: Vec<_> = fs::read_dir(&base)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains(".tmp-"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "scratch files left behind: {leftovers:?}"
        );
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
                game: None,
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
        assert!(
            err.contains("no injection") || err.contains("recorded"),
            "{err}"
        );
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
                game: None,
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

    fn pack_opts(
        game: PathBuf,
        output: PathBuf,
        pristine: PathBuf,
        project: PathBuf,
    ) -> PackOptions {
        PackOptions {
            game: None,
            game_path: game,
            lang: Some("es".into()),
            output,
            pristine: Some(pristine),
            engine: Some("renpy".into()),
            project,
            require_pristine: true,
        }
    }

    fn read_manifest(zip_path: &Path) -> PatchManifest {
        let file = fs::File::open(zip_path).unwrap();
        let mut archive = zip::ZipArchive::new(file).unwrap();
        let mut zf = archive.by_name(PatchManifest::FILENAME).unwrap();
        let mut json = String::new();
        zf.read_to_string(&mut json).unwrap();
        serde_json::from_str(&json).unwrap()
    }

    #[test]
    fn pack_pristine_hashes_present_originals_and_nulls_additions() {
        let base = tempfile::tempdir().unwrap();
        let game = base.path().join("game");
        let pristine = base.path().join("pristine");
        fs::create_dir(&game).unwrap();
        fs::create_dir(&pristine).unwrap();
        let replaced = game.join("script.rpy");
        let added = game.join("extra.txt");
        fs::write(&replaced, "Hola").unwrap();
        fs::write(&added, "nuevo").unwrap();
        fs::write(pristine.join("script.rpy"), "Hello").unwrap();
        let expected_original = sha256_file(&pristine.join("script.rpy")).unwrap().0;
        let db = Database::open_in_memory().unwrap();
        for (id, source, dest, translation) in [
            ("line", "Hello", replaced.clone(), "Hola"),
            ("extra", "nuevo", added.clone(), "nuevo"),
        ] {
            let mut entry = StringEntry::new(id, source, dest);
            entry.translation = Some(translation.into());
            entry.status = StringStatus::Translated;
            db.save_entries(&[entry]).unwrap();
        }
        db.record_injection(Some("es"), &game, &[replaced, added])
            .unwrap();

        let output = base.path().join("out.zip");
        let report = pack_injection_recording(
            &db,
            pack_opts(
                game,
                output.clone(),
                pristine,
                base.path().join("project.db"),
            ),
        )
        .unwrap();
        assert_eq!(report.files_packed, 2);
        assert_eq!(report.tier, "strict");

        let manifest = read_manifest(&output);
        let replaced_entry = manifest
            .files
            .iter()
            .find(|f| f.path == "script.rpy")
            .unwrap();
        let added_entry = manifest
            .files
            .iter()
            .find(|f| f.path == "extra.txt")
            .unwrap();
        assert_eq!(
            replaced_entry.original_sha256.as_deref(),
            Some(expected_original.as_str())
        );
        assert_eq!(added_entry.original_sha256, None);
    }

    #[test]
    fn pack_non_file_original_fails_and_keeps_existing_zip() {
        let base = tempfile::tempdir().unwrap();
        let game = base.path().join("game");
        let pristine = base.path().join("pristine");
        fs::create_dir(&game).unwrap();
        fs::create_dir(&pristine).unwrap();
        let script = game.join("script.rpy");
        fs::write(&script, "Hola").unwrap();
        fs::create_dir(pristine.join("script.rpy")).unwrap();
        let db = Database::open_in_memory().unwrap();
        let mut entry = StringEntry::new("line", "Hello", script.clone());
        entry.translation = Some("Hola".into());
        entry.status = StringStatus::Translated;
        db.save_entries(&[entry]).unwrap();
        db.record_injection(Some("es"), &game, std::slice::from_ref(&script))
            .unwrap();

        let output = base.path().join("out.zip");
        let sentinel = b"existing-patch-sentinel";
        fs::write(&output, sentinel).unwrap();

        let err = pack_injection_recording(
            &db,
            pack_opts(
                game,
                output.clone(),
                pristine,
                base.path().join("project.db"),
            ),
        )
        .unwrap_err()
        .to_string();
        assert!(
            err.contains("script.rpy") && err.contains("cannot hash original"),
            "{err}"
        );
        assert_eq!(fs::read(&output).unwrap(), sentinel);
        assert_eq!(fs::read_to_string(&script).unwrap(), "Hola");
    }

    #[cfg(windows)]
    #[test]
    fn pack_share_locked_original_fails_and_keeps_existing_zip() {
        use std::fs::OpenOptions;
        use std::os::windows::fs::OpenOptionsExt;

        let base = tempfile::tempdir().unwrap();
        let game = base.path().join("game");
        let pristine = base.path().join("pristine");
        fs::create_dir(&game).unwrap();
        fs::create_dir(&pristine).unwrap();
        let script = game.join("script.rpy");
        let original = pristine.join("script.rpy");
        fs::write(&script, "Hola").unwrap();
        fs::write(&original, "Hello").unwrap();
        let db = Database::open_in_memory().unwrap();
        let mut entry = StringEntry::new("line", "Hello", script.clone());
        entry.translation = Some("Hola".into());
        entry.status = StringStatus::Translated;
        db.save_entries(&[entry]).unwrap();
        db.record_injection(Some("es"), &game, std::slice::from_ref(&script))
            .unwrap();

        let output = base.path().join("out.zip");
        let sentinel = b"existing-patch-sentinel";
        fs::write(&output, sentinel).unwrap();

        let err = {
            let _exclusive = OpenOptions::new()
                .read(true)
                .share_mode(0)
                .open(&original)
                .unwrap();
            pack_injection_recording(
                &db,
                pack_opts(
                    game,
                    output.clone(),
                    pristine,
                    base.path().join("project.db"),
                ),
            )
            .unwrap_err()
            .to_string()
        };
        assert!(
            err.contains("script.rpy") && err.contains("cannot hash original"),
            "{err}"
        );
        assert_eq!(fs::read(&output).unwrap(), sentinel);
        assert_eq!(fs::read_to_string(&original).unwrap(), "Hello");
    }
}
