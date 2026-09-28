use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};

use crate::config::AppConfig;
use crate::database::{sha256_hex, Database};
use crate::error::{LocustError, Result};
use crate::extraction::{resolve_game_root, FormatRegistry};
use crate::models::{OutputMode, StringEntry};

/// Hold the shared game lock throughout extraction and database merge, so an
/// injector in another process cannot turn a mixed generation into new source.
/// A directly selected file uses the same parent lock as single-file injection.
pub fn lock_game_source(path: &Path) -> Result<crate::patch::GameLock> {
    let selected = path.canonicalize()?;
    let root = if selected.is_file() {
        selected
            .parent()
            .ok_or_else(|| LocustError::ProjectNotFound("selected file has no parent".into()))?
    } else {
        selected.as_path()
    };
    let lock = crate::patch::GameLock::acquire(root)?;
    crate::injection_transaction::ensure_no_pending_under_lock(&lock)?;
    Ok(lock)
}

/// Outcome of opening a game into the live [`Database`] (merge, never wipe).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProjectOpenOutcome {
    pub format_id: String,
    pub format_name: String,
    pub total_strings: usize,
    pub project_path: PathBuf,
    pub project_name: String,
    pub supported_modes: Vec<OutputMode>,
    pub database_path: PathBuf,
    pub added: usize,
    pub updated: usize,
    pub stale_source_reset: usize,
    pub removed: usize,
    pub preserved_translations: usize,
    #[serde(default)]
    pub extraction_warnings: Vec<String>,
}

/// Project-level diagnostics carried by Unreal extraction. Bound the persisted
/// presentation separately from the archive parser's detailed diagnostics.
pub fn extraction_warnings(entries: &[StringEntry]) -> Vec<String> {
    let mut warnings = std::collections::BTreeSet::new();
    for entry in entries {
        if let Some(values) = entry
            .metadata
            .get("iostore_unread_containers")
            .and_then(|v| v.as_array())
        {
            for value in values.iter().take(128) {
                if let Some(message) = value.as_str().filter(|s| !s.is_empty()) {
                    warnings.insert(message.chars().take(2048).collect::<String>());
                    if warnings.len() >= 128 {
                        return warnings.into_iter().collect();
                    }
                }
            }
        }
    }
    warnings.into_iter().collect()
}

pub fn saved_extraction_warnings(db: &Database) -> Result<Vec<String>> {
    match db.get_project_metadata("extraction_warnings")? {
        Some(value) => Ok(serde_json::from_value(value)?),
        None => Ok(Vec::new()),
    }
}

const WINDOWS_RESERVED: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM0", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7",
    "COM8", "COM9", "LPT0", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    "CLOCK$",
];

/// Sanitize a game folder name for use as a file stem (no separators, no
/// reserved Windows device names).
pub fn sanitize_project_stem(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for c in name.chars() {
        if c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
            out.push('_');
        } else {
            out.push(c);
        }
    }
    let trimmed = out
        .trim_matches(|c: char| c == ' ' || c == '.' || c == '_')
        .to_string();
    let mut stem = if trimmed.is_empty() {
        "project".to_string()
    } else {
        trimmed
    };
    let upper = stem.to_ascii_uppercase();
    if WINDOWS_RESERVED.iter().any(|r| *r == upper) {
        stem = format!("_{stem}");
    }
    stem
}

fn dir_is_writable(dir: &Path) -> bool {
    if !dir.exists() {
        return false;
    }
    let probe = dir.join(format!(".locust-write-{}", uuid::Uuid::new_v4()));
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
    {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

fn short_path_hash(game_root: &Path) -> String {
    let key = if game_root.is_absolute() {
        game_root.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(game_root))
            .unwrap_or_else(|_| game_root.to_path_buf())
    };
    let hex = sha256_hex(key.to_string_lossy().as_bytes());
    hex.chars().take(8).collect()
}

/// Preferred: `<parent>/<game_name>.locust.db`. Fallback when that directory
/// is not writable: `config_dir/projects/<sanitized>-<hash>.locust.db`.
pub fn resolve_project_db_path(game_root: &Path) -> PathBuf {
    resolve_project_db_path_with(game_root, dir_is_writable)
}

fn resolve_project_db_path_with(game_root: &Path, writable: impl Fn(&Path) -> bool) -> PathBuf {
    let raw_name = game_root
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "project".to_string());
    let stem = sanitize_project_stem(&raw_name);
    if let Some(parent) = game_root.parent() {
        if !parent.as_os_str().is_empty() && writable(parent) {
            return parent.join(format!("{stem}.locust.db"));
        }
    }
    let hash = short_path_hash(game_root);
    AppConfig::config_dir()
        .join("projects")
        .join(format!("{stem}-{hash}.locust.db"))
}

/// Adjacent sibling and profile-fallback DB paths for a resolved game root.
/// Both locations are returned without probing parent writability: an existing
/// file at either path is a saved project, even if a later extract would have
/// chosen the other location.
fn saved_project_db_candidates(game_root: &Path, config_dir: &Path) -> Vec<PathBuf> {
    let raw_name = game_root
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "project".to_string());
    let stem = sanitize_project_stem(&raw_name);
    let mut out = Vec::with_capacity(2);
    if let Some(parent) = game_root.parent() {
        if !parent.as_os_str().is_empty() {
            out.push(parent.join(format!("{stem}.locust.db")));
        }
    }
    let hash = short_path_hash(game_root);
    out.push(
        config_dir
            .join("projects")
            .join(format!("{stem}-{hash}.locust.db")),
    );
    out
}

