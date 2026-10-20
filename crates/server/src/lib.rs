use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::Router;
use dashmap::DashMap;
use locust_core::backup::BackupManager;
use locust_core::config::AppConfig;
use locust_core::database::{Database, GlobalMemoryDb};
use locust_core::extraction::FormatRegistry;
use locust_core::glossary::Glossary;
use locust_core::models::OutputMode;
use locust_core::translation::ProviderRegistry;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{broadcast, RwLock};
use tokio::task::AbortHandle;
use tower_http::cors::{AllowOrigin, Any, CorsLayer};

mod config_persistence;
mod font_patch;
mod injection_recovery;
mod routes;
pub use config_persistence::{public_app_config, remember_open_project, update_app_config};

use routes::backups::{delete_backup, list_backups, restore_backup};
use routes::config_auth::{
    auth_xai_poll, auth_xai_start, get_config, get_format_modes, health, list_formats,
    list_providers, patch_config, provider_health,
};
pub use routes::config_auth::{
    poll_xai_device_login, rebuild_provider_registry, start_xai_device_login, XaiAuthPollRequest,
    XaiAuthPollResponse, XaiAuthStartResponse, XaiAuthStatus,
};
use routes::glossary_memory::{
    add_glossary, clear_memory, delete_glossary, delete_memory_entry, get_glossary, list_memory,
    memory_lang_pairs, memory_stats, memory_suggestions,
};
use routes::inject::{inject, register_lang};
pub use routes::io::audit_project_fonts;
use routes::io::{
    export_po, export_xliff, import_po, import_xliff, list_validation_issues,
    set_validation_issue_ignored, validate, validation_summary,
};
use routes::patch::{
    patch_apply, patch_cancel, patch_identity, patch_pack, patch_recordings, patch_rollback,
    patch_status, patch_verify, patch_ws,
};
use routes::project::{
    pivot_project, project_current, project_open, project_open_db, project_preflight,
    project_resume,
};
use routes::strings::{
    batch_patch_strings, get_file_stats, get_stats, get_string, get_string_facets,
    get_string_history, get_strings, list_translation_runs, patch_string,
};
pub use routes::translate::{cancel_translation_job, spawn_translation_job};
use routes::translate::{translate_cancel, translate_start, translate_ws};

type ApiError = (StatusCode, String);

fn err(status: StatusCode, msg: impl ToString) -> ApiError {
    (status, msg.to_string())
}

/// Translation vs patch-apply — same `active_jobs` map, different terminal frames.
#[derive(Clone, Copy)]
enum JobKind {
    Translate,
    Patch,
}

/// Per-job state: abort handle + broadcast sender for progress events.
/// Shared by translation and patch-apply (JSON frames, not a second job system).
pub struct JobState {
    pub abort_handle: AbortHandle,
    pub progress_tx: broadcast::Sender<serde_json::Value>,
    replay: Arc<std::sync::Mutex<Vec<serde_json::Value>>>,
    cancel: tokio_util::sync::CancellationToken,
    kind: JobKind,
    /// Canonical game-folder key for an in-flight patch apply (`None` for translate).
    /// Two applies against the same folder must not race on disk.
    patch_game_key: Option<String>,
}

fn job_event_is_terminal(v: &serde_json::Value) -> bool {
    matches!(
        v.get("type").and_then(|t| t.as_str()),
        Some("completed" | "failed" | "done" | "error")
    )
}

/// Shown verbatim in the desktop "failed to open" toast when a translation
/// is still writing the live database.
pub const TRANSLATION_IN_FLIGHT_MESSAGE: &str = "A translation is still running. Wait for it to finish or cancel it before opening another project.";

/// Shown when a second patch apply targets a folder that already has one running.
pub const PATCH_APPLY_IN_FLIGHT_MESSAGE: &str = "A patch is already being applied to this game folder. Wait for it to finish or cancel it before starting another.";

/// Shown when open/open-db would swap the DB under inject, pivot, or similar.
pub const PROJECT_BUSY_MESSAGE: &str =
    "A project operation is still running. Wait for it to finish before opening another project.";

/// Empty `languages` used to succeed with zero recording — a silent broken patch.
pub const INJECT_EMPTY_LANGUAGES_MESSAGE: &str =
    "inject requires at least one language (e.g. [\"es\"])";

