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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum FormatStability {
    /// Extensively tested and reliable.
    Stable,
    /// Works but has known edge cases or limited testing.
    Experimental,
    /// Not yet functional — shown as "coming soon" in the UI.
    ComingSoon,
}

impl FormatStability {
    /// Wire / JSON value (`stable` | `experimental` | `comingsoon`).
    pub fn as_api_str(self) -> &'static str {
        match self {
            Self::Stable => "stable",
            Self::Experimental => "experimental",
            Self::ComingSoon => "comingsoon",
        }
    }

    /// Human label for CLI tables and logs.
    pub fn label(self) -> &'static str {
        match self {
            Self::Stable => "stable",
            Self::Experimental => "experimental",
            Self::ComingSoon => "coming soon",
        }
    }

    /// Sort key: usable formats first (stable → experimental → coming soon).
    pub fn rank(self) -> u8 {
        match self {
            Self::Stable => 0,
            Self::Experimental => 1,
            Self::ComingSoon => 2,
        }
    }
}

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
    /// Indicates how reliable this format plugin is for production use.
    /// The UI uses this to label/hide formats appropriately.
    fn stability(&self) -> FormatStability {
        FormatStability::Stable
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
    /// Paths of every file this injection actually wrote. `locust patch`
    /// packs from this list (persisted per language in the project database)
    /// because entries only name where text was READ: for archive-based
    /// engines that diverges — Ren'Py rewrites `file_path` to the `.rpa`
    /// while injection writes loose `.rpy` files plus a generated
    /// `zzz_locust_translate.rpy` that extraction deliberately skips, so the
    /// written files can never become database entries.
    ///
    /// `serde(default)` keeps reports from older WASM plugins deserializable;
    /// they simply record nothing.
    #[serde(default)]
    pub files_written: Vec<PathBuf>,
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
        let mut out: Vec<PluginInfo> = self
            .plugins
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
                stability: p.stability(),
            })
            .collect();

        // No display-only light-novel stub: TyranoBuilder, NScripter, KiriKiri,
        // and YU-RIS are real plugins in `locust-formats` (Experimental). Leftover
        // work is archive/engine-adjacent leftovers (cxdec, exotic YPF schemes, asar, NSA, …).

        // Usable engines first: stable → experimental → coming soon, then id.
        out.sort_by(|a, b| {
            a.stability
                .rank()
                .cmp(&b.stability.rank())
                .then_with(|| a.id.cmp(&b.id))
        });

        out
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
    #[serde(default = "default_stability")]
    pub stability: FormatStability,
}

fn default_stability() -> FormatStability {
    FormatStability::Stable
}