pub(crate) fn saved_db_handles_match(left: &std::fs::File, right: &std::fs::File) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        left.metadata()
            .ok()
            .zip(right.metadata().ok())
            .is_some_and(|(a, b)| a.dev() == b.dev() && a.ino() == b.ino())
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        #[repr(C)]
        #[derive(Default)]
        struct FileInformation {
            attributes: u32,
            creation: [u32; 2],
            access: [u32; 2],
            write: [u32; 2],
            volume: u32,
            size_high: u32,
            size_low: u32,
            links: u32,
            index_high: u32,
            index_low: u32,
        }
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetFileInformationByHandle(
                handle: *mut std::ffi::c_void,
                info: *mut FileInformation,
            ) -> i32;
        }
        let mut a = FileInformation::default();
        let mut b = FileInformation::default();
        // SAFETY: both handles remain open; structs match BY_HANDLE_FILE_INFORMATION.
        let ok = unsafe {
            GetFileInformationByHandle(left.as_raw_handle(), &mut a) != 0
                && GetFileInformationByHandle(right.as_raw_handle(), &mut b) != 0
        };
        ok && (a.volume, a.index_high, a.index_low) == (b.volume, b.index_high, b.index_low)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (left, right);
        false
    }
}

fn same_saved_db(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    if let (Ok(a), Ok(b)) = (left.canonicalize(), right.canonicalize()) {
        if a == b {
            return true;
        }
        #[cfg(windows)]
        {
            if a.as_os_str().eq_ignore_ascii_case(b.as_os_str()) {
                return true;
            }
        }
    }
    match (std::fs::File::open(left), std::fs::File::open(right)) {
        (Ok(a), Ok(b)) => saved_db_handles_match(&a, &b),
        _ => false,
    }
}

/// Existing files among the candidate locations, collapsing same-file aliases
/// (canonical path, case folding on Windows, hard links).
/// Absent paths and non-files are skipped. Any other metadata I/O error is
/// returned so callers do not treat an unreadable saved DB as missing.
#[cfg(test)]
fn unique_existing_saved_dbs(candidates: &[PathBuf]) -> Result<Vec<PathBuf>> {
    unique_existing_saved_dbs_with(candidates, |path| std::fs::metadata(path))
}

fn unique_existing_saved_dbs_with(
    candidates: &[PathBuf],
    metadata: impl Fn(&Path) -> std::io::Result<std::fs::Metadata>,
) -> Result<Vec<PathBuf>> {
    let mut existing: Vec<PathBuf> = Vec::new();
    for candidate in candidates {
        let meta = match metadata(candidate) {
            Ok(meta) => meta,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
            Err(err) => {
                return Err(LocustError::IoError(std::io::Error::new(
                    err.kind(),
                    format!(
                        "cannot read project database candidate {}: {err}",
                        candidate.display()
                    ),
                )));
            }
        };
        if !meta.is_file() {
            continue;
        }
        if existing.iter().any(|seen| same_saved_db(seen, candidate)) {
            continue;
        }
        existing.push(candidate.clone());
    }
    Ok(existing)
}

fn ambiguous_saved_dbs_error(paths: &[PathBuf]) -> LocustError {
    let listed = paths
        .iter()
        .map(|p| p.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    LocustError::IoError(std::io::Error::new(
        std::io::ErrorKind::InvalidInput,
        format!(
            "multiple distinct project databases exist for this game ({listed}); open a .locust.db explicitly"
        ),
    ))
}

/// Detect, extract, reopen the per-project DB, and merge entries. Shared by
/// the HTTP handler and the Tauri command.
pub fn open_project(
    db: &Database,
    registry: &FormatRegistry,
    raw_path: &Path,
    format_id: Option<&str>,
) -> Result<ProjectOpenOutcome> {
    if !raw_path.exists() {
        return Err(LocustError::ProjectNotFound(raw_path.display().to_string()));
    }

    let path = resolve_game_root(raw_path, registry);
    let _source_lock = lock_game_source(&path)?;

    let plugin = if let Some(fid) = format_id {
        registry
            .get(fid)
            .ok_or_else(|| LocustError::UnsupportedFormat(format!("Unknown format: {fid}")))?
    } else {
        registry
            .detect(&path)
            .ok_or_else(|| LocustError::UnsupportedFormat("format not detected".to_string()))?
    };

    let format_id = plugin.id().to_string();
    let format_name = plugin.name().to_string();
    let supported_modes = plugin.supported_modes();
    let entries = plugin.extract(&path)?;
    let extraction_warnings = extraction_warnings(&entries);

    let database_path = resolve_project_db_path(&path);
    db.reopen(&database_path)?;
    let merge = if extraction_warnings.is_empty() {
        db.merge_entries(&entries)?
    } else {
        db.merge_entries_preserving_missing(&entries)?
    };
    db.set_project_metadata(
        "extraction_warnings",
        &serde_json::json!(extraction_warnings),
    )?;
    let total_strings = db.get_stats()?.total;

    let project_name = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();

    Ok(ProjectOpenOutcome {
        format_id,
        format_name,
        total_strings,
        project_path: path,
        project_name,
        supported_modes,
        database_path,
        added: merge.added,
        updated: merge.updated,
        stale_source_reset: merge.stale_source_reset,
        removed: merge.removed,
        preserved_translations: merge.preserved_translations,
        extraction_warnings,
    })
}

fn not_a_locust_db(path: &Path) -> LocustError {
    LocustError::IoError(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        format!("not a Locust project database: {}", path.display()),
    ))
}

/// Read-only check that `path` is an existing Locust project database.
/// Does not create, migrate, or write. Returns the `strings` row count.
fn probe_locust_project_db(path: &Path) -> Result<usize> {
    if !path.exists() {
        return Err(LocustError::ProjectNotFound(path.display().to_string()));
    }
    if !path.is_file() {
        return Err(not_a_locust_db(path));
    }

    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|_| not_a_locust_db(path))?;

    let has_strings: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'strings'",
            [],
            |row| row.get(0),
        )
        .map_err(|_| not_a_locust_db(path))?;
    if has_strings == 0 {
        return Err(not_a_locust_db(path));
    }

    let cols: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('strings')
             WHERE name IN ('id', 'source', 'translation', 'status', 'file_path')",
            [],
            |row| row.get(0),
        )
        .map_err(|_| not_a_locust_db(path))?;
    if cols < 5 {
        return Err(not_a_locust_db(path));
    }

    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM strings", [], |row| row.get(0))
        .map_err(|_| not_a_locust_db(path))?;
    Ok(count as usize)
}