/// Owns the live project until the operation (including detached work) exits.
pub struct ProjectExclusiveGuard(Arc<AtomicUsize>);

impl ProjectExclusiveGuard {
    /// Legacy unconditional reservation. New operations should use
    /// `try_project_operation` to atomically refuse conflicting work.
    pub fn enter(counter: &Arc<AtomicUsize>) -> Self {
        counter.fetch_add(1, Ordering::SeqCst);
        Self(Arc::clone(counter))
    }

    pub fn try_enter(counter: &Arc<AtomicUsize>) -> Result<Self, &'static str> {
        counter
            .compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst)
            .map(|_| Self(Arc::clone(counter)))
            .map_err(|_| PROJECT_BUSY_MESSAGE)
    }
}

impl Drop for ProjectExclusiveGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Stable key for comparing game folders across apply requests.
fn patch_game_key(game_path: &str) -> String {
    let p = Path::new(game_path);
    std::fs::canonicalize(p)
        .or_else(|_| std::path::absolute(p))
        .unwrap_or_else(|_| p.to_path_buf())
        .to_string_lossy()
        .to_string()
}

/// Id of an unfinished patch-apply job targeting the same game folder, if any.
pub fn active_patch_job_for_game(state: &AppState, game_path: &str) -> Option<String> {
    let key = patch_game_key(game_path);
    for job in state.active_jobs.iter() {
        if !matches!(job.kind, JobKind::Patch) {
            continue;
        }
        if job.patch_game_key.as_ref() != Some(&key) {
            continue;
        }
        let finished = job
            .replay
            .lock()
            .map(|log| log.iter().any(job_event_is_terminal))
            .unwrap_or(false);
        if !finished {
            return Some(job.key().clone());
        }
    }
    None
}

/// Id of an in-flight translation job, if any.
///
/// Jobs linger for replay after the worker's event stream ends. This is a UI
/// status query, not admission: cancelled workers may still be draining SQLite
/// writes and retain their project guard. Use `try_project_operation` to enter.
/// Patch-apply jobs never touch `state.db`.
pub fn active_translation_job(state: &AppState) -> Option<String> {
    for job in state.active_jobs.iter() {
        if !matches!(job.kind, JobKind::Translate) {
            continue;
        }
        let finished = job
            .replay
            .lock()
            .map(|log| log.iter().any(job_event_is_terminal))
            .unwrap_or(false);
        if !finished {
            return Some(job.key().clone());
        }
    }
    None
}

/// Shared HTTP/Tauri admission. Acquire before awaiting project/provider state,
/// and move the returned guard into any detached task that uses the live DB.
pub fn try_project_operation(state: &AppState) -> Result<ProjectExclusiveGuard, String> {
    if active_translation_job(state).is_some() {
        return Err(TRANSLATION_IN_FLIGHT_MESSAGE.into());
    }
    ProjectExclusiveGuard::try_enter(&state.project_exclusive).map_err(str::to_owned)
}

/// Keep DB ownership through all awaited writes even if the HTTP/Tauri caller
/// drops its response future. SQLite's blocking tasks cannot be cancelled.
pub async fn run_owned_project_operation<T, F>(
    guard: ProjectExclusiveGuard,
    operation: F,
) -> Result<T, tokio::task::JoinError>
where
    T: Send + 'static,
    F: std::future::Future<Output = T> + Send + 'static,
{
    tokio::spawn(async move {
        let _guard = guard;
        operation.await
    })
    .await
}

fn publish_job_event(
    tx: &broadcast::Sender<serde_json::Value>,
    replay: &std::sync::Mutex<Vec<serde_json::Value>>,
    event: serde_json::Value,
) {
    if let Ok(mut log) = replay.lock() {
        // Cancellation and the worker race to publish a terminal event. The
        // first terminal result wins, including for reconnecting WebSockets.
        if log.last().is_some_and(job_event_is_terminal) {
            return;
        }
        log.push(event.clone());
        let _ = tx.send(event);
    }
}

