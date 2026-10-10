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

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InjectionMode {
    Direct,
    Add,
}

/// Committed transaction evidence, not a claim that current game bytes were verified.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppliedInjection {
    pub transaction_id: String,
    pub mode: Option<InjectionMode>,
    pub language: Option<String>,
    pub applied_at: Option<chrono::DateTime<chrono::Utc>>,
    pub changed_files: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GameInjectionStatus {
    pub injections: Vec<AppliedInjection>,
    pub injection_pending: bool,
}

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
    #[serde(default)]
    pub applied: Vec<AppliedInjection>,
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
    #[serde(default)]
    mode: Option<InjectionMode>,
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
    // Auxiliary file operations use recovery, but are not Direct/Add evidence.
    FilesCompleted,
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

fn load_active(root: &Path) -> Result<Option<Active>> {
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
    Ok(Some(active))
}

fn load(root: &Path) -> Result<Option<Operation>> {
    let Some(active) = load_active(root)? else {
        return Ok(None);
    };
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

/// Unpublished staging metadata is never a generation. Legacy UUID directories
/// without a phase are remnants only when empty (or carrying a valid identity).
/// No plan, originals, links, or unexpected children may be swept as preparation.
fn history_phase(root: &Path, entry: &fs::DirEntry) -> Result<Option<Phase>> {
    let name = entry.file_name().to_string_lossy().into_owned();
    let staged = name.starts_with(".locust-stage-");
    let id = name.strip_prefix(".locust-stage-").unwrap_or(&name);
    if uuid::Uuid::parse_str(id)
        .map(|v| v.to_string())
        .ok()
        .as_deref()
        != Some(id)
    {
        return Err(error("invalid injection history identity"));
    }
    let relative = Path::new(STORE_DIR).join("operations").join(&name);
    ensure_no_links(root, &relative)?;
    match inspect_history_phase(root, &entry.path(), id, staged) {
        // Status is lock-free: publication or guard cleanup can remove a
        // staging name after read_dir returned it. Published names stay strict.
        Err(LocustError::IoError(e)) if staged && e.kind() == std::io::ErrorKind::NotFound => {
            Ok(None)
        }
        result => result,
    }
}

fn inspect_history_phase(
    root: &Path,
    directory: &Path,
    id: &str,
    staged: bool,
) -> Result<Option<Phase>> {
    if !fs::symlink_metadata(directory)?.is_dir() {
        return Err(error("injection history entry is not a directory"));
    }
    if !staged {
        match fs::symlink_metadata(directory.join("phase.json")) {
            Ok(_) => return read(&directory.join("phase.json")).map(Some),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    if load_active(root)?.is_some_and(|active| active.transaction_id == id) {
        return Err(error(
            "active injection cannot be treated as an initialization remnant",
        ));
    }
    for child in fs::read_dir(directory)? {
        let child = child?;
        let name = child.file_name();
        ensure_no_links(directory, Path::new(&name))?;
        let metadata = fs::symlink_metadata(child.path())?;
        if !metadata.is_file()
            || metadata.len() > MAX_METADATA_BYTES
            || !(name == "identity.json" || staged && name == "phase.json")
        {
            return Err(error(
                "injection initialization remnant contains unrecognized data",
            ));
        }
        // Staged files can be partial writes: they were never published. A
        // legacy operation identity, if present, must still bind to this game.
        if !staged {
            let identity: Active = read(&child.path())?;
            if identity.schema_version != SCHEMA
                || identity.transaction_id != id
                || identity.game_root != root
            {
                return Err(error("invalid injection initialization identity"));
            }
        }
    }
    Ok(None)
}

fn cleanup_initialization_remnants(root: &Path, messages: &mut Vec<String>) -> Result<()> {
    if !validate_store(root)? {
        return Ok(());
    }
    let relative = Path::new(STORE_DIR).join("operations");
    ensure_no_links(root, &relative)?;
    let directory = root.join(relative);
    if !directory.try_exists()? {
        return Ok(());
    }
    // Validate the entire scan before removing even an empty remnant.
    let mut remnants = Vec::new();
    for entry in fs::read_dir(&directory)? {
        let entry = entry?;
        if history_phase(root, &entry)?.is_none() {
            remnants.push(entry.path());
        }
    }
    for remnant in remnants {
        if fs::read_dir(&remnant)?.next().transpose()?.is_some() {
            messages.push(format!(
                "kept removable injection initialization remnant: {}",
                display_path(&remnant).display()
            ));
        } else {
            // Nonrecursive deletion also refuses a child added after the scan.
            fs::remove_dir(&remnant)?;
            messages.push(format!(
                "removed empty injection initialization remnant: {}",
                display_path(&remnant).display()
            ));
        }
    }
    Ok(())
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
        let Some(phase) = history_phase(lock.root(), &entry)? else {
            continue;
        };
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

/// Add uses the same transaction machinery, recording the user's selected mode.
pub(crate) fn run_add<B, T>(
    selection: &Path,
    format: &str,
    language: Option<&str>,
    backup: impl FnOnce() -> Result<B>,
    inject: impl FnOnce(&Path, &Path) -> Result<InjectionReport>,
    record: impl FnOnce(&InjectionReport, &B) -> Result<T>,
) -> Result<(B, InjectionReport, T)> {
    run_with_mode_hook(
        selection,
        (format, language, InjectionMode::Add),
        backup,
        |work, selected, _| inject(work, selected),
        (record, |_, _, _| Ok(())),
        &mut |_| Ok(()),
    )
}

struct CommittedGeneration {
    directory: PathBuf,
    plan: Plan,
    restored: BTreeSet<PathBuf>,
}

/// Metadata-only, read-only scan. Never acquire a game lock or hash game files.
/// A phase marker is published atomically, and only Completed is applied evidence.
fn committed_generations(root: &Path) -> Result<Vec<CommittedGeneration>> {
    committed_generations_with(root, false)
}

fn committed_generations_with(
    root: &Path,
    refuse_pending: bool,
) -> Result<Vec<CommittedGeneration>> {
    if !validate_store(root)? {
        return Ok(Vec::new());
    }
    let relative = Path::new(STORE_DIR).join("operations");
    ensure_no_links(root, &relative)?;
    let directory = root.join(relative);
    if !directory.try_exists()? {
        return Ok(Vec::new());
    }
    let mut generations = Vec::new();
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let id = entry.file_name().to_string_lossy().into_owned();
        let Some(phase) = history_phase(root, &entry)? else {
            continue;
        };
        if refuse_pending && phase.pending().is_some() {
            return Err(error(
                "unfinished injection history; recover before resuming this project",
            ));
        }
        if phase != Phase::Completed {
            continue;
        }
        let plan: Plan = read(&entry.path().join("plan.json"))?;
        if plan.schema_version != SCHEMA
            || plan.transaction_id != id
            || plan.game_root != root
            || plan.files.len() > MAX_FILES
            || plan.created_dirs.len() > MAX_FILES
        {
            return Err(error(
                "invalid completed injection plan version/identity/root/size",
            ));
        }
        let mut seen = BTreeSet::new();
        for change in &plan.files {
            if !seen.insert(output_key(&safe_game_rel(&change.path)?))
                || !valid_hash(&change.result)
                || change.original.as_ref().is_some_and(|v| !valid_hash(v))
            {
                return Err(error("invalid or duplicate completed injection file"));
            }
        }
        let restored_path = entry.path().join("restored.json");
        let restored: BTreeSet<PathBuf> = if restored_path.try_exists()? {
            read(&restored_path)?
        } else {
            BTreeSet::new()
        };
        if !restored.is_subset(&seen) {
            return Err(error("invalid restored injection paths"));
        }
        generations.push(CommittedGeneration {
            directory: entry.path(),
            plan,
            restored,
        });
    }
    generations.sort_by(|a, b| {
        (a.plan.prepared_at, &a.plan.transaction_id)
            .cmp(&(b.plan.prepared_at, &b.plan.transaction_id))
    });
    Ok(generations)
}

fn committed_identity(
    root: &Path,
    generation: &CommittedGeneration,
    active: Option<&Operation>,
) -> Result<Option<Active>> {
    let path = generation.directory.join("identity.json");
    let identity: Option<Active> = if path.try_exists()? {
        Some(read(&path)?)
    } else {
        active
            .filter(|op| op.active.transaction_id == generation.plan.transaction_id)
            .map(|op| op.active.clone())
    };
    if identity.as_ref().is_some_and(|id| {
        id.schema_version != SCHEMA
            || id.game_root != root
            || id.transaction_id != generation.plan.transaction_id
    }) {
        return Err(error("invalid completed injection identity"));
    }
    Ok(identity)
}

fn committed_owners(
    generations: &[CommittedGeneration],
) -> Result<BTreeMap<PathBuf, (usize, &Change)>> {
    let mut owners = BTreeMap::new();
    for (index, generation) in generations.iter().enumerate() {
        for change in &generation.plan.files {
            let key = output_key(&safe_game_rel(&change.path)?);
            if !generation.restored.contains(&key) {
                owners.insert(key, (index, change));
            }
        }
    }
    Ok(owners)
}

/// Reconcile only completed Direct outputs explicitly retired by backup restore.
/// A restored marker alone is insufficient: bind it to the saved output and
/// verify the live pre-injection bytes (or absence for a generated file). Called
/// under GameLock before Direct creates its backup or recovery metadata.
pub(crate) fn verified_reverted_direct_files(
    root: &Path,
    recording: &crate::database::InjectionRecording,
) -> Result<BTreeSet<PathBuf>> {
    let root = root.canonicalize()?;
    let generations = committed_generations_with(&root, true)?;
    let owners = committed_owners(&generations)?;
    let mut latest = BTreeMap::new();
    for (index, generation) in generations.iter().enumerate() {
        for change in &generation.plan.files {
            latest.insert(output_key(&safe_game_rel(&change.path)?), (index, change));
        }
    }
    let mut reverted = BTreeSet::new();
    for file in &recording.files {
        let relative = safe_game_rel(&file.rel)?;
        let key = output_key(&relative);
        let Some((index, change)) = latest.get(&key) else {
            continue;
        };
        let generation = &generations[*index];
        if owners.contains_key(&key)
            || !generation.restored.contains(&key)
            || change.result.sha256 != file.hash
            || change.result.size != file.size
        {
            continue;
        }
        let Some(identity) = committed_identity(&root, generation, None)? else {
            continue;
        };
        if identity.mode != Some(InjectionMode::Direct) || identity.language != recording.lang {
            continue;
        }
        ensure_no_links(&root, &relative)?;
        let current = root.join(&relative);
        let matches_original = match &change.original {
            Some(original) => sha256_file(&current)
                .is_ok_and(|(hash, size)| hash == original.sha256 && size == original.size),
            None => matches!(
                fs::symlink_metadata(&current),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound
            ),
        };
        if matches_original {
            reverted.insert(key);
        }
    }
    Ok(reverted)
}

/// Prove the entire recording union against current committed ownership, not
/// status labels. No pristine-backup bytes are needed to resume saved DB rows.
/// Read-only and lock-free here: advisory callers may race; confirmation must
/// call this again while retaining the source lock.
pub(crate) fn verify_saved_direct_recordings(
    selection: &Path,
    recordings: &[crate::database::InjectionRecording],
) -> Result<Option<String>> {
    let root = root_for(selection)?;
    let active = load(&root)?;
    if active
        .as_ref()
        .is_some_and(|op| op.phase.pending().is_some())
    {
        return Err(error(
            "unfinished injection; recover before resuming this project",
        ));
    }
    let generations = committed_generations_with(&root, true)?;
    let owners = committed_owners(&generations)?;
    if owners.is_empty() && recordings.is_empty() {
        return Ok(None);
    }
    // Undated or simultaneous competing writers cannot prove which generation
    // owns the bytes; UUID ordering is not approval. Scan once, not per member.
    let mut written_at = BTreeMap::new();
    for generation in &generations {
        for change in &generation.plan.files {
            let key = output_key(&safe_game_rel(&change.path)?);
            if !generation.restored.contains(&key) {
                if let Some(previous) = written_at.insert(key, generation.plan.prepared_at) {
                    if previous.is_none()
                        || generation.plan.prepared_at.is_none()
                        || previous == generation.plan.prepared_at
                    {
                        return Err(error("ambiguous committed injection ownership"));
                    }
                }
            }
        }
    }
    let identities = generations
        .iter()
        .map(|generation| committed_identity(&root, generation, active.as_ref()))
        .collect::<Result<Vec<_>>>()?;
    let mut seen = BTreeSet::new();
    let mut format = None;
    for recording in recordings {
        if !crate::database::paths_identical(&recording.root, &root) {
            return Err(error(
                "saved injection recording belongs to a different game root",
            ));
        }
        crate::extraction::refuse_incompatible_direct_scope(selection, recording)?;
        for file in &recording.files {
            let key = output_key(&safe_game_rel(&file.rel)?);
            if !seen.insert(key.clone()) {
                return Err(error("duplicate or ambiguous saved injection member"));
            }
            let (index, change) = owners
                .get(&key)
                .ok_or_else(|| error("saved injection member has no current committed owner"))?;
            let identity = identities[*index]
                .as_ref()
                .ok_or_else(|| error("saved injection member has unknown injection mode"))?;
            if identity.mode != Some(InjectionMode::Direct)
                || identity.language != recording.lang
                || change.result.sha256 != file.hash
                || change.result.size != file.size
                || format.as_ref().is_some_and(|fid| fid != &identity.format)
            {
                return Err(error("saved injection member does not match committed Direct mode/format/language/hash/size"));
            }
            format.get_or_insert_with(|| identity.format.clone());
            crate::extraction::verify_recorded_injection_member(&root, file)?;
        }
    }
    if seen.len() != owners.len() || format.is_none() {
        return Err(error(
            "current injection ownership is not fully covered by the saved project",
        ));
    }
    Ok(format)
}

/// Complements ZIP status without changing its enum or claiming an unfinished
/// injection is an original game. Older generations may lack language and time.
pub fn game_status(game_path: &Path) -> Result<GameInjectionStatus> {
    let root = root_for(game_path)?;
    let active = load(&root)?;
    let generations = committed_generations(&root)?;
    // A later committed writer supersedes earlier evidence for the same path.
    // Restoring that later generation exposes the earlier generation again.
    let owners = committed_owners(&generations)?;
    let mut injections = Vec::new();
    for (index, generation) in generations.iter().enumerate() {
        let changed_files = owners.values().filter(|(owner, _)| *owner == index).count();
        if changed_files == 0 {
            continue;
        }
        let identity = committed_identity(&root, generation, active.as_ref())?;
        // Old plans cannot distinguish Direct creating a file from Add updating
        // one. Preserve the evidence without inventing a selected mode.
        let mode = identity.as_ref().and_then(|id| id.mode);
        let time_path = generation.directory.join("applied-at.json");
        let applied_at = if time_path.try_exists()? {
            Some(read(&time_path)?)
        } else {
            generation.plan.prepared_at
        };
        injections.push(AppliedInjection {
            transaction_id: generation.plan.transaction_id.clone(),
            mode,
            language: identity.and_then(|id| id.language),
            applied_at,
            changed_files,
        });
    }
    Ok(GameInjectionStatus {
        injections,
        injection_pending: active.is_some_and(|op| op.phase.pending().is_some()),
    })
}

/// Match a project recording to committed Add evidence for this exact root and
/// language. A later Direct generation wins even if its union retains Add files.
pub fn add_recording_under_lock(
    lock: &GameLock,
    db: &crate::database::Database,
    language: &str,
) -> Result<Option<crate::database::InjectionRecording>> {
    ensure_no_pending_under_lock(lock)?;
    matching_add_recording(lock.root(), db, language)
}

/// Internal transaction callers already hold the root lock before preparing a backup.
pub(crate) fn matching_add_recording(
    root: &Path,
    db: &crate::database::Database,
    language: &str,
) -> Result<Option<crate::database::InjectionRecording>> {
    let Some(recording) = db.get_injection(Some(language))? else {
        return Ok(None);
    };
    if !crate::database::paths_identical(&recording.root, root) {
        return Ok(None);
    }
    let active = load(root)?;
    for generation in committed_generations(root)?.iter().rev() {
        let Some(identity) = committed_identity(root, generation, active.as_ref())? else {
            continue;
        };
        if identity.language.as_deref() != Some(language) {
            continue;
        }
        if identity.mode != Some(InjectionMode::Add) {
            return Ok(None);
        }
        let matches = generation.plan.files.iter().any(|change| {
            !generation
                .restored
                .contains(&output_key(Path::new(&change.path)))
                && recording.files.iter().any(|file| {
                    file.rel == change.path
                        && file.hash == change.result.sha256
                        && file.size == change.result.size
                })
        });
        if !matches {
            continue;
        }
        for file in &recording.files {
            crate::extraction::verify_recorded_injection_member(root, file)?;
        }
        return Ok(Some(recording));
    }
    Ok(None)
}

/// Record only paths actually restored/deleted by a successful backup restore.
/// Preserve history, older injections, and any edited Add outputs that were kept.
/// Compare committed output hashes to the verified backup inventory; this also
/// reactivates older translations when restoring a backup which contains them.
/// Never reactivate a dated generation created after that backup.
pub(crate) fn record_backup_restore(
    lock: &GameLock,
    since: chrono::DateTime<chrono::Utc>,
    restored_files: &BTreeMap<PathBuf, Option<(String, u64)>>,
) -> Result<()> {
    for mut generation in committed_generations(lock.root())? {
        for change in &generation.plan.files {
            let key = output_key(&safe_game_rel(&change.path)?);
            let Some(restored) = restored_files.get(&key) else {
                continue;
            };
            let existed = generation.plan.prepared_at.is_none_or(|time| time <= since);
            let reinstated = existed
                && restored.as_ref().is_some_and(|(hash, size)| {
                    *hash == change.result.sha256 && *size == change.result.size
                });
            if reinstated {
                generation.restored.remove(&key);
            } else {
                generation.restored.insert(key);
            }
        }
        publish(
            lock.root(),
            &generation.directory.join("restored.json"),
            &generation.restored,
        )?;
    }
    Ok(())
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
        applied: game_status(root)?.injections,
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
    OperationCreated,
    OperationPrepared,
    OperationPublished,
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
#[cfg(test)]
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

#[cfg(test)]
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
    run_with_mode_hook(
        selection,
        (format, language, InjectionMode::Direct),
        backup,
        inject,
        callbacks,
        hook,
    )
}

fn run_with_mode_hook<B, T>(
    selection: &Path,
    identity: (&str, Option<&str>, InjectionMode),
    backup: impl FnOnce() -> Result<B>,
    inject: impl FnOnce(&Path, &Path, &B) -> Result<InjectionReport>,
    callbacks: (
        impl FnOnce(&InjectionReport, &B) -> Result<T>,
        impl FnOnce(&mut B, &mut InjectionReport, &T) -> Result<()>,
    ),
    hook: &mut impl FnMut(Step) -> Result<()>,
) -> Result<(B, InjectionReport, T)> {
    let (record, unchanged) = callbacks;
    let (format, language, mode) = identity;
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
    let store = initialize_store(root, hook)?;
    let id = uuid::Uuid::new_v4().to_string();
    let active = Active {
        mode: Some(mode),
        schema_version: SCHEMA,
        transaction_id: id,
        game_root: root.to_owned(),
        format: format.into(),
        language: language.map(str::to_owned),
    };
    let mut operation = initialize_operation(root, active, hook)?;
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
    publish(
        root,
        &operation.directory.join("applied-at.json"),
        &chrono::Utc::now(),
    )?;
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

fn initialize_store(root: &Path, hook: &mut impl FnMut(Step) -> Result<()>) -> Result<PathBuf> {
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
    Ok(store)
}

/// Replace prepared contents of existing game files as one recoverable operation.
/// The caller must hold this lock across source reads, preparation, and this call.
/// Paths are relative to `lock.root()`; duplicates, links and reserved paths are
/// refused before publication. `installed` is a progress callback under the lock;
/// a returned error rolls back, while process termination leaves public recovery
/// responsible for restoring the entire plan. This never records Direct/Add outputs.
pub fn write_files_under_lock(
    lock: &GameLock,
    files: &[(PathBuf, Vec<u8>)],
    operation_label: &str,
    installed: impl FnMut(usize) -> Result<()>,
) -> Result<()> {
    write_files_and_record_under_lock(lock, files, operation_label, installed, || Ok(()))
}

/// Commit the recording after all writes verify, before closing the recovery
/// operation. Recording errors roll back files under the same game lock.
pub fn write_files_and_record_under_lock(
    lock: &GameLock,
    files: &[(PathBuf, Vec<u8>)],
    operation_label: &str,
    mut installed: impl FnMut(usize) -> Result<()>,
    record: impl FnOnce() -> Result<()>,
) -> Result<()> {
    ensure_no_pending_under_lock(lock)?;
    ensure_no_links(lock.root(), Path::new(".locust"))?;
    if matches!(
        PatchStore::new(lock.root()).status()?,
        crate::patch::PatchStatus::Interrupted(_)
    ) {
        return Err(error("an installed patch operation is unfinished; recover that patch before modifying this game"));
    }
    let mut record = Some(record);
    let result = write_files_with_hook(lock.root(), files, operation_label, &mut |step| {
        if let Step::Installed(index) = step {
            installed(index)?;
        }
        if matches!(step, Step::FilesCommitted) {
            record.take().expect("recording callback runs once")()?;
        }
        Ok(())
    });
    if let Err(cause) = result {
        if let Err(rollback) = recover_locked_root(
            lock.root(),
            InjectionRecoveryOptions::default(),
            &mut |_| Ok(()),
        ) {
            return Err(error(format!("{cause}; rollback failed: {rollback}; inspect injection status and recover before retry")));
        }
        return Err(cause);
    }
    Ok(())
}

fn write_files_with_hook(
    root: &Path,
    files: &[(PathBuf, Vec<u8>)],
    operation_label: &str,
    hook: &mut impl FnMut(Step) -> Result<()>,
) -> Result<()> {
    use std::io::Write;

    if files.is_empty() {
        return Ok(());
    }
    if files.len() > MAX_FILES {
        return Err(error("too many prepared game files"));
    }
    let mut paths = BTreeSet::new();
    let mut changes = Vec::new();
    for (path, _) in files {
        let raw = path
            .to_str()
            .ok_or_else(|| error("non-UTF-8 game path"))?
            .replace('\\', "/");
        let relative = safe_game_rel(&raw)?;
        if !paths.insert(raw.to_lowercase()) {
            return Err(error("duplicate prepared game file"));
        }
        let original = inspect_target(root, &raw)?
            .ok_or_else(|| error(format!("missing prepared game file: {raw}")))?;
        if original.readonly {
            return Err(error(format!("read-only prepared game file: {raw}")));
        }
        // Check access without truncating. On Windows also check DELETE sharing,
        // so a reader denying replacement fails before journal/backup publication.
        let mut options = fs::OpenOptions::new();
        options.write(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            options.access_mode(0x4000_0000 | 0x0001_0000); // GENERIC_WRITE | DELETE
        }
        drop(options.open(root.join(relative))?);
        changes.push(Change {
            path: raw,
            original: Some(original.clone()),
            result: original,
        });
    }
    let store = initialize_store(root, hook)?;
    let active = Active {
        mode: None,
        schema_version: SCHEMA,
        transaction_id: uuid::Uuid::new_v4().to_string(),
        game_root: root.to_owned(),
        format: operation_label.into(),
        language: None,
    };
    let mut operation = initialize_operation(root, active, hook)?;
    publish(root, &store.join("active.json"), &operation.active)?;
    hook(Step::Preparing)?;
    fs::create_dir(operation.directory.join("originals"))?;
    fs::create_dir(operation.directory.join("results"))?;
    for (index, (change, (_, bytes))) in changes.iter_mut().zip(files).enumerate() {
        let source = root.join(safe_game_rel(&change.path)?);
        copy_verified(
            &source,
            &operation
                .directory
                .join("originals")
                .join(index.to_string()),
            change.original.as_ref().unwrap(),
        )?;
        let result = operation.directory.join("results").join(index.to_string());
        let mut output = File::create_new(&result)?;
        output.write_all(bytes)?;
        output.set_permissions(fs::metadata(&source)?.permissions())?;
        output.sync_all()?;
        drop(output);
        change.result = fingerprint(&result)?;
    }
    sync_directory_best_effort(&operation.directory.join("originals"));
    sync_directory_best_effort(&operation.directory.join("results"));
    let plan = Plan {
        schema_version: SCHEMA,
        transaction_id: operation.active.transaction_id.clone(),
        game_root: root.to_owned(),
        prepared_at: Some(chrono::Utc::now()),
        files: changes,
        created_dirs: Vec::new(),
    };
    publish(root, &operation.directory.join("plan.json"), &plan)?;
    operation.plan()?;
    hook(Step::OutputsPrepared)?;
    for change in &plan.files {
        if inspect_target(root, &change.path)? != change.original {
            return Err(error(format!(
                "game changed while preparing {}",
                change.path
            )));
        }
    }
    operation.set_phase(Phase::Applying)?;
    hook(Step::PlanPublished)?;
    for (index, change) in plan.files.iter().enumerate() {
        let destination = root.join(safe_game_rel(&change.path)?);
        let parent = destination.parent().unwrap();
        // Reuse the owned staging guard used by durable JSON publication,
        // staging beside this destination to guarantee a same-volume rename.
        let stage = crate::patch::stream::StagingDir::create_prepared(parent)?;
        let temp = stage.child("prepared");
        let mut output = stage.create_file("prepared")?;
        let source = operation.directory.join("results").join(index.to_string());
        std::io::copy(&mut File::open(&source)?, &mut output)?;
        output.set_permissions(fs::metadata(&source)?.permissions())?;
        output.sync_all()?;
        drop(output);
        if fingerprint(&temp)? != change.result {
            return Err(error(format!("prepared output changed: {}", change.path)));
        }
        if inspect_target(root, &change.path)? != change.original {
            return Err(error(format!(
                "game changed before install: {}",
                change.path
            )));
        }
        fs::rename(&temp, &destination)?;
        sync_directory_best_effort(parent);
        hook(Step::Installed(index))?;
    }
    for change in &plan.files {
        if inspect_target(root, &change.path)?.as_ref() != Some(&change.result) {
            return Err(error(format!("committed output changed: {}", change.path)));
        }
    }
    hook(Step::FilesCommitted)?;
    operation.set_phase(Phase::FilesCompleted)?;
    Ok(())
}

fn initialize_operation(
    root: &Path,
    active: Active,
    hook: &mut impl FnMut(Step) -> Result<()>,
) -> Result<Operation> {
    let parent = root.join(STORE_DIR).join("operations");
    let directory = parent.join(&active.transaction_id);
    let mut initial = crate::patch::stream::StagingDir::create_prepared(&parent)?;
    hook(Step::OperationCreated)?;
    for (name, bytes) in [
        ("phase.json", serde_json::to_vec(&Phase::Preparing)?),
        ("identity.json", serde_json::to_vec(&active)?),
    ] {
        use std::io::Write;
        let mut file = initial.create_file(name)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
    }
    sync_directory_best_effort(initial.path());
    hook(Step::OperationPrepared)?;
    let temporary = initial.path().to_owned();
    initial.disarm();
    drop(initial); // release the directory's no-share-delete handle before rename
    fs::rename(temporary, &directory)?;
    sync_directory_best_effort(&parent);
    hook(Step::OperationPublished)?;
    Ok(Operation {
        root: root.to_owned(),
        directory,
        active,
        phase: Phase::Preparing,
    })
}

// Match durable JSON publication: files are synced strictly; directory syncing
// is best effort because not every supported filesystem permits it.
fn sync_directory_best_effort(path: &Path) {
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(0x0200_0000); // FILE_FLAG_BACKUP_SEMANTICS
    }
    let _ = options.open(path).and_then(|file| file.sync_all());
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
    recover_locked_root(lock.root(), options, hook)
}

fn recover_locked_root(
    root: &Path,
    options: InjectionRecoveryOptions,
    hook: &mut impl FnMut(Step) -> Result<()>,
) -> Result<InjectionRecoveryReport> {
    let mut report = InjectionRecoveryReport {
        transaction_id: None,
        restored: 0,
        removed: 0,
        preserved_conflicts: Vec::new(),
        messages: Vec::new(),
    };
    let loaded = load(root)?;
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
    cleanup_initialization_remnants(root, &mut report.messages)?;
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
        .map(|c| inspect_target(root, &c.path))
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
                    &root.join(safe_game_rel(&change.path)?),
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
        publish(root, &preserved.join("manifest.json"), &preserved_manifest)?;
    }
    for (change, expected) in plan.files.iter().zip(&current) {
        if inspect_target(root, &change.path)? != *expected {
            return Err(error(format!(
                "file changed during recovery preflight: {}; no game files restored",
                change.path
            )));
        }
    }
    operation.set_phase(Phase::RollingBack)?;
    for (index, change) in plan.files.iter().enumerate() {
        let now = inspect_target(root, &change.path)?;
        if now != current[index] {
            return Err(error(format!(
                "file changed during recovery: {}; retry after inspection",
                change.path
            )));
        }
        if now == change.original {
            continue;
        }
        let destination = root.join(safe_game_rel(&change.path)?);
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
            ensure_no_links(root, &safe_game_rel(&change.path)?)?;
            if let Some(parent) = destination.parent() {
                fs::create_dir_all(parent)?;
            }
            if inspect_target(root, &change.path)? != now {
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
        ensure_no_links(root, &safe_game_rel(raw)?)?;
        let _ = fs::remove_dir(root.join(raw));
    }
    for change in &plan.files {
        if inspect_target(root, &change.path)? != change.original {
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

#[cfg(test)]
mod prepared_file_tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn registration_recording_failure_restores_files_and_preserves_database() {
        let (_temp, root, files) = fixture();
        let db = crate::database::Database::open_in_memory().unwrap();
        let pack = root.join("data/lang_g_es.json");
        fs::write(&pack, b"Spanish pack").unwrap();
        db.record_injection(Some("es"), &root, &[pack]).unwrap();
        let before = db.get_injection(Some("es")).unwrap().unwrap();
        db.fail_registration_recording();
        let backup = crate::database::RecordedBackup {
            id: "registration-backup".into(),
            source_path: root.clone(),
            storage_root: None,
        };
        let written: Vec<_> = files.iter().map(|(rel, _)| root.join(rel)).collect();
        let result = write_files_with_hook(&root, &files, "register-language", &mut |step| {
            if step == Step::FilesCommitted {
                db.extend_injection_with_registration_backup(&before, &written, &backup)?;
            }
            Ok(())
        });
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("recording failure"));
        recover_locked_root(&root, Default::default(), &mut |_| Ok(())).unwrap();
        for (rel, _) in files {
            assert_eq!(fs::read(root.join(rel)).unwrap(), b"original bytes");
        }
        assert_eq!(db.get_injection(Some("es")).unwrap().unwrap(), before);
        assert!(db.registration_backups(Some("es")).unwrap().is_empty());
    }

    // Private, exclusively owned fixtures exercise the transaction engine without
    // the per-user lock directory, which some CI sandboxes cannot write. Public
    // locking and process-death recovery are also tested by locust-formats.
    fn fixture() -> (tempfile::TempDir, PathBuf, Vec<(PathBuf, Vec<u8>)>) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        fs::create_dir(root.join("js")).unwrap();
        fs::create_dir(root.join("data")).unwrap();
        let files = vec![
            (
                PathBuf::from("js/plugins.js"),
                b"var languages = ['en', 'es'];".to_vec(),
            ),
            (
                PathBuf::from("data/Map001.json"),
                br#"{"languages":["en","es"]}"#.to_vec(),
            ),
        ];
        for (path, _) in &files {
            fs::write(root.join(path), b"original bytes").unwrap();
        }
        (temp, root, files)
    }

    #[test]
    fn recovery_restores_the_whole_prepared_plan_after_each_interruption() {
        for stop in [
            Step::Preparing,
            Step::OutputsPrepared,
            Step::PlanPublished,
            Step::Installed(0),
            Step::Installed(1),
            Step::FilesCommitted,
        ] {
            let (_temp, root, files) = fixture();
            let result = write_files_with_hook(&root, &files, "register-language", &mut |step| {
                if step == stop {
                    Err(error("interrupted"))
                } else {
                    Ok(())
                }
            });
            assert!(result.is_err(), "{stop:?}");
            let operation = load(&root).unwrap().unwrap();
            assert!(operation.phase.pending().is_some());
            if operation.phase != Phase::Preparing {
                let plan = operation.plan().unwrap();
                assert_eq!(plan.files.len(), files.len());
                assert!(conflicts(&root, &plan).unwrap().is_empty());
            }
            recover_locked_root(&root, Default::default(), &mut |_| Ok(())).unwrap();
            for (path, _) in &files {
                assert_eq!(fs::read(root.join(path)).unwrap(), b"original bytes");
            }
            assert!(load(&root).unwrap().unwrap().phase.pending().is_none());
            assert!(game_status(&root).unwrap().injections.is_empty());
            assert_eq!(
                recover_locked_root(&root, Default::default(), &mut |_| Ok(()))
                    .unwrap()
                    .restored,
                0
            );
        }
    }

    #[test]
    fn prepared_crash_child() {
        let Some(root) = std::env::var_os("LOCUST_PREPARED_CRASH_GAME") else {
            return;
        };
        let files = vec![
            (
                PathBuf::from("js/plugins.js"),
                b"complete new plugins".to_vec(),
            ),
            (
                PathBuf::from("data/Map001.json"),
                b"complete new map".to_vec(),
            ),
        ];
        write_files_with_hook(Path::new(&root), &files, "register-language", &mut |step| {
            if step == Step::Installed(0) {
                std::process::exit(86);
            }
            Ok(())
        })
        .unwrap();
        panic!("crash hook was not reached");
    }

    #[test]
    fn prepared_process_death_leaves_a_recoverable_unit() {
        let (_temp, root, files) = fixture();
        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "injection_transaction::prepared_file_tests::prepared_crash_child",
                "--nocapture",
            ])
            .env("LOCUST_PREPARED_CRASH_GAME", &root)
            .output()
            .unwrap();
        assert_eq!(
            child.status.code(),
            Some(86),
            "{}",
            String::from_utf8_lossy(&child.stderr)
        );
        assert_eq!(
            fs::read(root.join(&files[0].0)).unwrap(),
            b"complete new plugins"
        );
        assert_eq!(fs::read(root.join(&files[1].0)).unwrap(), b"original bytes");
        let operation = load(&root).unwrap().unwrap();
        assert_eq!(operation.phase, Phase::Applying);
        assert_eq!(operation.plan().unwrap().files.len(), 2);
        let recovered = recover_locked_root(&root, Default::default(), &mut |_| Ok(())).unwrap();
        assert_eq!(recovered.restored, 1);
        for (path, _) in files {
            assert_eq!(fs::read(root.join(path)).unwrap(), b"original bytes");
        }
    }

    #[test]
    fn prepared_files_replace_open_readers_and_preserve_injection_recordings() {
        let (_temp, root, files) = fixture();
        let mut readers: Vec<_> = files
            .iter()
            .map(|(path, _)| File::open(root.join(path)).unwrap())
            .collect();
        // Seed real Direct transaction evidence without touching a project DB.
        write_files_with_hook(&root, &files[..1], "rpgmaker_mv", &mut |_| Ok(())).unwrap();
        let mut prior = load(&root).unwrap().unwrap();
        prior.active.mode = Some(InjectionMode::Direct);
        publish(&root, &prior.directory.join("identity.json"), &prior.active).unwrap();
        publish(
            &root,
            &root.join(STORE_DIR).join("active.json"),
            &prior.active,
        )
        .unwrap();
        prior.set_phase(Phase::Completed).unwrap();
        let before = serde_json::to_value(game_status(&root).unwrap()).unwrap();
        let recorded_plan = fs::read(prior.directory.join("plan.json")).unwrap();
        write_files_with_hook(&root, &files, "register-language", &mut |_| Ok(())).unwrap();
        for ((path, expected), reader) in files.iter().zip(&mut readers) {
            let mut observed = Vec::new();
            reader.read_to_end(&mut observed).unwrap();
            assert_eq!(observed, b"original bytes");
            assert_eq!(&fs::read(root.join(path)).unwrap(), expected);
        }
        assert_eq!(
            serde_json::to_value(game_status(&root).unwrap()).unwrap(),
            before
        );
        assert_eq!(
            fs::read(prior.directory.join("plan.json")).unwrap(),
            recorded_plan
        );
        assert_eq!(load(&root).unwrap().unwrap().phase, Phase::FilesCompleted);
    }

    #[test]
    fn prepared_plan_refuses_unsafe_or_duplicate_paths_before_publication() {
        for invalid in [
            "../escape",
            ".locust/state",
            ".locust-injections/active.json",
            "js/plugins.js",
        ] {
            let (_temp, root, mut files) = fixture();
            files.push((invalid.into(), b"bad".to_vec()));
            assert!(
                write_files_with_hook(&root, &files, "register-language", &mut |_| Ok(())).is_err()
            );
            assert!(!root.join(STORE_DIR).exists());
            assert_eq!(
                fs::read(root.join("js/plugins.js")).unwrap(),
                b"original bytes"
            );
        }
    }

    #[test]
    fn recovery_preserves_external_edits_until_explicit_force() {
        let (_temp, root, files) = fixture();
        assert!(
            write_files_with_hook(&root, &files, "register-language", &mut |step| {
                if step == Step::Installed(0) {
                    Err(error("interrupted"))
                } else {
                    Ok(())
                }
            })
            .is_err()
        );
        fs::write(root.join(&files[0].0), b"external edit").unwrap();
        assert!(recover_locked_root(&root, Default::default(), &mut |_| Ok(())).is_err());
        assert_eq!(fs::read(root.join(&files[0].0)).unwrap(), b"external edit");
        let recovered = recover_locked_root(
            &root,
            InjectionRecoveryOptions {
                force: true,
                ..Default::default()
            },
            &mut |_| Ok(()),
        )
        .unwrap();
        assert_eq!(recovered.preserved_conflicts.len(), 1);
        assert_eq!(
            fs::read(&recovered.preserved_conflicts[0]).unwrap(),
            b"external edit"
        );
        for (path, _) in files {
            assert_eq!(fs::read(root.join(path)).unwrap(), b"original bytes");
        }
    }
}