/// Reopen an existing project database without extracting or merging.
/// Shared by HTTP `POST /api/project/open-db` and the Tauri `open_project_db`
/// command. Merge counters on the shared [`ProjectOpenOutcome`] are zero.
pub fn open_project_db(
    db: &Database,
    registry: &FormatRegistry,
    database_path: &Path,
    game_path: &Path,
    format_id: &str,
) -> Result<ProjectOpenOutcome> {
    let plugin = registry
        .get(format_id)
        .ok_or_else(|| LocustError::UnsupportedFormat(format!("Unknown format: {format_id}")))?;

    let total_strings = probe_locust_project_db(database_path)?;
    // Offline database editing remains available when its game was moved.
    let _source_lock = game_path
        .exists()
        .then(|| lock_game_source(game_path))
        .transpose()?;
    db.reopen(database_path)?;

    let project_name = game_path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();

    Ok(ProjectOpenOutcome {
        format_id: plugin.id().to_string(),
        format_name: plugin.name().to_string(),
        total_strings,
        project_path: game_path.to_path_buf(),
        project_name,
        supported_modes: plugin.supported_modes(),
        database_path: database_path.to_path_buf(),
        added: 0,
        updated: 0,
        stale_source_reset: 0,
        removed: 0,
        preserved_translations: 0,
        extraction_warnings: saved_extraction_warnings(db)?,
    })
}

/// Reopen a legacy recent that has a game path but no saved `database_path`.
/// Prefers an existing validated project DB at the adjacent or profile-fallback
/// location (no writability probe) and does not extract or merge. Missing saved
/// DB falls through to ordinary [`open_project`]. Distinct existing candidates
/// are never chosen arbitrarily.
pub fn open_recent_project(
    db: &Database,
    registry: &FormatRegistry,
    raw_path: &Path,
    format_id: Option<&str>,
) -> Result<ProjectOpenOutcome> {
    open_recent_project_with(db, registry, raw_path, format_id, &AppConfig::config_dir())
}

fn open_recent_project_with(
    db: &Database,
    registry: &FormatRegistry,
    raw_path: &Path,
    format_id: Option<&str>,
    config_dir: &Path,
) -> Result<ProjectOpenOutcome> {
    open_recent_project_with_metadata(db, registry, raw_path, format_id, config_dir, |path| {
        std::fs::metadata(path)
    })
}