// ─── State ─────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ProjectInfo {
    pub path: PathBuf,
    pub format_id: String,
    pub name: String,
    #[serde(default)]
    pub extraction_warnings: Vec<String>,
    #[serde(default)]
    pub database_path: Option<PathBuf>,
    #[serde(default)]
    pub supported_modes: Vec<OutputMode>,
    #[serde(default)]
    pub persistence_warning: Option<String>,
}

pub struct AppState {
    pub format_registry: Arc<FormatRegistry>,
    pub provider_registry: Arc<RwLock<ProviderRegistry>>,
    pub db: Arc<Database>,
    pub glossary: Arc<Glossary>,
    pub config: Arc<RwLock<AppConfig>>,
    /// Captured at state creation; tests never write the user's configuration.
    pub config_path: PathBuf,
    /// A fallback profile may be read, but never persisted over failed input.
    pub config_load_failed: bool,
    pub backup_manager: Arc<BackupManager>,
    pub global_memory: Arc<GlobalMemoryDb>,
    pub active_jobs: Arc<DashMap<String, JobState>>,
    /// In-flight xAI device-code grants, keyed by an opaque uuid handle.
    pub xai_pending: Arc<DashMap<String, XaiPendingAuth>>,
    /// Test override for the device-code endpoint. `None` uses production.
    pub xai_device_code_url: Arc<RwLock<Option<String>>>,
    /// Test override for the token endpoint. `None` uses production.
    pub xai_token_url: Arc<RwLock<Option<String>>>,
    pub current_project: Arc<RwLock<Option<ProjectInfo>>>,
    /// Atomic live-project admission shared by HTTP and desktop operations.
    pub project_exclusive: Arc<AtomicUsize>,
    /// Test fixture owners drop after the database and other state fields.
    #[allow(dead_code)] // Read by unit tests; production states keep this empty.
    test_temp_dirs: std::sync::Mutex<Vec<tempfile::TempDir>>,
}

/// One in-flight xAI device login. The map key is never the raw `device_code`.
pub struct XaiPendingAuth {
    pub device: Arc<locust_providers::xai_oauth::DeviceCode>,
    pub last_poll: Option<Instant>,
}

/// Create production AppState with persistent storage in the user data directory.
pub fn create_app_state() -> Arc<AppState> {
    let data_dir = AppConfig::config_dir();
    std::fs::create_dir_all(&data_dir).expect("Failed to create data directory");

    let db_path = data_dir.join("project.db");
    let db = Arc::new(Database::open(&db_path).expect("Failed to open project database"));
    let glossary = Arc::new(Glossary::new(db.clone()));
    let backup_root = data_dir.join("backups");
    std::fs::create_dir_all(&backup_root).ok();

    // Startup never prunes: project DB recordings may reference any backup; users delete in Settings → Data.

    let config_path = AppConfig::default_path();
    let (config, config_load_failed) = config_persistence::load_startup_config(&config_path);
    let format_registry = locust_formats::default_registry();
    let provider_registry = locust_providers::default_registry(&config);

    let global_memory = GlobalMemoryDb::open_default()
        .unwrap_or_else(|_| GlobalMemoryDb::open_in_memory().unwrap());

    Arc::new(AppState {
        format_registry: Arc::new(format_registry),
        provider_registry: Arc::new(RwLock::new(provider_registry)),
        db,
        glossary,
        config: Arc::new(RwLock::new(config)),
        config_path,
        config_load_failed,
        backup_manager: Arc::new(BackupManager::new(backup_root)),
        global_memory: Arc::new(global_memory),
        active_jobs: Arc::new(DashMap::new()),
        xai_pending: Arc::new(DashMap::new()),
        xai_device_code_url: Arc::new(RwLock::new(None)),
        xai_token_url: Arc::new(RwLock::new(None)),
        current_project: Arc::new(RwLock::new(None)),
        project_exclusive: Arc::new(AtomicUsize::new(0)),
        test_temp_dirs: std::sync::Mutex::new(Vec::new()),
    })
}

pub fn create_test_state() -> Arc<AppState> {
    create_test_state_inner(Arc::new(Database::open_in_memory().unwrap()))
}

/// Test state whose project database lives at `db_path` — for integration
/// tests that must verify persisted side effects (e.g., the injection
/// recording `locust patch` packs from) by reopening the same file after a
/// request completes.
pub fn create_test_state_with_db(db_path: &std::path::Path) -> Arc<AppState> {
    create_test_state_inner(Arc::new(Database::open(db_path).unwrap()))
}

