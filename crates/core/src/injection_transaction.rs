//! Recoverable Direct/Add injection, independent of installed ZIP patch receipts.
//!
//! Plugins run in an independent full copy: they may write in place, discover
//! extra containers, and emit unreported files. Only the inventory difference is
//! installed. Originals and a durable plan precede every game mutation. This
//! costs a full work copy plus changed-file originals, in addition to the legacy
//! user backup. No hardlinks are used. Recovery never writes a project database.
use crate::{
    database::sha256_file,
    error::{LocustError, Result},
    extraction::InjectionReport,
    patch::{
        zipsec::{ensure_no_links, safe_stored_rel},
        GameLock, PatchStore,
    },
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    path::{Path, PathBuf},
};

pub const STORE_DIR: &str = ".locust-injections";
const SCHEMA: u32 = 1;
const MAX_METADATA_BYTES: u64 = 64 * 1024 * 1024;
const MAX_FILES: usize = 200_000;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InjectionPhase {
    Preparing,
    Applying,
    CommittedUnrecorded,
    RollingBack,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoveryConflict {
    pub path: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingInjection {
    pub transaction_id: String,
    pub phase: InjectionPhase,
    pub format: String,
    pub language: Option<String>,
    pub changed_files: usize,
    pub backup_path: PathBuf,
    pub conflicts: Vec<RecoveryConflict>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InjectionStatus {
    pub game_root: PathBuf,
    pub pending: Option<PendingInjection>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct InjectionRecoveryOptions {
    #[serde(default)]
    pub force: bool,
    /// Bind a reviewed recovery to its operation. Checked under the game lock,
    /// before any mutation; a stale UI must never recover a newer injection.
    #[serde(default)]
    pub expected_transaction_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InjectionRecoveryReport {
    pub transaction_id: Option<String>,
    pub restored: usize,
    pub removed: usize,
    pub preserved_conflicts: Vec<PathBuf>,
    pub messages: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct Fingerprint {
    sha256: String,
    size: u64,
    readonly: bool,
    unix_mode: Option<u32>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
struct Inventory {
    files: BTreeMap<String, Fingerprint>,
    dirs: BTreeSet<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoreMarker {
    schema_version: u32,
    kind: String,
    game_root: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Active {
    schema_version: u32,
    transaction_id: String,
    game_root: PathBuf,
    format: String,
    language: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Phase {
    Preparing,
    Applying,
    CommittedUnrecorded,
    RollingBack,
    Completed,
    Aborted,
    RolledBack,
}

impl Phase {
    fn pending(&self) -> Option<InjectionPhase> {
        match self {
            Self::Preparing => Some(InjectionPhase::Preparing),
            Self::Applying => Some(InjectionPhase::Applying),
            Self::CommittedUnrecorded => Some(InjectionPhase::CommittedUnrecorded),
            Self::RollingBack => Some(InjectionPhase::RollingBack),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Change {
    path: String,
    original: Option<Fingerprint>,
    result: Fingerprint,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Plan {
    schema_version: u32,
    transaction_id: String,
    game_root: PathBuf,
    /// Missing in old journals: they cannot establish a backup time boundary.
    #[serde(default)]
    prepared_at: Option<chrono::DateTime<chrono::Utc>>,
    files: Vec<Change>,
    created_dirs: Vec<String>,
}

#[derive(Debug)]
struct Operation {
    root: PathBuf,
    directory: PathBuf,
    active: Active,
    phase: Phase,
}

fn error(message: impl Into<String>) -> LocustError {
    LocustError::InjectionError(message.into())
}

fn root_for(selection: &Path) -> Result<PathBuf> {
    let selected = selection.canonicalize()?;
    if selected.is_dir() {
        Ok(selected)
    } else if selected.is_file() {
        Ok(selected
            .parent()
            .ok_or_else(|| error("selected file has no parent"))?
            .to_owned())
    } else {
        Err(error(
            "injection selection must be a regular file or directory",
        ))
    }
}

fn fingerprint(path: &Path) -> Result<Fingerprint> {
    let (sha256, size) = sha256_file(path)?;
    let permissions = fs::metadata(path)?.permissions();
    #[cfg(unix)]
    let unix_mode = {
        use std::os::unix::fs::PermissionsExt;
        Some(permissions.mode() & 0o7777)
    };
    #[cfg(not(unix))]
    let unix_mode = None;
    Ok(Fingerprint {
        sha256,
        size,
        readonly: permissions.readonly(),
        unix_mode,
    })
}

fn read<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    let meta = fs::symlink_metadata(path)?;
    if !meta.is_file() || meta.len() > MAX_METADATA_BYTES {
        return Err(error(format!(
            "invalid or oversized recovery metadata: {}",
            path.display()
        )));
    }
    ensure_no_links(
        path.parent()
            .ok_or_else(|| error("metadata has no parent"))?,
        Path::new(path.file_name().unwrap()),
    )?;
    serde_json::from_slice(&fs::read(path)?)
        .map_err(|e| error(format!("invalid recovery metadata {}: {e}", path.display())))
}

fn publish<T: Serialize>(root: &Path, path: &Path, value: &T) -> Result<()> {
    PatchStore::new(root).write_durable_json(path, value)
}

/// True only for a recognized game-bound store. Unknown similarly named user
/// directories are errors, never adopted, swept or excluded by filename alone.
pub fn validate_store(game_root: &Path) -> Result<bool> {
    let root = game_root.canonicalize()?;
    let store = root.join(STORE_DIR);
    ensure_no_links(&root, Path::new(STORE_DIR))?;
    match fs::symlink_metadata(&store) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e.into()),
        Ok(meta) if !meta.is_dir() => {
            return Err(error("injection recovery store is not a directory"))
        }
        Ok(_) => {}
    }
    let marker: StoreMarker = read(&store.join("store.json"))?;
    if marker.schema_version != SCHEMA
        || marker.kind != "locust-injection-store"
        || marker.game_root != root
    {
        return Err(error(
            "unrecognized injection recovery store; preserve it for diagnosis",
        ));
    }
    Ok(true)
}

fn load(root: &Path) -> Result<Option<Operation>> {
    if !validate_store(root)? {
        return Ok(None);
    }
    let active_path = root.join(STORE_DIR).join("active.json");
    if !active_path.try_exists()? {
        return Ok(None);
    }
    let active: Active = read(&active_path)?;
    if active.schema_version != SCHEMA
        || active.game_root != root
        || uuid::Uuid::parse_str(&active.transaction_id)
            .map(|v| v.to_string())
            .ok()
            .as_ref()
            != Some(&active.transaction_id)
    {
        return Err(error("invalid active injection identity/root"));
    }
    let rel = Path::new(STORE_DIR)
        .join("operations")
        .join(&active.transaction_id);
    ensure_no_links(root, &rel)?;
    let directory = root.join(rel);
    let phase = read(&directory.join("phase.json"))?;
    Ok(Some(Operation {
        root: root.to_owned(),
        directory,
        active,
        phase,
    }))
}

pub fn ensure_no_pending(game_path: &Path) -> Result<()> {
    let lock = GameLock::acquire(&root_for(game_path)?)?;
    ensure_no_pending_under_lock(&lock)
}

pub fn ensure_no_pending_under_lock(lock: &GameLock) -> Result<()> {
    if let Some(operation) = load(lock.root())? {
        if operation.phase.pending().is_some() {
            return Err(error(format!("injection {} is unfinished ({:?}); inspect injection status and recover before modifying this game", operation.active.transaction_id, operation.phase)));
        }
    }
    Ok(())
}

/// Evidence only, never inferred from a filename or from a plugin's report.
#[derive(Default)]
pub(crate) struct CreatedOutputs {
    pub files: BTreeSet<PathBuf>,
    pub hashes: BTreeMap<PathBuf, BTreeSet<(String, u64)>>,
    pub directories: BTreeSet<PathBuf>,
}

pub(crate) fn output_key(path: &Path) -> PathBuf {
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    {
        PathBuf::from(crate::database::fold_path_case(
            &path.to_string_lossy().replace('\\', "/"),
        ))
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    path.to_owned()
}

/// Read all completed operations under the caller's game lock. A plan records
/// absence (`original: None`) and every installed digest, including subsequent
/// injections into that file. Old undated plans grant no deletion authority.
pub(crate) fn created_outputs_since(
    lock: &GameLock,
    since: chrono::DateTime<chrono::Utc>,
) -> Result<CreatedOutputs> {
    let mut outputs = CreatedOutputs::default();
    if !validate_store(lock.root())? {
        return Ok(outputs);
    }
    let relative = Path::new(STORE_DIR).join("operations");
    ensure_no_links(lock.root(), &relative)?;
    let directory = lock.root().join(relative);
    if !directory.try_exists()? {
        return Ok(outputs);
    }
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let id = entry.file_name().to_string_lossy().into_owned();
        if uuid::Uuid::parse_str(&id)
            .map(|v| v.to_string())
            .ok()
            .as_ref()
            != Some(&id)
        {
            return Err(error("invalid injection history identity"));
        }
        ensure_no_links(
            lock.root(),
            &Path::new(STORE_DIR).join("operations").join(&id),
        )?;
        let phase: Phase = read(&entry.path().join("phase.json"))?;
        if phase != Phase::Completed {
            continue;
        }
        let plan: Plan = read(&entry.path().join("plan.json"))?;
        if plan.schema_version != SCHEMA
            || plan.transaction_id != id
            || plan.game_root != lock.root()
            || plan.files.len() > MAX_FILES
            || plan.created_dirs.len() > MAX_FILES
        {
            return Err(error(
                "invalid completed injection plan version/identity/root/size",
            ));
        }
        let Some(prepared_at) = plan.prepared_at else {
            continue;
        };
        if prepared_at <= since {
            continue;
        }
        let mut seen = BTreeSet::new();
        for change in plan.files {
            let key = output_key(&safe_game_rel(&change.path)?);
            if !seen.insert(key.clone())
                || !valid_hash(&change.result)
                || change.original.as_ref().is_some_and(|v| !valid_hash(v))
            {
                return Err(error("invalid or duplicate completed injection file"));
            }
            if change.original.is_none() {
                outputs.files.insert(key.clone());
            }
            outputs
                .hashes
                .entry(key)
                .or_default()
                .insert((change.result.sha256, change.result.size));
        }
        let mut seen_dirs = BTreeSet::new();
        for dir in plan.created_dirs {
            let key = output_key(&safe_game_rel(&dir)?);
            if seen.contains(&key) || !seen_dirs.insert(key.clone()) {
                return Err(error("invalid duplicate completed injection directory"));
            }
            outputs.directories.insert(key);
        }
    }
    Ok(outputs)
}

fn safe_game_rel(raw: &str) -> Result<PathBuf> {
    let rel = safe_stored_rel(raw)?;
    let canonical = rel.to_string_lossy().replace('\\', "/");
    if canonical != raw
        || raw.chars().any(char::is_control)
        || rel.components().next().is_some_and(|c| {
            c.as_os_str()
                .to_string_lossy()
                .eq_ignore_ascii_case(STORE_DIR)
        })
    {
        return Err(error(format!("invalid injection recovery path: {raw}")));
    }
    Ok(rel)
}

fn valid_hash(value: &Fingerprint) -> bool {
    value.sha256.len() == 64
        && value
            .sha256
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn inspect_target(root: &Path, path: &str) -> Result<Option<Fingerprint>> {
    let rel = safe_game_rel(path)?;
    ensure_no_links(root, &rel)?;
    match fs::symlink_metadata(root.join(&rel)) {
        Ok(meta) if meta.is_file() => fingerprint(&root.join(rel)).map(Some),
        Ok(_) => Err(error(format!(
            "injection target is not a regular file: {path}"
        ))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

impl Operation {
    fn set_phase(&mut self, phase: Phase) -> Result<()> {
        publish(&self.root, &self.directory.join("phase.json"), &phase)?;
        self.phase = phase;
        Ok(())
    }
    fn plan(&self) -> Result<Plan> {
        let plan: Plan = read(&self.directory.join("plan.json"))?;
        if plan.schema_version != SCHEMA
            || plan.transaction_id != self.active.transaction_id
            || plan.game_root != self.root
            || plan.files.len() > MAX_FILES
        {
            return Err(error("invalid injection plan version/identity/root/size"));
        }
        let mut seen = BTreeSet::new();
        for (index, change) in plan.files.iter().enumerate() {
            let rel = safe_game_rel(&change.path)?;
            if !seen.insert(change.path.to_lowercase())
                || !valid_hash(&change.result)
                || change.original.as_ref().is_some_and(|v| !valid_hash(v))
            {
                return Err(error("invalid or duplicate injection plan file"));
            }
            ensure_no_links(&self.root, &rel)?;
            if let Some(original) = &change.original {
                let rel = Path::new("originals").join(index.to_string());
                ensure_no_links(&self.directory, &rel)?;
                if fingerprint(&self.directory.join(rel))? != *original {
                    return Err(error(format!(
                        "injection backup hash/size mismatch: {}",
                        change.path
                    )));
                }
            }
        }
        let mut seen_dirs = BTreeSet::new();
        for dir in &plan.created_dirs {
            let rel = safe_game_rel(dir)?;
            if !seen_dirs.insert(dir.to_lowercase()) || seen.contains(&dir.to_lowercase()) {
                return Err(error("invalid duplicate recovery directory"));
            }
            ensure_no_links(&self.root, &rel)?;
            match fs::symlink_metadata(self.root.join(rel)) {
                Ok(meta) if !meta.is_dir() => {
                    return Err(error(format!("recovery directory became a file: {dir}")))
                }
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
                _ => {}
            }
        }
        Ok(plan)
    }
}

fn conflicts(root: &Path, plan: &Plan) -> Result<Vec<RecoveryConflict>> {
    let current = plan
        .files
        .iter()
        .map(|change| inspect_target(root, &change.path))
        .collect::<Result<Vec<_>>>()?;
    Ok(conflicts_from_snapshot(plan, &current))
}

fn conflicts_from_snapshot(plan: &Plan, current: &[Option<Fingerprint>]) -> Vec<RecoveryConflict> {
    let mut conflicts = Vec::new();
    for (change, current) in plan.files.iter().zip(current) {
        if current.is_some()
            && current != &change.original
            && current.as_ref() != Some(&change.result)
        {
            conflicts.push(RecoveryConflict { path: change.path.clone(), reason: "current hash/size matches neither the original nor this injection; explicit force must preserve these bytes before recovery".into() });
        }
    }
    conflicts
}

pub fn status(game_path: &Path) -> Result<InjectionStatus> {
    let lock = GameLock::acquire(&root_for(game_path)?)?;
    let root = lock.root();
    let pending = match load(root)? {
        Some(operation) => match operation.phase.pending() {
            Some(phase) => {
                let (changed_files, conflicts) = if phase == InjectionPhase::Preparing {
                    (0, Vec::new())
                } else {
                    let plan = operation.plan()?;
                    (plan.files.len(), conflicts(root, &plan)?)
                };
                Some(PendingInjection {
                    transaction_id: operation.active.transaction_id,
                    phase,
                    format: operation.active.format,
                    language: operation.active.language,
                    changed_files,
                    backup_path: operation.directory.join("originals"),
                    conflicts,
                })
            }
            None => None,
        },
        None => None,
    };
    Ok(InjectionStatus {
        game_root: root.to_owned(),
        pending,
    })
}

fn inventory(root: &Path, selection: &Path, skip_recovery: bool) -> Result<Inventory> {
    let mut result = Inventory::default();
    for item in walkdir::WalkDir::new(selection)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| {
            !(skip_recovery
                && e.path().parent() == Some(root)
                && (e
                    .file_name()
                    .to_string_lossy()
                    .eq_ignore_ascii_case(".locust")
                    || e.file_name()
                        .to_string_lossy()
                        .eq_ignore_ascii_case(STORE_DIR)))
        })
    {
        let item = item.map_err(|e| error(e.to_string()))?;
        let relative = item
            .path()
            .strip_prefix(root)
            .map_err(|e| error(e.to_string()))?;
        if relative.as_os_str().is_empty() {
            continue;
        }
        let key = relative
            .to_str()
            .ok_or_else(|| error("injection requires UTF-8 representable paths"))?
            .replace('\\', "/");
        let rel = safe_game_rel(&key)?;
        ensure_no_links(root, &rel)?;
        if item.file_type().is_file() {
            result.files.insert(key, fingerprint(item.path())?);
        } else if item.file_type().is_dir() {
            result.dirs.insert(key);
        } else {
            return Err(error(format!(
                "unsupported injection file type: {}",
                item.path().display()
            )));
        }
        if result.files.len() + result.dirs.len() > MAX_FILES {
            return Err(error("injection inventory exceeds file limit"));
        }
    }
    Ok(result)
}

fn copy_verified(source: &Path, dest: &Path, expected: &Fingerprint) -> Result<()> {
    let mut output = File::create_new(dest)?;
    std::io::copy(&mut File::open(source)?, &mut output)?;
    output.sync_all()?;
    drop(output);
    fs::set_permissions(dest, fs::metadata(source)?.permissions())?;
    if fingerprint(dest)? != *expected {
        return Err(error(format!(
            "file changed or copy failed verification: {}",
            source.display()
        )));
    }
    Ok(())
}

// Only files named in a durable inventory, with exactly matching bytes, are
// removed. New/edited unknown children stay for diagnosis. Never follow links.
fn cleanup_inventory(root: &Path, inventory: &Inventory) -> Result<()> {
    for (raw, expected) in &inventory.files {
        let rel = safe_game_rel(raw)?;
        ensure_no_links(root, &rel)?;
        let path = root.join(rel);
        if path.is_file() && fingerprint(&path)? == *expected {
            fs::remove_file(path)?;
        }
    }
    let mut dirs: Vec<_> = inventory.dirs.iter().collect();
    dirs.sort_by_key(|s| std::cmp::Reverse(s.matches('/').count()));
    for raw in dirs {
        let rel = safe_game_rel(raw)?;
        ensure_no_links(root, &rel)?;
        let _ = fs::remove_dir(root.join(rel));
    }
    let _ = fs::remove_dir(root);
    Ok(())
}

fn cleanup_work(operation: &Operation) -> Result<()> {
    let path = operation.directory.join("work-inventory.json");
    if !path.try_exists()? {
        return Ok(());
    }
    let inventory: Inventory = read(&path)?;
    ensure_no_links(&operation.directory, Path::new("work"))?;
    if operation.directory.join("work").is_dir() {
        cleanup_inventory(&operation.directory.join("work"), &inventory)?;
    }
    if operation.directory.join("plan.json").try_exists()? {
        let plan = operation.plan()?;
        ensure_no_links(&operation.directory, Path::new("results"))?;
        for (index, change) in plan.files.iter().enumerate() {
            let relative = Path::new("results").join(index.to_string());
            ensure_no_links(&operation.directory, &relative)?;
            let path = operation.directory.join(relative);
            if path.is_file() && fingerprint(&path)? == change.result {
                fs::remove_file(path)?;
            }
        }
        let _ = fs::remove_dir(operation.directory.join("results"));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    StorePrepared,
    Preparing,
    OutputsPrepared,
    PlanPublished,
    Installed(usize),
    FilesCommitted,
    Recorded,
    RecoverySnapshot,
    Restored(usize),
}

/// The backup callback runs under the real GameLock before any transaction
/// metadata is created. The plugin receives only its independent copy. The
/// record callback completes SQLite recording before the terminal marker.
pub(crate) fn run<B, T>(
    selection: &Path,
    format: &str,
    language: Option<&str>,
    backup: impl FnOnce() -> Result<B>,
    inject: impl FnOnce(&Path, &Path) -> Result<InjectionReport>,
    record: impl FnOnce(&InjectionReport) -> Result<T>,
) -> Result<(B, InjectionReport, T)> {
    run_with_hook(
        selection,
        format,
        language,
        backup,
        inject,
        record,
        &mut |_| Ok(()),
    )
}

fn run_with_hook<B, T>(
    selection: &Path,
    format: &str,
    language: Option<&str>,
    backup: impl FnOnce() -> Result<B>,
    inject: impl FnOnce(&Path, &Path) -> Result<InjectionReport>,
    record: impl FnOnce(&InjectionReport) -> Result<T>,
    hook: &mut impl FnMut(Step) -> Result<()>,
) -> Result<(B, InjectionReport, T)> {
    run_with_backup_hook(
        selection,
        format,
        language,
        backup,
        |work, selected, _| inject(work, selected),
        (|report, _| record(report), |_, _, _| Ok(())),
        hook,
    )
}

/// Supplies the backup while the game lock is held so provenance can be committed
/// atomically with output hashes before completing the injection transaction.
/// `unchanged` runs only after successful completion/cleanup with an empty plan
/// and identical live inventory, while the same game lock is still held.
pub(crate) fn run_with_backup<B, T>(
    selection: &Path,
    format: &str,
    language: Option<&str>,
    backup: impl FnOnce() -> Result<B>,
    inject: impl FnOnce(&Path, &Path, &B) -> Result<InjectionReport>,
    record: impl FnOnce(&InjectionReport, &B) -> Result<T>,
    unchanged: impl FnOnce(&mut B, &mut InjectionReport, &T) -> Result<()>,
) -> Result<(B, InjectionReport, T)> {
    run_with_backup_hook(
        selection,
        format,
        language,
        backup,
        inject,
        (record, unchanged),
        &mut |_| Ok(()),
    )
}

fn run_with_backup_hook<B, T>(
    selection: &Path,
    format: &str,
    language: Option<&str>,
    backup: impl FnOnce() -> Result<B>,
    inject: impl FnOnce(&Path, &Path, &B) -> Result<InjectionReport>,
    callbacks: (
        impl FnOnce(&InjectionReport, &B) -> Result<T>,
        impl FnOnce(&mut B, &mut InjectionReport, &T) -> Result<()>,
    ),
    hook: &mut impl FnMut(Step) -> Result<()>,
) -> Result<(B, InjectionReport, T)> {
    let (record, unchanged) = callbacks;
    let selected = selection.canonicalize()?;
    let lock = GameLock::acquire(&root_for(&selected)?)?;
    ensure_no_pending_under_lock(&lock)?;
    let root = lock.root();
    ensure_no_links(root, Path::new(".locust"))?;
    if matches!(
        PatchStore::new(root).status()?,
        crate::patch::PatchStatus::Interrupted(_)
    ) {
        return Err(error(
            "an installed patch operation is unfinished; recover that patch before injecting",
        ));
    }
    let mut backup = backup()?;
    let before = inventory(root, &selected, true)?;
    let store = root.join(STORE_DIR);
    if !validate_store(root)? {
        use std::io::Write;
        let mut initial = crate::patch::stream::StagingDir::create_prepared(root)?;
        let mut file = initial.create_file("store.json")?;
        let marker = StoreMarker {
            schema_version: SCHEMA,
            kind: "locust-injection-store".into(),
            game_root: root.to_owned(),
        };
        file.write_all(&serde_json::to_vec(&marker)?)?;
        file.sync_all()?;
        drop(file);
        hook(Step::StorePrepared)?;
        let temporary = initial.path().to_owned();
        initial.disarm();
        drop(initial); // release the directory's no-share-delete handle
        fs::rename(temporary, &store)?;
    }
    ensure_no_links(root, &Path::new(STORE_DIR).join("operations"))?;
    fs::create_dir_all(store.join("operations"))?;
    let id = uuid::Uuid::new_v4().to_string();
    let directory = store.join("operations").join(&id);
    fs::create_dir(&directory)?;
    let active = Active {
        schema_version: SCHEMA,
        transaction_id: id,
        game_root: root.to_owned(),
        format: format.into(),
        language: language.map(str::to_owned),
    };
    let mut operation = Operation {
        root: root.to_owned(),
        directory,
        active,
        phase: Phase::Preparing,
    };
    operation.set_phase(Phase::Preparing)?;
    publish(root, &store.join("active.json"), &operation.active)?;
    hook(Step::Preparing)?;
    let prepared = prepare(&operation, &selected, &before, |work, selected| {
        inject(work, selected, &backup)
    });
    let (mut report, plan) = match prepared {
        Ok(value) => value,
        Err(e) => {
            operation.set_phase(Phase::Aborted)?;
            let _ = cleanup_work(&operation);
            return Err(e);
        }
    };
    hook(Step::OutputsPrepared)?;
    let empty_plan = plan.files.is_empty() && plan.created_dirs.is_empty();
    if empty_plan && inventory(root, &selected, true)? != before {
        operation.set_phase(Phase::Aborted)?;
        let _ = cleanup_work(&operation);
        return Err(error("game changed while staging an unchanged injection; no output installed and backup retained"));
    }
    // Validate every target and all original bytes before activating the plan.
    operation.plan()?;
    for change in &plan.files {
        let current = inspect_target(root, &change.path)?;
        if current != change.original || current.as_ref().is_some_and(|f| f.readonly) {
            operation.set_phase(Phase::Aborted)?;
            let _ = cleanup_work(&operation);
            return Err(error(format!(
                "game changed while staging injection: {}; no output installed",
                change.path
            )));
        }
    }
    for directory in &plan.created_dirs {
        if root.join(safe_game_rel(directory)?).try_exists()? {
            operation.set_phase(Phase::Aborted)?;
            let _ = cleanup_work(&operation);
            return Err(error(format!(
                "directory appeared while staging injection: {directory}"
            )));
        }
    }
    operation.set_phase(Phase::Applying)?;
    hook(Step::PlanPublished)?;
    let mut created_dirs: Vec<_> = plan.created_dirs.iter().collect();
    created_dirs.sort_by_key(|s| s.matches('/').count());
    for directory in created_dirs {
        let relative = safe_game_rel(directory)?;
        ensure_no_links(root, &relative)?;
        fs::create_dir(root.join(relative))?;
    }
    for (index, change) in plan.files.iter().enumerate() {
        if inspect_target(root, &change.path)? != change.original {
            return Err(error(format!(
                "game changed before install: {}; recover injection {}",
                change.path, operation.active.transaction_id
            )));
        }
        let destination = root.join(safe_game_rel(&change.path)?);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        let staged = operation.directory.join("results").join(index.to_string());
        if fingerprint(&staged)? != change.result {
            return Err(error(format!("prepared output changed: {}", change.path)));
        }
        if inspect_target(root, &change.path)? != change.original {
            return Err(error(format!(
                "game changed during install preparation: {}",
                change.path
            )));
        }
        fs::rename(staged, destination)?;
        hook(Step::Installed(index))?;
    }
    for change in &plan.files {
        if inspect_target(root, &change.path)?.as_ref() != Some(&change.result) {
            return Err(error(format!("committed output changed: {}", change.path)));
        }
    }
    operation.set_phase(Phase::CommittedUnrecorded)?;
    hook(Step::FilesCommitted)?;
    let recorded = record(&report, &backup).map_err(|e| error(format!("injection {} committed files but recording failed: {e}; inspect injection status and recover before retry", operation.active.transaction_id)))?;
    hook(Step::Recorded)?;
    operation.set_phase(Phase::Completed)?;
    let work_cleaned = match cleanup_work(&operation) {
        Ok(()) => true,
        Err(e) => {
            report.warnings.push(format!(
                "private injection copy retained at {}: {e}",
                display_path(&operation.directory.join("work")).display()
            ));
            false
        }
    };
    if empty_plan && work_cleaned {
        // Recheck after recording and cleanup; neither a plugin nor an
        // external writer may turn an empty staged plan into data loss.
        if inventory(root, &selected, true)? != before {
            return Err(error(
                "game changed while completing an unchanged injection; backup retained",
            ));
        }
        unchanged(&mut backup, &mut report, &recorded)?;
    }
    report.warnings.push(format!(
        "injection {} completed; verified originals retained at {}",
        operation.active.transaction_id,
        display_path(&operation.directory.join("originals")).display()
    ));
    Ok((backup, report, recorded))
}

/// User-facing spelling of a path. `canonicalize` yields `\\?\` verbatim
/// prefixes on Windows; those stay on paths that are locked, opened, or
/// compared, and are removed only when a path is shown in a report.
pub fn display_path(path: &Path) -> PathBuf {
    use std::path::{Component, Prefix};

    let mut components = path.components().peekable();
    let Some(Component::Prefix(prefix)) = components.next() else {
        return path.to_path_buf();
    };
    let mut out = match prefix.kind() {
        Prefix::VerbatimDisk(disk) => {
            let letter = disk as char;
            let rooted = matches!(components.peek(), Some(Component::RootDir));
            PathBuf::from(if rooted {
                format!("{letter}:\\")
            } else {
                format!("{letter}:")
            })
        }
        Prefix::VerbatimUNC(server, share) => PathBuf::from(format!(
            r"\\{}\{}",
            server.to_string_lossy(),
            share.to_string_lossy()
        )),
        _ => return path.to_path_buf(),
    };
    for component in components {
        if matches!(component, Component::RootDir) {
            continue;
        }
        out.push(component.as_os_str());
    }
    out
}

fn new_scratch(path: &str, before: &Inventory) -> bool {
    let mut prefix = String::new();
    for component in path.split('/') {
        if !prefix.is_empty() {
            prefix.push('/');
        }
        prefix.push_str(component);
        if component
            .strip_prefix(".locust-stage-")
            .is_some_and(|id| uuid::Uuid::parse_str(id).is_ok())
            && !before.dirs.contains(&prefix)
        {
            return true;
        }
    }
    false
}

fn prepare(
    operation: &Operation,
    selected: &Path,
    before: &Inventory,
    inject: impl FnOnce(&Path, &Path) -> Result<InjectionReport>,
) -> Result<(InjectionReport, Plan)> {
    let work = operation.directory.join("work");
    fs::create_dir(&work)?;
    // Publishing the expected copied inventory first permits conservative
    // cleanup even if copying or plugin execution is interrupted.
    publish(
        &operation.root,
        &operation.directory.join("work-inventory.json"),
        before,
    )?;
    for dir in &before.dirs {
        fs::create_dir_all(work.join(safe_game_rel(dir)?))?;
    }
    for (path, value) in &before.files {
        let rel = safe_game_rel(path)?;
        if let Some(parent) = work.join(&rel).parent() {
            fs::create_dir_all(parent)?;
        }
        copy_verified(&operation.root.join(&rel), &work.join(&rel), value)?;
    }
    let selected_copy = if selected.is_file() {
        work.join(selected.file_name().unwrap())
    } else {
        work.clone()
    };
    let (injected, tracked) =
        crate::patch::stream::track_prepared_staging(|| inject(&work, &selected_copy));
    let mut report = injected?;
    let after = inventory(&work, &work, false)?;
    for path in after.files.keys() {
        if new_scratch(path, before) && !tracked.owns_file(&work.join(path)) {
            return Err(error(format!("untracked staging data left by plugin: {path}; private copy retained for diagnosis")));
        }
    }
    publish(
        &operation.root,
        &operation.directory.join("work-inventory.json"),
        &after,
    )?;
    for path in before.files.keys() {
        if !after.files.contains_key(path) {
            return Err(error(format!(
                "plugin removed an existing file; transactional deletion is unsupported: {path}"
            )));
        }
    }
    for path in &before.dirs {
        if !after.dirs.contains(path) {
            return Err(error(format!(
                "plugin removed an existing directory: {path}"
            )));
        }
    }
    for reported in &report.files_written {
        let absolute = if reported.is_absolute() {
            reported.clone()
        } else {
            work.join(reported)
        };
        // Archive virtual locators may use '/' even in a Windows verbatim
        // prefix. Compare resolved filesystem paths, as for the locked root.
        // The inventory above has already rejected links in the private copy.
        let absolute = absolute.canonicalize()?;
        let relative = absolute.strip_prefix(&work).map_err(|_| {
            error(format!(
                "plugin reported output outside its private copy: {}",
                reported.display()
            ))
        })?;
        let path = relative.to_string_lossy().replace('\\', "/");
        safe_game_rel(&path)?;
        if !after.files.contains_key(&path) || tracked.owns_file(&work.join(&path)) {
            return Err(error(format!(
                "plugin reported missing or private output: {path}"
            )));
        }
    }
    fs::create_dir(operation.directory.join("originals"))?;
    fs::create_dir(operation.directory.join("results"))?;
    let mut changes = Vec::new();
    for (path, result) in &after.files {
        if before.files.get(path) == Some(result) || tracked.owns_file(&work.join(path)) {
            continue;
        }
        let index = changes.len();
        let original = before.files.get(path).cloned();
        if let Some(value) = &original {
            copy_verified(
                &operation.root.join(safe_game_rel(path)?),
                &operation
                    .directory
                    .join("originals")
                    .join(index.to_string()),
                value,
            )?;
        }
        copy_verified(
            &work.join(safe_game_rel(path)?),
            &operation.directory.join("results").join(index.to_string()),
            result,
        )?;
        changes.push(Change {
            path: path.clone(),
            original,
            result: result.clone(),
        });
    }
    let created_dirs = after
        .dirs
        .difference(&before.dirs)
        .filter(|path| !tracked.owns_directory(&work.join(path)) && !new_scratch(path, before))
        .cloned()
        .collect();
    let plan = Plan {
        schema_version: SCHEMA,
        transaction_id: operation.active.transaction_id.clone(),
        game_root: operation.root.clone(),
        prepared_at: Some(chrono::Utc::now()),
        files: changes,
        created_dirs,
    };
    publish(
        &operation.root,
        &operation.directory.join("plan.json"),
        &plan,
    )?;
    report.files_modified = plan.files.len();
    report.files_written = plan
        .files
        .iter()
        .map(|f| display_path(&operation.root.join(&f.path)))
        .collect();
    report.classify_remaining_skips();
    Ok((report, plan))
}

pub fn recover(
    game_path: &Path,
    options: InjectionRecoveryOptions,
) -> Result<InjectionRecoveryReport> {
    recover_with_hook(game_path, options, &mut |_| Ok(()))
}

fn recover_with_hook(
    game_path: &Path,
    options: InjectionRecoveryOptions,
    hook: &mut impl FnMut(Step) -> Result<()>,
) -> Result<InjectionRecoveryReport> {
    let lock = GameLock::acquire(&root_for(game_path)?)?;
    let mut report = InjectionRecoveryReport {
        transaction_id: None,
        restored: 0,
        removed: 0,
        preserved_conflicts: Vec::new(),
        messages: Vec::new(),
    };
    let loaded = load(lock.root())?;
    if let Some(expected) = &options.expected_transaction_id {
        let actual = loaded.as_ref().and_then(|operation| {
            operation
                .phase
                .pending()
                .map(|_| operation.active.transaction_id.as_str())
        });
        if actual != Some(expected.as_str()) {
            return Err(error(
                "injection recovery operation changed; inspect its status again before recovering",
            ));
        }
    }
    let Some(mut operation) = loaded else {
        report.messages.push("no unfinished injection".into());
        return Ok(report);
    };
    if operation.phase.pending().is_none() {
        report.messages.push("no unfinished injection".into());
        return Ok(report);
    }
    report.transaction_id = Some(operation.active.transaction_id.clone());
    if operation.phase == Phase::Preparing {
        operation.set_phase(Phase::Aborted)?;
        if let Err(e) = cleanup_work(&operation) {
            report
                .messages
                .push(format!("private preparation retained for diagnosis: {e}"));
        }
        report
            .messages
            .push("interrupted preparation canceled; no game files were installed".into());
        return Ok(report);
    }
    let plan = operation.plan()?;
    let current: Vec<_> = plan
        .files
        .iter()
        .map(|c| inspect_target(lock.root(), &c.path))
        .collect::<Result<_>>()?;
    hook(Step::RecoverySnapshot)?;
    for (change, value) in plan.files.iter().zip(&current) {
        if value != &change.original && value.as_ref().is_some_and(|f| f.readonly) {
            return Err(error(format!(
                "read-only recovery destination: {}; clear its read-only flag before retrying",
                change.path
            )));
        }
    }
    // Classification and force preservation share one immutable byte snapshot.
    // Never adopt a later read of unknown bytes as an authorized baseline.
    let conflicts = conflicts_from_snapshot(&plan, &current);
    if !conflicts.is_empty() && !options.force {
        return Err(error(format!("injection recovery refused before any mutation: {}. Preserve or explicitly force recovery of these edited files", conflicts.iter().map(|c| c.path.as_str()).collect::<Vec<_>>().join(", "))));
    }
    // Preserve every conflicting regular file before the first restore/delete.
    // Force never bypasses path/type/backup validation above.
    if !conflicts.is_empty() {
        ensure_no_links(&operation.directory, Path::new("conflicts"))?;
        fs::create_dir_all(operation.directory.join("conflicts"))?;
        let preserved = operation
            .directory
            .join("conflicts")
            .join(uuid::Uuid::new_v4().to_string());
        fs::create_dir(&preserved)?;
        let mut preserved_manifest = BTreeMap::new();
        for (index, change) in plan.files.iter().enumerate() {
            if conflicts.iter().any(|c| c.path == change.path) {
                let destination = preserved.join(index.to_string());
                copy_verified(
                    &lock.root().join(safe_game_rel(&change.path)?),
                    &destination,
                    current[index].as_ref().unwrap(),
                )?;
                preserved_manifest.insert(
                    index.to_string(),
                    (change.path.clone(), current[index].clone()),
                );
                report.preserved_conflicts.push(destination);
            }
        }
        publish(
            lock.root(),
            &preserved.join("manifest.json"),
            &preserved_manifest,
        )?;
    }
    for (change, expected) in plan.files.iter().zip(&current) {
        if inspect_target(lock.root(), &change.path)? != *expected {
            return Err(error(format!(
                "file changed during recovery preflight: {}; no game files restored",
                change.path
            )));
        }
    }
    operation.set_phase(Phase::RollingBack)?;
    for (index, change) in plan.files.iter().enumerate() {
        let now = inspect_target(lock.root(), &change.path)?;
        if now != current[index] {
            return Err(error(format!(
                "file changed during recovery: {}; retry after inspection",
                change.path
            )));
        }
        if now == change.original {
            continue;
        }
        let destination = lock.root().join(safe_game_rel(&change.path)?);
        if let Some(original) = &change.original {
            let stage = crate::patch::stream::StagingDir::create_prepared(&operation.directory)?;
            let temp = stage.child("restore");
            let mut output = stage.create_file("restore")?;
            std::io::copy(
                &mut File::open(
                    operation
                        .directory
                        .join("originals")
                        .join(index.to_string()),
                )?,
                &mut output,
            )?;
            output.sync_all()?;
            drop(output);
            fs::set_permissions(
                &temp,
                fs::metadata(
                    operation
                        .directory
                        .join("originals")
                        .join(index.to_string()),
                )?
                .permissions(),
            )?;
            if fingerprint(&temp)? != *original {
                return Err(error(format!(
                    "backup changed during recovery: {}",
                    change.path
                )));
            }
            ensure_no_links(lock.root(), &safe_game_rel(&change.path)?)?;
            if let Some(parent) = destination.parent() {
                fs::create_dir_all(parent)?;
            }
            if inspect_target(lock.root(), &change.path)? != now {
                return Err(error(format!(
                    "file changed while preparing recovery: {}",
                    change.path
                )));
            }
            fs::rename(temp, destination)?;
            report.restored += 1;
        } else if now.is_some() {
            fs::remove_file(destination)?;
            report.removed += 1;
        }
        hook(Step::Restored(index))?;
    }
    let mut dirs: Vec<_> = plan.created_dirs.iter().collect();
    dirs.sort_by_key(|s| std::cmp::Reverse(s.matches('/').count()));
    for raw in dirs {
        ensure_no_links(lock.root(), &safe_game_rel(raw)?)?;
        let _ = fs::remove_dir(lock.root().join(raw));
    }
    for change in &plan.files {
        if inspect_target(lock.root(), &change.path)? != change.original {
            return Err(error(format!(
                "file changed before recovery completion: {}",
                change.path
            )));
        }
    }
    operation.set_phase(Phase::RolledBack)?;
    if let Err(e) = cleanup_work(&operation) {
        report
            .messages
            .push(format!("private copy retained for diagnosis: {e}"));
    }
    report.messages.push("injection restored to its verified pre-injection files; project recordings were not changed".into());
    Ok(report)
}

#[cfg(test)]
#[path = "injection_transaction_tests.rs"]
mod tests;
