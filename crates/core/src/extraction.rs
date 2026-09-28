use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use walkdir::WalkDir;

use crate::backup::{BackupManager, RevisionOriginal};
use crate::database::{Database, EntryFilter, RecordedBackup};
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
            self.supported_extensions().iter().any(|supported| {
                let s = supported.strip_prefix('.').unwrap_or(supported);
                s.to_lowercase() == ext_lower
            })
        } else {
            false
        }
    }

    fn extract(&self, path: &Path) -> Result<Vec<StringEntry>>;

    fn inject(&self, path: &Path, entries: &[StringEntry]) -> Result<InjectionReport>;

    /// Retarget old source locators to a verified previous Direct result.
    /// Keys are files in the private current-game copy; values are their exact
    /// original backup readers. Core verifies the prior result and backup first;
    /// readers verify the exact bytes consumed against that immutable snapshot.
    /// Implementations may change only these transient entries, never either
    /// tree. Unsupported/ambiguous locators retain normal source-match checks.
    fn prepare_revision_entries(
        &self,
        _entries: &mut [StringEntry],
        _originals: &HashMap<PathBuf, RevisionOriginal>,
    ) -> Result<()> {
        Ok(())
    }

    /// Inject while the caller retains exclusive access to this selection.
    /// Plugins which acquire their own game lock must override this method and
    /// reuse the validated guard instead of acquiring a second lock.
    fn inject_under_lock(
        &self,
        path: &Path,
        entries: &[StringEntry],
        game_lock: &crate::patch::GameLock,
    ) -> Result<InjectionReport> {
        game_lock.validate_selection(path)?;
        self.inject(path, entries)
    }

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InjectionReport {
    /// Counts by reason; older plugins may leave this empty. Consumers must
    /// present the remainder as unclassified, never infer it means too long.
    #[serde(default)]
    pub skip_reasons: std::collections::BTreeMap<String, usize>,
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

impl InjectionReport {
    pub fn skip(&mut self, reason: &str, count: usize) {
        self.strings_skipped += count;
        *self.skip_reasons.entry(reason.to_string()).or_default() += count;
    }

    pub fn classify_remaining_skips(&mut self) {
        let classified: usize = self.skip_reasons.values().sum();
        if classified < self.strings_skipped {
            *self.skip_reasons.entry("unclassified".into()).or_default() +=
                self.strings_skipped - classified;
        }
    }
}

/// Result of [`inject_direct`] — shape compatible with MultiLangReport UI fields
/// plus backup path for the desktop/CLI tables.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirectInjectReport {
    /// Always `"direct"`.
    pub mode: String,
    pub languages_processed: Vec<String>,
    pub languages_failed: Vec<(String, String)>,
    /// Retained backup, or empty when an unchanged run discarded its duplicate
    /// and there is no recorded original backup to show.
    pub backup_id: String,
    /// Backup associated with the files being packed, which can precede a no-op.
    #[serde(default)]
    pub pristine_backup_id: Option<String>,
    /// Absolute path of the retained backup. An unchanged run may show its
    /// previously recorded original, or None if it has none. A pre-write backup
    /// is still mandatory even when its redundant copy is discarded afterward.
    pub backup_path: Option<String>,
    pub files_modified: usize,
    pub strings_written: usize,
    pub strings_skipped: usize,
    pub warnings: Vec<String>,
    pub files_written: Vec<PathBuf>,
    /// Same injection stats under each language key (one inject pass).
    pub reports: HashMap<String, InjectionReport>,
    /// Per-key recording outcome — callers MUST surface the zero-write cases.
    pub outcomes: Vec<(String, RecordOutcome)>,
}

// Used only in the outside-root containment error: for direct mode that means
// the project db references files from a DIFFERENT copy of the game, so the
// unblocking advice is to inject against that copy — not to restore backups.
const DIRECT_RECORD_REMEDY: &str =
    "The project database references files outside this directory — it was likely \
     extracted from a DIFFERENT copy of the game (the one containing the file(s) \
     above). Run inject --direct against that game copy instead, with the same \
     -P project and language(s).";

const REPLACE_RECORD_REMEDY: &str =
    "The copied output does not contain these translations. Choose a new output \
     folder and re-run Replace with the same project database and language.";

#[cfg(test)]
std::thread_local! {
    static REPLACE_BEFORE_DEST_LOCK: std::cell::Cell<Option<fn(&Path)>> =
        const { std::cell::Cell::new(None) };
    static REPLACE_AFTER_DEST_LOCK: std::cell::Cell<Option<fn(&Path)>> =
        const { std::cell::Cell::new(None) };
    static DIRECT_REVISION_AFTER_VERIFY: std::cell::Cell<Option<fn(&Path)>> =
        const { std::cell::Cell::new(None) };
}

/// Inject translated strings **into the game tree in place**, create a backup
/// first, and record the injection for `locust patch` packing. Shared by CLI
/// `--direct`, HTTP, and the desktop app.
pub fn inject_direct(
    registry: &FormatRegistry,
    db: &Database,
    backup_manager: &BackupManager,
    game_path: &Path,
    format_id: &str,
    languages: &[String],
) -> Result<DirectInjectReport> {
    if languages.len() > 1 {
        return Err(LocustError::InjectionError(
            "one project database can inject only one target language per run; use a separate pivot/database for each target language"
                .into(),
        ));
    }
    let plugin = registry
        .get(format_id)
        .ok_or_else(|| LocustError::UnsupportedFormat(format!("format not found: {format_id}")))?;

    let entries = db.get_entries(&EntryFilter::default())?;
    let translated: Vec<_> = entries
        .into_iter()
        .filter(|e| e.translation.is_some())
        .collect();
    let translated = restore_physical_sources(entries_for_target(translated, game_path)?)?;
    let selected = game_path.canonicalize()?;
    let game_path = selected.as_path();

    let recording_root = if game_path.is_file() {
        game_path.parent().unwrap()
    } else {
        game_path
    };
    let (run, report, outcomes) = crate::injection_transaction::run_with_backup(
        game_path,
        format_id,
        languages.first().map(String::as_str),
        || prepare_direct_backup(db, backup_manager, game_path, recording_root, languages),
        |work, selected_copy, run| {
            let mut entries = entries_for_copy(translated, game_path, work)?;
            prepare_direct_revision(
                plugin,
                &mut entries,
                backup_manager,
                recording_root,
                work,
                run,
            )?;
            plugin.inject(selected_copy, &entries)
        },
        |report, run| {
            record_direct_injection(db, backup_manager, languages, recording_root, report, run)
        },
        |run, report, outcomes| {
            discard_direct_noop_backup(db, backup_manager, run, report, outcomes)
        },
    )?;
    let pristine_backup_id = db
        .get_injection(languages.first().map(String::as_str))?
        .filter(|record| crate::database::paths_identical(&record.root, recording_root))
        .and_then(|record| record.pristine_backup.map(|backup| backup.id));
    let (backup_id, backup_path) = match run.report_backup {
        Some((id, path)) => (id, Some(path.display().to_string())),
        None => (String::new(), None),
    };

    let lang_keys: Vec<String> = if languages.is_empty() {
        vec!["(unspecified)".into()]
    } else {
        languages.to_vec()
    };
    let mut reports = HashMap::new();
    for lang in &lang_keys {
        reports.insert(lang.clone(), report.clone());
    }

    Ok(DirectInjectReport {
        mode: "direct".into(),
        languages_processed: lang_keys,
        languages_failed: vec![],
        backup_id,
        pristine_backup_id,
        backup_path,
        files_modified: report.files_modified,
        strings_written: report.strings_written,
        strings_skipped: report.strings_skipped,
        warnings: report.warnings.clone(),
        files_written: report.files_written.clone(),
        reports,
        outcomes,
    })
}

struct DirectBackupRun {
    entry: crate::backup::BackupEntry,
    prior: Option<DirectPriorMerge>,
    report_backup: Option<(String, PathBuf)>,
}

/// Called only for a completed, empty transaction while GameLock remains held.
fn discard_direct_noop_backup(
    db: &Database,
    manager: &BackupManager,
    run: &mut DirectBackupRun,
    report: &mut InjectionReport,
    outcomes: &[(String, RecordOutcome)],
) -> Result<()> {
    if outcomes.is_empty()
        || !outcomes.iter().all(|(_, outcome)| {
            matches!(
                outcome,
                RecordOutcome::KeptPrevious { .. } | RecordOutcome::NothingRecorded
            )
        })
    {
        return Ok(());
    }
    // Never prune other generations or infer that another language cannot use
    // this one. The new ID must remain unreferenced even in this project.
    for lang in db.list_recorded_langs()? {
        if db
            .get_injection(lang.as_deref())?
            .and_then(|r| r.pristine_backup)
            .is_some_and(|backup| backup.id == run.entry.id)
        {
            report.warnings.push(
                "unchanged injection backup retained because a recording references it".into(),
            );
            return Ok(());
        }
    }
    manager
        .ensure_redundant_backup(&run.entry)
        .map_err(|error| {
            LocustError::InjectionError(format!(
                "{error}; backup retained at {}",
                run.entry.path.display()
            ))
        })?;
    let retained = run
        .prior
        .as_ref()
        .and_then(|prior| prior.provenance.as_ref());
    let retained = retained
        .map(|backup| {
            let root = backup.storage_root.as_deref().unwrap_or(manager.root());
            Ok::<_, LocustError>((
                backup.id.clone(),
                std::path::absolute(root.join(&backup.id))?,
            ))
        })
        .transpose()?;
    // Even a partial deletion failure must not advertise a damaged duplicate
    // as a usable backup. Pack keeps its separately recorded original ID.
    run.report_backup = retained;
    match manager.delete_backup(&run.entry.id) {
        Ok(()) => report.warnings.push("unchanged injection: redundant new backup removed".into()),
        Err(error) => report.warnings.push(format!(
            "unchanged injection completed, but duplicate backup cleanup was incomplete at {}: {error}; inspect that directory before manual cleanup",
            run.entry.path.display()
        )),
    }
    Ok(())
}

struct DirectPriorMerge {
    files: Vec<PathBuf>,
    provenance: Option<RecordedBackup>,
}

fn prepare_direct_revision(
    plugin: &dyn FormatPlugin,
    entries: &mut [StringEntry],
    backup_manager: &BackupManager,
    recording_root: &Path,
    work: &Path,
    run: &DirectBackupRun,
) -> Result<()> {
    let Some(prior) = &run.prior else {
        return Ok(());
    };
    let Some(provenance) = &prior.provenance else {
        return Ok(());
    };
    let manager = provenance
        .storage_root
        .as_ref()
        .map(|root| BackupManager::new(root.clone()));
    let work = work.canonicalize()?;
    manager
        .as_ref()
        .unwrap_or(backup_manager)
        .with_verified_pristine_tree(&provenance.id, &provenance.source_path, |pristine| {
            #[cfg(test)]
            DIRECT_REVISION_AFTER_VERIFY.with(|hook| {
                if let Some(hook) = hook.take() {
                    hook(pristine.root());
                }
            });
            let mut originals = HashMap::new();
            let entry_files: std::collections::HashSet<_> = entries
                .iter()
                .map(|entry| entry.file_path.clone())
                .collect();
            for file in &prior.files {
                let relative =
                    crate::database::rel_under_root(file, recording_root).ok_or_else(|| {
                        LocustError::InjectionError("prior file escaped recording root".into())
                    })?;
                let current = work.join(&relative);
                // Generated overlays have no original counterpart. Never use
                // another file as a guessed baseline for those outputs.
                if !entry_files.contains(&current) {
                    continue;
                }
                if let Some(original) = pristine.revision_original(Path::new(&relative))? {
                    originals.insert(current, original);
                }
            }
            plugin.prepare_revision_entries(entries, &originals)
        })
}

/// GameLock is already held by `injection_transaction::run_with_backup`. Read prior state, hash live
/// members, and validate O *before* creating this run's A backup or recovery
/// metadata so a drifted/missing generation cannot install new output.
fn prepare_direct_backup(
    db: &Database,
    backup_manager: &BackupManager,
    selection: &Path,
    recording_root: &Path,
    languages: &[String],
) -> Result<DirectBackupRun> {
    let prior = db.get_injection(languages.first().map(String::as_str))?;
    let merge = match prior {
        Some(record) if crate::database::paths_identical(&record.root, recording_root) => Some(
            prepare_same_root_direct_merge(backup_manager, selection, recording_root, &record)?,
        ),
        _ => None,
    };
    // Other projects and no-op recordings can still require older originals.
    // Automatic global pruning cannot establish that they are unreferenced.
    let entry = backup_manager.create_backup(selection).map_err(|e| {
        LocustError::BackupError(format!(
            "{e} — direct inject is refused without a backup. Free disk space or fix the backup directory, then re-run."
        ))
    })?;
    Ok(DirectBackupRun {
        report_backup: Some((entry.id.clone(), std::path::absolute(&entry.path)?)),
        entry,
        prior: merge,
    })
}

fn prepare_same_root_direct_merge(
    backup_manager: &BackupManager,
    selection: &Path,
    recording_root: &Path,
    prior: &crate::database::InjectionRecording,
) -> Result<DirectPriorMerge> {
    refuse_incompatible_direct_scope(selection, prior)?;
    let mut files = Vec::with_capacity(prior.files.len());
    for recorded in &prior.files {
        files.push(verify_recorded_injection_member(recording_root, recorded)?);
    }
    if let Some(provenance) = &prior.pristine_backup {
        validate_recorded_pristine_backup(backup_manager, provenance)?;
    }
    Ok(DirectPriorMerge {
        files,
        provenance: prior.pristine_backup.clone(),
    })
}

/// A selected file and a leftover folder inventory share the parent
/// `recording_root`. Hashing those siblings would attribute a foreign file to
/// this invocation; refuse the incompatible scope instead of guessing.
fn refuse_incompatible_direct_scope(
    selection: &Path,
    prior: &crate::database::InjectionRecording,
) -> Result<()> {
    if !selection.is_file() {
        return Ok(());
    }
    let allowed = selection
        .file_name()
        .ok_or_else(|| LocustError::InjectionError("selected game file has no name".into()))?;
    let foreign: Vec<&str> = prior
        .files
        .iter()
        .filter(|file| {
            Path::new(&file.rel)
                .components()
                .next()
                .map(|component| component.as_os_str())
                != Some(allowed)
        })
        .map(|file| file.rel.as_str())
        .collect();
    if foreign.is_empty() {
        return Ok(());
    }
    Err(LocustError::InjectionError(format!(
        "previous same-root Direct recording includes {} file(s) outside the selected file \"{}\" (e.g. {}). Refusing before hashing a sibling or foreign path; inject the folder that owns that inventory, or start a new generation at a different root.",
        foreign.len(),
        allowed.to_string_lossy(),
        foreign.iter().take(3).copied().collect::<Vec<_>>().join(", "),
    )))
}