fn create_test_state_inner(db: Arc<Database>) -> Arc<AppState> {
    let glossary = Arc::new(Glossary::new(db.clone()));
    let backup_root_guard = tempfile::Builder::new()
        .prefix("locust_srv_")
        .tempdir()
        .unwrap();
    let backup_root = backup_root_guard.path().to_path_buf();
    let format_registry = locust_formats::default_registry();
    let config = AppConfig::default();
    let provider_registry = locust_providers::default_registry(&config);

    Arc::new(AppState {
        format_registry: Arc::new(format_registry),
        provider_registry: Arc::new(RwLock::new(provider_registry)),
        db,
        glossary,
        config: Arc::new(RwLock::new(config)),
        config_path: backup_root.join("config.json"),
        config_load_failed: false,
        backup_manager: Arc::new(BackupManager::new(backup_root.clone())),
        global_memory: Arc::new(GlobalMemoryDb::open_in_memory().unwrap()),
        active_jobs: Arc::new(DashMap::new()),
        xai_pending: Arc::new(DashMap::new()),
        xai_device_code_url: Arc::new(RwLock::new(None)),
        xai_token_url: Arc::new(RwLock::new(None)),
        current_project: Arc::new(RwLock::new(None)),
        project_exclusive: Arc::new(AtomicUsize::new(0)),
        test_temp_dirs: std::sync::Mutex::new(vec![backup_root_guard]),
    })
}

// ─── Loopback guard ────────────────────────────────────────────────────────
//
// Binding to 127.0.0.1 is not enough: a browser page on any site can still
// call that address, and DNS rebinding can present a foreign Host. CORS only
// hides responses, so "simple" requests (for example text/plain import) would
// still run. Refuse the request itself.

fn origin_allowed(origin: &str) -> bool {
    matches!(
        origin,
        "tauri://localhost" | "http://tauri.localhost" | "https://tauri.localhost"
    ) || loopback_http_origin(origin)
}

fn loopback_http_origin(origin: &str) -> bool {
    let Some(authority) = origin.strip_prefix("http://") else {
        return false;
    };
    if authority
        .bytes()
        .any(|b| matches!(b, b'/' | b'?' | b'#' | b'@' | b' ' | b'\\'))
    {
        return false;
    }
    let port = if let Some(rest) = authority.strip_prefix('[') {
        let Some((addr, after)) = rest.split_once(']') else {
            return false;
        };
        if !addr.eq_ignore_ascii_case("::1") {
            return false;
        }
        let Some(port) = after.strip_prefix(':') else {
            return false;
        };
        port
    } else {
        let Some((host, port)) = authority.split_once(':') else {
            return false;
        };
        if !(host.eq_ignore_ascii_case("localhost") || host == "127.0.0.1") {
            return false;
        }
        port
    };
    is_tcp_port(port)
}

fn host_allowed(host: &str) -> bool {
    match host_header_hostname(host) {
        Some(name) => {
            name.eq_ignore_ascii_case("localhost")
                || name == "127.0.0.1"
                || name.eq_ignore_ascii_case("::1")
        }
        None => false,
    }
}

/// Hostname from a `Host` header. Strips a numeric port and IPv6 brackets.
fn host_header_hostname(host: &str) -> Option<&str> {
    let host = host.trim();
    if host.is_empty() {
        return None;
    }
    if host.eq_ignore_ascii_case("::1") {
        return Some(host);
    }
    if let Some(rest) = host.strip_prefix('[') {
        let (addr, after) = rest.split_once(']')?;
        if addr.is_empty() {
            return None;
        }
        if after.is_empty() {
            return Some(addr);
        }
        let port = after.strip_prefix(':')?;
        if !is_tcp_port(port) {
            return None;
        }
        return Some(addr);
    }
    match host.split_once(':') {
        Some((name, port)) if !name.is_empty() && !name.contains(':') && is_tcp_port(port) => {
            Some(name)
        }
        Some(_) => None,
        None => Some(host),
    }
}

