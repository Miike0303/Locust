use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};

use crate::config::AppConfig;
use crate::database::{sha256_hex, Database};
use crate::error::{LocustError, Result};
use crate::extraction::{resolve_game_root, FormatRegistry};
use crate::models::OutputMode;

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

    let database_path = resolve_project_db_path(&path);
    db.reopen(&database_path)?;
    let merge = db.merge_entries(&entries)?;
    let total_strings = merge.added + merge.updated;

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
    })
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
            Ok(out)
        }
        fn inject(&self, _path: &Path, _entries: &[StringEntry]) -> Result<InjectionReport> {
            Ok(InjectionReport {
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
        let game = PathBuf::from(r"Z:\locked-parent\My Game");
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
}
