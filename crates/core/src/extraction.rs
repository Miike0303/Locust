use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use walkdir::WalkDir;

use crate::backup::BackupManager;
use crate::database::Database;
use crate::error::{LocustError, Result};
use crate::models::{OutputMode, ProgressEvent, StringEntry};

pub trait FormatPlugin: Send + Sync {
    fn id(&self) -> &str;
    fn name(&self) -> &str;
    fn description(&self) -> &str {
        ""
    }
    fn supported_extensions(&self) -> &[&str];
    fn supported_modes(&self) -> Vec<OutputMode> {
        vec![OutputMode::Replace]
    }

    fn detect(&self, path: &Path) -> bool {
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            let ext_lower = ext.to_lowercase();
            self.supported_extensions()
                .iter()
                .any(|supported| {
                    let s = supported.strip_prefix('.').unwrap_or(supported);
                    s.to_lowercase() == ext_lower
                })
        } else {
            false
        }
    }

    fn extract(&self, path: &Path) -> Result<Vec<StringEntry>>;

    fn inject(&self, path: &Path, entries: &[StringEntry]) -> Result<InjectionReport>;

    fn inject_add(
        &self,
        _path: &Path,
        _lang: &str,
        _entries: &[StringEntry],
    ) -> Result<InjectionReport> {
        Err(LocustError::UnsupportedFormat(format!(
            "{} does not support Add mode",
            self.name()
        )))
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct InjectionReport {
    pub files_modified: usize,
    pub strings_written: usize,
    pub strings_skipped: usize,
    pub warnings: Vec<String>,
}

pub struct FormatRegistry {
    plugins: Vec<Box<dyn FormatPlugin>>,
}

impl FormatRegistry {
    pub fn new() -> Self {
        Self {
            plugins: Vec::new(),
        }
    }

    pub fn register(&mut self, plugin: Box<dyn FormatPlugin>) {
        self.plugins.push(plugin);
    }

    pub fn detect(&self, path: &Path) -> Option<&dyn FormatPlugin> {
        self.plugins.iter().find(|p| p.detect(path)).map(|p| p.as_ref())
    }

    pub fn get(&self, id: &str) -> Option<&dyn FormatPlugin> {
        self.plugins
            .iter()
            .find(|p| p.id() == id)
            .map(|p| p.as_ref())
    }

    pub fn list(&self) -> Vec<PluginInfo> {
        self.plugins
            .iter()
            .map(|p| PluginInfo {
                id: p.id().to_string(),
                name: p.name().to_string(),
                description: p.description().to_string(),
                extensions: p
                    .supported_extensions()
                    .iter()
                    .map(|e| e.to_string())
                    .collect(),
                supported_modes: p.supported_modes(),
            })
            .collect()
    }
}

impl Default for FormatRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Resolve a file path (executable, .html, .rpy, etc.) to the game root directory.
/// If the path is already a directory, return it as-is.
/// If it's a file, walk up to find the directory that a plugin can detect.
pub fn resolve_game_root(path: &Path, registry: &FormatRegistry) -> PathBuf {
    if path.is_dir() {
        return path.to_path_buf();
    }

    // If a plugin can detect the file directly (e.g., .html, .rpy, .rpa), return it
    if registry.detect(path).is_some() {
        return path.to_path_buf();
    }

    // Walk up parent directories to find one a plugin recognizes
    let mut current = path.parent();
    while let Some(dir) = current {
        if registry.detect(dir).is_some() {
            return dir.to_path_buf();
        }
        current = dir.parent();
    }

    // Fallback: return the parent directory of the file
    path.parent().unwrap_or(path).to_path_buf()
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PluginInfo {
    pub id: String,
    pub name: String,
    pub description: String,
    pub extensions: Vec<String>,
    pub supported_modes: Vec<OutputMode>,
}

// ─── Multi-language injection pipeline ──────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct MultiLangReport {
    pub mode: OutputMode,
    pub languages_processed: Vec<String>,
    pub languages_failed: Vec<(String, String)>,
    pub backup_id: String,
    pub reports: HashMap<String, InjectionReport>,
}

pub struct MultiLangInjector {
    pub registry: Arc<FormatRegistry>,
    pub db: Arc<Database>,
    pub backup_manager: Arc<BackupManager>,
}

impl MultiLangInjector {
    pub fn new(
        registry: Arc<FormatRegistry>,
        db: Arc<Database>,
        backup_manager: Arc<BackupManager>,
    ) -> Self {
        Self {
            registry,
            db,
            backup_manager,
        }
    }

    pub async fn inject(
        &self,
        project_path: &Path,
        format_id: &str,
        mode: OutputMode,
        languages: Vec<String>,
        output_dir: Option<PathBuf>,
        tx: mpsc::Sender<ProgressEvent>,
    ) -> Result<MultiLangReport> {
        // Create backup (best-effort — skip if paths too long on Windows)
        let backup_id = match self.backup_manager.create_backup(project_path) {
            Ok(backup) => backup.id.clone(),
            Err(e) => {
                tracing::warn!("Backup failed (continuing without backup): {}", e);
                "none".to_string()
            }
        };

        let plugin = self.registry.get(format_id).ok_or_else(|| {
            LocustError::UnsupportedFormat(format!("format not found: {}", format_id))
        })?;

        match mode {
            OutputMode::Replace => {
                self.inject_replace(
                    project_path,
                    plugin,
                    languages,
                    output_dir.ok_or_else(|| {
                        LocustError::InjectionError(
                            "output_dir is required for Replace mode".to_string(),
                        )
                    })?,
                    backup_id,
                    tx,
                )
                .await
            }
            OutputMode::Add => {
                self.inject_add(project_path, plugin, languages, backup_id, tx)
                    .await
            }
        }
    }

    async fn inject_replace(
        &self,
        project_path: &Path,
        plugin: &dyn FormatPlugin,
        languages: Vec<String>,
        output_dir: PathBuf,
        backup_id: String,
        tx: mpsc::Sender<ProgressEvent>,
    ) -> Result<MultiLangReport> {
        let total = languages.len();
        let mut languages_processed = Vec::new();
        let mut languages_failed = Vec::new();
        let mut reports = HashMap::new();

        let game_name = project_path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        for (idx, lang) in languages.iter().enumerate() {
            let dest = output_dir.join(format!("{}-{}", game_name, lang));

            // Copy project to dest
            if let Err(e) = copy_dir_for_inject(project_path, &dest) {
                languages_failed.push((lang.clone(), e.to_string()));
                continue;
            }

            // Load entries from db with tag filter for this language
            let entries = self
                .db
                .get_entries(&crate::database::EntryFilter::default())?;

            match plugin.inject(&dest, &entries) {
                Ok(report) => {
                    reports.insert(lang.clone(), report);
                    languages_processed.push(lang.clone());
                }
                Err(e) => {
                    languages_failed.push((lang.clone(), e.to_string()));
                }
            }

            let _ = tx
                .send(ProgressEvent::BatchCompleted {
                    completed: idx + 1,
                    total,
                    cost_so_far: 0.0,
                    language: Some(lang.clone()),
                })
                .await;
        }

        Ok(MultiLangReport {
            mode: OutputMode::Replace,
            languages_processed,
            languages_failed,
            backup_id,
            reports,
        })
    }

    async fn inject_add(
        &self,
        project_path: &Path,
        plugin: &dyn FormatPlugin,
        languages: Vec<String>,
        backup_id: String,
        tx: mpsc::Sender<ProgressEvent>,
    ) -> Result<MultiLangReport> {
        let total = languages.len();
        let mut languages_processed = Vec::new();
        let mut languages_failed = Vec::new();
        let mut reports = HashMap::new();

        for (idx, lang) in languages.iter().enumerate() {
            let entries = self
                .db
                .get_entries(&crate::database::EntryFilter::default())?;

            match plugin.inject_add(project_path, lang, &entries) {
                Ok(report) => {
                    reports.insert(lang.clone(), report);
                    languages_processed.push(lang.clone());
                }
                Err(e) => {
                    languages_failed.push((lang.clone(), e.to_string()));
                }
            }

            let _ = tx
                .send(ProgressEvent::BatchCompleted {
                    completed: idx + 1,
                    total,
                    cost_so_far: 0.0,
                    language: Some(lang.clone()),
                })
                .await;
        }

        Ok(MultiLangReport {
            mode: OutputMode::Add,
            languages_processed,
            languages_failed,
            backup_id,
            reports,
        })
    }
}

fn copy_dir_for_inject(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst)?;

    let media_extensions = ["png", "ogg", "wav", "m4a", "mp4", "jpg", "jpeg", "bmp", "mp3"];

    for entry in WalkDir::new(src).follow_links(false) {
        let entry = entry.map_err(|e| LocustError::IoError(std::io::Error::new(std::io::ErrorKind::Other, e)))?;
        let rel = entry.path().strip_prefix(src).map_err(|e| {
            LocustError::InjectionError(e.to_string())
        })?;
        let dest = dst.join(rel);

        if entry.file_type().is_dir() {
            std::fs::create_dir_all(&dest)?;
        } else if entry.file_type().is_file() {
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent)?;
            }

            let is_media = entry
                .path()
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| media_extensions.contains(&e.to_lowercase().as_str()))
                .unwrap_or(false);

            if is_media {
                #[cfg(unix)]
                {
                    if std::fs::hard_link(entry.path(), &dest).is_err() {
                        std::fs::copy(entry.path(), &dest)?;
                    }
                }
                #[cfg(not(unix))]
                {
                    std::fs::copy(entry.path(), &dest)?;
                }
            } else {
                std::fs::copy(entry.path(), &dest)?;
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    struct MockFormatPlugin;

    impl FormatPlugin for MockFormatPlugin {
        fn id(&self) -> &str {
            "mock"
        }
        fn name(&self) -> &str {
            "Mock Format"
        }
        fn supported_extensions(&self) -> &[&str] {
            &[".mock"]
        }
        fn supported_modes(&self) -> Vec<OutputMode> {
            vec![OutputMode::Replace, OutputMode::Add]
        }

        fn extract(&self, _path: &Path) -> Result<Vec<StringEntry>> {
            let entries = vec![
                StringEntry::new("mock#0", "Hello", PathBuf::from("game.mock")),
                StringEntry::new("mock#1", "World", PathBuf::from("game.mock")),
                StringEntry::new("mock#2", "Test", PathBuf::from("game.mock")),
            ];
            Ok(entries)
        }

        fn inject(&self, path: &Path, entries: &[StringEntry]) -> Result<InjectionReport> {
            let out_path = path.with_extension("injected");
            let mut lines = Vec::new();
            let mut written = 0;
            let mut skipped = 0;
            for entry in entries {
                if let Some(ref t) = entry.translation {
                    lines.push(format!("{}={}", entry.id, t));
                    written += 1;
                } else {
                    skipped += 1;
                }
            }
            fs::write(&out_path, lines.join("\n"))?;
            Ok(InjectionReport {
                files_modified: 1,
                strings_written: written,
                strings_skipped: skipped,
                warnings: Vec::new(),
            })
        }

        fn inject_add(
            &self,
            path: &Path,
            lang: &str,
            entries: &[StringEntry],
        ) -> Result<InjectionReport> {
            let lang_dir = path.join("tl").join(lang);
            fs::create_dir_all(&lang_dir)?;
            let out_path = lang_dir.join("mock.txt");
            let mut lines = Vec::new();
            let mut written = 0;
            let mut skipped = 0;
            for entry in entries {
                if let Some(ref t) = entry.translation {
                    lines.push(format!("{}={}", entry.id, t));
                    written += 1;
                } else {
                    skipped += 1;
                }
            }
            fs::write(&out_path, lines.join("\n"))?;
            Ok(InjectionReport {
                files_modified: 1,
                strings_written: written,
                strings_skipped: skipped,
                warnings: Vec::new(),
            })
        }
    }

    struct MockFormatPlugin2;

    impl FormatPlugin for MockFormatPlugin2 {
        fn id(&self) -> &str {
            "mock2"
        }
        fn name(&self) -> &str {
            "Mock Format 2"
        }
        fn supported_extensions(&self) -> &[&str] {
            &[".mock"]
        }
        fn extract(&self, _path: &Path) -> Result<Vec<StringEntry>> {
            Ok(vec![])
        }
        fn inject(&self, _path: &Path, _entries: &[StringEntry]) -> Result<InjectionReport> {
            Ok(InjectionReport {
                files_modified: 0,
                strings_written: 0,
                strings_skipped: 0,
                warnings: Vec::new(),
            })
        }
    }

    fn make_registry() -> FormatRegistry {
        let mut reg = FormatRegistry::new();
        reg.register(Box::new(MockFormatPlugin));
        reg
    }

    #[test]
    fn test_registry_detect_by_extension() {
        let reg = make_registry();
        assert!(reg.detect(Path::new("game.mock")).is_some());
    }

    #[test]
    fn test_registry_detect_case_insensitive() {
        let reg = make_registry();
        assert!(reg.detect(Path::new("game.MOCK")).is_some());
    }

    #[test]
    fn test_registry_unknown_extension() {
        let reg = make_registry();
        assert!(reg.detect(Path::new("game.xyz")).is_none());
    }

    #[test]
    fn test_registry_get_by_id() {
        let reg = make_registry();
        assert!(reg.get("mock").is_some());
        assert_eq!(reg.get("mock").unwrap().id(), "mock");
    }

    #[test]
    fn test_registry_list() {
        let reg = make_registry();
        let list = reg.list();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, "mock");
    }

    #[test]
    fn test_mock_extract_returns_3_entries() {
        let tmp = tempdir();
        let file = tmp.join("game.mock");
        fs::write(&file, "").unwrap();
        let plugin = MockFormatPlugin;
        let entries = plugin.extract(&file).unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].id, "mock#0");
        assert_eq!(entries[1].source, "World");
    }

    #[test]
    fn test_inject_replace_roundtrip() {
        let tmp = tempdir();
        let file = tmp.join("game.mock");
        fs::write(&file, "").unwrap();
        let plugin = MockFormatPlugin;
        let mut entries = plugin.extract(&file).unwrap();
        entries[0].translation = Some("Hola".to_string());
        entries[1].translation = Some("Mundo".to_string());
        entries[2].translation = Some("Prueba".to_string());
        plugin.inject(&file, &entries).unwrap();
        let injected = fs::read_to_string(file.with_extension("injected")).unwrap();
        assert!(injected.contains("mock#0=Hola"));
        assert!(injected.contains("mock#1=Mundo"));
        assert!(injected.contains("mock#2=Prueba"));
    }

    #[test]
    fn test_inject_add_creates_lang_dir() {
        let tmp = tempdir();
        let plugin = MockFormatPlugin;
        let mut entries = plugin.extract(&tmp).unwrap();
        entries[0].translation = Some("Hola".to_string());
        plugin.inject_add(&tmp, "es", &entries).unwrap();
        let lang_file = tmp.join("tl").join("es").join("mock.txt");
        assert!(lang_file.exists());
    }

    #[test]
    fn test_inject_report_counts() {
        let tmp = tempdir();
        let file = tmp.join("game.mock");
        fs::write(&file, "").unwrap();
        let plugin = MockFormatPlugin;
        let mut entries = plugin.extract(&file).unwrap();
        entries[0].translation = Some("Hola".to_string());
        entries[1].translation = Some("Mundo".to_string());
        // entries[2] has no translation
        let report = plugin.inject(&file, &entries).unwrap();
        assert_eq!(report.strings_written, 2);
        assert_eq!(report.strings_skipped, 1);
    }

    #[test]
    fn test_detect_prefers_first_registered() {
        let mut reg = FormatRegistry::new();
        reg.register(Box::new(MockFormatPlugin));
        reg.register(Box::new(MockFormatPlugin2));
        let detected = reg.detect(Path::new("game.mock")).unwrap();
        assert_eq!(detected.id(), "mock");
    }

    fn tempdir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("locust_test_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    // ─── MultiLangInjector tests ────────────────────────────

    use crate::backup::BackupManager;
    use crate::database::Database;
    use std::sync::Arc;
    use tokio::sync::mpsc;

    fn make_game_dir() -> PathBuf {
        let dir = tempdir().join("mygame");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("game.mock"), "").unwrap();
        fs::write(dir.join("image.png"), "fake png data").unwrap();
        dir
    }

    fn setup_injector() -> (MultiLangInjector, PathBuf, PathBuf) {
        let game_dir = make_game_dir();
        let backup_root = tempdir().join("backups");
        let output_dir = tempdir().join("output");
        fs::create_dir_all(&output_dir).unwrap();

        let db = Arc::new(Database::open_in_memory().unwrap());
        let backup = Arc::new(BackupManager::new(backup_root.clone()));

        // Save some entries with translations
        let mut entries = vec![
            StringEntry::new("mock#0", "Hello", PathBuf::from("game.mock")),
            StringEntry::new("mock#1", "World", PathBuf::from("game.mock")),
            StringEntry::new("mock#2", "Test", PathBuf::from("game.mock")),
        ];
        for e in &mut entries {
            e.translation = Some(format!("[translated] {}", e.source));
        }
        db.save_entries(&entries).unwrap();

        let mut registry = FormatRegistry::new();
        registry.register(Box::new(MockFormatPlugin));

        let injector = MultiLangInjector::new(Arc::new(registry), db, backup);
        (injector, game_dir, output_dir)
    }

    #[tokio::test]
    async fn test_replace_single_language() {
        let (injector, game_dir, output_dir) = setup_injector();
        let (tx, mut rx) = mpsc::channel(100);

        let report = injector
            .inject(
                &game_dir,
                "mock",
                OutputMode::Replace,
                vec!["es".to_string()],
                Some(output_dir.clone()),
                tx,
            )
            .await
            .unwrap();

        rx.close();
        while rx.recv().await.is_some() {}

        assert_eq!(report.languages_processed, vec!["es"]);
        let dest = output_dir.join("mygame-es");
        assert!(dest.exists());
        assert!(dest.join("game.mock").exists());
    }

    #[tokio::test]
    async fn test_replace_multi_language() {
        let (injector, game_dir, output_dir) = setup_injector();
        let (tx, mut rx) = mpsc::channel(100);

        let report = injector
            .inject(
                &game_dir,
                "mock",
                OutputMode::Replace,
                vec!["es".to_string(), "fr".to_string(), "de".to_string()],
                Some(output_dir.clone()),
                tx,
            )
            .await
            .unwrap();

        rx.close();
        while rx.recv().await.is_some() {}

        assert_eq!(report.languages_processed.len(), 3);
        assert!(output_dir.join("mygame-es").exists());
        assert!(output_dir.join("mygame-fr").exists());
        assert!(output_dir.join("mygame-de").exists());
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn test_replace_copies_on_windows() {
        let (injector, game_dir, output_dir) = setup_injector();
        let (tx, mut rx) = mpsc::channel(100);

        let report = injector
            .inject(
                &game_dir,
                "mock",
                OutputMode::Replace,
                vec!["es".to_string()],
                Some(output_dir.clone()),
                tx,
            )
            .await
            .unwrap();

        rx.close();
        while rx.recv().await.is_some() {}

        assert_eq!(report.languages_processed.len(), 1);
        let png = output_dir.join("mygame-es").join("image.png");
        assert!(png.exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_replace_uses_hardlinks_on_unix() {
        let (injector, game_dir, output_dir) = setup_injector();
        let (tx, mut rx) = mpsc::channel(100);

        injector
            .inject(
                &game_dir,
                "mock",
                OutputMode::Replace,
                vec!["es".to_string()],
                Some(output_dir.clone()),
                tx,
            )
            .await
            .unwrap();

        rx.close();
        while rx.recv().await.is_some() {}

        let png = output_dir.join("mygame-es").join("image.png");
        let meta = fs::metadata(&png).unwrap();
        assert!(meta.nlink() > 1);
    }

    #[tokio::test]
    async fn test_add_single_language() {
        let (injector, game_dir, _output_dir) = setup_injector();
        let (tx, mut rx) = mpsc::channel(100);

        let report = injector
            .inject(
                &game_dir,
                "mock",
                OutputMode::Add,
                vec!["fr".to_string()],
                None,
                tx,
            )
            .await
            .unwrap();

        rx.close();
        while rx.recv().await.is_some() {}

        assert_eq!(report.languages_processed, vec!["fr"]);
        assert!(game_dir.join("tl").join("fr").exists());
    }

    #[tokio::test]
    async fn test_add_multi_language() {
        let (injector, game_dir, _output_dir) = setup_injector();
        let (tx, mut rx) = mpsc::channel(100);

        let report = injector
            .inject(
                &game_dir,
                "mock",
                OutputMode::Add,
                vec!["fr".to_string(), "de".to_string()],
                None,
                tx,
            )
            .await
            .unwrap();

        rx.close();
        while rx.recv().await.is_some() {}

        assert_eq!(report.languages_processed.len(), 2);
        assert!(game_dir.join("tl").join("fr").exists());
        assert!(game_dir.join("tl").join("de").exists());
    }

    #[tokio::test]
    async fn test_backup_created_before_inject() {
        let (injector, game_dir, output_dir) = setup_injector();
        let (tx, mut rx) = mpsc::channel(100);

        injector
            .inject(
                &game_dir,
                "mock",
                OutputMode::Replace,
                vec!["es".to_string()],
                Some(output_dir),
                tx,
            )
            .await
            .unwrap();

        rx.close();
        while rx.recv().await.is_some() {}

        let backups = injector.backup_manager.list_backups().unwrap();
        assert!(!backups.is_empty());
    }

    #[tokio::test]
    async fn test_failed_language_continues() {
        // Use a plugin that fails for one specific call
        let game_dir = make_game_dir();
        let backup_root = tempdir().join("backups");

        let db = Arc::new(Database::open_in_memory().unwrap());
        let backup = Arc::new(BackupManager::new(backup_root));

        let entries = vec![StringEntry::new("mock#0", "Hello", PathBuf::from("game.mock"))];
        db.save_entries(&entries).unwrap();

        // Register a plugin where inject_add fails for "bad" lang
        struct FailOnBadLang;
        impl FormatPlugin for FailOnBadLang {
            fn id(&self) -> &str { "failmock" }
            fn name(&self) -> &str { "Fail Mock" }
            fn supported_extensions(&self) -> &[&str] { &[".mock"] }
            fn supported_modes(&self) -> Vec<OutputMode> { vec![OutputMode::Add] }
            fn extract(&self, _: &Path) -> Result<Vec<StringEntry>> { Ok(vec![]) }
            fn inject(&self, _: &Path, _: &[StringEntry]) -> Result<InjectionReport> {
                Ok(InjectionReport { files_modified: 0, strings_written: 0, strings_skipped: 0, warnings: vec![] })
            }
            fn inject_add(&self, _path: &Path, lang: &str, _entries: &[StringEntry]) -> Result<InjectionReport> {
                if lang == "bad" {
                    return Err(LocustError::InjectionError("bad language".to_string()));
                }
                fs::create_dir_all(_path.join("tl").join(lang))?;
                Ok(InjectionReport { files_modified: 1, strings_written: 1, strings_skipped: 0, warnings: vec![] })
            }
        }

        let mut registry = FormatRegistry::new();
        registry.register(Box::new(FailOnBadLang));
        let injector = MultiLangInjector::new(Arc::new(registry), db, backup);

        let (tx, mut rx) = mpsc::channel(100);

        let report = injector
            .inject(
                &game_dir,
                "failmock",
                OutputMode::Add,
                vec!["good".to_string(), "bad".to_string(), "also_good".to_string()],
                None,
                tx,
            )
            .await
            .unwrap();

        rx.close();
        while rx.recv().await.is_some() {}

        assert_eq!(report.languages_processed.len(), 2);
        assert_eq!(report.languages_failed.len(), 1);
        assert_eq!(report.languages_failed[0].0, "bad");
    }

    #[tokio::test]
    async fn test_multilang_report_structure() {
        let (injector, game_dir, output_dir) = setup_injector();
        let (tx, mut rx) = mpsc::channel(100);

        let report = injector
            .inject(
                &game_dir,
                "mock",
                OutputMode::Replace,
                vec!["es".to_string(), "fr".to_string()],
                Some(output_dir),
                tx,
            )
            .await
            .unwrap();

        rx.close();
        while rx.recv().await.is_some() {}

        assert_eq!(report.mode, OutputMode::Replace);
        assert_eq!(report.languages_processed.len(), 2);
        assert!(report.languages_failed.is_empty());
        assert!(!report.backup_id.is_empty());
        assert!(report.reports.contains_key("es"));
        assert!(report.reports.contains_key("fr"));
    }
}