fn is_tcp_port(port: &str) -> bool {
    (1..=5).contains(&port.len())
        && port.bytes().all(|b| b.is_ascii_digit())
        && port.parse::<u16>().is_ok()
}

fn local_cors() -> CorsLayer {
    // Same origin predicate as the guard. Methods are unrestricted; headers
    // include content-type (PO/XLIFF import is text/plain, JSON is application/json).
    // The desktop client reads response bodies only (`res.text()`), never
    // response headers, so nothing is exposed beyond the CORS-safelisted set.
    CorsLayer::new()
        .allow_origin(AllowOrigin::predicate(|origin, _| {
            origin.to_str().is_ok_and(origin_allowed)
        }))
        .allow_methods(Any)
        .allow_headers(Any)
}

async fn guard_local_client(request: axum::extract::Request, next: Next) -> Response {
    if let Some(host) = request.headers().get(axum::http::header::HOST) {
        if !host.to_str().is_ok_and(host_allowed) {
            return StatusCode::FORBIDDEN.into_response();
        }
    }
    if let Some(origin) = request.headers().get(axum::http::header::ORIGIN) {
        if !origin.to_str().is_ok_and(origin_allowed) {
            return StatusCode::FORBIDDEN.into_response();
        }
    }
    next.run(request).await
}

// ─── Router ────────────────────────────────────────────────────────────────

pub fn create_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/api/formats", get(list_formats))
        .route("/api/formats/:id/modes", get(get_format_modes))
        .route("/api/providers", get(list_providers))
        .route("/api/providers/:id/health", post(provider_health))
        .route("/api/project/preflight", post(project_preflight))
        .route("/api/project/resume", post(project_resume))
        .route("/api/project/open", post(project_open))
        .route("/api/project/open-db", post(project_open_db))
        .route("/api/project/current", get(project_current))
        .route("/api/pivot", post(pivot_project))
        .route("/api/strings", get(get_strings))
        // Static paths before `:id` so "batch" / "facets" are never captured as ids.
        .route("/api/strings/batch", post(batch_patch_strings))
        .route("/api/strings/facets", get(get_string_facets))
        .route("/api/strings/:id", get(get_string).patch(patch_string))
        .route("/api/strings/:id/history", get(get_string_history))
        .route("/api/stats", get(get_stats))
        .route("/api/stats/files", get(get_file_stats))
        .route("/api/runs", get(list_translation_runs))
        .route("/api/translate/start", post(translate_start))
        .route("/api/translate/cancel/:job_id", post(translate_cancel))
        .route("/api/translate/ws/:job_id", get(translate_ws))
        .route("/api/inject", post(inject))
        .route("/api/inject/status", post(injection_recovery::status))
        .route("/api/inject/recover", post(injection_recovery::recover))
        .route("/api/register-lang", post(register_lang))
        .route("/api/patch/verify", post(patch_verify))
        .route("/api/patch/apply", post(patch_apply))
        .route("/api/patch/cancel/:job_id", post(patch_cancel))
        .route("/api/patch/ws/:job_id", get(patch_ws))
        .route("/api/patch/rollback", post(patch_rollback))
        .route("/api/patch/status", post(patch_status))
        .route("/api/patch/pack", post(patch_pack))
        .route("/api/patch/identity", get(patch_identity))
        .route("/api/patch/font", post(font_patch::generate))
        .route("/api/patch/recordings", get(patch_recordings))
        .route("/api/validate", post(validate))
        .route("/api/validation/issues", get(list_validation_issues))
        .route("/api/validation/summary", get(validation_summary))
        .route(
            "/api/validation/issues/:id",
            axum::routing::patch(set_validation_issue_ignored),
        )
        .route("/api/glossary", get(get_glossary).post(add_glossary))
        .route("/api/glossary/:term", delete(delete_glossary))
        .route("/api/export/po", get(export_po))
        .route("/api/import/po", post(import_po))
        .route("/api/export/xliff", get(export_xliff))
        .route("/api/import/xliff", post(import_xliff))
        .route("/api/auth/xai/start", post(auth_xai_start))
        .route("/api/auth/xai/poll", post(auth_xai_poll))
        .route("/api/config", get(get_config).patch(patch_config))
        .route("/api/memory/stats", get(memory_stats))
        .route("/api/memory", get(list_memory).delete(clear_memory))
        .route("/api/memory/:hash/:lang_pair", delete(delete_memory_entry))
        .route("/api/memory/lang-pairs", get(memory_lang_pairs))
        .route("/api/memory/suggestions", get(memory_suggestions))
        .route("/api/backups", get(list_backups))
        .route("/api/backups/:id/restore", post(restore_backup))
        .route("/api/backups/:id", delete(delete_backup))
        // CORS is inner so allowed preflights are answered here. The guard is
        // the last layer, so axum runs it first and refuses foreign Origin/Host
        // before a handler (or this CORS layer) can run.
        .layer(local_cors())
        .layer(middleware::from_fn(guard_local_client))
        .with_state(state)
}