fn verify_recorded_injection_member(
    recording_root: &Path,
    file: &crate::database::RecordedFile,
) -> Result<PathBuf> {
    let relative = Path::new(&file.rel);
    ensure_injection_path(recording_root, relative)?;
    let absolute = recording_root.join(relative);
    let Some(rel) = crate::database::rel_under_root(&absolute, recording_root) else {
        return Err(LocustError::InjectionError(format!(
            "previously injected file \"{}\" is not under this recording root",
            file.rel
        )));
    };
    if rel != file.rel {
        return Err(LocustError::InjectionError(format!(
            "previously injected file \"{}\" does not round-trip under this recording root",
            file.rel
        )));
    }
    match crate::database::sha256_file(&absolute) {
        Ok((hash, size)) if hash == file.hash && size == file.size => Ok(absolute),
        Ok(_) => Err(LocustError::InjectionError(format!(
            "previously injected file \"{}\" no longer matches its recorded hash/size; refusing to install new output until that file is restored",
            file.rel
        ))),
        Err(error) => Err(LocustError::InjectionError(format!(
            "previously injected file \"{}\" cannot be verified ({error}); refusing to install new output",
            file.rel
        ))),
    }
}

fn validate_recorded_pristine_backup(
    fallback: &BackupManager,
    provenance: &RecordedBackup,
) -> Result<()> {
    let stored = match &provenance.storage_root {
        Some(root) if !root.is_absolute() => {
            return Err(LocustError::InjectionError(
                "recorded backup store must be absolute".into(),
            ));
        }
        Some(root) => Some(BackupManager::new(root.clone())),
        None => None,
    };
    stored.as_ref().unwrap_or(fallback).with_pristine_tree(
        &provenance.id,
        &provenance.source_path,
        |_| Ok(()),
    )?;
    Ok(())
}

fn record_direct_injection(
    db: &Database,
    backup_manager: &BackupManager,
    languages: &[String],
    recording_root: &Path,
    report: &InjectionReport,
    run: &DirectBackupRun,
) -> Result<Vec<(String, RecordOutcome)>> {
    let lang = languages.first().map(String::as_str);
    let generation = RecordedBackup {
        id: run.entry.id.clone(),
        source_path: run.entry.source_path.clone(),
        storage_root: Some(std::path::absolute(backup_manager.root())?),
    };
    let (files, provenance): (Vec<PathBuf>, Option<&RecordedBackup>) =
        if report.files_written.is_empty() {
            (Vec::new(), None)
        } else if let Some(prior) = &run.prior {
            (
                union_recorded_paths(&report.files_written, &prior.files),
                prior.provenance.as_ref(),
            )
        } else {
            (report.files_written.clone(), Some(&generation))
        };
    let outcome = record_injection_for_lang_with_backup(
        db,
        lang,
        recording_root,
        &files,
        DIRECT_RECORD_REMEDY,
        Some(&run.entry.id),
        provenance,
    )?;
    Ok(vec![(lang.unwrap_or("(unspecified)").to_owned(), outcome)])
}

fn union_recorded_paths(written: &[PathBuf], prior: &[PathBuf]) -> Vec<PathBuf> {
    let mut files = written.to_vec();
    // Resolve each spelling once. Pairwise paths_identical calls repeatedly
    // canonicalized both sides, turning large partial injections into O(n²)
    // filesystem lookups. Preserve the order and existing written duplicates.
    let mut identities: std::collections::HashSet<_> = written
        .iter()
        .map(|path| crate::database::path_identity_key(path))
        .collect();
    for path in prior {
        let identity = crate::database::path_identity_key(path);
        // paths_identical deliberately does not equate empty identity keys.
        if identity.is_empty() || identities.insert(identity) {
            files.push(path.clone());
        }
    }
    files
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
        self.plugins
            .iter()
            .find(|p| p.detect(path))
            .map(|p| p.as_ref())
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
    /// Add and Replace record while their GameLock(s) are still held. Callers
    /// return these immutable outcomes instead of rehashing files after commit.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub recording_outcomes: HashMap<String, RecordOutcome>,
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
        if languages.len() != 1 {
            return Err(LocustError::InjectionError(
                "one project database can inject exactly one target language per run; use a separate pivot/database for each target language"
                    .into(),
            ));
        }
        let mut entries = self.db.get_entries(&EntryFilter::default())?;
        if mode == OutputMode::Add {
            entries = restore_physical_sources(entries_for_target(entries, project_path)?)?;
        } else {
            // Validate before backup/copy, retaining raw paths for warnings and
            // semantic sources for the physical-source checks after remapping.
            check_entries_for_target(&mut entries, project_path, false)?;
            for entry in &entries {
                validated_injection_source(entry)?;
            }
        }
        let plugin = self.registry.get(format_id).ok_or_else(|| {
            LocustError::UnsupportedFormat(format!("format not found: {}", format_id))
        })?;
        if mode == OutputMode::Add {
            return self
                .inject_add(project_path, plugin, languages, entries, tx)
                .await;
        }
        match mode {
            OutputMode::Replace => {
                self.inject_replace(
                    project_path,
                    plugin,
                    languages,
                    entries,
                    output_dir.ok_or_else(|| {
                        LocustError::InjectionError(
                            "output_dir is required for Replace mode".to_string(),
                        )
                    })?,
                    tx,
                )
                .await
            }
            OutputMode::Add => {
                unreachable!("Add mode is handled by the transaction above")
            }
        }
    }

    async fn inject_replace(
        &self,
        project_path: &Path,
        plugin: &dyn FormatPlugin,
        languages: Vec<String>,
        entries: Vec<StringEntry>,
        output_dir: PathBuf,
        tx: mpsc::Sender<ProgressEvent>,
    ) -> Result<MultiLangReport> {
        let selected = project_path.canonicalize()?;
        let source_root = if selected.is_file() {
            selected.parent().unwrap()
        } else {
            &selected
        };
        // Keep the source generation stable from recovery preflight through
        // backup, copy, plugin dispatch and atomic provenance recording.
        let source_lock = crate::patch::GameLock::acquire(source_root)?;
        crate::injection_transaction::ensure_no_pending_under_lock(&source_lock)?;
        let backup = self.backup_manager.create_backup(&selected).map_err(|e| {
            LocustError::BackupError(format!(
                "{e} — injection is refused without a backup for recovery. \
                 Free up disk space or fix the backup directory, then re-run."
            ))
        })?;
        let provenance = RecordedBackup {
            id: backup.id.clone(),
            source_path: backup.source_path.clone(),
            storage_root: Some(std::path::absolute(self.backup_manager.root())?),
        };
        let backup_id = backup.id.clone();
        let total = languages.len();
        let mut languages_processed = Vec::new();
        let mut languages_failed = Vec::new();
        let mut reports = HashMap::new();
        let mut injected_roots = HashMap::new();
        let mut recording_outcomes = HashMap::new();

        let game_name = project_path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        // inject accepts exactly one language, which consumes the loaded rows.
        for (idx, (lang, entries)) in languages.iter().zip([entries]).enumerate() {
            let dest = output_dir.join(format!("{}-{}", game_name, lang));

            let dest_lock = match copy_dir_for_inject_under_lock(&selected, &dest, &source_lock) {
                Ok(lock) => lock,
                Err(e) => {
                    languages_failed.push((lang.clone(), e.to_string()));
                    continue;
                }
            };

            emit_binary_slot_preflight(&entries, &tx).await;

            // Plugins such as Unity write entry.file_path directly. Remap
            // BEFORE dispatch, not after writes when recording containment.
            let entries = match entries_for_copy(entries, project_path, &dest) {
                Ok(entries) => match restore_physical_sources(entries) {
                    Ok(entries) => entries,
                    Err(error) => {
                        languages_failed.push((lang.clone(), error.to_string()));
                        continue;
                    }
                },
                Err(error) => {
                    languages_failed.push((lang.clone(), error.to_string()));
                    continue;
                }
            };
            match plugin.inject_under_lock(&dest, &entries, &dest_lock) {
                Ok(mut report) => {
                    report.classify_remaining_skips();
                    #[cfg(test)]
                    REPLACE_AFTER_DEST_LOCK.with(|hook| {
                        if let Some(callback) = hook.get() {
                            callback(dest_lock.root());
                        }
                    });
                    let outcome = record_injection_for_lang_with_backup(
                        &self.db,
                        Some(lang),
                        &dest,
                        &report.files_written,
                        REPLACE_RECORD_REMEDY,
                        Some(&backup_id),
                        Some(&provenance),
                    )?;
                    recording_outcomes.insert(lang.clone(), outcome);
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
                    cost_is_complete: true,
                    language: Some(lang.clone()),
                })
                .await;
            drop(dest_lock);
        }

        Ok(MultiLangReport {
            mode: OutputMode::Replace,
            languages_processed,
            languages_failed,
            backup_id,
            reports,
            injected_roots,
            recording_outcomes,
        })
    }

    async fn inject_add(
        &self,
        project_path: &Path,
        plugin: &dyn FormatPlugin,
        languages: Vec<String>,
        entries: Vec<StringEntry>,
        tx: mpsc::Sender<ProgressEvent>,
    ) -> Result<MultiLangReport> {
        let total = languages.len();
        let mut languages_processed = Vec::new();
        let mut languages_failed = Vec::new();
        let mut reports = HashMap::new();
        let mut injected_roots = HashMap::new();
        let mut backup_id = String::new();
        let mut recording_outcomes = HashMap::new();

        // inject accepts exactly one language, which consumes the preflight rows.
        for (idx, (lang, entries)) in languages.iter().zip([entries]).enumerate() {
            let selected = project_path.canonicalize()?;
            emit_binary_slot_preflight(&entries, &tx).await;

            let recording_root = if selected.is_file() {
                selected.parent().unwrap()
            } else {
                &selected
            };
            let mut created_backup = None;
            let result = crate::injection_transaction::run(
                &selected,
                plugin.id(),
                Some(lang),
                || {
                    let backup = self.backup_manager.create_backup(&selected).map_err(|e| {
                        LocustError::BackupError(format!(
                            "{e} — injection is refused without a backup for recovery"
                        ))
                    })?;
                    created_backup = Some(backup.id.clone());
                    Ok(backup)
                },
                |work, selected_copy| {
                    let entries = entries_for_copy(entries, &selected, work)?;
                    plugin.inject_add(selected_copy, lang, &entries)
                },
                |report| {
                    record_injection_for_lang(
                        &self.db,
                        Some(lang),
                        recording_root,
                        &report.files_written,
                        "Inspect the injection transaction and recover before retrying.",
                        None,
                    )
                },
            );
            if let Some(id) = created_backup {
                backup_id = id;
            }
            match result {
                Ok((_, mut report, outcome)) => {
                    recording_outcomes.insert(lang.clone(), outcome);
                    report.classify_remaining_skips();
                    reports.insert(lang.clone(), report);
                    injected_roots.insert(
                        lang.clone(),
                        if selected.is_file() {
                            recording_root.to_path_buf()
                        } else {
                            project_path.to_path_buf()
                        },
                    );
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
                    cost_is_complete: true,
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
            recording_outcomes,
        })
    }
}

/// Convert semantic pivot rows to the immutable physical source expected by
/// format plugins. Only `source` changes; IDs, translations, paths and locator
/// metadata remain byte-for-byte/logically unchanged.
fn restore_physical_sources(mut entries: Vec<StringEntry>) -> Result<Vec<StringEntry>> {
    for entry in &mut entries {
        entry.source = validated_injection_source(entry)?.to_string();
    }
    Ok(entries)
}

fn validated_injection_source(entry: &StringEntry) -> Result<&str> {
    entry
        .require_current_translation()
        .map_err(LocustError::InjectionError)?;
    entry
        .require_preserved_translation_controls()
        .map_err(LocustError::InjectionError)?;
    crate::validation::binary_slot_budget(entry).map_err(|error| {
        LocustError::InjectionError(format!(
            "invalid injection provenance for '{}': {error}",
            entry.id
        ))
    })?;
    entry
        .injection_source()
        .map_err(LocustError::InjectionError)
}