fn open_recent_project_with_metadata(
    db: &Database,
    registry: &FormatRegistry,
    raw_path: &Path,
    format_id: Option<&str>,
    config_dir: &Path,
    metadata: impl Fn(&Path) -> std::io::Result<std::fs::Metadata>,
) -> Result<ProjectOpenOutcome> {
    if !raw_path.exists() {
        return Err(LocustError::ProjectNotFound(raw_path.display().to_string()));
    }

    let path = resolve_game_root(raw_path, registry);
    let existing =
        unique_existing_saved_dbs_with(&saved_project_db_candidates(&path, config_dir), metadata)?;
    match existing.as_slice() {
        [] => open_project(db, registry, raw_path, format_id),
        [database_path] => {
            let plugin = if let Some(fid) = format_id {
                registry.get(fid).ok_or_else(|| {
                    LocustError::UnsupportedFormat(format!("Unknown format: {fid}"))
                })?
            } else {
                registry.detect(&path).ok_or_else(|| {
                    LocustError::UnsupportedFormat("format not detected".to_string())
                })?
            };
            open_project_db(db, registry, database_path, &path, plugin.id())
        }
        _ => Err(ambiguous_saved_dbs_error(&existing)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extraction::{FormatPlugin, InjectionReport};
    use crate::models::{StringEntry, StringStatus};

    struct TsvPlugin;

    impl FormatPlugin for TsvPlugin {
        fn id(&self) -> &str {
            "tsv-test"
        }
        fn name(&self) -> &str {
            "TSV Test"
        }
        fn supported_extensions(&self) -> &[&str] {
            &[".tsv"]
        }
        fn detect(&self, path: &Path) -> bool {
            path.join("extract.tsv").is_file()
        }
        fn extract(&self, path: &Path) -> Result<Vec<StringEntry>> {
            let text = std::fs::read_to_string(path.join("extract.tsv"))?;
            let mut out = Vec::new();
            for line in text.lines() {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                let (id, source) = line.split_once('\t').unwrap_or((line, line));
                out.push(StringEntry::new(id, source, path.join("extract.tsv")));
            }
            if path.join("unread-container").exists() {
                for entry in &mut out {
                    entry.metadata.insert(
                        "iostore_unread_containers".into(),
                        serde_json::json!(["extra.utoc: encrypted"]),
                    );
                }
            }
            Ok(out)
        }
        fn inject(&self, _path: &Path, _entries: &[StringEntry]) -> Result<InjectionReport> {
            Ok(InjectionReport {
                skip_reasons: Default::default(),
                files_modified: 0,
                strings_written: 0,
                strings_skipped: 0,
                warnings: Vec::new(),
                files_written: Vec::new(),
            })
        }
    }

    fn registry() -> FormatRegistry {
        let mut reg = FormatRegistry::new();
        reg.register(Box::new(TsvPlugin));
        reg
    }

    #[test]
    fn opening_busy_game_keeps_live_database_and_source_untouched() {
        let fixture = tempfile::tempdir().unwrap();
        let game = fixture.path().join("game");
        std::fs::create_dir(&game).unwrap();
        std::fs::write(game.join("extract.tsv"), "new\tNew").unwrap();
        let live = Database::open_in_memory().unwrap();
        live.save_entries(&[StringEntry::new("old", "Old", game.join("extract.tsv"))])
            .unwrap();
        let guard = crate::patch::GameLock::acquire(&game).unwrap();
        assert!(open_project(&live, &registry(), &game, None)
            .unwrap_err()
            .to_string()
            .contains("game busy"));
        assert_eq!(live.path(), PathBuf::from(":memory:"));
        assert!(live.get_entry("old").unwrap().is_some());
        assert!(live.get_entry("new").unwrap().is_none());
        assert!(lock_game_source(&game.join("extract.tsv")).is_err());
        drop(guard);
        assert_eq!(
            open_project(&live, &registry(), &game, None)
                .unwrap()
                .total_strings,
            1
        );
    }

    #[tokio::test]
    async fn partial_extraction_preserves_missing_translations_and_persists_warning() {
        let fixture = tempfile::tempdir().unwrap();
        let game = fixture.path().join("game");
        std::fs::create_dir(&game).unwrap();
        std::fs::write(game.join("extract.tsv"), "hero\tHello\nnpc\tWelcome").unwrap();
        let db = Database::open_in_memory().unwrap();
        let reg = registry();
        let first = open_project(&db, &reg, &game, None).unwrap();
        db.save_translation("npc", "Bienvenido", "mock")
            .await
            .unwrap();
        db.update_entry_status("npc", StringStatus::Approved)
            .await
            .unwrap();
        std::fs::write(game.join("extract.tsv"), "hero\tHello").unwrap();
        std::fs::write(game.join("unread-container"), "").unwrap();
        let partial = open_project(&db, &reg, &game, None).unwrap();
        assert_eq!(partial.total_strings, 2);
        assert_eq!(partial.removed, 0);
        assert_eq!(partial.extraction_warnings, ["extra.utoc: encrypted"]);
        let npc = db.get_entry("npc").unwrap().unwrap();
        assert_eq!(npc.translation.as_deref(), Some("Bienvenido"));
        assert_eq!(npc.status, StringStatus::Approved);
        let reopened = open_project_db(&db, &reg, &first.database_path, &game, "tsv-test").unwrap();
        assert_eq!(reopened.extraction_warnings, partial.extraction_warnings);
        std::fs::remove_file(game.join("unread-container")).unwrap();
        let complete = open_project(&db, &reg, &game, None).unwrap();
        assert_eq!(complete.total_strings, 1);
        assert_eq!(complete.removed, 1);
        assert!(complete.extraction_warnings.is_empty());
        assert!(saved_extraction_warnings(&db).unwrap().is_empty());
    }

    fn game_dir(label: &str, tsv: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("locust_proj_{label}_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("extract.tsv"), tsv).unwrap();
        dir
    }

    #[test]
    fn sanitize_project_stem_strips_separators_and_reserved_names() {
        assert_eq!(sanitize_project_stem("My/Game"), "My_Game");
        assert_eq!(sanitize_project_stem("CON"), "_CON");
        assert_eq!(sanitize_project_stem("..."), "project");
    }

    #[test]
    fn resolve_project_db_path_prefers_sibling_when_parent_writable() {
        let game = game_dir("sib", "a\tA");
        let db_path = resolve_project_db_path(&game);
        let parent = game.parent().unwrap();
        let stem = sanitize_project_stem(game.file_name().unwrap().to_str().unwrap());
        assert_eq!(db_path, parent.join(format!("{stem}.locust.db")));
        let _ = std::fs::remove_dir_all(&game);
        let _ = std::fs::remove_file(&db_path);
    }

    #[test]
    fn resolve_project_db_path_falls_back_when_parent_not_writable() {
        let game = Path::new("locked-parent").join("My Game");
        let db_path = resolve_project_db_path_with(&game, |_| false);
        let name = db_path.file_name().unwrap().to_string_lossy();
        assert!(name.starts_with("My Game-") || name.starts_with("My_Game-"));
        assert!(name.ends_with(".locust.db"));
        assert!(db_path.components().any(|c| c.as_os_str() == "projects"));
    }

    #[tokio::test]
    async fn test_open_same_game_preserves_translations() {
        let game = game_dir("same", "hero\tHello\nnpc\tWelcome");
        let db = Database::open_in_memory().unwrap();
        let reg = registry();

        let first = open_project(&db, &reg, &game, None).unwrap();
        assert!(first.added >= 2);
        assert_eq!(first.preserved_translations, 0);

        assert!(db.save_translation("hero", "Hola", "mock").await.unwrap());
        db.update_entry_status("hero", StringStatus::Approved)
            .await
            .unwrap();

        let second = open_project(&db, &reg, &game, None).unwrap();
        assert!(second.preserved_translations > 0);
        assert_eq!(second.added, 0);
        let hero = db.get_entry("hero").unwrap().unwrap();
        assert_eq!(hero.translation.as_deref(), Some("Hola"));
        assert_eq!(hero.status, StringStatus::Approved);

        let db_path = db.path();
        drop(db);
        let _ = std::fs::remove_file(&db_path);
        let _ = std::fs::remove_dir_all(&game);
    }

    #[tokio::test]
    async fn test_open_game_b_does_not_destroy_game_a() {
        let game_a = game_dir("A", "hero\tHello");
        let game_b = game_dir("B", "villain\tEvil");
        let db = Database::open_in_memory().unwrap();
        let reg = registry();

        open_project(&db, &reg, &game_a, None).unwrap();
        assert!(db.save_translation("hero", "Hola", "mock").await.unwrap());
        let db_a = db.path();

        open_project(&db, &reg, &game_b, None).unwrap();
        assert!(db.get_entry("hero").unwrap().is_none());
        assert!(db.get_entry("villain").unwrap().is_some());

        open_project(&db, &reg, &game_a, None).unwrap();
        let hero = db.get_entry("hero").unwrap().unwrap();
        assert_eq!(hero.translation.as_deref(), Some("Hola"));
        assert!(db.get_entry("villain").unwrap().is_none());

        let live_path = db.path();
        drop(db);
        let _ = std::fs::remove_file(&live_path);
        let _ = std::fs::remove_file(&db_a);
        let _ = std::fs::remove_dir_all(&game_a);
        let _ = std::fs::remove_dir_all(&game_b);
    }

    #[tokio::test]
    async fn test_open_stale_source_resets_status() {
        let game = game_dir("stale", "npc\tWelcome");
        let db = Database::open_in_memory().unwrap();
        let reg = registry();
        open_project(&db, &reg, &game, None).unwrap();
        assert!(db
            .save_translation("npc", "Bienvenido", "mock")
            .await
            .unwrap());
        db.update_entry_status("npc", StringStatus::Approved)
            .await
            .unwrap();

        std::fs::write(game.join("extract.tsv"), "npc\tWelcome, traveler").unwrap();
        let out = open_project(&db, &reg, &game, None).unwrap();
        assert_eq!(out.stale_source_reset, 1);
        let npc = db.get_entry("npc").unwrap().unwrap();
        assert_eq!(npc.source, "Welcome, traveler");
        assert_eq!(npc.translation.as_deref(), Some("Bienvenido"));
        assert_eq!(npc.status, StringStatus::Pending);

        let db_path = db.path();
        drop(db);
        let _ = std::fs::remove_file(&db_path);
        let _ = std::fs::remove_dir_all(&game);
    }

    #[tokio::test]
    async fn test_open_removes_entries_gone_from_game() {
        let game = game_dir("gone", "keep\tStay\ndrop\tLeave");
        let db = Database::open_in_memory().unwrap();
        let reg = registry();
        open_project(&db, &reg, &game, None).unwrap();
        assert!(db.get_entry("drop").unwrap().is_some());

        std::fs::write(game.join("extract.tsv"), "keep\tStay").unwrap();
        let out = open_project(&db, &reg, &game, None).unwrap();
        assert_eq!(out.removed, 1);
        assert!(db.get_entry("drop").unwrap().is_none());
        assert!(db.get_entry("keep").unwrap().is_some());

        let db_path = db.path();
        drop(db);
        let _ = std::fs::remove_file(&db_path);
        let _ = std::fs::remove_dir_all(&game);
    }

    #[test]
    fn test_http_and_tauri_share_open_project() {
        // Both HTTP POST /api/project/open and the Tauri command call
        // `open_project` below. Same inputs → same merge result.
        let game = game_dir("share", "a\tA");
        let db_http = Database::open_in_memory().unwrap();
        let db_tauri = Database::open_in_memory().unwrap();
        let reg = registry();

        let http = open_project(&db_http, &reg, &game, None).unwrap();
        // Re-open onto a second live Database as the other UI path would.
        // Windows will not unlink a SQLite file while this handle is open.
        drop(db_http);
        let _ = std::fs::remove_file(&http.database_path);
        let tauri = open_project(&db_tauri, &reg, &game, None).unwrap();

        assert_eq!(http.added, tauri.added);
        assert_eq!(http.updated, tauri.updated);
        assert_eq!(http.stale_source_reset, tauri.stale_source_reset);
        assert_eq!(http.removed, tauri.removed);
        assert_eq!(http.preserved_translations, tauri.preserved_translations);
        assert_eq!(http.database_path, tauri.database_path);

        drop(db_tauri);
        let _ = std::fs::remove_file(&tauri.database_path);
        let _ = std::fs::remove_dir_all(&game);
    }

    #[test]
    fn open_project_is_reachable_as_shared_core_entry() {
        // Compile-time contract: callers import this single function.
        let _: fn(&Database, &FormatRegistry, &Path, Option<&str>) -> Result<ProjectOpenOutcome> =
            open_project;
    }

    struct PanicExtractPlugin;

    impl FormatPlugin for PanicExtractPlugin {
        fn id(&self) -> &str {
            "tsv-no-extract"
        }
        fn name(&self) -> &str {
            "No Extract"
        }
        fn supported_extensions(&self) -> &[&str] {
            &[".tsv"]
        }
        fn detect(&self, _path: &Path) -> bool {
            true
        }
        fn extract(&self, _path: &Path) -> Result<Vec<StringEntry>> {
            panic!("open_project_db must not extract");
        }
        fn inject(&self, _path: &Path, _entries: &[StringEntry]) -> Result<InjectionReport> {
            Ok(InjectionReport {
                skip_reasons: Default::default(),
                files_modified: 0,
                strings_written: 0,
                strings_skipped: 0,
                warnings: Vec::new(),
                files_written: Vec::new(),
            })
        }
    }

    fn panic_extract_registry() -> FormatRegistry {
        let mut reg = FormatRegistry::new();
        reg.register(Box::new(PanicExtractPlugin));
        reg
    }

    fn write_locust_db(label: &str, rows: &[(&str, &str, Option<&str>)]) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "locust_opendb_{label}_{}.locust.db",
            uuid::Uuid::new_v4()
        ));
        let db = Database::open(&path).unwrap();
        let entries: Vec<StringEntry> = rows
            .iter()
            .map(|(id, source, translation)| {
                let mut e = StringEntry::new(*id, *source, PathBuf::from("data/a.json"));
                if let Some(t) = translation {
                    e.translation = Some((*t).to_string());
                    e.status = StringStatus::Translated;
                }
                e
            })
            .collect();
        db.save_entries(&entries).unwrap();
        drop(db);
        path
    }

    fn snapshot_db_sidecars(path: &Path) -> Vec<(String, Vec<u8>)> {
        let mut out = Vec::new();
        for suffix in ["", "-wal", "-shm"] {
            let p = PathBuf::from(format!("{}{suffix}", path.display()));
            if p.is_file() {
                out.push((suffix.to_string(), std::fs::read(&p).unwrap()));
            }
        }
        out
    }

    #[test]
    fn open_project_db_is_reachable_as_shared_core_entry() {
        let _: fn(&Database, &FormatRegistry, &Path, &Path, &str) -> Result<ProjectOpenOutcome> =
            open_project_db;
    }

    #[test]
    fn open_project_db_opens_pivoted_db_without_extract_or_row_change() {
        use crate::database::EntryFilter;

        let source_path = write_locust_db(
            "src",
            &[
                ("a", "Hello", Some("Hola")),
                ("b", "World", Some("Mundo")),
                ("c", "Skip", None),
            ],
        );
        let source_db = Database::open(&source_path).unwrap();
        let pivot_path = std::env::temp_dir().join(format!(
            "locust_opendb_pivot_{}.locust.db",
            uuid::Uuid::new_v4()
        ));
        let pivoted = source_db.pivot_to(&pivot_path).unwrap();
        assert_eq!(pivoted.entries, 2);
        drop(source_db);
        let source_snapshot = snapshot_db_sidecars(&source_path);

        let snap = Database::open(&pivot_path).unwrap();
        let before = snap.get_entries(&EntryFilter::default()).unwrap();
        drop(snap);

        let live = Database::open_in_memory().unwrap();
        let game =
            std::env::temp_dir().join(format!("locust_opendb_game_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&game).unwrap();
        std::fs::write(game.join("extract.tsv"), "hero\tEXTRACTED").unwrap();

        let reg = panic_extract_registry();
        let out = open_project_db(&live, &reg, &pivot_path, &game, "tsv-no-extract").unwrap();

        assert_eq!(out.total_strings, 2);
        assert_eq!(out.added, 0);
        assert_eq!(out.updated, 0);
        assert_eq!(out.stale_source_reset, 0);
        assert_eq!(out.removed, 0);
        assert_eq!(out.preserved_translations, 0);
        assert_eq!(out.format_id, "tsv-no-extract");
        assert_eq!(out.format_name, "No Extract");
        assert_eq!(out.project_path, game);
        assert_eq!(out.database_path, pivot_path);
        assert_eq!(live.path(), pivot_path);

        let after = live.get_entries(&EntryFilter::default()).unwrap();
        assert_eq!(after.len(), before.len());
        for e in &before {
            let got = after.iter().find(|x| x.id == e.id).expect(&e.id);
            assert_eq!(got.source, e.source);
            assert_eq!(got.translation, e.translation);
            assert_eq!(got.status, e.status);
        }
        assert!(after.iter().any(|e| e.source == "Hola"));
        assert!(after.iter().any(|e| e.source == "Mundo"));
        assert!(!after
            .iter()
            .any(|e| e.source == "EXTRACTED" || e.id == "hero"));
        assert!(!after.iter().any(|e| e.id == "c"));

        assert_eq!(snapshot_db_sidecars(&source_path), source_snapshot);

        drop(live);
        let _ = std::fs::remove_file(&source_path);
        let _ = std::fs::remove_file(&pivot_path);
        let _ = std::fs::remove_dir_all(&game);
    }

    #[test]
    fn open_project_db_rejects_unrelated_db_and_leaves_live_connection() {
        let live_path = write_locust_db("live", &[("keep", "Stay", Some("Queda"))]);
        let live = Database::open(&live_path).unwrap();

        let other =
            std::env::temp_dir().join(format!("locust_opendb_other_{}.db", uuid::Uuid::new_v4()));
        {
            let conn = rusqlite::Connection::open(&other).unwrap();
            conn.execute("CREATE TABLE foo (id INTEGER PRIMARY KEY)", [])
                .unwrap();
            conn.execute("INSERT INTO foo (id) VALUES (1)", []).unwrap();
        }
        let other_bytes = std::fs::read(&other).unwrap();

        let game = std::env::temp_dir().join(format!("locust_opendb_g_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&game).unwrap();

        let err = open_project_db(&live, &registry(), &other, &game, "tsv-test").unwrap_err();
        let msg = err.to_string().to_lowercase();
        assert!(
            msg.contains("not a locust project database"),
            "clean error: {err}"
        );

        assert_eq!(live.path(), live_path);
        let keep = live.get_entry("keep").unwrap().unwrap();
        assert_eq!(keep.source, "Stay");
        assert_eq!(keep.translation.as_deref(), Some("Queda"));
        assert_eq!(std::fs::read(&other).unwrap(), other_bytes);

        drop(live);
        let _ = std::fs::remove_file(&live_path);
        let _ = std::fs::remove_file(&other);
        let _ = std::fs::remove_dir_all(&game);
    }

    #[test]
    fn open_project_db_rejects_missing_file() {
        let live = Database::open_in_memory().unwrap();
        let missing = std::env::temp_dir().join(format!(
            "locust_opendb_missing_{}.locust.db",
            uuid::Uuid::new_v4()
        ));
        let err = open_project_db(&live, &registry(), &missing, Path::new("game"), "tsv-test")
            .unwrap_err();
        match err {
            LocustError::ProjectNotFound(p) => {
                assert!(p.contains("locust_opendb_missing_"), "{p}");
            }
            other => panic!("expected ProjectNotFound, got {other}"),
        }
        assert_eq!(live.path(), PathBuf::from(":memory:"));
    }

    #[test]
    fn open_project_db_rejects_directory() {
        let live = Database::open_in_memory().unwrap();
        let dir = std::env::temp_dir().join(format!("locust_opendb_dir_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let err = open_project_db(&live, &registry(), &dir, &dir, "tsv-test").unwrap_err();
        let msg = err.to_string().to_lowercase();
        assert!(
            msg.contains("not a locust project database"),
            "clean error: {err}"
        );
        assert_eq!(live.path(), PathBuf::from(":memory:"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn open_project_db_rejects_unknown_format() {
        let db_path = write_locust_db("fmt", &[("a", "A", None)]);
        let live = Database::open_in_memory().unwrap();
        let err = open_project_db(
            &live,
            &registry(),
            &db_path,
            Path::new("game"),
            "not-a-format",
        )
        .unwrap_err();
        match err {
            LocustError::UnsupportedFormat(msg) => {
                assert!(msg.contains("not-a-format"), "{msg}");
            }
            other => panic!("expected UnsupportedFormat, got {other}"),
        }
        assert_eq!(live.path(), PathBuf::from(":memory:"));
        drop(live);
        let _ = std::fs::remove_file(&db_path);
    }

    fn copy_translated_db(src: &Path, dest: &Path) {
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::copy(src, dest).unwrap();
    }

    #[test]
    fn open_recent_project_is_reachable_as_shared_core_entry() {
        let _: fn(&Database, &FormatRegistry, &Path, Option<&str>) -> Result<ProjectOpenOutcome> =
            open_recent_project;
    }

    #[tokio::test]
    async fn open_recent_preserves_translations_when_game_bytes_change() {
        let game = game_dir("legacy_saved", "hero\tHello\nnpc\tWelcome");
        let db = Database::open_in_memory().unwrap();
        let reg = registry();
        let first = open_project(&db, &reg, &game, None).unwrap();
        assert!(db.save_translation("hero", "Hola", "mock").await.unwrap());
        db.update_entry_status("hero", StringStatus::Approved)
            .await
            .unwrap();

        std::fs::write(
            game.join("extract.tsv"),
            "hero\tHello, traveler\nnpc\tWelcome",
        )
        .unwrap();
        let saved = open_recent_project(&db, &reg, &game, None).unwrap();
        assert_eq!(saved.stale_source_reset, 0);
        assert_eq!(saved.added, 0);
        assert_eq!(saved.updated, 0);
        assert_eq!(saved.removed, 0);
        assert_eq!(saved.database_path, first.database_path);
        let hero = db.get_entry("hero").unwrap().unwrap();
        assert_eq!(hero.source, "Hello");
        assert_eq!(hero.translation.as_deref(), Some("Hola"));
        assert_eq!(hero.status, StringStatus::Approved);

        let explicit = open_project(&db, &reg, &game, None).unwrap();
        assert_eq!(explicit.stale_source_reset, 1);
        let hero = db.get_entry("hero").unwrap().unwrap();
        assert_eq!(hero.source, "Hello, traveler");
        assert_eq!(hero.translation.as_deref(), Some("Hola"));
        assert_eq!(hero.status, StringStatus::Pending);

        let db_path = db.path();
        drop(db);
        let _ = std::fs::remove_file(&db_path);
        let _ = std::fs::remove_dir_all(&game);
    }

    #[test]
    fn open_recent_missing_saved_db_extracts() {
        let game = game_dir("legacy_missing", "hero\tHello");
        let live = Database::open_in_memory().unwrap();
        let data = tempfile::tempdir().unwrap();
        let out = open_recent_project_with(&live, &registry(), &game, None, data.path()).unwrap();
        assert!(out.added >= 1);
        assert_eq!(live.get_entry("hero").unwrap().unwrap().source, "Hello");
        drop(live);
        let _ = std::fs::remove_file(&out.database_path);
        let _ = std::fs::remove_dir_all(&game);
    }

    #[test]
    fn open_recent_invalid_saved_db_does_not_extract() {
        let game = game_dir("legacy_invalid", "hero\tEXTRACTED");
        let adjacent = saved_project_db_candidates(&game, Path::new("/unused"))[0].clone();
        std::fs::write(&adjacent, b"not-a-sqlite-database").unwrap();
        let live = Database::open_in_memory().unwrap();
        let err = open_recent_project_with(
            &live,
            &panic_extract_registry(),
            &game,
            Some("tsv-no-extract"),
            Path::new("/unused"),
        )
        .unwrap_err();
        let msg = err.to_string().to_lowercase();
        assert!(
            msg.contains("not a locust project database"),
            "invalid saved DB must surface: {err}"
        );
        assert_eq!(live.path(), PathBuf::from(":memory:"));
        assert!(live.get_entry("hero").unwrap().is_none());
        drop(live);
        let _ = std::fs::remove_file(&adjacent);
        let _ = std::fs::remove_dir_all(&game);
    }

    #[test]
    fn open_recent_metadata_error_does_not_extract() {
        let game = game_dir("legacy_meta_err", "hero\tEXTRACTED");
        let data = tempfile::tempdir().unwrap();
        let candidates = saved_project_db_candidates(&game, data.path());
        let adjacent = candidates[0].clone();
        std::fs::write(&adjacent, b"existing-unreadable-candidate").unwrap();

        let live = Database::open_in_memory().unwrap();
        live.save_entries(&[StringEntry::new("keep", "Stay", game.join("extract.tsv"))])
            .unwrap();
        let live_bytes = std::fs::read(&adjacent).unwrap();

        let err = open_recent_project_with_metadata(
            &live,
            &panic_extract_registry(),
            &game,
            Some("tsv-no-extract"),
            data.path(),
            |path| {
                Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    format!("candidate metadata denied: {}", path.display()),
                ))
            },
        )
        .unwrap_err();
        let msg = err.to_string().to_lowercase();
        assert!(
            msg.contains("cannot read project database candidate") && msg.contains("denied"),
            "metadata failure must surface, not extract: {err}"
        );
        assert_eq!(live.path(), PathBuf::from(":memory:"));
        assert_eq!(live.get_entry("keep").unwrap().unwrap().source, "Stay");
        assert!(live.get_entry("hero").unwrap().is_none());
        assert_eq!(std::fs::read(&adjacent).unwrap(), live_bytes);

        drop(live);
        let _ = std::fs::remove_file(&adjacent);
        let _ = std::fs::remove_dir_all(&game);
    }

    #[test]
    fn unique_existing_saved_dbs_skips_missing_and_non_files() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("no.locust.db");
        let folder = dir.path().join("dir.locust.db");
        std::fs::create_dir(&folder).unwrap();
        let found = unique_existing_saved_dbs(&[missing, folder]).unwrap();
        assert!(found.is_empty());
    }

    #[tokio::test]
    async fn open_recent_uses_profile_fallback_without_writability_probe() {
        let game = game_dir("legacy_fallback", "hero\tHello");
        let db = Database::open_in_memory().unwrap();
        let first = open_project(&db, &registry(), &game, None).unwrap();
        assert!(db.save_translation("hero", "Hola", "mock").await.unwrap());
        let original = first.database_path.clone();
        drop(db);

        let data = tempfile::tempdir().unwrap();
        let candidates = saved_project_db_candidates(&game, data.path());
        let fallback = candidates[1].clone();
        copy_translated_db(&original, &fallback);
        std::fs::remove_file(&original).unwrap();

        std::fs::write(game.join("extract.tsv"), "hero\tChanged").unwrap();
        let live = Database::open_in_memory().unwrap();
        let out = open_recent_project_with(&live, &registry(), &game, None, data.path()).unwrap();
        assert_eq!(out.database_path, fallback);
        assert_eq!(out.stale_source_reset, 0);
        let hero = live.get_entry("hero").unwrap().unwrap();
        assert_eq!(hero.source, "Hello");
        assert_eq!(hero.translation.as_deref(), Some("Hola"));

        drop(live);
        let _ = std::fs::remove_file(&fallback);
        let _ = std::fs::remove_dir_all(&game);
    }

    #[test]
    fn open_recent_rejects_ambiguous_distinct_saved_dbs() {
        let game = game_dir("legacy_ambiguous", "hero\tHello");
        let db = Database::open_in_memory().unwrap();
        let first = open_project(&db, &registry(), &game, None).unwrap();
        let original = first.database_path.clone();
        drop(db);

        let data = tempfile::tempdir().unwrap();
        let candidates = saved_project_db_candidates(&game, data.path());
        copy_translated_db(&original, &candidates[1]);
        assert!(candidates[0].is_file(), "adjacent from first open");
        assert!(candidates[1].is_file());
        assert!(!same_saved_db(&candidates[0], &candidates[1]));

        let live = Database::open_in_memory().unwrap();
        let err =
            open_recent_project_with(&live, &registry(), &game, None, data.path()).unwrap_err();
        let msg = err.to_string();
        let lower = msg.to_lowercase();
        assert!(lower.contains("open a .locust.db explicitly"), "{msg}");
        assert!(msg.contains(&candidates[0].display().to_string()), "{msg}");
        assert!(msg.contains(&candidates[1].display().to_string()), "{msg}");
        assert_eq!(live.path(), PathBuf::from(":memory:"));

        drop(live);
        let _ = std::fs::remove_file(&candidates[0]);
        let _ = std::fs::remove_file(&candidates[1]);
        let _ = std::fs::remove_dir_all(&game);
    }

    #[tokio::test]
    async fn open_recent_same_file_aliases_are_one_candidate() {
        let game = game_dir("legacy_alias", "hero\tHello");
        let db = Database::open_in_memory().unwrap();
        let first = open_project(&db, &registry(), &game, None).unwrap();
        assert!(db.save_translation("hero", "Hola", "mock").await.unwrap());
        let original = first.database_path.clone();
        drop(db);

        let data = tempfile::tempdir().unwrap();
        let candidates = saved_project_db_candidates(&game, data.path());
        std::fs::create_dir_all(candidates[1].parent().unwrap()).unwrap();
        std::fs::hard_link(&original, &candidates[1]).expect("NTFS hard link");
        assert!(same_saved_db(&candidates[0], &candidates[1]));

        std::fs::write(game.join("extract.tsv"), "hero\tChanged").unwrap();
        let live = Database::open_in_memory().unwrap();
        let out = open_recent_project_with(&live, &registry(), &game, None, data.path()).unwrap();
        assert_eq!(out.stale_source_reset, 0);
        assert_eq!(live.get_entry("hero").unwrap().unwrap().source, "Hello");

        drop(live);
        let _ = std::fs::remove_file(&candidates[0]);
        let _ = std::fs::remove_file(&candidates[1]);
        let _ = std::fs::remove_dir_all(&game);
    }

    struct FilePlugin;

    impl FormatPlugin for FilePlugin {
        fn id(&self) -> &str {
            "file-test"
        }
        fn name(&self) -> &str {
            "File Test"
        }
        fn supported_extensions(&self) -> &[&str] {
            &[".txt"]
        }
        fn detect(&self, path: &Path) -> bool {
            path.is_file() && path.extension().is_some_and(|e| e == "txt")
        }
        fn extract(&self, path: &Path) -> Result<Vec<StringEntry>> {
            let text = std::fs::read_to_string(path)?;
            Ok(text
                .lines()
                .filter(|line| !line.trim().is_empty())
                .map(|line| {
                    let (id, source) = line.split_once('\t').unwrap_or((line, line));
                    StringEntry::new(id, source, path.to_path_buf())
                })
                .collect())
        }
        fn inject(&self, _path: &Path, _entries: &[StringEntry]) -> Result<InjectionReport> {
            Ok(InjectionReport {
                skip_reasons: Default::default(),
                files_modified: 0,
                strings_written: 0,
                strings_skipped: 0,
                warnings: Vec::new(),
                files_written: Vec::new(),
            })
        }
    }

    #[tokio::test]
    async fn open_recent_single_file_root_uses_sibling_db() {
        let dir = std::env::temp_dir().join(format!("locust_proj_file_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let game = dir.join("story.txt");
        std::fs::write(&game, "hero\tHello").unwrap();
        let mut reg = FormatRegistry::new();
        reg.register(Box::new(FilePlugin));
        let db = Database::open_in_memory().unwrap();
        let first = open_project(&db, &reg, &game, None).unwrap();
        assert!(db.save_translation("hero", "Hola", "mock").await.unwrap());
        assert_eq!(
            first.database_path.file_name().unwrap().to_string_lossy(),
            "story.txt.locust.db"
        );

        std::fs::write(&game, "hero\tChanged").unwrap();
        let saved = open_recent_project(&db, &reg, &game, None).unwrap();
        assert_eq!(saved.stale_source_reset, 0);
        assert_eq!(saved.database_path, first.database_path);
        assert_eq!(db.get_entry("hero").unwrap().unwrap().source, "Hello");

        drop(db);
        let _ = std::fs::remove_file(&first.database_path);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