pub async fn start_server(state: Arc<AppState>, port: u16) -> anyhow::Result<()> {
    // Loopback only by default: patch apply/rollback take absolute filesystem
    // paths and must not be reachable from the LAN without an explicit opt-in
    // (see start_server_on). `create_router` also refuses a non-loopback Host
    // (DNS rebinding) and a foreign Origin, so another website cannot drive
    // the API even on 127.0.0.1. Desktop and CLI talk over localhost.
    start_server_on(state, format!("127.0.0.1:{port}")).await
}

/// Bind the API on an explicit address (`127.0.0.1:7842` or `0.0.0.0:7842`).
/// Prefer loopback unless the operator knowingly exposes the process.
pub async fn start_server_on(state: Arc<AppState>, addr: impl AsRef<str>) -> anyhow::Result<()> {
    let listener = tokio::net::TcpListener::bind(addr.as_ref()).await?;
    start_server_with_listener(state, listener).await
}

/// Serve an already-bound listener, preserving ownership of an OS-assigned
/// port between desktop startup and the background runtime.
pub async fn start_server_with_listener(
    state: Arc<AppState>,
    listener: tokio::net::TcpListener,
) -> anyhow::Result<()> {
    let app = create_router(state);
    tracing::info!("Server listening on {}", listener.local_addr()?);
    axum::serve(listener, app).await?;
    Ok(())
}