// ─── Multi-language injection pipeline ──────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct MultiLangReport {
    pub mode: OutputMode,
    pub languages_processed: Vec<String>,
    pub languages_failed: Vec<(String, String)>,
    pub backup_id: String,
    pub reports: HashMap<String, InjectionReport>,
    /// The root each language's injection targeted: the per-language copy in
    /// Replace mode, the game path itself in Add mode. The recording `locust
    /// patch` packs from is keyed on this root — only the injector knows
    /// which tree it wrote into, so it must say so per language.
    pub injected_roots: HashMap<String, PathBuf>,
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
        // Always back up. This used to skip the backup for Replace mode with an
        // output_dir on the assumption that "the original is untouched", and that
        // assumption is false: Unity (unity.rs), Unreal (unreal.rs) and Wolf RPG
        // (wolf_rpg.rs) key their injection on `entry.file_path` and write straight
        // back to the ORIGINAL file, and Ren'Py does the same for loose scripts —
        // they never write into the copy at all. So the one mode users reach for
        // BECAUSE it is meant to be safe was the mode that mutated their game with
        // no way back.
        //
        // ponytail: unconditional backup costs a full copy on every Replace run.
        // Narrow it again only once each plugin provably writes inside the root it
        // is handed, and prove that with a containment check rather than a comment.
        //
        // A FAILED backup is fatal, for the same reason the backup is
        // unconditional: the entry-tree writers mutate the original game in
        // every mode, and every recovery path this pipeline advises
        // ("restore it from the backup listed above") starts at the backup.
        // Continuing with backup_id "none" made those remedies hollow
        // exactly when they were needed.
        let backup_id = self
            .backup_manager
            .create_backup(project_path)
            .map(|backup| backup.id)
            .map_err(|e| {
                LocustError::BackupError(format!(
                    "{e} — injection is refused without a backup: several engines \
                     write translations into the ORIGINAL game files, and recovering \
                     from a bad injection starts at this backup. Free up disk space \
                     or fix the backup directory, then re-run."
                ))
            })?;

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
        let mut injected_roots = HashMap::new();

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

            emit_binary_slot_preflight(&entries, &tx).await;

            match plugin.inject(&dest, &entries) {
                Ok(report) => {
                    reports.insert(lang.clone(), report);
                    injected_roots.insert(lang.clone(), dest.clone());
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
            injected_roots,
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
        let mut injected_roots = HashMap::new();

        for (idx, lang) in languages.iter().enumerate() {
            let entries = self
                .db
                .get_entries(&crate::database::EntryFilter::default())?;

            emit_binary_slot_preflight(&entries, &tx).await;

            match plugin.inject_add(project_path, lang, &entries) {
                Ok(report) => {
                    reports.insert(lang.clone(), report);
                    injected_roots.insert(lang.clone(), project_path.to_path_buf());
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
            injected_roots,
        })
    }
}

/// Warn (trace + progress event) when translations exceed tagged binary inject
/// slots so Unity/Unreal/Wolf injects do not silently skip oversize strings
/// without a client-visible signal. CLI `inject --direct` also prints a count.
async fn emit_binary_slot_preflight(
    entries: &[crate::models::StringEntry],
    tx: &mpsc::Sender<ProgressEvent>,
) {
    let issues = crate::validation::binary_slot_oversize_issues(entries);
    if issues.is_empty() {
        return;
    }
    tracing::warn!(
        count = issues.len(),
        "translations exceed binary inject slot length (UTF-8 / UTF-16LE / Shift-JIS); \
         engine will skip them — run locust validate for entry IDs"
    );
    let _ = tx
        .send(ProgressEvent::ValidationFailed { issues })
        .await;
}

/// What [`record_injection_for_lang`] did for one language key.
#[derive(Debug, PartialEq, Eq)]
pub enum RecordOutcome {
    /// The written files were containment-checked and persisted.
    Recorded { files: usize },
    /// Zero files written; the previous recording for this key (dated
    /// `recorded_at`) was kept — its files are still on disk, and the
    /// pack-time hash check catches real staleness. Callers MUST surface
    /// this: silent keeps were the stale-recording hazard.
    KeptPrevious { recorded_at: String },
    /// Zero files written and no previous recording exists — `locust patch`
    /// will refuse until an inject writes at least one file. Callers MUST
    /// tell the user why nothing was recorded and name a remedy that fits
    /// (skipped translations, or an already-mutated original tree).
    NothingRecorded,
}

/// Containment-check `files` against `root`, then persist the recording for
/// `lang` (`None` = the reserved language-unspecified key). Any file outside
/// the root records NOTHING for the language and fails loudly with `remedy` —
/// recording it would point `locust patch` at a tree injection never targeted,
/// which is exactly how patches silently shipped original files.
pub fn record_injection_for_lang(
    db: &Database,
    lang: Option<&str>,
    root: &Path,
    files: &[PathBuf],
    remedy: &str,
    backup_id: Option<&str>,
) -> Result<RecordOutcome> {
    use crate::database::rel_under_root;
    let root_abs = std::path::absolute(root)?;
    let label = lang.unwrap_or("(unspecified)");
    let outside: Vec<&PathBuf> = files
        .iter()
        .filter(|p| rel_under_root(p, &root_abs).is_none())
        .collect();
    if !outside.is_empty() {
        let shown: Vec<String> = outside
            .iter()
            .take(3)
            .map(|p| format!("  {}", p.display()))
            .collect();
        let more = if outside.len() > shown.len() {
            format!("\n  ... and {} more", outside.len() - shown.len())
        } else {
            String::new()
        };
        let backup_note = match backup_id {
            Some(id) if id != "none" => {
                format!("\nBackup {id} holds the pre-injection files.")
            }
            _ => String::new(),
        };
        return Err(LocustError::InjectionError(format!(
            "injection for language \"{label}\" wrote {} file(s) OUTSIDE its target \
             root \"{}\":\n{}{more}\nNothing was recorded for \"{label}\" — the output \
             at \"{}\" does not contain these translations.{backup_note}\n{remedy}",
            outside.len(),
            root_abs.display(),
            shown.join("\n"),
            root_abs.display(),
        )));
    }
    if files.is_empty() {
        return Ok(match db.get_injection(lang)? {
            Some(prev) => RecordOutcome::KeptPrevious {
                recorded_at: prev.recorded_at,
            },
            None => RecordOutcome::NothingRecorded,
        });
    }
    db.record_injection(lang, &root_abs, files)?;
    Ok(RecordOutcome::Recorded { files: files.len() })
}

/// Persist, for EVERY language `report` successfully injected, the files that
/// injection actually wrote under the root it wrote them into. This is THE
/// mandatory companion to [`MultiLangInjector::inject`]: every caller — the
/// CLI, the HTTP server, the desktop app — must record through here, or
/// `locust patch` on that project hard-errors with "no injection has been
/// recorded". `remedy(lang)` supplies the caller's unblocking advice, appended
/// to a containment hard-error. Languages are visited in `languages` order so
/// a failure reports the same way every run; the returned outcomes let the
/// caller surface zero-write runs (see [`RecordOutcome`]).
pub fn record_multilang_injection(
    db: &Database,
    report: &MultiLangReport,
    languages: &[String],
    remedy: &dyn Fn(&str) -> String,
) -> Result<Vec<(String, RecordOutcome)>> {
    let mut outcomes = Vec::new();
    for lang in languages {
        let Some(rep) = report.reports.get(lang) else {
            continue; // failed language — the caller reports it, nothing to record
        };
        let root = report.injected_roots.get(lang).ok_or_else(|| {
            LocustError::InjectionError(format!(
                "internal error: no injection root reported for language \"{lang}\""
            ))
        })?;
        let outcome = record_injection_for_lang(
            db,
            Some(lang),
            root,
            &rep.files_written,
            &remedy(lang),
            Some(&report.backup_id),
        )?;
        outcomes.push((lang.clone(), outcome));
    }
    Ok(outcomes)
}

fn copy_dir_for_inject(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst)?;

    // Canonicalize dst so we can skip it during the walk — prevents infinite
    // recursion when dst is a subdirectory of src (e.g. inject output placed
    // next to the game inside the same parent folder).
    let dst_canon = dst.canonicalize().ok();

    let media_extensions = ["png", "ogg", "wav", "m4a", "mp4", "jpg", "jpeg", "bmp", "mp3"];

    for entry in WalkDir::new(src).follow_links(false).into_iter().filter_entry(|e| {
        // Skip the destination directory itself to avoid recursive copy loops
        if let Some(ref dc) = dst_canon {
            if let Ok(entry_canon) = e.path().canonicalize() {
                if entry_canon == *dc {
                    return false;
                }
            }
        }
        true
    }) {
        let entry = entry.map_err(|e| LocustError::IoError(std::io::Error::other(e)))?;
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
                // Try hardlink first (works on all platforms), fall back to copy
                if std::fs::hard_link(entry.path(), &dest).is_err() {
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

    #[test]
    fn format_stability_labels_and_rank() {
        assert_eq!(FormatStability::Stable.as_api_str(), "stable");
        assert_eq!(FormatStability::Experimental.as_api_str(), "experimental");
        assert_eq!(FormatStability::ComingSoon.as_api_str(), "comingsoon");
        assert_eq!(FormatStability::ComingSoon.label(), "coming soon");
        assert!(FormatStability::Stable.rank() < FormatStability::Experimental.rank());
        assert!(FormatStability::Experimental.rank() < FormatStability::ComingSoon.rank());
    }

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
                files_written: vec![out_path],
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
                files_written: vec![out_path],
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
                files_written: Vec::new(),
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
        // Registered plugins only — light-novel ComingSoon stub removed (Tyrano is real).
        // QSP is a real formats crate plugin, not a core stub.
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, "mock");
        assert!(list.iter().all(|p| p.id != "light-novel"));
        assert!(list.iter().all(|p| p.id != "qsp"));
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
        // The injector is the only party that knows which tree it targeted;
        // the recording `locust patch` packs from is keyed on this root.
        assert_eq!(
            report.injected_roots.get("es"),
            Some(&dest),
            "Replace mode must report the per-language copy as the injected root"
        );
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
        assert_eq!(
            report.injected_roots.get("fr"),
            Some(&game_dir),
            "Add mode must report the game path itself as the injected root"
        );
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

        // Replace mode with output_dir skips backup (original untouched)
        let report = injector
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

        // Replace-with-output_dir MUST still take a backup. Several plugins (Unity,
        // Unreal, Wolf RPG, and Ren'Py for loose scripts) write to entry.file_path
        // and so mutate the ORIGINAL game even in this mode. This assertion used to
        // require the sentinel "skip-replace-mode", which locked the unsafe
        // assumption in as the expected contract.
        assert_ne!(
            report.backup_id, "skip-replace-mode",
            "Replace mode with an output_dir must not skip the backup"
        );
        assert_ne!(
            report.backup_id, "none",
            "a backup must have been created, not merely attempted"
        );
    }

    #[tokio::test]
    async fn test_backup_failure_is_fatal_and_nothing_is_injected() {
        // Every recovery path this pipeline advises ("restore it from the
        // backup listed above") starts at the backup, and several engines
        // mutate the ORIGINAL game files in every mode. Continuing with
        // backup_id "none" after a failed backup made those remedies hollow
        // exactly when they were needed.
        let game_dir = make_game_dir();
        let output_dir = tempdir().join("output");
        fs::create_dir_all(&output_dir).unwrap();
        // A backup root that is a FILE: create_backup cannot create its
        // timestamp directory under it, on any platform.
        let bad_backup_root = tempdir().join("not_a_dir");
        fs::write(&bad_backup_root, b"occupied").unwrap();

        let db = Arc::new(Database::open_in_memory().unwrap());
        let mut entries = vec![StringEntry::new("mock#0", "Hello", PathBuf::from("game.mock"))];
        entries[0].translation = Some("Hola".to_string());
        db.save_entries(&entries).unwrap();
        let mut registry = FormatRegistry::new();
        registry.register(Box::new(MockFormatPlugin));
        let injector = MultiLangInjector::new(
            Arc::new(registry),
            db,
            Arc::new(BackupManager::new(bad_backup_root)),
        );

        let (tx, mut rx) = mpsc::channel(100);
        let err = injector
            .inject(
                &game_dir,
                "mock",
                OutputMode::Replace,
                vec!["es".to_string()],
                Some(output_dir.clone()),
                tx,
            )
            .await
            .expect_err("a failed backup must refuse the injection, not shrug");
        rx.close();
        while rx.recv().await.is_some() {}

        assert!(
            err.to_string().contains("without a backup"),
            "the refusal must say why the backup matters: {err}"
        );
        assert!(
            !output_dir.join("mygame-es").exists(),
            "nothing may be copied or injected after the backup failed"
        );
    }

    #[tokio::test]
    async fn test_backup_created_for_add_mode() {
        let (injector, game_dir, _output_dir) = setup_injector();
        let (tx, mut rx) = mpsc::channel(100);

        // Add mode should create a real backup
        injector
            .inject(
                &game_dir,
                "mock",
                OutputMode::Add,
                vec!["es".to_string()],
                None,
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
                Ok(InjectionReport { files_modified: 0, strings_written: 0, strings_skipped: 0, warnings: vec![], files_written: vec![] })
            }
            fn inject_add(&self, _path: &Path, lang: &str, _entries: &[StringEntry]) -> Result<InjectionReport> {
                if lang == "bad" {
                    return Err(LocustError::InjectionError("bad language".to_string()));
                }
                fs::create_dir_all(_path.join("tl").join(lang))?;
                Ok(InjectionReport { files_modified: 1, strings_written: 1, strings_skipped: 0, warnings: vec![], files_written: vec![] })
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

    // ─── Recording seam tests: record_multilang_injection is the mandatory
    // companion to MultiLangInjector::inject for EVERY caller ───────────────

    /// A plugin that writes INSIDE the tree it is handed — the containment
    /// check must pass and the write must be recorded under that root.
    struct ContainedMock;

    impl FormatPlugin for ContainedMock {
        fn id(&self) -> &str {
            "contained"
        }
        fn name(&self) -> &str {
            "Contained Mock"
        }
        fn supported_extensions(&self) -> &[&str] {
            &[".mock"]
        }
        fn supported_modes(&self) -> Vec<OutputMode> {
            vec![OutputMode::Replace]
        }
        fn extract(&self, _path: &Path) -> Result<Vec<StringEntry>> {
            Ok(vec![])
        }
        fn inject(&self, path: &Path, entries: &[StringEntry]) -> Result<InjectionReport> {
            let out = path.join("game.injected");
            let lines: Vec<String> = entries
                .iter()
                .filter_map(|e| e.translation.as_ref().map(|t| format!("{}={}", e.id, t)))
                .collect();
            fs::write(&out, lines.join("\n"))?;
            Ok(InjectionReport {
                files_modified: 1,
                strings_written: lines.len(),
                strings_skipped: 0,
                warnings: Vec::new(),
                files_written: vec![out],
            })
        }
    }

    fn setup_contained_injector() -> (MultiLangInjector, PathBuf, PathBuf) {
        let game_dir = make_game_dir();
        let backup_root = tempdir().join("backups");
        let output_dir = tempdir().join("output");
        fs::create_dir_all(&output_dir).unwrap();

        let db = Arc::new(Database::open_in_memory().unwrap());
        let backup = Arc::new(BackupManager::new(backup_root));
        let mut entries = vec![StringEntry::new("mock#0", "Hello", PathBuf::from("game.mock"))];
        entries[0].translation = Some("Hola".to_string());
        db.save_entries(&entries).unwrap();

        let mut registry = FormatRegistry::new();
        registry.register(Box::new(ContainedMock));
        let injector = MultiLangInjector::new(Arc::new(registry), db, backup);
        (injector, game_dir, output_dir)
    }

    #[tokio::test]
    async fn test_record_multilang_injection_records_every_processed_language() {
        let (injector, game_dir, output_dir) = setup_contained_injector();
        let (tx, mut rx) = mpsc::channel(100);
        let langs = vec!["es".to_string(), "fr".to_string()];
        let report = injector
            .inject(
                &game_dir,
                "contained",
                OutputMode::Replace,
                langs.clone(),
                Some(output_dir.clone()),
                tx,
            )
            .await
            .unwrap();
        rx.close();
        while rx.recv().await.is_some() {}

        let outcomes =
            record_multilang_injection(&injector.db, &report, &langs, &|_| "remedy".to_string())
                .unwrap();
        assert_eq!(
            outcomes,
            vec![
                ("es".to_string(), RecordOutcome::Recorded { files: 1 }),
                ("fr".to_string(), RecordOutcome::Recorded { files: 1 }),
            ]
        );

        for lang in ["es", "fr"] {
            let rec = injector
                .db
                .get_injection(Some(lang))
                .unwrap()
                .unwrap_or_else(|| panic!("a recording must exist for {lang}"));
            assert!(
                crate::database::paths_identical(
                    &rec.root,
                    &output_dir.join(format!("mygame-{lang}"))
                ),
                "the recording root must be the per-language copy, got {}",
                rec.root.display()
            );
            assert_eq!(rec.files.len(), 1);
            assert_eq!(rec.files[0].rel, "game.injected");
        }
    }

    #[tokio::test]
    async fn test_record_multilang_injection_containment_failure_is_loud_and_records_nothing() {
        // MockFormatPlugin writes `<dest>.injected` — a SIBLING of the
        // per-language copy, outside its root. Recording must hard-fail with
        // the caller's remedy attached and persist nothing for that language.
        let (injector, game_dir, output_dir) = setup_injector();
        let (tx, mut rx) = mpsc::channel(100);
        let langs = vec!["es".to_string()];
        let report = injector
            .inject(
                &game_dir,
                "mock",
                OutputMode::Replace,
                langs.clone(),
                Some(output_dir),
                tx,
            )
            .await
            .unwrap();
        rx.close();
        while rx.recv().await.is_some() {}

        let err = record_multilang_injection(&injector.db, &report, &langs, &|l| {
            format!("REMEDY-FOR-{l}")
        })
        .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("OUTSIDE its target root"),
            "the containment violation must be loud: {msg}"
        );
        assert!(
            msg.contains("Nothing was recorded"),
            "the caller must know no recording exists: {msg}"
        );
        assert!(
            msg.contains("REMEDY-FOR-es"),
            "the caller's remedy must be attached verbatim: {msg}"
        );
        assert!(
            msg.contains(&format!("Backup {}", report.backup_id)),
            "the backup that holds the pre-injection files must be named: {msg}"
        );
        assert!(
            injector.db.get_injection(Some("es")).unwrap().is_none(),
            "a containment failure must record NOTHING"
        );
    }

    #[test]
    fn test_record_injection_for_lang_zero_write_outcomes() {
        let db = Database::open_in_memory().unwrap();
        let root = tempdir();
        let file = root.join("data.bin");
        fs::write(&file, b"translated bytes").unwrap();

        // First-ever zero-write run: nothing to keep, nothing recorded.
        let outcome =
            record_injection_for_lang(&db, Some("es"), &root, &[], "remedy", None).unwrap();
        assert_eq!(outcome, RecordOutcome::NothingRecorded);
        assert!(db.get_injection(Some("es")).unwrap().is_none());

        // A real write records.
        let outcome =
            record_injection_for_lang(&db, Some("es"), &root, &[file], "remedy", None).unwrap();
        assert_eq!(outcome, RecordOutcome::Recorded { files: 1 });

        // A later zero-write run keeps the previous recording, visibly.
        let prev = db.get_injection(Some("es")).unwrap().unwrap();
        let outcome =
            record_injection_for_lang(&db, Some("es"), &root, &[], "remedy", None).unwrap();
        assert_eq!(
            outcome,
            RecordOutcome::KeptPrevious {
                recorded_at: prev.recorded_at.clone()
            }
        );
        assert!(
            db.get_injection(Some("es")).unwrap().is_some(),
            "a zero-write run must not clobber the last good recording"
        );
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
        // Every processed language carries the root its injection targeted.
        assert!(report.injected_roots.contains_key("es"));
        assert!(report.injected_roots.contains_key("fr"));
    }
}