/// Compare lexical paths without following interior links. The database's
/// rel_under_root resolves existing paths, which is appropriate for recording
/// identity but would hide a link before the pre-write safety check.
fn injection_relative(path: &Path, root: &Path) -> Option<PathBuf> {
    #[cfg(windows)]
    fn comparable(path: &Path) -> PathBuf {
        let path = path.as_os_str().to_string_lossy();
        if let Some(rest) = path.strip_prefix(r"\\?\UNC\") {
            PathBuf::from(format!(r"\\{rest}"))
        } else {
            PathBuf::from(path.strip_prefix(r"\\?\").unwrap_or(&path))
        }
    }
    #[cfg(not(windows))]
    fn comparable(path: &Path) -> PathBuf {
        path.to_path_buf()
    }
    let path = comparable(path);
    let root = comparable(root);
    let parts: Vec<_> = path.components().collect();
    let root_parts: Vec<_> = root.components().collect();
    if parts.len() <= root_parts.len() {
        return None;
    }
    if !parts.iter().zip(&root_parts).all(|(a, b)| {
        #[cfg(windows)]
        {
            a.as_os_str()
                .to_string_lossy()
                .eq_ignore_ascii_case(&b.as_os_str().to_string_lossy())
        }
        #[cfg(not(windows))]
        {
            a == b
        }
    }) {
        return None;
    }
    Some(parts[root_parts.len()..].iter().collect())
}

fn validate_injection_spelling(path: &Path) -> Result<()> {
    use std::path::Component;
    if path.as_os_str().is_empty()
        || path.components().any(|c| {
            matches!(c, Component::ParentDir)
                || matches!(c, Component::Normal(name) if name.to_string_lossy().contains(':'))
        })
        || (!path.is_absolute()
            && path
                .components()
                .any(|c| matches!(c, Component::Prefix(_) | Component::RootDir)))
    {
        return Err(LocustError::InjectionError(format!(
            "unsafe injection path: {}",
            path.display()
        )));
    }
    Ok(())
}

/// Inspect only real filesystem components. A UnityFS entry is represented as
/// bundle-file/CAB-node; probing its virtual suffix would fail with ENOTDIR.
/// All suffix components are still validated before the physical walk stops.
fn ensure_injection_path(root: &Path, relative: &Path) -> Result<()> {
    use std::path::Component;
    if relative.as_os_str().is_empty()
        || relative
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err(LocustError::InjectionError(format!(
            "unsafe relative injection path: {}",
            relative.display()
        )));
    }
    if relative.components().next().is_some_and(|component| {
        component
            .as_os_str()
            .to_string_lossy()
            .eq_ignore_ascii_case(".locust")
            || component
                .as_os_str()
                .to_string_lossy()
                .eq_ignore_ascii_case(crate::injection_transaction::STORE_DIR)
    }) {
        return Err(LocustError::InjectionError(
            "reserved recovery directory cannot be an injection target".into(),
        ));
    }
    let mut prefix = PathBuf::new();
    for component in relative.components() {
        prefix.push(component.as_os_str());
        crate::patch::zipsec::ensure_no_links(root, &prefix)?;
        match std::fs::symlink_metadata(root.join(&prefix)) {
            Ok(metadata) if metadata.is_file() => break,
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

/// Normalize every dispatched entry to the selected physical game root before
/// backup or plugin writes. Missing ordinary targets remain reportable by the
/// format plugin; external paths, traversal and existing interior links fail
/// the entire preflight. An explicitly selected root junction is resolved once.
fn entries_for_target(mut entries: Vec<StringEntry>, selection: &Path) -> Result<Vec<StringEntry>> {
    check_entries_for_target(&mut entries, selection, true)?;
    Ok(entries)
}

fn check_entries_for_target(
    entries: &mut [StringEntry],
    selection: &Path,
    normalize_paths: bool,
) -> Result<()> {
    let absolute = std::path::absolute(selection)?;
    let selected = selection.canonicalize()?;
    let is_file = selected.is_file();
    let alias_root = if is_file {
        absolute.parent().unwrap()
    } else {
        &absolute
    };
    let root = if is_file {
        selected.parent().unwrap()
    } else {
        &selected
    };
    for entry in entries {
        validate_injection_spelling(&entry.file_path)?;
        let from_cwd = std::path::absolute(&entry.file_path)?;
        let relative = injection_relative(&from_cwd, alias_root)
            .or_else(|| injection_relative(&from_cwd, root))
            .or_else(|| {
                (!entry.file_path.is_absolute()).then(|| {
                    entry
                        .file_path
                        .components()
                        .filter(|component| !matches!(component, std::path::Component::CurDir))
                        .collect()
                })
            })
            .ok_or_else(|| {
                LocustError::InjectionError(format!(
                    "entry '{}' references a file outside the selected game: {}",
                    entry.id,
                    entry.file_path.display()
                ))
            })?;
        // A selected file authorizes that file (and its virtual container
        // nodes), not every sibling file in its parent directory.
        if is_file && !relative.starts_with(selected.file_name().unwrap()) {
            return Err(LocustError::InjectionError(format!(
                "entry '{}' is outside the selected game file",
                entry.id
            )));
        }
        ensure_injection_path(root, &relative)?;
        if normalize_paths {
            entry.file_path = root.join(relative);
        }
    }
    Ok(())
}

fn entries_for_copy(
    entries: Vec<StringEntry>,
    source: &Path,
    destination: &Path,
) -> Result<Vec<StringEntry>> {
    let mut entries = entries_for_target(entries, source)?;
    let selected = source.canonicalize()?;
    let root = if selected.is_file() {
        selected.parent().unwrap()
    } else {
        &selected
    };
    let destination = destination.canonicalize()?;
    if !destination.is_dir() {
        return Err(LocustError::InjectionError(
            "injection copy must be a directory".into(),
        ));
    }
    for entry in &mut entries {
        let relative = injection_relative(&entry.file_path, root).ok_or_else(|| {
            LocustError::InjectionError(format!("entry '{}' is outside the source game", entry.id))
        })?;
        ensure_injection_path(&destination, &relative)?;
        entry.file_path = destination.join(relative);
    }
    Ok(entries)
}
/// Warn (trace + progress event) when translations exceed tagged binary slots.
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
    let _ = tx.send(ProgressEvent::ValidationFailed { issues }).await;
}

/// What [`record_injection_for_lang`] did for one language key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
    record_injection_for_lang_with_backup(db, lang, root, files, remedy, backup_id, None)
}

fn record_injection_for_lang_with_backup(
    db: &Database,
    lang: Option<&str>,
    root: &Path,
    files: &[PathBuf],
    remedy: &str,
    backup_id: Option<&str>,
    pristine_backup: Option<&RecordedBackup>,
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
    db.record_injection_with_backup(lang, &root_abs, files, pristine_backup)?;
    Ok(RecordOutcome::Recorded { files: files.len() })
}

/// Return the recordings already committed under lock for every language
/// `report` successfully injected. This is THE mandatory companion to
/// [`MultiLangInjector::inject`]: every caller — the CLI, the HTTP server, the
/// desktop app — must go through here. Add and Replace persist hashes and
/// provenance before those locks drop; this function returns those outcomes
/// and refuses to rehash files that may have changed. `remedy` remains part of
/// the caller contract (containment now fails inside the injector). Languages
/// are visited in `languages` order so a failure reports the same way every
/// run; the returned outcomes let the caller surface zero-write runs (see
/// [`RecordOutcome`]).
pub fn record_multilang_injection(
    db: &Database,
    report: &MultiLangReport,
    languages: &[String],
    remedy: &dyn Fn(&str) -> String,
) -> Result<Vec<(String, RecordOutcome)>> {
    let _ = (db, remedy);
    let mut outcomes = Vec::new();
    for lang in languages {
        if !report.reports.contains_key(lang) {
            continue; // failed language — the caller reports it, nothing to record
        }
        let kind = match report.mode {
            OutputMode::Add => "Add",
            OutputMode::Replace => "Replace",
        };
        let outcome = report.recording_outcomes.get(lang).ok_or_else(|| {
            LocustError::InjectionError(format!(
                "{kind} injection has no committed recording outcome for {lang}; refusing to record possibly changed files after its transaction"
            ))
        })?;
        outcomes.push((lang.clone(), outcome.clone()));
    }
    Ok(outcomes)
}

#[cfg(test)]
fn copy_dir_for_inject(src: &Path, dst: &Path) -> Result<()> {
    let source = src.canonicalize()?;
    let root = if source.is_file() {
        source.parent().unwrap()
    } else {
        &source
    };
    let lock = crate::patch::GameLock::acquire(root)?;
    let _dest_lock = copy_dir_for_inject_under_lock(&source, dst, &lock)?;
    Ok(())
}

fn copy_dir_for_inject_under_lock(
    src: &Path,
    dst: &Path,
    lock: &crate::patch::GameLock,
) -> Result<crate::patch::GameLock> {
    let source = src.canonicalize()?;
    let root = if source.is_file() {
        source.parent().unwrap()
    } else {
        &source
    };
    if root != lock.root() {
        return Err(LocustError::InjectionError(
            "copy source does not match the held game lock".into(),
        ));
    }
    // A store is game-bound authority, so it must not follow a copied game.
    // Reject pending or unrecognized metadata before creating any output;
    // skip only the exact, validated root namespace, preserving similar names.
    crate::injection_transaction::ensure_no_pending_under_lock(lock)?;
    let exclude_store = crate::injection_transaction::validate_store(root)?;
    // An existing output can contain junctions/hardlinks back to the original.
    // Reserve a fresh directory atomically instead of overwriting such a tree.
    if std::fs::symlink_metadata(dst).is_ok() {
        return Err(LocustError::InjectionError(
            "output copy already exists; choose a new output folder to preserve existing files"
                .into(),
        ));
    }
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::create_dir(dst)?;
    let target = dst.canonicalize()?;
    #[cfg(test)]
    REPLACE_BEFORE_DEST_LOCK.with(|hook| {
        if let Some(callback) = hook.get() {
            callback(&target);
        }
    });
    let dest_lock = crate::patch::GameLock::acquire(&target)?;
    if dest_lock.root() != target.as_path() {
        return Err(LocustError::InjectionError(
            "destination lock does not match the reserved copy".into(),
        ));
    }
    #[cfg(test)]
    REPLACE_AFTER_DEST_LOCK.with(|hook| {
        if let Some(callback) = hook.get() {
            callback(dest_lock.root());
        }
    });
    for entry in WalkDir::new(&source)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| {
            let is_store = e.path().parent() == Some(root)
                && if cfg!(windows) {
                    e.file_name()
                        .to_string_lossy()
                        .eq_ignore_ascii_case(crate::injection_transaction::STORE_DIR)
                } else {
                    e.file_name() == crate::injection_transaction::STORE_DIR
                };
            e.path() != target && !(exclude_store && is_store)
        })
    {
        let entry = entry.map_err(|e| LocustError::IoError(std::io::Error::other(e)))?;
        let rel = entry
            .path()
            .strip_prefix(root)
            .map_err(|e| LocustError::InjectionError(e.to_string()))?;
        crate::patch::zipsec::ensure_no_links(root, rel)?;
        crate::patch::zipsec::ensure_no_links(&target, rel)?;
        let destination = target.join(rel);
        if entry.file_type().is_dir() {
            std::fs::create_dir_all(&destination)?;
        } else if entry.file_type().is_file() {
            if let Some(parent) = destination.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut input = std::fs::File::open(entry.path())?;
            let mut output = std::fs::File::create_new(&destination)?;
            std::io::copy(&mut input, &mut output)?;
            output.set_permissions(input.metadata()?.permissions())?;
        }
    }
    Ok(dest_lock)
}

#[cfg(test)]
mod tests {
    fn reference_path_union(written: &[PathBuf], prior: &[PathBuf]) -> Vec<PathBuf> {
        let mut files = written.to_vec();
        for path in prior {
            if !files
                .iter()
                .any(|existing| crate::database::paths_identical(existing, path))
            {
                files.push(path.clone());
            }
        }
        files
    }

    #[test]
    fn indexed_path_union_preserves_alias_identity_and_order() {
        let base = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
        let first = base.path().join("First.html");
        let second = base.path().join("Second.html");
        std::fs::write(&first, b"first").unwrap();
        std::fs::write(&second, b"second").unwrap();
        let written = vec![first.clone(), first.clone()];
        let mut prior = vec![
            first.canonicalize().unwrap(),
            second.clone(),
            base.path().join(".").join("Second.html"),
            second
                .strip_prefix(std::env::current_dir().unwrap())
                .unwrap()
                .to_path_buf(),
            base.path().join("missing.html"),
            base.path().join("missing.html"),
        ];
        #[cfg(windows)]
        prior.push(PathBuf::from(second.to_string_lossy().to_uppercase()));
        // Root's empty Unix identity is intentionally never considered equal.
        prior.push(PathBuf::from(std::path::MAIN_SEPARATOR_STR));
        prior.push(PathBuf::from(std::path::MAIN_SEPARATOR_STR));
        assert_eq!(
            union_recorded_paths(&written, &prior),
            reference_path_union(&written, &prior)
        );
    }

    #[test]
    #[ignore = "explicit filesystem path-identity performance measurement"]
    fn measure_indexed_path_union_against_pairwise_reference() {
        let base = tempfile::tempdir().unwrap();
        for count in [250, 500] {
            let paths: Vec<_> = (0..count)
                .map(|i| base.path().join(format!("{i}.html")))
                .collect();
            for path in &paths {
                std::fs::write(path, b"original").unwrap();
            }
            let started = std::time::Instant::now();
            let expected = reference_path_union(&paths[..1], &paths[1..]);
            let reference = started.elapsed();
            let started = std::time::Instant::now();
            let indexed = union_recorded_paths(&paths[..1], &paths[1..]);
            let optimized = started.elapsed();
            assert_eq!(indexed, expected);
            eprintln!(
                "path union count={count} pairwise_ms={} indexed_ms={}",
                reference.as_secs_f64() * 1000.0,
                optimized.as_secs_f64() * 1000.0
            );
        }
    }

    use super::*;
    use std::cell::{Cell, RefCell};
    use std::fs;
    use std::path::PathBuf;

    #[test]
    fn injection_cannot_target_recovery_metadata() {
        let game = tempfile::tempdir().unwrap();
        let entry = StringEntry::new(
            "bad",
            "source",
            game.path().join(".locust/backup/original.txt"),
        );
        let error = entries_for_target(vec![entry], game.path()).unwrap_err();
        assert!(error.to_string().contains("reserved recovery directory"));
    }

    #[test]
    fn plugin_conversion_restores_only_physical_source() {
        let mut entry = StringEntry::new("same-id", "English", PathBuf::from("game.mock"));
        entry.translation = Some("Español".into());
        entry.metadata.insert(
            "locust_injection_source".into(),
            serde_json::json!("日本語"),
        );
        entry
            .metadata
            .insert("locator".into(), serde_json::json!({"offset": 7}));
        let original_metadata = entry.metadata.clone();
        let converted = restore_physical_sources(vec![entry]).unwrap().remove(0);
        assert_eq!(converted.id, "same-id");
        assert_eq!(converted.source, "日本語");
        assert_eq!(converted.translation.as_deref(), Some("Español"));
        assert_eq!(converted.file_path, PathBuf::from("game.mock"));
        assert_eq!(converted.metadata, original_metadata);
    }

    #[test]
    fn direct_rejects_multiple_languages_before_backup_or_dispatch() {
        let game = tempfile::tempdir().unwrap();
        let backup = game.path().join("backups");
        let error = inject_direct(
            &FormatRegistry::new(),
            &Database::open_in_memory().unwrap(),
            &BackupManager::new(backup.clone()),
            game.path(),
            "missing",
            &["es".into(), "fr".into()],
        )
        .unwrap_err();
        assert!(error.to_string().contains("separate pivot/database"));
        assert!(!backup.exists());
    }

    struct EntryPathWriter {
        calls: Arc<std::sync::atomic::AtomicUsize>,
    }

    impl FormatPlugin for EntryPathWriter {
        fn id(&self) -> &str {
            "entry-path-writer"
        }
        fn name(&self) -> &str {
            "Test entry path writer"
        }
        fn supported_extensions(&self) -> &[&str] {
            &[".mock"]
        }
        fn supported_modes(&self) -> Vec<OutputMode> {
            vec![OutputMode::Add, OutputMode::Replace]
        }
        fn extract(&self, _: &Path) -> Result<Vec<StringEntry>> {
            Ok(Vec::new())
        }
        fn inject(&self, _: &Path, entries: &[StringEntry]) -> Result<InjectionReport> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let mut files = Vec::new();
            for entry in entries {
                // Simulate both ordinary entry.file_path writes and UnityFS's
                // virtual node -> physical container resolution.
                let physical = entry
                    .file_path
                    .ancestors()
                    .find(|p| p.is_file())
                    .ok_or_else(|| {
                        LocustError::InjectionError("test backing file missing".into())
                    })?;
                fs::write(physical, entry.translation.as_deref().unwrap_or("written"))?;
                files.push(physical.to_path_buf());
            }
            Ok(InjectionReport {
                skip_reasons: Default::default(),
                files_modified: files.len(),
                strings_written: entries.len(),
                strings_skipped: 0,
                warnings: Vec::new(),
                files_written: files,
            })
        }
        fn inject_add(
            &self,
            path: &Path,
            _: &str,
            entries: &[StringEntry],
        ) -> Result<InjectionReport> {
            self.inject(path, entries)
        }
    }

    #[test]
    fn direct_entry_preflight_rejects_external_and_traversal_before_backup() {
        let fixture = tempfile::tempdir().unwrap();
        let game = fixture.path().join("game");
        fs::create_dir(&game).unwrap();
        let external = fixture.path().join("outside.mock");
        fs::write(&external, "ORIGINAL OUTSIDE").unwrap();
        fs::write(game.join("inside.mock"), "ORIGINAL INSIDE").unwrap();
        for (i, bad) in [
            external.clone(),
            PathBuf::from("../outside.mock"),
            game.join("../outside.mock"),
            PathBuf::from("nested/../inside.mock"),
        ]
        .into_iter()
        .enumerate()
        {
            let db = Database::open_in_memory().unwrap();
            let mut entry = StringEntry::new("bad", "ORIGINAL", bad);
            entry.translation = Some("CORRUPTED".into());
            db.save_entries(&[entry]).unwrap();
            let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let mut registry = FormatRegistry::new();
            registry.register(Box::new(EntryPathWriter {
                calls: calls.clone(),
            }));
            let backup = fixture.path().join(format!("backup-{i}"));
            assert!(inject_direct(
                &registry,
                &db,
                &BackupManager::new(backup.clone()),
                &game,
                "entry-path-writer",
                &["es".into()]
            )
            .is_err());
            assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);
            assert!(
                !backup.exists(),
                "Invalid input must not create/prune backups"
            );
            assert_eq!(fs::read_to_string(&external).unwrap(), "ORIGINAL OUTSIDE");
            assert_eq!(
                fs::read_to_string(game.join("inside.mock")).unwrap(),
                "ORIGINAL INSIDE"
            );
        }
    }

    #[tokio::test]
    async fn add_entry_preflight_rejects_external_before_backup_or_plugin() {
        let fixture = tempfile::tempdir().unwrap();
        let game = fixture.path().join("game");
        fs::create_dir(&game).unwrap();
        fs::write(game.join("inside.mock"), "ORIGINAL INSIDE").unwrap();
        let external = fixture.path().join("outside.mock");
        fs::write(&external, "ORIGINAL OUTSIDE").unwrap();
        let db = Arc::new(Database::open_in_memory().unwrap());
        let mut entry = StringEntry::new("external", "ORIGINAL", external.clone());
        entry.translation = Some("CORRUPTED".into());
        db.save_entries(&[entry]).unwrap();
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut registry = FormatRegistry::new();
        registry.register(Box::new(EntryPathWriter {
            calls: calls.clone(),
        }));
        let backup = fixture.path().join("backup");
        let injector = MultiLangInjector::new(
            Arc::new(registry),
            db,
            Arc::new(BackupManager::new(backup.clone())),
        );
        let (tx, _rx) = mpsc::channel(10);
        assert!(injector
            .inject(
                &game,
                "entry-path-writer",
                OutputMode::Add,
                vec!["es".into()],
                None,
                tx
            )
            .await
            .is_err());
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert!(!backup.exists());
        assert_eq!(fs::read_to_string(&external).unwrap(), "ORIGINAL OUTSIDE");
    }

    #[test]
    fn virtual_bundle_and_relative_paths_map_to_only_the_copy() {
        let fixture = tempfile::tempdir().unwrap();
        let game = fixture.path().join("game");
        let copy = fixture.path().join("copy");
        fs::create_dir(&game).unwrap();
        fs::write(game.join("data.unity3d"), "ORIGINAL BUNDLE").unwrap();
        fs::write(game.join("inside.mock"), "ORIGINAL INSIDE").unwrap();
        copy_dir_for_inject(&game, &copy).unwrap();
        let mut virtual_entry = StringEntry::new(
            "virtual",
            "ORIGINAL",
            PathBuf::from("data.unity3d/CAB-node/level0"),
        );
        virtual_entry.translation = Some("TRANSLATED BUNDLE".into());
        let mut relative = StringEntry::new("relative", "ORIGINAL", PathBuf::from("./inside.mock"));
        relative.translation = Some("TRANSLATED INSIDE".into());
        let relocated = entries_for_copy(vec![virtual_entry, relative], &game, &copy).unwrap();
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        EntryPathWriter { calls }.inject(&copy, &relocated).unwrap();
        assert_eq!(
            fs::read_to_string(game.join("data.unity3d")).unwrap(),
            "ORIGINAL BUNDLE"
        );
        assert_eq!(
            fs::read_to_string(game.join("inside.mock")).unwrap(),
            "ORIGINAL INSIDE"
        );
        assert_eq!(
            fs::read_to_string(copy.join("data.unity3d")).unwrap(),
            "TRANSLATED BUNDLE"
        );
        assert_eq!(
            fs::read_to_string(copy.join("inside.mock")).unwrap(),
            "TRANSLATED INSIDE"
        );
        let missing = StringEntry::new("missing", "source", PathBuf::from("not-present.mock"));
        assert!(
            entries_for_target(vec![missing], &game).is_ok(),
            "Plugin must report normal missing targets"
        );
    }

    #[test]
    fn selected_file_scope_rejects_siblings_and_copies_its_virtual_nodes() {
        let fixture = tempfile::tempdir().unwrap();
        let file = fixture.path().join("data.unity3d");
        fs::write(&file, "BUNDLE").unwrap();
        let sibling = fixture.path().join("sibling.mock");
        fs::write(&sibling, "OUTSIDE SELECTED FILE").unwrap();
        assert!(
            entries_for_target(vec![StringEntry::new("sibling", "source", sibling)], &file)
                .is_err()
        );
        let copy = fixture.path().join("copy");
        copy_dir_for_inject(&file, &copy).unwrap();
        let result = entries_for_copy(
            vec![StringEntry::new("virtual", "source", file.join("CAB-node"))],
            &file,
            &copy,
        )
        .unwrap();
        assert_eq!(
            result[0].file_path,
            copy.canonicalize().unwrap().join("data.unity3d/CAB-node")
        );
        assert_eq!(
            fs::read_to_string(copy.join("data.unity3d")).unwrap(),
            "BUNDLE"
        );
    }

    #[cfg(any(windows, unix))]
    fn make_injection_directory_link(target: &Path, link: &Path) {
        #[cfg(unix)]
        std::os::unix::fs::symlink(target, link).unwrap();
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            let output = std::process::Command::new("powershell.exe")
                .args(["-NoProfile", "-NonInteractive", "-Command", "New-Item -ItemType Junction -Path $env:LOCUST_ENTRY_LINK -Target $env:LOCUST_ENTRY_TARGET -ErrorAction Stop | Out-Null"])
                .env("LOCUST_ENTRY_LINK", link).env("LOCUST_ENTRY_TARGET", target)
                .creation_flags(0x08000000).output().unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }

    #[test]
    #[cfg(any(windows, unix))]
    fn entry_preflight_accepts_selected_root_link_but_rejects_interior_links() {
        let fixture = tempfile::tempdir().unwrap();
        let game = fixture.path().join("game");
        let outside = fixture.path().join("outside");
        fs::create_dir(&game).unwrap();
        fs::create_dir(&outside).unwrap();
        fs::write(game.join("inside.mock"), "INSIDE").unwrap();
        fs::write(outside.join("outside.mock"), "OUTSIDE").unwrap();
        let selected = fixture.path().join("selected-link");
        let interior = game.join("interior-link");
        make_injection_directory_link(&game, &selected);
        make_injection_directory_link(&outside, &interior);
        let good = entries_for_target(
            vec![StringEntry::new(
                "inside",
                "source",
                selected.join("inside.mock"),
            )],
            &selected,
        );
        let bad = entries_for_target(
            vec![StringEntry::new(
                "outside",
                "source",
                selected.join("interior-link/outside.mock"),
            )],
            &selected,
        );
        #[cfg(windows)]
        {
            fs::remove_dir(&interior).unwrap();
            fs::remove_dir(&selected).unwrap();
        }
        #[cfg(unix)]
        {
            fs::remove_file(&interior).unwrap();
            fs::remove_file(&selected).unwrap();
        }
        assert_eq!(
            good.unwrap()[0].file_path,
            game.canonicalize().unwrap().join("inside.mock")
        );
        assert!(bad.is_err());
        assert_eq!(
            fs::read_to_string(outside.join("outside.mock")).unwrap(),
            "OUTSIDE"
        );
    }

    #[test]
    fn copy_reserves_new_output_and_keeps_media_independent() {
        let base = tempfile::tempdir().unwrap();
        let original = base.path().join("original");
        let copy = base.path().join("copy");
        fs::create_dir(&original).unwrap();
        fs::write(original.join("movie.mp4"), b"original media").unwrap();
        copy_dir_for_inject(&original, &copy).unwrap();
        fs::write(copy.join("movie.mp4"), b"edited media").unwrap();
        assert_eq!(
            fs::read(original.join("movie.mp4")).unwrap(),
            b"original media"
        );
        assert!(copy_dir_for_inject(&original, &copy).is_err());
        assert_eq!(fs::read(copy.join("movie.mp4")).unwrap(), b"edited media");
        let nested = original.join("new-output");
        copy_dir_for_inject(&original, &nested).unwrap();
        assert!(nested.join("movie.mp4").is_file());
        assert!(!nested.join("new-output").exists());
    }

    #[test]
    fn copy_entries_relocate_before_a_path_based_plugin_can_write() {
        let base = std::env::temp_dir().join(format!("locust_copy_paths_{}", uuid::Uuid::new_v4()));
        let original = base.join("original");
        let copy = base.join("copy");
        fs::create_dir_all(&original).unwrap();
        fs::create_dir_all(&copy).unwrap();
        fs::write(original.join("dialogue.assets"), "original").unwrap();
        fs::write(copy.join("dialogue.assets"), "original").unwrap();
        let entry = StringEntry::new("line", "original", original.join("dialogue.assets"));
        let relocated = entries_for_copy(vec![entry], &original, &copy).unwrap();
        fs::write(&relocated[0].file_path, "translated").unwrap();
        assert_eq!(
            fs::read_to_string(original.join("dialogue.assets")).unwrap(),
            "original"
        );
        assert_eq!(
            fs::read_to_string(copy.join("dialogue.assets")).unwrap(),
            "translated"
        );
        let external = StringEntry::new("bad", "original", base.join("outside.assets"));
        assert!(entries_for_copy(vec![external], &original, &copy).is_err());
        let traversal = StringEntry::new("bad", "original", PathBuf::from("../outside.assets"));
        assert!(entries_for_copy(vec![traversal], &original, &copy).is_err());
        fs::remove_dir_all(base).unwrap();
    }

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
                skip_reasons: Default::default(),
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
                skip_reasons: Default::default(),
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
                skip_reasons: Default::default(),
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
            // Keep this fixture about injection/recording. Bracketed prefixes
            // are Ren'Py controls and the shared preflight now rejects extras.
            e.translation = Some(format!("translated: {}", e.source));
        }
        db.save_entries(&entries).unwrap();

        let mut registry = FormatRegistry::new();
        registry.register(Box::new(MockFormatPlugin));

        let injector = MultiLangInjector::new(Arc::new(registry), db, backup);
        (injector, game_dir, output_dir)
    }

    async fn assert_inject_materializes_entries_once(mode: OutputMode) {
        let fixture = tempfile::tempdir().unwrap();
        let game = fixture.path().join("game");
        fs::create_dir(&game).unwrap();
        fs::write(game.join("game.mock"), "original").unwrap();
        let db = Arc::new(Database::open_in_memory().unwrap());
        let entries: Vec<_> = (0..32)
            .map(|i| {
                let mut entry = StringEntry::new(format!("line-{i}"), "Hello", "game.mock".into());
                entry.translation = Some("Hola".into());
                entry
            })
            .collect();
        db.save_entries(&entries).unwrap();
        let mut registry = make_registry();
        registry.register(Box::new(ContainedMock));
        let injector = MultiLangInjector::new(
            Arc::new(registry),
            db,
            Arc::new(BackupManager::new(fixture.path().join("backups"))),
        );
        let format = if mode == OutputMode::Add {
            "mock"
        } else {
            "contained"
        };
        let (tx, _rx) = mpsc::channel(100);
        crate::database::ENTRY_ROWS_MATERIALIZED.with(|count| count.set(Some(0)));
        let result = injector
            .inject(
                &game,
                format,
                mode,
                vec!["es".into()],
                Some(fixture.path().join("output")),
                tx,
            )
            .await;
        let materialized =
            crate::database::ENTRY_ROWS_MATERIALIZED.with(|count| count.replace(None));
        let report = result.unwrap();
        assert!(
            report.languages_failed.is_empty(),
            "{:?}",
            report.languages_failed
        );
        assert_eq!(report.languages_processed, vec!["es"]);
        assert_eq!(report.reports["es"].strings_written, entries.len());
        assert_eq!(materialized, Some(entries.len()));
    }

    #[tokio::test]
    async fn add_materializes_entries_once() {
        assert_inject_materializes_entries_once(OutputMode::Add).await;
    }

    #[tokio::test]
    async fn replace_materializes_entries_once() {
        assert_inject_materializes_entries_once(OutputMode::Replace).await;
    }

    #[tokio::test]
    async fn test_replace_single_language() {
        let (injector, game_dir, output_dir) = setup_contained_injector();
        let (tx, mut rx) = mpsc::channel(100);

        let report = injector
            .inject(
                &game_dir,
                "contained",
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
    async fn test_replace_rejects_multiple_languages_for_one_database() {
        let (injector, game_dir, output_dir) = setup_injector();
        let (tx, _rx) = mpsc::channel(100);

        let error = injector
            .inject(
                &game_dir,
                "mock",
                OutputMode::Replace,
                vec!["es".to_string(), "fr".to_string(), "de".to_string()],
                Some(output_dir.clone()),
                tx,
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("separate pivot/database"));
        assert!(!output_dir.join("mygame-es").exists());
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn test_replace_copies_on_windows() {
        let (injector, game_dir, output_dir) = setup_contained_injector();
        let (tx, mut rx) = mpsc::channel(100);

        let report = injector
            .inject(
                &game_dir,
                "contained",
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
    async fn test_replace_output_does_not_share_inodes_with_original_on_unix() {
        let (injector, game_dir, output_dir) = setup_contained_injector();
        let (tx, mut rx) = mpsc::channel(100);

        injector
            .inject(
                &game_dir,
                "contained",
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
        use std::os::unix::fs::MetadataExt;
        let meta = fs::metadata(&png).unwrap();
        let original = fs::metadata(game_dir.join("image.png")).unwrap();
        assert_ne!((meta.dev(), meta.ino()), (original.dev(), original.ino()));
        assert_eq!(meta.nlink(), 1);
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
    async fn separate_language_databases_keep_separate_values() {
        let (es, es_game, _) = setup_injector();
        let (fr, fr_game, _) = setup_injector();
        es.db
            .save_translation("mock#0", "Español", "manual")
            .await
            .unwrap();
        fr.db
            .save_translation("mock#0", "Français", "manual")
            .await
            .unwrap();

        let (es_tx, _es_rx) = mpsc::channel(10);
        es.inject(
            &es_game,
            "mock",
            OutputMode::Add,
            vec!["es".into()],
            None,
            es_tx,
        )
        .await
        .unwrap();
        let (fr_tx, _fr_rx) = mpsc::channel(10);
        fr.inject(
            &fr_game,
            "mock",
            OutputMode::Add,
            vec!["fr".into()],
            None,
            fr_tx,
        )
        .await
        .unwrap();

        let es_text = fs::read_to_string(es_game.join("tl/es/mock.txt")).unwrap();
        let fr_text = fs::read_to_string(fr_game.join("tl/fr/mock.txt")).unwrap();
        assert!(es_text.contains("mock#0=Español"));
        assert!(fr_text.contains("mock#0=Français"));
        assert!(!es_text.contains("Français"));
        assert!(!fr_text.contains("Español"));
    }

    #[tokio::test]
    async fn test_add_rejects_multiple_languages_for_one_database() {
        let (injector, game_dir, _output_dir) = setup_injector();
        let (tx, _rx) = mpsc::channel(100);

        let error = injector
            .inject(
                &game_dir,
                "mock",
                OutputMode::Add,
                vec!["fr".to_string(), "de".to_string()],
                None,
                tx,
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("separate pivot/database"));
        assert!(!game_dir.join("tl").exists());
    }

    #[tokio::test]
    async fn test_backup_created_before_inject() {
        let (injector, game_dir, output_dir) = setup_contained_injector();
        let (tx, mut rx) = mpsc::channel(100);

        // Replace mode with output_dir skips backup (original untouched)
        let report = injector
            .inject(
                &game_dir,
                "contained",
                OutputMode::Replace,
                vec!["es".to_string()],
                Some(output_dir),
                tx,
            )
            .await
            .unwrap();

        rx.close();
        while rx.recv().await.is_some() {}

        // Replace retains its recovery backup even though entry paths are now
        // remapped into the output copy before plugin dispatch.
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
        // An unsuccessful backup must refuse the operation before any copy or
        // injection, keeping the promised recovery path available.
        let game_dir = make_game_dir();
        let output_dir = tempdir().join("output");
        fs::create_dir_all(&output_dir).unwrap();
        // A backup root that is a FILE: create_backup cannot create its
        // timestamp directory under it, on any platform.
        let bad_backup_root = tempdir().join("not_a_dir");
        fs::write(&bad_backup_root, b"occupied").unwrap();

        let db = Arc::new(Database::open_in_memory().unwrap());
        let mut entries = vec![StringEntry::new(
            "mock#0",
            "Hello",
            PathBuf::from("game.mock"),
        )];
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
    async fn test_single_language_plugin_failure_is_reported() {
        let game_dir = make_game_dir();
        let backup_root = tempdir().join("backups");

        let db = Arc::new(Database::open_in_memory().unwrap());
        let backup = Arc::new(BackupManager::new(backup_root));

        let entries = vec![StringEntry::new(
            "mock#0",
            "Hello",
            PathBuf::from("game.mock"),
        )];
        db.save_entries(&entries).unwrap();

        // Register a plugin where inject_add fails for the requested language.
        struct FailOnBadLang;
        impl FormatPlugin for FailOnBadLang {
            fn id(&self) -> &str {
                "failmock"
            }
            fn name(&self) -> &str {
                "Fail Mock"
            }
            fn supported_extensions(&self) -> &[&str] {
                &[".mock"]
            }
            fn supported_modes(&self) -> Vec<OutputMode> {
                vec![OutputMode::Add]
            }
            fn extract(&self, _: &Path) -> Result<Vec<StringEntry>> {
                Ok(vec![])
            }
            fn inject(&self, _: &Path, _: &[StringEntry]) -> Result<InjectionReport> {
                Ok(InjectionReport {
                    skip_reasons: Default::default(),
                    files_modified: 0,
                    strings_written: 0,
                    strings_skipped: 0,
                    warnings: vec![],
                    files_written: vec![],
                })
            }
            fn inject_add(
                &self,
                _path: &Path,
                lang: &str,
                _entries: &[StringEntry],
            ) -> Result<InjectionReport> {
                if lang == "bad" {
                    return Err(LocustError::InjectionError("bad language".to_string()));
                }
                fs::create_dir_all(_path.join("tl").join(lang))?;
                Ok(InjectionReport {
                    skip_reasons: Default::default(),
                    files_modified: 1,
                    strings_written: 1,
                    strings_skipped: 0,
                    warnings: vec![],
                    files_written: vec![],
                })
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
                vec!["bad".to_string()],
                None,
                tx,
            )
            .await
            .unwrap();

        rx.close();
        while rx.recv().await.is_some() {}

        assert!(report.languages_processed.is_empty());
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
                skip_reasons: Default::default(),
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
        let mut entries = vec![StringEntry::new(
            "mock#0",
            "Hello",
            PathBuf::from("game.mock"),
        )];
        entries[0].translation = Some("Hola".to_string());
        db.save_entries(&entries).unwrap();

        let mut registry = FormatRegistry::new();
        registry.register(Box::new(ContainedMock));
        let injector = MultiLangInjector::new(Arc::new(registry), db, backup);
        (injector, game_dir, output_dir)
    }

    #[tokio::test]
    async fn replace_after_direct_does_not_copy_game_bound_history() {
        let (injector, game, output) = setup_contained_injector();
        fs::create_dir(game.join(".locust-injections-notes")).unwrap();
        fs::write(game.join(".locust-injections-notes/user.txt"), b"user").unwrap();
        fs::create_dir_all(game.join("assets/.locust-injections")).unwrap();
        fs::write(game.join("assets/.locust-injections/user.txt"), b"nested").unwrap();
        fs::create_dir_all(game.join(".locust")).unwrap();
        fs::write(game.join(".locust/user.txt"), b"existing metadata").unwrap();
        inject_direct(
            &injector.registry,
            &injector.db,
            &injector.backup_manager,
            &game,
            "contained",
            &["es".into()],
        )
        .unwrap();
        assert!(crate::injection_transaction::validate_store(&game).unwrap());
        let source_marker = fs::read(game.join(".locust-injections/store.json")).unwrap();
        let (tx, _rx) = mpsc::channel(10);
        let report = injector
            .inject(
                &game,
                "contained",
                OutputMode::Replace,
                vec!["es".into()],
                Some(output),
                tx,
            )
            .await
            .unwrap();
        assert!(
            report.languages_failed.is_empty(),
            "{:?}",
            report.languages_failed
        );
        let copy = &report.injected_roots["es"];
        assert!(!copy.join(crate::injection_transaction::STORE_DIR).exists());
        assert_eq!(
            fs::read(copy.join(".locust-injections-notes/user.txt")).unwrap(),
            b"user"
        );
        assert_eq!(
            fs::read(copy.join("assets/.locust-injections/user.txt")).unwrap(),
            b"nested"
        );
        assert_eq!(
            fs::read(copy.join(".locust/user.txt")).unwrap(),
            b"existing metadata"
        );
        // Relative entries are deliberately reusable against the selected copy.
        inject_direct(
            &injector.registry,
            &injector.db,
            &injector.backup_manager,
            copy,
            "contained",
            &["es".into()],
        )
        .unwrap();
        assert!(crate::injection_transaction::validate_store(copy).unwrap());
        assert!(crate::injection_transaction::status(copy)
            .unwrap()
            .pending
            .is_none());
        assert_eq!(
            fs::read(game.join(".locust-injections/store.json")).unwrap(),
            source_marker
        );
        assert_ne!(
            fs::read(copy.join(".locust-injections/store.json")).unwrap(),
            source_marker
        );
    }

    #[tokio::test]
    async fn replace_rejects_unknown_and_pending_before_backup_or_output() {
        for pending in [false, true] {
            let (injector, game, output) = setup_contained_injector();
            let metadata = game.join(crate::injection_transaction::STORE_DIR);
            if pending {
                let result = crate::injection_transaction::run(
                    &game,
                    "contained",
                    Some("es"),
                    || Ok(()),
                    |_, selection| ContainedMock.inject(selection, &[]),
                    |_| Err::<(), _>(LocustError::InjectionError("record interrupted".into())),
                );
                assert!(result.is_err());
                assert!(crate::injection_transaction::status(&game)
                    .unwrap()
                    .pending
                    .is_some());
            } else {
                fs::create_dir(&metadata).unwrap();
                fs::write(metadata.join("user.txt"), b"unrecognized user data").unwrap();
            }
            let output_count = fs::read_dir(&output).unwrap().count();
            let (tx, _rx) = mpsc::channel(10);
            assert!(injector
                .inject(
                    &game,
                    "contained",
                    OutputMode::Replace,
                    vec!["es".into()],
                    Some(output.clone()),
                    tx
                )
                .await
                .is_err());
            assert!(injector.backup_manager.list_backups().unwrap().is_empty());
            assert_eq!(fs::read_dir(&output).unwrap().count(), output_count);
            // The standalone copying seam must enforce the same preflight.
            let destination = output.join("standalone");
            assert!(copy_dir_for_inject(&game, &destination).is_err());
            assert!(!destination.exists());
            if !pending {
                assert_eq!(
                    fs::read(metadata.join("user.txt")).unwrap(),
                    b"unrecognized user data"
                );
            }
        }
    }

    #[tokio::test]
    async fn replace_keeps_source_lock_through_plugin_dispatch() {
        struct CheckLock(PathBuf);
        impl FormatPlugin for CheckLock {
            fn id(&self) -> &str {
                "locked-copy"
            }
            fn name(&self) -> &str {
                "Locked Copy"
            }
            fn supported_extensions(&self) -> &[&str] {
                &["mock"]
            }
            fn extract(&self, _: &Path) -> Result<Vec<StringEntry>> {
                Ok(vec![])
            }
            fn inject(&self, path: &Path, entries: &[StringEntry]) -> Result<InjectionReport> {
                assert!(crate::patch::GameLock::acquire(&self.0).is_err());
                assert!(
                    crate::patch::GameLock::acquire(path).is_err(),
                    "destination must stay locked through plugin inject"
                );
                ContainedMock.inject(path, entries)
            }
        }
        let (mut injector, game, output) = setup_contained_injector();
        let mut registry = FormatRegistry::new();
        registry.register(Box::new(CheckLock(game.clone())));
        injector.registry = Arc::new(registry);
        let (tx, _rx) = mpsc::channel(10);
        let report = injector
            .inject(
                &game,
                "locked-copy",
                OutputMode::Replace,
                vec!["es".into()],
                Some(output),
                tx,
            )
            .await
            .unwrap();
        assert!(report.languages_failed.is_empty());
        assert_eq!(report.languages_processed, vec!["es"]);
        assert!(crate::patch::GameLock::acquire(&game).is_ok());
        let dest = report.injected_roots.get("es").expect("dest root");
        assert!(crate::patch::GameLock::acquire(dest).is_ok());
    }

    #[test]
    fn replace_copy_rejects_a_lock_for_another_game() {
        let source = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let lock = crate::patch::GameLock::acquire(other.path()).unwrap();
        let destination = other.path().join("output");
        assert!(copy_dir_for_inject_under_lock(source.path(), &destination, &lock).is_err());
        assert!(!destination.exists());
    }

    #[test]
    fn default_plugin_dispatch_rejects_a_guard_for_another_root() {
        let source = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let lock = crate::patch::GameLock::acquire(other.path()).unwrap();
        let result = ContainedMock.inject_under_lock(source.path(), &[], &lock);
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("different game root"));
        assert_eq!(fs::read_dir(source.path()).unwrap().count(), 0);
    }

    #[cfg(windows)]
    #[test]
    fn replace_copy_excludes_validated_history_with_windows_case_alias() {
        let source = tempfile::tempdir().unwrap();
        let output = tempfile::tempdir().unwrap();
        fs::write(source.path().join("game.mock"), b"Hello").unwrap();
        let metadata = source.path().join(".LOCUST-INJECTIONS");
        fs::create_dir(&metadata).unwrap();
        fs::write(
            metadata.join("store.json"),
            serde_json::to_vec(&serde_json::json!({
                "schema_version": 1,
                "kind": "locust-injection-store",
                "game_root": source.path().canonicalize().unwrap()
            }))
            .unwrap(),
        )
        .unwrap();
        assert!(crate::injection_transaction::validate_store(source.path()).unwrap());
        let destination = output.path().join("copy");
        copy_dir_for_inject(source.path(), &destination).unwrap();
        assert!(!destination.join(".LOCUST-INJECTIONS").exists());
        assert_eq!(fs::read(destination.join("game.mock")).unwrap(), b"Hello");
    }

    thread_local! {
        static STOLEN_DEST_LOCK: RefCell<Option<crate::patch::GameLock>> = const { RefCell::new(None) };
        static DEST_LOCK_OBSERVATIONS: Cell<usize> = const { Cell::new(0) };
    }

    struct DestLockSeam;
    impl Drop for DestLockSeam {
        fn drop(&mut self) {
            REPLACE_BEFORE_DEST_LOCK.with(|hook| hook.set(None));
            REPLACE_AFTER_DEST_LOCK.with(|hook| hook.set(None));
            STOLEN_DEST_LOCK.with(|slot| *slot.borrow_mut() = None);
            DEST_LOCK_OBSERVATIONS.with(|count| count.set(0));
        }
    }

    fn steal_reserved_dest(path: &Path) {
        STOLEN_DEST_LOCK.with(|slot| {
            *slot.borrow_mut() = Some(crate::patch::GameLock::acquire(path).unwrap());
        });
    }

    fn observe_dest_locked(path: &Path) {
        assert!(
            crate::patch::GameLock::acquire(path).is_err(),
            "destination must be locked: {}",
            path.display()
        );
        DEST_LOCK_OBSERVATIONS.with(|count| count.set(count.get() + 1));
    }

    #[tokio::test]
    async fn test_record_multilang_injection_records_every_processed_language() {
        let (injector, game_dir, output_dir) = setup_contained_injector();
        let decoy = injector.backup_manager.create_backup(&game_dir).unwrap();
        let (tx, mut rx) = mpsc::channel(100);
        let langs = vec!["es".to_string()];
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

        assert_eq!(
            report.recording_outcomes.get("es"),
            Some(&RecordOutcome::Recorded { files: 1 })
        );
        let rec = injector
            .db
            .get_injection(Some("es"))
            .unwrap()
            .expect("Replace must record under lock before returning");
        let backup = rec
            .pristine_backup
            .as_ref()
            .expect("Replace must store exact backup provenance");
        assert_eq!(backup.id, report.backup_id);
        assert_ne!(
            backup.id, decoy.id,
            "must not pair the latest leftover backup"
        );
        assert!(
            crate::database::paths_identical(&backup.source_path, &game_dir),
            "source_path must be the original selected game, got {}",
            backup.source_path.display()
        );
        let expected_store = std::path::absolute(injector.backup_manager.root()).unwrap();
        assert_eq!(
            backup.storage_root.as_ref(),
            Some(&expected_store),
            "storage_root must be the absolute backup store"
        );
        assert!(
            crate::database::paths_identical(&rec.root, &output_dir.join("mygame-es")),
            "the recording root must be the per-language copy, got {}",
            rec.root.display()
        );
        assert_eq!(rec.files.len(), 1);
        assert_eq!(rec.files[0].rel, "game.injected");

        let recorded_before = serde_json::to_value(&rec).unwrap();
        let outcomes =
            record_multilang_injection(&injector.db, &report, &langs, &|_| "remedy".to_string())
                .unwrap();
        assert_eq!(
            outcomes,
            vec![("es".to_string(), RecordOutcome::Recorded { files: 1 })]
        );
        let recorded_after = injector.db.get_injection(Some("es")).unwrap().unwrap();
        assert_eq!(
            serde_json::to_value(&recorded_after).unwrap(),
            recorded_before,
            "the Replace companion must never rehash or rewrite a completed recording"
        );
    }

    #[tokio::test]
    async fn test_record_multilang_injection_containment_failure_is_loud_and_records_nothing() {
        // MockFormatPlugin writes `<dest>.injected` — a SIBLING of the
        // per-language copy, outside its root. Recording must hard-fail inside
        // the injector and persist nothing for that language.
        let (injector, game_dir, output_dir) = setup_injector();
        let (tx, mut rx) = mpsc::channel(100);
        let langs = vec!["es".to_string()];
        let err = injector
            .inject(
                &game_dir,
                "mock",
                OutputMode::Replace,
                langs.clone(),
                Some(output_dir),
                tx,
            )
            .await
            .expect_err("outside writes must fail Replace before a companion rehash");
        rx.close();
        while rx.recv().await.is_some() {}

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
            msg.contains(REPLACE_RECORD_REMEDY),
            "the injector remedy must be attached: {msg}"
        );
        let backups = injector.backup_manager.list_backups().unwrap();
        assert_eq!(backups.len(), 1);
        assert!(
            msg.contains(&format!("Backup {}", backups[0].id)),
            "the backup that holds the pre-injection files must be named: {msg}"
        );
        assert!(
            injector.db.get_injection(Some("es")).unwrap().is_none(),
            "a containment failure must record NOTHING"
        );
    }

    #[test]
    fn replace_copy_holds_destination_lock_before_bytes() {
        let _seam = DestLockSeam;
        let source = tempfile::tempdir().unwrap();
        fs::write(source.path().join("game.mock"), b"Hello").unwrap();
        let destination = source.path().join("copy");
        REPLACE_AFTER_DEST_LOCK.with(|hook| hook.set(Some(observe_dest_locked)));
        copy_dir_for_inject(source.path(), &destination).unwrap();
        assert!(
            DEST_LOCK_OBSERVATIONS.with(|count| count.get()) >= 1,
            "destination lock must be observed before copy bytes"
        );
        assert_eq!(fs::read(destination.join("game.mock")).unwrap(), b"Hello");
        assert!(crate::patch::GameLock::acquire(&destination).is_ok());
    }

    #[test]
    fn replace_busy_destination_after_reserve_does_not_delete() {
        let _seam = DestLockSeam;
        let source = tempfile::tempdir().unwrap();
        fs::write(source.path().join("game.mock"), b"Hello").unwrap();
        let output = tempfile::tempdir().unwrap();
        let destination = output.path().join("copy");
        REPLACE_BEFORE_DEST_LOCK.with(|hook| hook.set(Some(steal_reserved_dest)));
        let err = copy_dir_for_inject(source.path(), &destination).unwrap_err();
        assert!(
            err.to_string().contains("game busy"),
            "a competing dest lock must fail closed: {err}"
        );
        assert!(destination.exists(), "busy destination must not be deleted");
        assert!(
            !destination.join("game.mock").exists(),
            "bytes must not be copied under a competing dest lock"
        );
        STOLEN_DEST_LOCK.with(|slot| *slot.borrow_mut() = None);
        assert_eq!(fs::read(source.path().join("game.mock")).unwrap(), b"Hello");
    }

    #[tokio::test]
    async fn replace_holds_destination_lock_through_copy_inject_and_record() {
        let _seam = DestLockSeam;
        let (injector, game, output) = setup_contained_injector();
        REPLACE_AFTER_DEST_LOCK.with(|hook| hook.set(Some(observe_dest_locked)));
        let (tx, _rx) = mpsc::channel(10);
        let report = injector
            .inject(
                &game,
                "contained",
                OutputMode::Replace,
                vec!["es".into()],
                Some(output),
                tx,
            )
            .await
            .unwrap();
        assert!(report.languages_failed.is_empty());
        assert_eq!(
            DEST_LOCK_OBSERVATIONS.with(|count| count.get()),
            2,
            "destination lock must be observed during copy and again during record"
        );
        let dest = report.injected_roots.get("es").unwrap();
        assert!(crate::patch::GameLock::acquire(dest).is_ok());
        assert!(injector.db.get_injection(Some("es")).unwrap().is_some());
    }

    #[tokio::test]
    async fn replace_plugin_failure_does_not_record() {
        struct FailInject;
        impl FormatPlugin for FailInject {
            fn id(&self) -> &str {
                "fail-replace"
            }
            fn name(&self) -> &str {
                "Fail Replace"
            }
            fn supported_extensions(&self) -> &[&str] {
                &[".mock"]
            }
            fn extract(&self, _: &Path) -> Result<Vec<StringEntry>> {
                Ok(vec![])
            }
            fn inject(&self, _: &Path, _: &[StringEntry]) -> Result<InjectionReport> {
                Err(LocustError::InjectionError("plugin refused".into()))
            }
        }
        let (mut injector, game, output) = setup_contained_injector();
        let mut registry = FormatRegistry::new();
        registry.register(Box::new(FailInject));
        injector.registry = Arc::new(registry);
        let (tx, _rx) = mpsc::channel(10);
        let report = injector
            .inject(
                &game,
                "fail-replace",
                OutputMode::Replace,
                vec!["es".into()],
                Some(output),
                tx,
            )
            .await
            .unwrap();
        assert_eq!(report.languages_failed.len(), 1);
        assert!(report.languages_processed.is_empty());
        assert!(report.recording_outcomes.is_empty());
        assert!(injector.db.get_injection(Some("es")).unwrap().is_none());
    }

    #[tokio::test]
    async fn replace_zero_write_preserves_previous_provenance() {
        struct ZeroWrite;
        impl FormatPlugin for ZeroWrite {
            fn id(&self) -> &str {
                "zero-replace"
            }
            fn name(&self) -> &str {
                "Zero Replace"
            }
            fn supported_extensions(&self) -> &[&str] {
                &[".mock"]
            }
            fn extract(&self, _: &Path) -> Result<Vec<StringEntry>> {
                Ok(vec![])
            }
            fn inject(&self, _: &Path, _: &[StringEntry]) -> Result<InjectionReport> {
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
        let (injector, game, output) = setup_contained_injector();
        let (tx, _rx) = mpsc::channel(10);
        let first = injector
            .inject(
                &game,
                "contained",
                OutputMode::Replace,
                vec!["es".into()],
                Some(output.clone()),
                tx,
            )
            .await
            .unwrap();
        let previous = injector.db.get_injection(Some("es")).unwrap().unwrap();
        assert_eq!(
            first.recording_outcomes.get("es"),
            Some(&RecordOutcome::Recorded { files: 1 })
        );

        let mut registry = FormatRegistry::new();
        registry.register(Box::new(ZeroWrite));
        let zero = MultiLangInjector::new(
            Arc::new(registry),
            injector.db.clone(),
            injector.backup_manager.clone(),
        );
        let empty_out = tempdir().join("output-zero");
        fs::create_dir_all(&empty_out).unwrap();
        let (tx, _rx) = mpsc::channel(10);
        let second = zero
            .inject(
                &game,
                "zero-replace",
                OutputMode::Replace,
                vec!["es".into()],
                Some(empty_out),
                tx,
            )
            .await
            .unwrap();
        assert_eq!(
            second.recording_outcomes.get("es"),
            Some(&RecordOutcome::KeptPrevious {
                recorded_at: previous.recorded_at.clone()
            })
        );
        let kept = injector.db.get_injection(Some("es")).unwrap().unwrap();
        assert_eq!(
            serde_json::to_value(&kept).unwrap(),
            serde_json::to_value(&previous).unwrap(),
            "a zero-write Replace must not clobber prior recording or provenance"
        );
    }

    #[tokio::test]
    async fn replace_records_file_source_with_destination_root() {
        let parent = tempdir();
        let file = parent.join("game.mock");
        fs::write(&file, b"Hello").unwrap();
        let backup_root = tempdir().join("backups");
        let output_dir = tempdir().join("output");
        fs::create_dir_all(&output_dir).unwrap();
        let db = Arc::new(Database::open_in_memory().unwrap());
        let mut entries = vec![StringEntry::new("mock#0", "Hello", file.clone())];
        entries[0].translation = Some("Hola".into());
        db.save_entries(&entries).unwrap();
        let mut registry = FormatRegistry::new();
        registry.register(Box::new(ContainedMock));
        let injector = MultiLangInjector::new(
            Arc::new(registry),
            db,
            Arc::new(BackupManager::new(backup_root.clone())),
        );
        let (tx, _rx) = mpsc::channel(10);
        let report = injector
            .inject(
                &file,
                "contained",
                OutputMode::Replace,
                vec!["es".into()],
                Some(output_dir.clone()),
                tx,
            )
            .await
            .unwrap();
        assert!(
            report.languages_failed.is_empty(),
            "{:?}",
            report.languages_failed
        );
        let dest = output_dir.join("game.mock-es");
        assert_eq!(report.injected_roots.get("es"), Some(&dest));
        let rec = injector.db.get_injection(Some("es")).unwrap().unwrap();
        assert!(crate::database::paths_identical(&rec.root, &dest));
        let backup = rec.pristine_backup.expect("file-source provenance");
        assert!(
            crate::database::paths_identical(&backup.source_path, &file),
            "source_path must be the selected file, got {}",
            backup.source_path.display()
        );
        assert_eq!(
            backup.storage_root.as_ref(),
            Some(&std::path::absolute(&backup_root).unwrap())
        );
        assert_eq!(fs::read(&file).unwrap(), b"Hello");
    }

    #[test]
    fn replace_companion_refuses_missing_recording_outcomes() {
        let db = Database::open_in_memory().unwrap();
        let mut reports = HashMap::new();
        reports.insert(
            "es".into(),
            InjectionReport {
                skip_reasons: Default::default(),
                files_modified: 1,
                strings_written: 1,
                strings_skipped: 0,
                warnings: Vec::new(),
                files_written: vec![PathBuf::from("game.injected")],
            },
        );
        let mut injected_roots = HashMap::new();
        injected_roots.insert("es".into(), PathBuf::from("copies/game-es"));
        let report = MultiLangReport {
            mode: OutputMode::Replace,
            languages_processed: vec!["es".into()],
            languages_failed: vec![],
            backup_id: "unused".into(),
            reports,
            injected_roots,
            recording_outcomes: HashMap::new(),
        };
        let err = record_multilang_injection(&db, &report, &["es".into()], &|_| "REMEDY".into())
            .unwrap_err();
        assert!(
            err.to_string()
                .contains("refusing to record possibly changed files after its transaction"),
            "{err}"
        );
        assert!(db.get_injection(Some("es")).unwrap().is_none());
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
        let (injector, game_dir, output_dir) = setup_contained_injector();
        let (tx, mut rx) = mpsc::channel(100);

        let report = injector
            .inject(
                &game_dir,
                "contained",
                OutputMode::Replace,
                vec!["es".to_string()],
                Some(output_dir),
                tx,
            )
            .await
            .unwrap();

        rx.close();
        while rx.recv().await.is_some() {}

        assert_eq!(report.mode, OutputMode::Replace);
        assert_eq!(report.languages_processed.len(), 1);
        assert!(report.languages_failed.is_empty());
        assert!(!report.backup_id.is_empty());
        assert!(report.reports.contains_key("es"));
        // Every processed language carries the root its injection targeted.
        assert!(report.injected_roots.contains_key("es"));
        assert_eq!(
            report.recording_outcomes.get("es"),
            Some(&RecordOutcome::Recorded { files: 1 })
        );
    }

    /// Writes the first translation over `target_rel` inside the game tree.
    /// Stands in for the 10 plugins `mutates_original_tree` used to skip —
    /// those all overwrite originals, same as this.
    struct InPlaceWritePlugin {
        id: &'static str,
        target_rel: &'static str,
    }

    #[test]
    fn noop_retention_reuses_original_backup_across_profiles() {
        let base = tempfile::tempdir().unwrap();
        let game = base.path().join("game");
        fs::create_dir(&game).unwrap();
        let source = game.join("story.txt");
        fs::write(&source, "Original").unwrap();
        let db = Database::open_in_memory().unwrap();
        let mut entry = StringEntry::new("line", "Original", source.clone());
        entry.translation = Some("Traducido".into());
        db.save_entries(&[entry]).unwrap();
        let mut registry = FormatRegistry::new();
        registry.register(Box::new(InPlaceWritePlugin {
            id: "fixture",
            target_rel: "story.txt",
        }));
        let first_store = BackupManager::new(base.path().join("first-store"));
        let next_store = BackupManager::new(base.path().join("next-store"));
        let other = base.path().join("other.txt");
        fs::write(&other, "Unrelated backup").unwrap();
        let unrelated = first_store.create_backup(&other).unwrap();
        let first = inject_direct(
            &registry,
            &db,
            &first_store,
            &game,
            "fixture",
            &["es".into()],
        )
        .unwrap();
        let saved = db.get_injection(Some("es")).unwrap();
        for store in [&first_store, &next_store, &next_store] {
            let report =
                inject_direct(&registry, &db, store, &game, "fixture", &["es".into()]).unwrap();
            assert_eq!(report.files_modified, 0);
            assert!(matches!(
                report.outcomes[0].1,
                RecordOutcome::KeptPrevious { .. }
            ));
            assert_eq!(db.get_injection(Some("es")).unwrap(), saved);
            assert_eq!(first_store.list_backups().unwrap().len(), 2);
            assert!(next_store.list_backups().unwrap().is_empty());
            assert_eq!(report.backup_id, first.backup_id);
            assert_eq!(report.backup_path, first.backup_path);
            assert_eq!(report.pristine_backup_id, first.pristine_backup_id);
            assert!(Path::new(report.backup_path.as_ref().unwrap()).is_dir());
        }
        assert_eq!(fs::read_to_string(source).unwrap(), "Traducido");
        assert!(unrelated.path.join("manifest.json").is_file());
    }

    #[test]
    fn noop_retention_first_identity_injection_has_no_dangling_backup() {
        let base = tempfile::tempdir().unwrap();
        let game = base.path().join("game");
        fs::create_dir(&game).unwrap();
        let source = game.join("story.txt");
        fs::write(&source, "Original").unwrap();
        let db = Database::open_in_memory().unwrap();
        let mut entry = StringEntry::new("line", "Original", source.clone());
        entry.translation = Some("Original".into());
        db.save_entries(&[entry]).unwrap();
        let mut registry = FormatRegistry::new();
        registry.register(Box::new(InPlaceWritePlugin {
            id: "fixture",
            target_rel: "story.txt",
        }));
        let store = BackupManager::new(base.path().join("backups"));
        let report = inject_direct(&registry, &db, &store, &game, "fixture", &[]).unwrap();
        assert_eq!(report.files_modified, 0);
        assert!(matches!(
            report.outcomes[0].1,
            RecordOutcome::NothingRecorded
        ));
        assert!(store.list_backups().unwrap().is_empty());
        assert!(report.backup_id.is_empty());
        assert!(report.backup_path.is_none());
        assert!(report.pristine_backup_id.is_none());
        assert!(db.get_injection(None).unwrap().is_none());
        assert_eq!(fs::read_to_string(source).unwrap(), "Original");
    }

    #[test]
    fn noop_retention_keeps_a_backup_referenced_by_another_language() {
        let base = tempfile::tempdir().unwrap();
        let game = base.path().join("game");
        fs::create_dir(&game).unwrap();
        let source = game.join("story.txt");
        fs::write(&source, "Original").unwrap();
        let db = Database::open_in_memory().unwrap();
        let manager = BackupManager::new(base.path().join("backups"));
        let entry = manager.create_backup(&game).unwrap();
        let provenance = RecordedBackup {
            id: entry.id.clone(),
            source_path: entry.source_path.clone(),
            storage_root: Some(manager.root().to_path_buf()),
        };
        db.record_injection_with_backup(Some("fr"), &game, &[source], Some(&provenance))
            .unwrap();
        let mut run = DirectBackupRun {
            report_backup: Some((entry.id.clone(), entry.path.clone())),
            entry,
            prior: None,
        };
        let mut report = InPlaceWritePlugin {
            id: "fixture",
            target_rel: "story.txt",
        }
        .inject(&game, &[])
        .unwrap();
        let _lock = crate::patch::GameLock::acquire(&game).unwrap();
        discard_direct_noop_backup(
            &db,
            &manager,
            &mut run,
            &mut report,
            &[("es".into(), RecordOutcome::NothingRecorded)],
        )
        .unwrap();
        assert!(run.entry.path.join("manifest.json").is_file());
        assert_eq!(run.report_backup.as_ref().unwrap().0, provenance.id);
        assert!(report
            .warnings
            .iter()
            .any(|s| s.contains("recording references")));
    }

    #[test]
    #[cfg(windows)]
    fn noop_retention_cleanup_failure_never_reports_the_damaged_duplicate() {
        use std::os::windows::fs::OpenOptionsExt;
        use std::sync::{Arc, Mutex};
        struct LockDuplicate {
            store: PathBuf,
            held: Arc<Mutex<Option<fs::File>>>,
        }
        impl FormatPlugin for LockDuplicate {
            fn id(&self) -> &str {
                "fixture"
            }
            fn name(&self) -> &str {
                "fixture"
            }
            fn supported_extensions(&self) -> &[&str] {
                &[]
            }
            fn extract(&self, _: &Path) -> Result<Vec<StringEntry>> {
                Ok(vec![])
            }
            fn inject(&self, path: &Path, entries: &[StringEntry]) -> Result<InjectionReport> {
                let manager = BackupManager::new(self.store.clone());
                let backup = manager.list_backups()?.remove(0);
                let file = fs::OpenOptions::new()
                    .read(true)
                    .share_mode(1 | 2)
                    .open(backup.path.join("manifest.json"))?;
                *self.held.lock().unwrap() = Some(file);
                InPlaceWritePlugin {
                    id: "fixture",
                    target_rel: "story.txt",
                }
                .inject(path, entries)
            }
        }
        let base = tempfile::tempdir().unwrap();
        let game = base.path().join("game");
        fs::create_dir(&game).unwrap();
        let source = game.join("story.txt");
        fs::write(&source, "Original").unwrap();
        let db = Database::open_in_memory().unwrap();
        let mut entry = StringEntry::new("line", "Original", source.clone());
        entry.translation = Some("Traducido".into());
        db.save_entries(&[entry]).unwrap();
        let store = BackupManager::new(base.path().join("backups"));
        let mut registry = FormatRegistry::new();
        registry.register(Box::new(InPlaceWritePlugin {
            id: "fixture",
            target_rel: "story.txt",
        }));
        let first =
            inject_direct(&registry, &db, &store, &game, "fixture", &["es".into()]).unwrap();
        let saved = db.get_injection(Some("es")).unwrap();
        let held = Arc::new(Mutex::new(None));
        let mut locked_registry = FormatRegistry::new();
        locked_registry.register(Box::new(LockDuplicate {
            store: store.root().to_path_buf(),
            held: held.clone(),
        }));
        let report = inject_direct(
            &locked_registry,
            &db,
            &store,
            &game,
            "fixture",
            &["es".into()],
        )
        .unwrap();
        assert!(report
            .warnings
            .iter()
            .any(|s| s.contains("cleanup was incomplete")));
        assert_eq!(report.backup_id, first.backup_id);
        assert_eq!(report.backup_path, first.backup_path);
        assert_eq!(db.get_injection(Some("es")).unwrap(), saved);
        assert_eq!(fs::read_to_string(source).unwrap(), "Traducido");
        assert_eq!(
            fs::read_to_string(
                Path::new(report.backup_path.as_ref().unwrap()).join("payload/story.txt")
            )
            .unwrap(),
            "Original"
        );
        held.lock().unwrap().take();
    }

    impl FormatPlugin for InPlaceWritePlugin {
        fn id(&self) -> &str {
            self.id
        }
        fn name(&self) -> &str {
            self.id
        }
        fn supported_extensions(&self) -> &[&str] {
            &[]
        }
        fn extract(&self, _: &Path) -> Result<Vec<StringEntry>> {
            Ok(vec![])
        }
        fn inject(&self, path: &Path, entries: &[StringEntry]) -> Result<InjectionReport> {
            let target = path.join(self.target_rel);
            let Some(text) = entries.iter().find_map(|e| e.translation.as_deref()) else {
                return Ok(InjectionReport {
                    skip_reasons: Default::default(),
                    files_modified: 0,
                    strings_written: 0,
                    strings_skipped: 0,
                    warnings: Vec::new(),
                    files_written: Vec::new(),
                });
            };
            fs::write(&target, text.as_bytes())?;
            Ok(InjectionReport {
                skip_reasons: Default::default(),
                files_modified: 1,
                strings_written: 1,
                strings_skipped: 0,
                warnings: Vec::new(),
                files_written: vec![target],
            })
        }
    }

    fn copy_tree(src: &Path, dst: &Path) {
        fs::create_dir_all(dst).unwrap();
        for entry in WalkDir::new(src).follow_links(false) {
            let entry = entry.unwrap();
            let rel = entry.path().strip_prefix(src).unwrap();
            let dest = dst.join(rel);
            if entry.file_type().is_dir() {
                fs::create_dir_all(&dest).unwrap();
            } else if entry.file_type().is_file() {
                if let Some(parent) = dest.parent() {
                    fs::create_dir_all(parent).unwrap();
                }
                fs::copy(entry.path(), &dest).unwrap();
            }
        }
    }

    fn rpgmaker_mv_fixture_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("formats")
            .join("tests")
            .join("fixtures")
            .join("rpgmaker_mv")
    }

    #[test]
    fn test_inject_direct_backs_up_rpgmaker_mv_fixture_before_write() {
        // rpgmaker-mv was not in the stale mutates_original_tree list, so
        // direct inject used to write the fixture with backup_id "none".
        let fixture = rpgmaker_mv_fixture_dir();
        let actors_rel = Path::new("data").join("Actors.json");
        assert!(
            fixture.join(&actors_rel).is_file(),
            "expected RPG Maker MV fixture at {}",
            fixture.display()
        );

        let game_dir = tempdir().join("rpg");
        copy_tree(&fixture, &game_dir);
        let actors = game_dir.join(&actors_rel);
        let original = fs::read(&actors).unwrap();

        let db = Database::open_in_memory().unwrap();
        let mut entry = StringEntry::new("Actors.json#1#name", "Hero", actors.clone());
        entry.translation = Some("Héroe".into());
        db.save_entries(&[entry]).unwrap();

        let mut registry = FormatRegistry::new();
        registry.register(Box::new(InPlaceWritePlugin {
            id: "rpgmaker-mv",
            target_rel: "data/Actors.json",
        }));
        let mgr = BackupManager::new(tempdir().join("bak"));

        let report = inject_direct(
            &registry,
            &db,
            &mgr,
            &game_dir,
            "rpgmaker-mv",
            &["es".into()],
        )
        .unwrap();

        assert_ne!(report.backup_id, "none", "must be a real backup id");
        assert!(!report.backup_id.is_empty());
        let backup_path = report
            .backup_path
            .as_deref()
            .expect("backup_path must be Some");
        let backed = Path::new(backup_path).join("payload").join(&actors_rel);
        assert!(backed.is_file(), "backup must contain {actors_rel:?}");
        let backed_bytes = fs::read(&backed).unwrap();
        assert_eq!(
            backed_bytes, original,
            "backup must hold the pre-inject fixture bytes"
        );
        let after = fs::read(&actors).unwrap();
        assert_eq!(after, "Héroe".as_bytes());
        assert_ne!(
            backed_bytes, after,
            "backup taken before the write, not after"
        );
    }

    #[test]
    fn test_inject_direct_backs_up_kirikiri_before_write() {
        // Same stale-list miss as RPG Maker — KiriKiri inject overwrites .ks.
        let game_dir = tempdir().join("krkr");
        fs::create_dir_all(&game_dir).unwrap();
        let ks = game_dir.join("scenario.ks");
        let original = b"; comment\n*start\nHello, world!\nThis is narration.\n";
        fs::write(&ks, original).unwrap();

        let db = Database::open_in_memory().unwrap();
        let mut entry = StringEntry::new("scenario.ks#3", "Hello, world!", ks.clone());
        entry.translation = Some("Hola, mundo!".into());
        db.save_entries(&[entry]).unwrap();

        let mut registry = FormatRegistry::new();
        registry.register(Box::new(InPlaceWritePlugin {
            id: "kirikiri",
            target_rel: "scenario.ks",
        }));
        let mgr = BackupManager::new(tempdir().join("bak"));

        let report =
            inject_direct(&registry, &db, &mgr, &game_dir, "kirikiri", &["es".into()]).unwrap();

        assert_ne!(report.backup_id, "none");
        let backup_path = report
            .backup_path
            .as_deref()
            .expect("backup_path must be Some");
        let backed = Path::new(backup_path).join("payload").join("scenario.ks");
        let backed_bytes = fs::read(&backed).unwrap();
        assert_eq!(backed_bytes, original);
        let after = fs::read(&ks).unwrap();
        assert_eq!(after, b"Hola, mundo!");
        assert_ne!(backed_bytes, after);
    }

    #[test]
    fn test_inject_direct_backup_failure_refuses_and_does_not_write() {
        // A previously excluded format used to skip backup entirely, so a
        // broken backup root still let inject overwrite the game.
        let game_dir = tempdir().join("rpg");
        let data = game_dir.join("data");
        fs::create_dir_all(&data).unwrap();
        let actors = data.join("Actors.json");
        fs::write(&actors, b"ORIGINAL").unwrap();

        let db = Database::open_in_memory().unwrap();
        let mut entry = StringEntry::new("Actors.json#1#name", "Hero", actors.clone());
        entry.translation = Some("Héroe".into());
        db.save_entries(&[entry]).unwrap();

        let mut registry = FormatRegistry::new();
        registry.register(Box::new(InPlaceWritePlugin {
            id: "rpgmaker-mv",
            target_rel: "data/Actors.json",
        }));
        let bad_root = tempdir().join("not_a_dir");
        fs::write(&bad_root, b"occupied").unwrap();
        let mgr = BackupManager::new(bad_root);

        let err = inject_direct(
            &registry,
            &db,
            &mgr,
            &game_dir,
            "rpgmaker-mv",
            &["es".into()],
        )
        .expect_err("a failed backup must refuse the inject");
        assert!(
            err.to_string().contains("without a backup"),
            "refusal must say why the backup matters: {err}"
        );
        assert_eq!(
            fs::read(&actors).unwrap(),
            b"ORIGINAL",
            "inject must not write after a failed backup"
        );
    }

    struct SourceMatchWriter;

    struct RevisionReader;
    impl FormatPlugin for RevisionReader {
        fn id(&self) -> &str {
            "source-match"
        }
        fn name(&self) -> &str {
            "revision-reader"
        }
        fn supported_extensions(&self) -> &[&str] {
            &["html"]
        }
        fn extract(&self, _: &Path) -> Result<Vec<StringEntry>> {
            Ok(vec![])
        }
        fn inject(&self, path: &Path, entries: &[StringEntry]) -> Result<InjectionReport> {
            SourceMatchWriter.inject(path, entries)
        }
        fn prepare_revision_entries(
            &self,
            _: &mut [StringEntry],
            originals: &HashMap<PathBuf, RevisionOriginal>,
        ) -> Result<()> {
            for original in originals.values() {
                let _ = original.read_text()?;
            }
            Ok(())
        }
    }

    fn direct_revision_mutation_refused(mutate: fn(&Path), single_file: bool) {
        let (game, one, _) = two_html_game();
        let selection = if single_file { &one } else { &game };
        let db = Database::open_in_memory().unwrap();
        save_translated(&db, "one", "ONE", PathBuf::from("one.html"), "Uno");
        let mgr = BackupManager::new(tempdir().join("backups"));
        let first = inject_direct(
            &direct_html_registry(),
            &db,
            &mgr,
            selection,
            "source-match",
            &["es".into()],
        )
        .unwrap();
        let before = db.get_injection(Some("es")).unwrap();
        let mut registry = FormatRegistry::new();
        registry.register(Box::new(RevisionReader));
        DIRECT_REVISION_AFTER_VERIFY.with(|hook| hook.set(Some(mutate)));
        let result = inject_direct(
            &registry,
            &db,
            &mgr,
            selection,
            "source-match",
            &["es".into()],
        );
        DIRECT_REVISION_AFTER_VERIFY.with(|hook| assert!(hook.take().is_none()));
        assert!(result.is_err(), "changed original accepted: {result:?}");
        assert_eq!(fs::read(&one).unwrap(), b"Uno");
        assert_eq!(db.get_injection(Some("es")).unwrap(), before);
        assert!(Path::new(first.backup_path.as_ref().unwrap()).is_dir());
        assert_eq!(
            mgr.list_backups().unwrap().len(),
            2,
            "failed attempt retains recovery backup"
        );
        crate::injection_transaction::ensure_no_pending(selection).unwrap();
    }

    #[test]
    fn direct_revision_original_same_size_drift_refused() {
        direct_revision_mutation_refused(
            |tree| fs::write(tree.join("one.html"), b"BAD").unwrap(),
            false,
        );
    }
    #[test]
    fn direct_revision_original_growth_refused() {
        direct_revision_mutation_refused(
            |tree| fs::write(tree.join("one.html"), b"GROWN").unwrap(),
            false,
        );
    }
    #[test]
    fn direct_revision_original_shrink_refused() {
        direct_revision_mutation_refused(
            |tree| fs::write(tree.join("one.html"), b"O").unwrap(),
            false,
        );
    }
    #[test]
    fn direct_revision_original_deleted_refused() {
        direct_revision_mutation_refused(
            |tree| fs::remove_file(tree.join("one.html")).unwrap(),
            false,
        );
    }
    #[test]
    fn direct_revision_original_single_file_drift_refused() {
        direct_revision_mutation_refused(
            |tree| fs::write(tree.join("one.html"), b"BAD").unwrap(),
            true,
        );
    }

    impl FormatPlugin for SourceMatchWriter {
        fn id(&self) -> &str {
            "source-match"
        }
        fn name(&self) -> &str {
            "source-match"
        }
        fn supported_extensions(&self) -> &[&str] {
            &[".html"]
        }
        fn extract(&self, _: &Path) -> Result<Vec<StringEntry>> {
            Ok(vec![])
        }
        fn inject(&self, _: &Path, entries: &[StringEntry]) -> Result<InjectionReport> {
            let mut files = Vec::new();
            let mut skipped = 0;
            for entry in entries {
                let Some(text) = entry.translation.as_deref() else {
                    skipped += 1;
                    continue;
                };
                match fs::read(&entry.file_path) {
                    Ok(current) if current == entry.source.as_bytes() => {
                        fs::write(&entry.file_path, text.as_bytes())?;
                        files.push(entry.file_path.clone());
                    }
                    Ok(_) => skipped += 1,
                    Err(error) => return Err(error.into()),
                }
            }
            Ok(InjectionReport {
                skip_reasons: Default::default(),
                files_modified: files.len(),
                strings_written: files.len(),
                strings_skipped: skipped,
                warnings: Vec::new(),
                files_written: files,
            })
        }
    }

    fn two_html_game() -> (PathBuf, PathBuf, PathBuf) {
        let game = tempdir().join("game");
        fs::create_dir_all(&game).unwrap();
        let one = game.join("one.html");
        let two = game.join("two.html");
        fs::write(&one, b"ONE").unwrap();
        fs::write(&two, b"TWO").unwrap();
        (game, one, two)
    }

    fn direct_html_registry() -> FormatRegistry {
        let mut registry = FormatRegistry::new();
        registry.register(Box::new(SourceMatchWriter));
        registry
    }

    fn save_translated(db: &Database, id: &str, source: &str, path: PathBuf, translation: &str) {
        let mut entry = StringEntry::new(id, source, path);
        entry.translation = Some(translation.into());
        db.save_entries(&[entry]).unwrap();
    }

    fn recorded_rels(db: &Database) -> Vec<String> {
        db.get_injection(Some("es"))
            .unwrap()
            .expect("recording")
            .files
            .into_iter()
            .map(|file| file.rel)
            .collect()
    }

    fn payload_file(backup_path: &str, rel: &str) -> PathBuf {
        Path::new(backup_path).join("payload").join(rel)
    }

    #[test]
    fn direct_first_injection_records_written_files_and_new_backup_as_pristine() {
        let (game, one, two) = two_html_game();
        let original_one = fs::read(&one).unwrap();
        let original_two = fs::read(&two).unwrap();
        let db = Database::open_in_memory().unwrap();
        save_translated(&db, "one", "ONE", PathBuf::from("one.html"), "Uno");
        let mgr = BackupManager::new(tempdir().join("bak"));
        let report = inject_direct(
            &direct_html_registry(),
            &db,
            &mgr,
            &game,
            "source-match",
            &["es".into()],
        )
        .unwrap();

        assert_eq!(report.files_written.len(), 1);
        assert_eq!(
            report.outcomes,
            vec![("es".into(), RecordOutcome::Recorded { files: 1 })]
        );
        assert_eq!(
            report.pristine_backup_id.as_deref(),
            Some(report.backup_id.as_str())
        );
        let rec = db.get_injection(Some("es")).unwrap().unwrap();
        assert!(crate::database::paths_identical(&rec.root, &game));
        assert_eq!(recorded_rels(&db), vec!["one.html".to_string()]);
        assert_eq!(rec.pristine_backup.as_ref().unwrap().id, report.backup_id);
        assert_eq!(fs::read(&one).unwrap(), b"Uno");
        assert_eq!(fs::read(&two).unwrap(), original_two);
        assert_eq!(
            fs::read(payload_file(
                report.backup_path.as_deref().unwrap(),
                "one.html"
            ))
            .unwrap(),
            original_one
        );
    }

    #[test]
    fn direct_partial_injections_keep_union_and_original_provenance() {
        let (game, one, two) = two_html_game();
        let original_one = fs::read(&one).unwrap();
        let original_two = fs::read(&two).unwrap();
        let first_store = tempdir().join("store1");
        let db = Database::open_in_memory().unwrap();
        save_translated(&db, "one", "ONE", PathBuf::from("one.html"), "Uno");
        let first_mgr = BackupManager::new(first_store.clone());
        let first = inject_direct(
            &direct_html_registry(),
            &db,
            &first_mgr,
            &game,
            "source-match",
            &["es".into()],
        )
        .unwrap();
        let original_id = first.pristine_backup_id.clone().unwrap();
        let original_path = first.backup_path.clone().unwrap();

        save_translated(&db, "two", "TWO", PathBuf::from("two.html"), "Dos");
        let second_mgr = BackupManager::new(tempdir().join("store2"));
        let decoy = second_mgr.create_backup(&game).unwrap();
        let second = inject_direct(
            &direct_html_registry(),
            &db,
            &second_mgr,
            &game,
            "source-match",
            &["es".into()],
        )
        .unwrap();

        assert_eq!(second.files_written.len(), 1);
        assert!(
            second
                .files_written
                .iter()
                .any(|path| path.file_name() == Some(std::ffi::OsStr::new("two.html"))),
            "this run must report only the newly written file: {:?}",
            second.files_written
        );
        assert_eq!(
            second.outcomes,
            vec![("es".into(), RecordOutcome::Recorded { files: 2 })]
        );
        assert_eq!(
            second.pristine_backup_id.as_deref(),
            Some(original_id.as_str())
        );
        assert_ne!(second.backup_id, original_id, "report backup is new A");
        assert_ne!(
            second.backup_id, decoy.id,
            "must not pick a later store leftover"
        );
        let rec = db.get_injection(Some("es")).unwrap().unwrap();
        let mut rels = recorded_rels(&db);
        rels.sort();
        assert_eq!(rels, vec!["one.html".to_string(), "two.html".to_string()]);
        let provenance = rec.pristine_backup.expect("kept O provenance");
        assert_eq!(provenance.id, original_id);
        assert_eq!(
            provenance.storage_root.as_ref(),
            Some(&std::path::absolute(&first_store).unwrap())
        );
        assert_eq!(fs::read(&one).unwrap(), b"Uno");
        assert_eq!(fs::read(&two).unwrap(), b"Dos");
        assert_eq!(
            fs::read(payload_file(&original_path, "one.html")).unwrap(),
            original_one
        );
        assert_eq!(
            fs::read(payload_file(&original_path, "two.html")).unwrap(),
            original_two
        );
        let a_path = second.backup_path.as_deref().unwrap();
        assert_eq!(fs::read(payload_file(a_path, "one.html")).unwrap(), b"Uno");
        assert_eq!(
            fs::read(payload_file(a_path, "two.html")).unwrap(),
            original_two
        );
    }

    #[test]
    fn direct_drifted_prior_file_refuses_before_new_write() {
        let (game, one, two) = two_html_game();
        let original_two = fs::read(&two).unwrap();
        let db = Database::open_in_memory().unwrap();
        save_translated(&db, "one", "ONE", PathBuf::from("one.html"), "Uno");
        let mgr = BackupManager::new(tempdir().join("bak"));
        inject_direct(
            &direct_html_registry(),
            &db,
            &mgr,
            &game,
            "source-match",
            &["es".into()],
        )
        .unwrap();
        fs::write(&one, b"tampered").unwrap();
        save_translated(&db, "two", "TWO", PathBuf::from("two.html"), "Dos");
        let err = inject_direct(
            &direct_html_registry(),
            &db,
            &mgr,
            &game,
            "source-match",
            &["es".into()],
        )
        .unwrap_err();
        assert!(
            err.to_string()
                .contains("no longer matches its recorded hash/size"),
            "{err}"
        );
        assert_eq!(fs::read(&two).unwrap(), original_two);
        assert_eq!(fs::read(&one).unwrap(), b"tampered");
        assert_eq!(recorded_rels(&db), vec!["one.html".to_string()]);
    }

    #[test]
    fn direct_invalid_original_backup_refuses_before_writes() {
        for kind in ["missing", "corrupt", "wrong"] {
            let (game, one, two) = two_html_game();
            let original_two = fs::read(&two).unwrap();
            let db = Database::open_in_memory().unwrap();
            save_translated(&db, "one", "ONE", PathBuf::from("one.html"), "Uno");
            let mgr = BackupManager::new(tempdir().join("bak"));
            let first = inject_direct(
                &direct_html_registry(),
                &db,
                &mgr,
                &game,
                "source-match",
                &["es".into()],
            )
            .unwrap();
            let original_path = PathBuf::from(first.backup_path.as_ref().unwrap());
            match kind {
                "missing" => fs::remove_dir_all(&original_path).unwrap(),
                "corrupt" => {
                    fs::write(original_path.join("payload").join("one.html"), b"CORRUPT").unwrap();
                }
                "wrong" => {
                    let other = tempdir().join("other");
                    fs::create_dir(&other).unwrap();
                    fs::write(other.join("n.txt"), b"nope").unwrap();
                    let wrong = mgr.create_backup(&other).unwrap();
                    fs::remove_dir_all(&original_path).unwrap();
                    copy_tree(&wrong.path, &original_path);
                }
                _ => unreachable!(),
            }
            save_translated(&db, "two", "TWO", PathBuf::from("two.html"), "Dos");
            let err = inject_direct(
                &direct_html_registry(),
                &db,
                &mgr,
                &game,
                "source-match",
                &["es".into()],
            )
            .expect_err(kind);
            let message = err.to_string();
            assert!(
                message.contains("backup")
                    || message.contains("manifest")
                    || message.contains("inventory")
                    || message.contains("different game"),
                "{kind}: {message}"
            );
            assert_eq!(
                fs::read(&two).unwrap(),
                original_two,
                "{kind} must not write two.html"
            );
            assert_eq!(
                fs::read(&one).unwrap(),
                b"Uno",
                "{kind} must not rewrite one.html"
            );
            assert_eq!(recorded_rels(&db), vec!["one.html".to_string()], "{kind}");
        }
    }

    #[test]
    fn direct_root_mismatch_starts_new_generation() {
        let (game_a, one_a, two_a) = two_html_game();
        let (game_b, one_b, two_b) = two_html_game();
        let db = Database::open_in_memory().unwrap();
        save_translated(&db, "one", "ONE", PathBuf::from("one.html"), "Uno");
        save_translated(&db, "two", "TWO", PathBuf::from("two.html"), "Dos");
        let mgr = BackupManager::new(tempdir().join("bak"));
        let first = inject_direct(
            &direct_html_registry(),
            &db,
            &mgr,
            &game_a,
            "source-match",
            &["es".into()],
        )
        .unwrap();
        let first_id = first.pristine_backup_id.clone().unwrap();
        let after_a_one = fs::read(&one_a).unwrap();
        let after_a_two = fs::read(&two_a).unwrap();
        let second = inject_direct(
            &direct_html_registry(),
            &db,
            &mgr,
            &game_b,
            "source-match",
            &["es".into()],
        )
        .unwrap();
        let rec = db.get_injection(Some("es")).unwrap().unwrap();
        assert!(crate::database::paths_identical(&rec.root, &game_b));
        assert!(!crate::database::paths_identical(&rec.root, &game_a));
        assert_eq!(rec.pristine_backup.as_ref().unwrap().id, second.backup_id);
        assert_ne!(second.backup_id, first_id);
        let mut rels = recorded_rels(&db);
        rels.sort();
        assert_eq!(rels, vec!["one.html".to_string(), "two.html".to_string()]);
        assert_eq!(fs::read(&one_b).unwrap(), b"Uno");
        assert_eq!(fs::read(&two_b).unwrap(), b"Dos");
        assert_eq!(fs::read(&one_a).unwrap(), after_a_one);
        assert_eq!(fs::read(&two_a).unwrap(), after_a_two);
        assert_eq!(
            fs::read(payload_file(
                first.backup_path.as_deref().unwrap(),
                "one.html"
            ))
            .unwrap(),
            b"ONE"
        );
    }

    #[test]
    fn direct_noop_keeps_full_prior_recording() {
        let (game, one, two) = two_html_game();
        let db = Database::open_in_memory().unwrap();
        save_translated(&db, "one", "ONE", PathBuf::from("one.html"), "Uno");
        save_translated(&db, "two", "TWO", PathBuf::from("two.html"), "Dos");
        let mgr = BackupManager::new(tempdir().join("bak"));
        let first = inject_direct(
            &direct_html_registry(),
            &db,
            &mgr,
            &game,
            "source-match",
            &["es".into()],
        )
        .unwrap();
        let before = db.get_injection(Some("es")).unwrap().unwrap();
        let second = inject_direct(
            &direct_html_registry(),
            &db,
            &mgr,
            &game,
            "source-match",
            &["es".into()],
        )
        .unwrap();
        assert!(second.files_written.is_empty());
        assert_eq!(
            second.outcomes,
            vec![(
                "es".into(),
                RecordOutcome::KeptPrevious {
                    recorded_at: before.recorded_at.clone()
                }
            )]
        );
        assert_eq!(
            second.backup_id, first.backup_id,
            "no-op reports the retained original, not its discarded duplicate"
        );
        assert_eq!(mgr.list_backups().unwrap().len(), 1);
        assert_eq!(
            second.pristine_backup_id.as_deref(),
            Some(first.backup_id.as_str())
        );
        let after = db.get_injection(Some("es")).unwrap().unwrap();
        assert_eq!(after.recorded_at, before.recorded_at);
        assert_eq!(after.pristine_backup, before.pristine_backup);
        let mut rels = recorded_rels(&db);
        rels.sort();
        assert_eq!(rels, vec!["one.html".to_string(), "two.html".to_string()]);
        assert_eq!(fs::read(&one).unwrap(), b"Uno");
        assert_eq!(fs::read(&two).unwrap(), b"Dos");
    }

    #[test]
    fn direct_selected_file_refuses_sibling_folder_inventory() {
        let (game, one, two) = two_html_game();
        let original_two = fs::read(&two).unwrap();
        let db = Database::open_in_memory().unwrap();
        save_translated(&db, "one", "ONE", PathBuf::from("one.html"), "Uno");
        let mgr = BackupManager::new(tempdir().join("bak"));
        inject_direct(
            &direct_html_registry(),
            &db,
            &mgr,
            &game,
            "source-match",
            &["es".into()],
        )
        .unwrap();
        // Retain the injection inventory while letting the current entries
        // pass selection preflight, so this tests the prior-inventory guard.
        db.clear_entries().unwrap();
        save_translated(&db, "two", "TWO", PathBuf::from("two.html"), "Dos");
        let err = inject_direct(
            &direct_html_registry(),
            &db,
            &mgr,
            &two,
            "source-match",
            &["es".into()],
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("outside the selected file"),
            "{err}"
        );
        assert_eq!(fs::read(&two).unwrap(), original_two);
        assert_eq!(fs::read(&one).unwrap(), b"Uno");
        assert_eq!(recorded_rels(&db), vec!["one.html".to_string()]);
    }
}