pub async fn start_test_server(state: Arc<AppState>) -> (String, tokio::task::JoinHandle<()>) {
    let app = create_router(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let url = format!("http://{}", addr);
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (url, handle)
}

#[cfg(test)]
#[path = "../tests/common/mod.rs"]
mod test_server;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routes::test_support::*;
    use locust_core::models::ProgressEvent;
    use std::time::Duration;

    static DATA_DIR_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    struct DataDirRestore(Option<std::ffi::OsString>);

    impl DataDirRestore {
        fn capture() -> Self {
            Self(std::env::var_os("LOCUST_DATA_DIR"))
        }
    }

    impl Drop for DataDirRestore {
        fn drop(&mut self) {
            match self.0.take() {
                Some(value) => std::env::set_var("LOCUST_DATA_DIR", value),
                None => std::env::remove_var("LOCUST_DATA_DIR"),
            }
        }
    }

    #[test]
    fn startup_keeps_all_existing_backups() {
        let _lock = DATA_DIR_LOCK.lock().unwrap();
        let _data_dir_restore = DataDirRestore::capture();
        let data_dir = tempfile::tempdir().unwrap();
        std::env::set_var("LOCUST_DATA_DIR", data_dir.path());

        let backup_manager = BackupManager::new(data_dir.path().join("backups"));
        for index in 0..6 {
            let game = data_dir.path().join(format!("game-{index}"));
            std::fs::create_dir(&game).unwrap();
            std::fs::write(game.join("story.txt"), format!("original-{index}")).unwrap();
            backup_manager.create_backup(&game).unwrap();
        }
        assert_eq!(backup_manager.list_backups().unwrap().len(), 6);
        drop(backup_manager);

        let state = create_app_state();
        assert_eq!(state.backup_manager.list_backups().unwrap().len(), 6);
    }

    #[test]
    fn recoverable_batch_failure_keeps_job_stream_open() {
        let event = serde_json::to_value(ProgressEvent::BatchFailed {
            entry_id: None,
            error: "provider unavailable; trying fallback".into(),
        })
        .unwrap();
        assert_eq!(event["type"], "batch_failed");
        assert!(!job_event_is_terminal(&event));
        let terminal = serde_json::to_value(ProgressEvent::Failed {
            entry_id: None,
            error: "budget exhausted".into(),
        })
        .unwrap();
        assert!(job_event_is_terminal(&terminal));
    }

    #[tokio::test]
    async fn active_patch_job_for_game_ignores_finished_and_other_folders() {
        let state = create_test_state();
        let game_a_guard = tempfile::Builder::new()
            .prefix("locust_patch_a_")
            .tempdir()
            .unwrap();
        let game_a = game_a_guard.path().to_path_buf();
        let game_b_guard = tempfile::Builder::new()
            .prefix("locust_patch_b_")
            .tempdir()
            .unwrap();
        let game_b = game_b_guard.path().to_path_buf();
        std::fs::create_dir_all(&game_a).unwrap();
        std::fs::create_dir_all(&game_b).unwrap();
        let key_a = super::patch_game_key(&game_a.to_string_lossy());

        park_job_with_patch_key(
            &state,
            "done",
            JobKind::Patch,
            Some("done"),
            Some(key_a.clone()),
        );
        assert!(active_patch_job_for_game(&state, &game_a.to_string_lossy()).is_none());

        park_job_with_patch_key(&state, "live", JobKind::Patch, None, Some(key_a));
        assert_eq!(
            active_patch_job_for_game(&state, &game_a.to_string_lossy()).as_deref(),
            Some("live")
        );
        assert!(active_patch_job_for_game(&state, &game_b.to_string_lossy()).is_none());

        let _ = std::fs::remove_dir_all(&game_a);
        let _ = std::fs::remove_dir_all(&game_b);
    }

    #[test]
    fn origin_and_host_allow_lists() {
        for origin in [
            "tauri://localhost",
            "http://tauri.localhost",
            "https://tauri.localhost",
            "http://localhost:1420",
            "http://localhost:80",
            "http://127.0.0.1:7842",
            "http://127.0.0.1:1",
            "http://[::1]:1420",
            "http://[::1]:80",
        ] {
            assert!(origin_allowed(origin), "{origin}");
        }
        for origin in [
            "https://evil.example",
            "http://evil.example",
            "http://localhost.evil.example",
            "http://localhost.evil.example:80",
            "http://127.0.0.1.evil.example",
            "http://127.0.0.1.evil.example:80",
            "null",
            "http://[::1]",
            "http://::1:1420",
            "https://localhost:1420",
            "https://127.0.0.1:1420",
            "http://localhost",
            "tauri://evil.example",
        ] {
            assert!(!origin_allowed(origin), "{origin}");
        }

        for host in [
            "localhost",
            "localhost:1420",
            "127.0.0.1",
            "127.0.0.1:7842",
            "[::1]",
            "[::1]:1420",
            "::1",
        ] {
            assert!(host_allowed(host), "{host}");
        }
        for host in [
            "evil.example",
            "evil.example:80",
            "localhost.evil.example",
            "127.0.0.1.evil.example",
            "[::1",
            "127.0.0.1.1",
            "",
        ] {
            assert!(!host_allowed(host), "{host}");
        }
    }

    fn assert_no_acao(headers: &reqwest::header::HeaderMap) {
        assert!(
            headers.get("access-control-allow-origin").is_none(),
            "refused request must not grant CORS: {headers:?}"
        );
    }

    #[tokio::test]
    async fn foreign_origin_preflight_is_forbidden() {
        let (url, _h) = setup().await;
        let resp = client()
            .request(
                reqwest::Method::OPTIONS,
                format!("{url}/api/patch/rollback"),
            )
            .header("Origin", "https://evil.example")
            .header("Access-Control-Request-Method", "POST")
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status().as_u16(), 403);
        assert_no_acao(resp.headers());
    }

    #[tokio::test]
    async fn foreign_origin_post_does_not_mutate() {
        let (url, _h, state) = setup_with_state().await;
        mark_project_open(&state).await;
        let resp = client()
            .post(format!("{url}/api/glossary"))
            .header("Origin", "https://evil.example")
            .header("Content-Type", "text/plain")
            .body(
                r#"{"term":"HP","translation":"PV","lang_pair":"en-es","context":null,"case_sensitive":false}"#,
            )
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status().as_u16(), 403);
        assert_no_acao(resp.headers());

        let resp = client()
            .get(format!("{url}/api/glossary?lang_pair=en-es"))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status().as_u16(), 200);
        let body: Vec<serde_json::Value> = resp.json().await.unwrap();
        assert!(
            body.is_empty(),
            "foreign origin must not insert a glossary row"
        );
    }

    #[tokio::test]
    async fn tauri_origin_preflight_is_allowed() {
        let (url, _h) = setup().await;
        let resp = client()
            .request(
                reqwest::Method::OPTIONS,
                format!("{url}/api/patch/rollback"),
            )
            .header("Origin", "http://tauri.localhost")
            .header("Access-Control-Request-Method", "POST")
            .header("Access-Control-Request-Headers", "content-type")
            .send()
            .await
            .unwrap();
        assert!(resp.status().is_success(), "{}", resp.status());
        assert_eq!(
            resp.headers()
                .get("access-control-allow-origin")
                .and_then(|v| v.to_str().ok()),
            Some("http://tauri.localhost")
        );
        let allow_headers = resp
            .headers()
            .get("access-control-allow-headers")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        assert!(
            allow_headers == "*" || allow_headers.to_ascii_lowercase().contains("content-type"),
            "{allow_headers}"
        );
    }

    #[tokio::test]
    async fn request_without_origin_is_allowed() {
        let (url, _h) = setup().await;
        let health = client().get(format!("{url}/health")).send().await.unwrap();
        assert_eq!(health.status().as_u16(), 200);
        let formats = client()
            .get(format!("{url}/api/formats"))
            .send()
            .await
            .unwrap();
        assert_eq!(formats.status().as_u16(), 200);
    }

    async fn raw_http(addr: &str, request: &str) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut buf = [0u8; 2048];
        let n = tokio::time::timeout(Duration::from_secs(5), stream.read(&mut buf))
            .await
            .expect("timed out waiting for the response")
            .unwrap();
        String::from_utf8_lossy(&buf[..n]).into_owned()
    }

    #[tokio::test]
    async fn foreign_host_is_forbidden() {
        let (url, _h) = setup().await;
        let addr = url.trim_start_matches("http://");
        let text = raw_http(
            addr,
            "GET /health HTTP/1.1\r\nHost: evil.example\r\nConnection: close\r\n\r\n",
        )
        .await;
        assert!(
            text.starts_with("HTTP/1.1 403"),
            "foreign host must be refused, got {text}"
        );
        assert!(
            !text
                .to_ascii_lowercase()
                .contains("access-control-allow-origin"),
            "{text}"
        );
    }

    #[tokio::test]
    async fn foreign_origin_websocket_is_forbidden() {
        let (url, _h) = setup().await;
        let addr = url.trim_start_matches("http://");
        let request = format!(
            "GET /api/translate/ws/x HTTP/1.1\r\n\
             Host: {addr}\r\n\
             Origin: https://evil.example\r\n\
             Connection: Upgrade\r\n\
             Upgrade: websocket\r\n\
             Sec-WebSocket-Version: 13\r\n\
             Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
             \r\n"
        );
        let text = raw_http(addr, &request).await;
        assert!(
            text.starts_with("HTTP/1.1 403"),
            "foreign websocket origin must be refused, got {text}"
        );
        assert!(
            !text
                .to_ascii_lowercase()
                .contains("access-control-allow-origin"),
            "{text}"
        );
    }

    #[tokio::test]
    async fn test_cors_header_present() {
        let (url, _h) = setup().await;
        let resp = client()
            .get(format!("{url}/health"))
            .header("Origin", "http://127.0.0.1:1420")
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status().as_u16(), 200);
        assert_eq!(
            resp.headers()
                .get("access-control-allow-origin")
                .and_then(|v| v.to_str().ok()),
            Some("http://127.0.0.1:1420"),
            "allowed loopback origins are echoed, not *"
        );

        let resp = client()
            .get(format!("{url}/health"))
            .header("Origin", "https://evil.example")
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status().as_u16(), 403);
        assert_no_acao(resp.headers());
    }
}

#[cfg(test)]
mod exclusion_tests;
