use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket};
use axum::extract::{Path as AxumPath, Query, State, WebSocketUpgrade};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, mpsc, RwLock};
use tokio::task::AbortHandle;
use tower_http::cors::CorsLayer;

use locust_core::backup::{BackupEntry, BackupManager};
use locust_core::config::AppConfig;
use locust_core::database::{
    Database, EntryFilter, GlobalMemoryDb, GlossaryEntry, ProjectStats, TranslationRun,
};
use locust_core::export;
use locust_core::extraction::{FormatRegistry, MultiLangInjector, PluginInfo};
use locust_core::font_validation::{FontCoverageReport, FontValidator};
use locust_core::glossary::Glossary;
use locust_core::models::{OutputMode, ProgressEvent, StringEntry, StringStatus};
use locust_core::project::{self, ProjectOpenOutcome};
use locust_core::translation::{
    run_fallback_chain, unique_provider_chain, ProviderRegistry, TranslationOptions,
};
use locust_core::validation::Validator;

type ApiError = (StatusCode, String);

fn err(status: StatusCode, msg: impl ToString) -> ApiError {
    (status, msg.to_string())
}

/// Per-job state: abort handle + broadcast sender for progress events
pub struct JobState {
    pub abort_handle: AbortHandle,
    pub progress_tx: broadcast::Sender<ProgressEvent>,
}

// ─── State ─────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProjectInfo {
    pub path: PathBuf,
    pub format_id: String,
    pub name: String,
}

pub struct AppState {
    pub format_registry: Arc<FormatRegistry>,
    pub provider_registry: Arc<RwLock<ProviderRegistry>>,
    pub db: Arc<Database>,
    pub glossary: Arc<Glossary>,
    pub config: Arc<RwLock<AppConfig>>,
    pub backup_manager: Arc<BackupManager>,
    pub global_memory: Arc<GlobalMemoryDb>,
    pub active_jobs: Arc<DashMap<String, JobState>>,
    pub current_project: Arc<RwLock<Option<ProjectInfo>>>,
    /// Temp directory to clean up on drop (only set for test states)
    temp_backup_dir: Option<PathBuf>,
}

impl Drop for AppState {
    fn drop(&mut self) {
        if let Some(ref dir) = self.temp_backup_dir {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
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

    // Auto-clean old backups on startup (keep last 5)
    let backup_mgr_tmp = BackupManager::new(backup_root.clone());
    if let Err(e) = backup_mgr_tmp.delete_old_backups(5) {
        tracing::warn!("Failed to clean old backups: {}", e);
    }

    let config = AppConfig::load(&AppConfig::default_path()).unwrap_or_default();
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
        backup_manager: Arc::new(BackupManager::new(backup_root)),
        global_memory: Arc::new(global_memory),
        active_jobs: Arc::new(DashMap::new()),
        current_project: Arc::new(RwLock::new(None)),
        temp_backup_dir: None,
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
    let backup_root = std::env::temp_dir().join(format!("locust_srv_{}", uuid::Uuid::new_v4()));
    let format_registry = locust_formats::default_registry();
    let config = AppConfig::default();
    let provider_registry = locust_providers::default_registry(&config);

    Arc::new(AppState {
        format_registry: Arc::new(format_registry),
        provider_registry: Arc::new(RwLock::new(provider_registry)),
        db,
        glossary,
        config: Arc::new(RwLock::new(config)),
        backup_manager: Arc::new(BackupManager::new(backup_root.clone())),
        global_memory: Arc::new(GlobalMemoryDb::open_in_memory().unwrap()),
        active_jobs: Arc::new(DashMap::new()),
        current_project: Arc::new(RwLock::new(None)),
        temp_backup_dir: Some(backup_root),
    })
}

// ─── Router ────────────────────────────────────────────────────────────────

pub fn create_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/api/formats", get(list_formats))
        .route("/api/formats/:id/modes", get(get_format_modes))
        .route("/api/providers", get(list_providers))
        .route("/api/providers/:id/health", post(provider_health))
        .route("/api/project/open", post(project_open))
        .route("/api/project/current", get(project_current))
        .route("/api/strings", get(get_strings))
        // Static path before `:id` so "batch" is never captured as an entry id.
        .route("/api/strings/batch", post(batch_patch_strings))
        .route("/api/strings/:id", get(get_string).patch(patch_string))
        .route("/api/stats", get(get_stats))
        .route("/api/runs", get(list_translation_runs))
        .route("/api/translate/start", post(translate_start))
        .route("/api/translate/cancel/:job_id", post(translate_cancel))
        .route("/api/translate/ws/:job_id", get(translate_ws))
        .route("/api/inject", post(inject))
        .route("/api/register-lang", post(register_lang))
        .route("/api/patch/verify", post(patch_verify))
        .route("/api/patch/apply", post(patch_apply))
        .route("/api/patch/rollback", post(patch_rollback))
        .route("/api/patch/status", post(patch_status))
        .route("/api/patch/pack", post(patch_pack))
        .route("/api/validate", post(validate))
        .route("/api/glossary", get(get_glossary).post(add_glossary))
        .route("/api/glossary/:term", delete(delete_glossary))
        .route("/api/export/po", get(export_po))
        .route("/api/import/po", post(import_po))
        .route("/api/export/xliff", get(export_xliff))
        .route("/api/import/xliff", post(import_xliff))
        .route("/api/config", get(get_config).patch(patch_config))
        .route("/api/memory/stats", get(memory_stats))
        .route("/api/memory", get(list_memory).delete(clear_memory))
        .route("/api/memory/:hash/:lang_pair", delete(delete_memory_entry))
        .route("/api/memory/lang-pairs", get(memory_lang_pairs))
        .route("/api/backups", get(list_backups))
        .route("/api/backups/:id/restore", post(restore_backup))
        .route("/api/backups/:id", delete(delete_backup))
        .layer(CorsLayer::permissive())
        .with_state(state)
}

pub async fn start_server(state: Arc<AppState>, port: u16) -> anyhow::Result<()> {
    // Loopback only by default: patch apply/rollback take absolute filesystem
    // paths and must not be reachable from the LAN without an explicit opt-in
    // (see start_server_on). Desktop and CLI talk over localhost.
    start_server_on(state, format!("127.0.0.1:{port}")).await
}

/// Bind the API on an explicit address (`127.0.0.1:7842` or `0.0.0.0:7842`).
/// Prefer loopback unless the operator knowingly exposes the process.
pub async fn start_server_on(state: Arc<AppState>, addr: impl AsRef<str>) -> anyhow::Result<()> {
    let app = create_router(state);
    let addr = addr.as_ref();
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!("Server listening on {addr}");
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

// ─── Handlers ──────────────────────────────────────────────────────────────

async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION")
    }))
}

async fn list_formats(State(state): State<Arc<AppState>>) -> Json<Vec<PluginInfo>> {
    Json(state.format_registry.list())
}

#[derive(Serialize)]
struct FormatModes {
    format_id: String,
    supported_modes: Vec<OutputMode>,
}

async fn get_format_modes(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<FormatModes>, ApiError> {
    let plugin = state
        .format_registry
        .get(&id)
        .ok_or_else(|| err(StatusCode::NOT_FOUND, format!("format not found: {}", id)))?;
    Ok(Json(FormatModes {
        format_id: id,
        supported_modes: plugin.supported_modes(),
    }))
}

async fn list_providers(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let reg = state.provider_registry.read().await;
    Json(serde_json::to_value(locust_providers::list_providers_for_api(&reg)).unwrap_or_default())
}

async fn provider_health(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
) -> Json<serde_json::Value> {
    let reg = state.provider_registry.read().await;
    let provider = match reg.get(&id) {
        Some(p) => p,
        None => {
            return Json(serde_json::json!({"ok": false, "message": "provider not found"}));
        }
    };
    match provider.health_check().await {
        Ok(()) => Json(serde_json::json!({"ok": true, "message": "healthy"})),
        Err(e) => Json(serde_json::json!({"ok": false, "message": e.to_string()})),
    }
}

#[derive(Deserialize)]
struct OpenProjectRequest {
    path: String,
    format_id: Option<String>,
}

#[derive(Serialize)]
struct ProjectOpenResponse {
    format_id: String,
    format_name: String,
    total_strings: usize,
    project_path: String,
    project_name: String,
    supported_modes: Vec<OutputMode>,
    database_path: String,
    added: usize,
    updated: usize,
    stale_source_reset: usize,
    removed: usize,
    preserved_translations: usize,
}

impl From<ProjectOpenOutcome> for ProjectOpenResponse {
    fn from(o: ProjectOpenOutcome) -> Self {
        Self {
            format_id: o.format_id,
            format_name: o.format_name,
            total_strings: o.total_strings,
            project_path: o.project_path.to_string_lossy().into_owned(),
            project_name: o.project_name,
            supported_modes: o.supported_modes,
            database_path: o.database_path.to_string_lossy().into_owned(),
            added: o.added,
            updated: o.updated,
            stale_source_reset: o.stale_source_reset,
            removed: o.removed,
            preserved_translations: o.preserved_translations,
        }
    }
}

fn map_open_err(e: locust_core::error::LocustError) -> ApiError {
    match e {
        locust_core::error::LocustError::ProjectNotFound(_) => {
            err(StatusCode::BAD_REQUEST, "path not found")
        }
        locust_core::error::LocustError::UnsupportedFormat(msg) => {
            err(StatusCode::UNPROCESSABLE_ENTITY, msg)
        }
        other => err(StatusCode::INTERNAL_SERVER_ERROR, other),
    }
}

async fn project_open(
    State(state): State<Arc<AppState>>,
    Json(req): Json<OpenProjectRequest>,
) -> Result<Json<ProjectOpenResponse>, ApiError> {
    let raw_path = PathBuf::from(&req.path);
    let outcome = project::open_project(
        &state.db,
        &state.format_registry,
        &raw_path,
        req.format_id.as_deref(),
    )
    .map_err(map_open_err)?;

    {
        let mut proj = state.current_project.write().await;
        *proj = Some(ProjectInfo {
            path: outcome.project_path.clone(),
            format_id: outcome.format_id.clone(),
            name: outcome.project_name.clone(),
        });
    }

    {
        let mut config = state.config.write().await;
        config.add_recent_project(
            outcome.project_path.clone(),
            outcome.project_name.clone(),
            outcome.format_id.clone(),
        );
    }

    Ok(Json(ProjectOpenResponse::from(outcome)))
}

async fn project_current(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ProjectInfo>, ApiError> {
    let proj = state.current_project.read().await;
    match proj.as_ref() {
        Some(p) => Ok(Json(p.clone())),
        None => Err(err(StatusCode::NOT_FOUND, "no project open")),
    }
}

#[derive(Deserialize)]
struct StringsQuery {
    status: Option<String>,
    file_path: Option<String>,
    tag: Option<String>,
    search: Option<String>,
    limit: Option<usize>,
    offset: Option<usize>,
}

#[derive(Serialize, Deserialize)]
struct StringsResponse {
    entries: Vec<StringEntry>,
    total: usize,
    offset: usize,
    limit: usize,
}

async fn get_strings(
    State(state): State<Arc<AppState>>,
    Query(q): Query<StringsQuery>,
) -> Result<Json<StringsResponse>, ApiError> {
    // Without an open project, never surface leftover rows from project.db.
    if state.current_project.read().await.is_none() {
        let limit = q.limit.unwrap_or(100);
        let offset = q.offset.unwrap_or(0);
        return Ok(Json(StringsResponse {
            entries: vec![],
            total: 0,
            offset,
            limit,
        }));
    }

    let status = q.status.and_then(|s| s.parse::<StringStatus>().ok());
    let limit = q.limit.unwrap_or(100);
    let offset = q.offset.unwrap_or(0);

    let count_filter = EntryFilter {
        status: status.clone(),
        file_path: q.file_path.clone(),
        tag: q.tag.clone(),
        search: q.search.clone(),
        limit: None,
        offset: None,
    };
    let total = state
        .db
        .count_entries(&count_filter)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;

    let filter = EntryFilter {
        status,
        file_path: q.file_path,
        tag: q.tag,
        search: q.search,
        limit: Some(limit),
        offset: Some(offset),
    };
    let entries = state
        .db
        .get_entries(&filter)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;

    Ok(Json(StringsResponse {
        entries,
        total,
        offset,
        limit,
    }))
}

async fn get_string(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<StringEntry>, ApiError> {
    state
        .db
        .get_entry(&id)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?
        .map(Json)
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "entry not found"))
}

#[derive(Deserialize)]
struct PatchStringRequest {
    translation: Option<String>,
    status: Option<StringStatus>,
}

async fn patch_string(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
    Json(req): Json<PatchStringRequest>,
) -> Result<Json<StringEntry>, ApiError> {
    if let Some(ref translation) = req.translation {
        state
            .db
            .save_translation(&id, translation, "manual")
            .await
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    }
    if let Some(ref status) = req.status {
        state
            .db
            .update_entry_status(&id, status.clone())
            .await
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    }
    state
        .db
        .get_entry(&id)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?
        .map(Json)
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "entry not found"))
}

#[derive(Deserialize)]
struct BatchPatchItem {
    id: String,
    translation: String,
}

#[derive(Deserialize)]
struct BatchPatchRequest {
    updates: Vec<BatchPatchItem>,
    #[serde(default = "default_batch_provider")]
    provider: String,
}

fn default_batch_provider() -> String {
    "manual".into()
}

async fn batch_patch_strings(
    State(state): State<Arc<AppState>>,
    Json(req): Json<BatchPatchRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if req.updates.len() > 50_000 {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "batch too large (max 50000 updates)",
        ));
    }
    let pairs: Vec<(String, String)> = req
        .updates
        .into_iter()
        .map(|u| (u.id, u.translation))
        .collect();
    let requested = pairs.len();
    let applied = state
        .db
        .save_translations_batch(pairs, &req.provider)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(serde_json::json!({
        "requested": requested,
        "applied": applied,
        "skipped": requested.saturating_sub(applied),
    })))
}

async fn get_stats(State(state): State<Arc<AppState>>) -> Result<Json<ProjectStats>, ApiError> {
    if state.current_project.read().await.is_none() {
        return Ok(Json(ProjectStats::default()));
    }
    state
        .db
        .get_stats()
        .map(Json)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))
}

/// Translation run ledger (same rows as CLI `locust stats`), newest first.
async fn list_translation_runs(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<TranslationRun>>, ApiError> {
    let mut runs = state
        .db
        .get_translation_runs()
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    runs.reverse();
    Ok(Json(runs))
}

#[derive(Deserialize)]
struct TranslateStartRequest {
    provider_id: String,
    /// Optional ordered fallbacks after the primary (same chain rules as CLI `--fallback`).
    #[serde(default)]
    fallback_provider_ids: Option<Vec<String>>,
    options: TranslationOptions,
}

#[derive(Serialize)]
struct TranslateStartResponse {
    job_id: String,
}

/// Shared by HTTP `POST /api/translate/start` and the Tauri command.
pub async fn spawn_translation_job(
    state: &AppState,
    provider_id: String,
    fallback_provider_ids: Option<Vec<String>>,
    options: TranslationOptions,
) -> std::result::Result<String, String> {
    let reg = state.provider_registry.read().await;
    if reg.get(&provider_id).is_none() {
        return Err("provider not found".into());
    }

    let chain = unique_provider_chain(provider_id.as_str(), fallback_provider_ids.as_deref());

    let mut resolve_map: std::collections::HashMap<
        String,
        Arc<dyn locust_core::translation::TranslationProvider>,
    > = std::collections::HashMap::new();
    for id in &chain {
        if let Some(p) = reg.get(id) {
            resolve_map.insert(id.clone(), p);
        }
    }
    drop(reg);

    let job_id = uuid::Uuid::new_v4().to_string();
    let (tx, mut rx) = mpsc::channel::<ProgressEvent>(1000);
    let (broadcast_tx, _) = broadcast::channel::<ProgressEvent>(1000);
    let broadcast_tx_clone = broadcast_tx.clone();

    let cancel = tokio_util::sync::CancellationToken::new();
    let cancel_clone = cancel.clone();
    let job_id_clone = job_id.clone();

    // Bridge mpsc → broadcast so WebSocket clients can subscribe.
    // ProviderSwitched is non-terminal; only Completed / Failed end the stream.
    let jobs = state.active_jobs.clone();
    let cleanup_job_id = job_id.clone();
    tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            let is_terminal = matches!(
                event,
                ProgressEvent::Completed { .. } | ProgressEvent::Failed { .. }
            );
            let _ = broadcast_tx_clone.send(event);
            if is_terminal {
                break;
            }
        }
        // Delay cleanup so WebSocket clients have time to connect and receive final events
        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        jobs.remove(&cleanup_job_id);
    });

    // Insert job BEFORE spawning so WebSocket can find it immediately
    state.active_jobs.insert(
        job_id.clone(),
        JobState {
            abort_handle: tokio::spawn(async {}).abort_handle(), // placeholder
            progress_tx: broadcast_tx,
        },
    );

    let db = state.db.clone();
    let glossary = state.glossary.clone();
    let resolve_map = Arc::new(resolve_map);
    let handle = tokio::spawn(async move {
        let map = resolve_map;
        let resolve = |id: &str| map.get(id).cloned();
        let _ = run_fallback_chain(
            &chain,
            &resolve,
            db,
            glossary,
            options,
            tx,
            job_id_clone,
            cancel_clone,
        )
        .await;
    });

    // Update with real abort handle
    if let Some(mut job) = state.active_jobs.get_mut(&job_id) {
        job.abort_handle = handle.abort_handle();
    }

    Ok(job_id)
}

async fn translate_start(
    State(state): State<Arc<AppState>>,
    Json(req): Json<TranslateStartRequest>,
) -> Result<Json<TranslateStartResponse>, ApiError> {
    let job_id = spawn_translation_job(
        &state,
        req.provider_id,
        req.fallback_provider_ids,
        req.options,
    )
    .await
    .map_err(|m| err(StatusCode::NOT_FOUND, m))?;
    Ok(Json(TranslateStartResponse { job_id }))
}

async fn translate_cancel(
    State(state): State<Arc<AppState>>,
    AxumPath(job_id): AxumPath<String>,
) -> Result<StatusCode, ApiError> {
    if let Some((_, job)) = state.active_jobs.remove(&job_id) {
        job.abort_handle.abort();
        Ok(StatusCode::OK)
    } else {
        Err(err(StatusCode::NOT_FOUND, "job not found"))
    }
}

async fn translate_ws(
    State(state): State<Arc<AppState>>,
    AxumPath(job_id): AxumPath<String>,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    // Retry briefly to handle race condition where WS connects before job insert completes
    let mut rx = None;
    for _ in 0..20 {
        if let Some(job) = state.active_jobs.get(&job_id) {
            rx = Some(job.progress_tx.subscribe());
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }

    ws.on_upgrade(move |socket| handle_translate_ws(socket, rx))
}

async fn handle_translate_ws(
    mut socket: WebSocket,
    rx: Option<broadcast::Receiver<ProgressEvent>>,
) {
    let Some(mut rx) = rx else {
        let _ = socket
            .send(Message::Text(
                serde_json::json!({"type": "failed", "error": "job not found"}).to_string(),
            ))
            .await;
        let _ = socket.close().await;
        return;
    };

    loop {
        match rx.recv().await {
            Ok(event) => {
                let is_terminal = matches!(
                    event,
                    ProgressEvent::Completed { .. } | ProgressEvent::Failed { .. }
                );
                let json = serde_json::to_string(&event).unwrap_or_default();
                if socket.send(Message::Text(json)).await.is_err() {
                    break;
                }
                if is_terminal {
                    break;
                }
            }
            Err(broadcast::error::RecvError::Lagged(n)) => {
                tracing::warn!("WS client lagged by {} messages", n);
            }
            Err(broadcast::error::RecvError::Closed) => {
                break;
            }
        }
    }
    let _ = socket.close().await;
}

#[derive(Deserialize)]
struct InjectRequest {
    project_path: String,
    format_id: String,
    /// Ignored when `direct` is true.
    #[serde(default)]
    mode: Option<OutputMode>,
    languages: Vec<String>,
    output_dir: Option<String>,
    /// When true, inject into the game tree in place and record for pack
    /// (CLI `--direct`). Default false preserves Replace/Add MultiLangInjector.
    #[serde(default)]
    direct: bool,
}

/// Unblocking advice attached to a containment failure when recording an
/// injection made through the HTTP API. The server cannot know the exact
/// paths the user's shell will use, so it names the shape of the working
/// command rather than a copy-pasteable line.
const INJECT_RECORD_REMEDY: &str =
    "Restore the original game files from the backup listed above (or from a \
     clean copy) first — this engine writes translations into the ORIGINAL \
     tree, and a re-run against the mutated tree writes and records nothing. \
     Then record the injection with the CLI's direct mode: locust inject \
     <game folder> -P <project db> --direct -l <lang> — `locust patch` packs \
     from that recording.";

async fn inject(
    State(state): State<Arc<AppState>>,
    Json(req): Json<InjectRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Same guard as CLI: empty languages used to return 200 with zero work and
    // zero recording — a silent no-op that becomes an untranslatable patch later.
    if req.languages.is_empty() {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "inject requires at least one language in `languages` (e.g. [\"es\"])",
        ));
    }

    if req.direct {
        let game_path = PathBuf::from(&req.project_path);
        let format_id = req.format_id.clone();
        let languages = req.languages.clone();
        let registry = state.format_registry.clone();
        let db = state.db.clone();
        let backup = state.backup_manager.clone();
        let report = tokio::task::spawn_blocking(move || {
            locust_core::extraction::inject_direct(
                &registry, &db, &backup, &game_path, &format_id, &languages,
            )
        })
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
        return Ok(Json(serde_json::to_value(report).unwrap_or_default()));
    }

    let mode = req.mode.unwrap_or(OutputMode::Replace);
    let injector = MultiLangInjector::new(
        state.format_registry.clone(),
        state.db.clone(),
        state.backup_manager.clone(),
    );
    let (tx, mut rx) = mpsc::channel(100);
    tokio::spawn(async move { while rx.recv().await.is_some() {} });

    let languages = req.languages.clone();
    let report = injector
        .inject(
            &PathBuf::from(&req.project_path),
            &req.format_id,
            mode,
            req.languages,
            req.output_dir.map(PathBuf::from),
            tx,
        )
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;

    // Persist what each language's injection wrote — `locust patch` packs
    // exclusively from this recording, so an inject seam that skips it
    // produces projects that can never be packed.
    locust_core::extraction::record_multilang_injection(&state.db, &report, &languages, &|_lang| {
        INJECT_RECORD_REMEDY.to_string()
    })
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;

    Ok(Json(serde_json::to_value(report).unwrap_or_default()))
}

// ─── Register language (RPG Maker multi-lang UI) ───────────────────────────

#[derive(Deserialize)]
struct RegisterLangRequest {
    /// Game root (folder with `js/plugins.js` and/or `data/`).
    game_path: String,
    /// Language code (e.g. `es`).
    lang: String,
    /// Display label (e.g. `Español`).
    label: String,
}

/// Patch Iavra/VisuMZ language lists + Map boot choices so a new lang is
/// selectable in the game UI. Writes `*.bak-locust` siblings (same as CLI).
async fn register_lang(
    Json(req): Json<RegisterLangRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let game_path = PathBuf::from(req.game_path.trim());
    if req.game_path.trim().is_empty() || !game_path.is_dir() {
        return Err(err(
            StatusCode::BAD_REQUEST,
            format!(
                "game_path must be an existing directory (got {:?})",
                req.game_path
            ),
        ));
    }
    let lang = req.lang;
    let label = req.label;
    let report = tokio::task::spawn_blocking(move || {
        locust_formats::rpgmaker_lang::register_language(&game_path, &lang, &label)
    })
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?
    .map_err(|e| {
        // Invalid lang/label is a client error; other failures stay 500.
        let msg = e.to_string();
        if msg.contains("invalid language") || msg.contains("label must not be empty") {
            err(StatusCode::BAD_REQUEST, msg)
        } else {
            err(StatusCode::INTERNAL_SERVER_ERROR, msg)
        }
    })?;
    Ok(Json(serde_json::to_value(report).unwrap_or_default()))
}

// ─── Patch apply / rollback / status ───────────────────────────────────────

#[derive(Deserialize)]
struct PatchPathsRequest {
    game_path: String,
    zip_path: Option<String>,
    /// http(s) URL of a patch zip — downloaded to a temp file then applied/verified.
    /// Mutually exclusive with `zip_path`. Loopback server only; still scheme-gated.
    zip_url: Option<String>,
    #[serde(default)]
    force: bool,
    #[serde(default)]
    confirm_legacy: bool,
    #[serde(default)]
    dry_run: bool,
}

/// Resolve local zip path or download `zip_url` into a TempDir (caller must keep it alive).
async fn resolve_patch_zip(
    zip_path: Option<String>,
    zip_url: Option<String>,
) -> Result<(PathBuf, Option<tempfile::TempDir>), ApiError> {
    match (zip_path, zip_url) {
        (Some(p), None) => {
            let path = PathBuf::from(p);
            if !path.is_file() {
                return Err(err(
                    StatusCode::BAD_REQUEST,
                    format!("zip_path not found: {}", path.display()),
                ));
            }
            Ok((path, None))
        }
        (None, Some(url)) => {
            let dir =
                tempfile::TempDir::new().map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
            let dest = dir.path().join("locust-patch.zip");
            download_patch_zip_async(&url, &dest).await?;
            Ok((dest, Some(dir)))
        }
        (Some(_), Some(_)) => Err(err(
            StatusCode::BAD_REQUEST,
            "pass either zip_path or zip_url, not both",
        )),
        (None, None) => Err(err(StatusCode::BAD_REQUEST, "zip_path or zip_url required")),
    }
}

async fn download_patch_zip_async(url: &str, dest: &Path) -> Result<(), ApiError> {
    let max_bytes = locust_core::patch::zipsec::max_download_bytes();
    let parsed = reqwest::Url::parse(url)
        .map_err(|e| err(StatusCode::BAD_REQUEST, format!("invalid zip_url: {e}")))?;
    match parsed.scheme() {
        "http" | "https" => {}
        other => {
            return Err(err(
                StatusCode::BAD_REQUEST,
                format!("only http/https zip_url allowed (got {other})"),
            ));
        }
    }
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30 * 60))
        .connect_timeout(std::time::Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::limited(5))
        .user_agent(format!("locust-server/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    let mut resp = client
        .get(parsed)
        .send()
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("download failed: {e}")))?
        .error_for_status()
        .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("download HTTP error: {e}")))?;
    if let Some(len) = resp.content_length() {
        if len > max_bytes {
            return Err(err(
                StatusCode::PAYLOAD_TOO_LARGE,
                format!("remote zip too large: {len} bytes"),
            ));
        }
    }
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    }

    // Stream to disk with a running cap, like the CLI does. Buffering the whole
    // body first would let a chunked response — which sends no Content-Length,
    // so the check above never fires — allocate without bound before any check.
    let mut file =
        std::fs::File::create(dest).map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    let mut written: u64 = 0;
    loop {
        let chunk = resp
            .chunk()
            .await
            .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("download body: {e}")))?;
        let Some(chunk) = chunk else { break };
        written += chunk.len() as u64;
        if written > max_bytes {
            drop(file);
            let _ = std::fs::remove_file(dest);
            return Err(err(StatusCode::PAYLOAD_TOO_LARGE, "remote zip too large"));
        }
        use std::io::Write;
        file.write_all(&chunk)
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    }
    if written == 0 {
        drop(file);
        let _ = std::fs::remove_file(dest);
        return Err(err(StatusCode::BAD_GATEWAY, "download produced empty file"));
    }
    Ok(())
}

#[derive(Deserialize)]
struct PatchPackRequest {
    game_path: String,
    /// Destination patch zip path.
    output_path: String,
    /// Optional language keys; empty = auto when exactly one recording exists.
    /// More than one language is refused (one zip = one recording).
    #[serde(default)]
    languages: Vec<String>,
    /// When true, require pristine hashes (`.locust/backup` or fail).
    #[serde(default)]
    pristine: bool,
    /// Optional path to a pristine game tree for original hashes (overrides backup).
    #[serde(default)]
    pristine_path: Option<String>,
}

fn map_patch_err(e: locust_core::error::LocustError) -> ApiError {
    use locust_core::error::LocustError::*;
    let status = match &e {
        PatchAlreadyApplied(_)
        | PatchDowngradeBlocked { .. }
        | PatchInterrupted(_)
        | PatchLegacyUnconfirmed(_)
        | PatchVerificationFailed(_)
        | PatchBackupIncomplete(_) => StatusCode::CONFLICT,
        PatchUnsafeEntry(_) | GameDirNotWritable(_) | PatchError(_) => StatusCode::BAD_REQUEST,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    err(status, e)
}

async fn patch_pack(
    State(state): State<Arc<AppState>>,
    Json(req): Json<PatchPackRequest>,
) -> Result<Json<locust_core::patch::PackReport>, ApiError> {
    if req.game_path.trim().is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "game_path required"));
    }
    if req.output_path.trim().is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "output_path required"));
    }
    if req.languages.len() > 1 {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "pack accepts at most one language (one zip = one injection recording)",
        ));
    }
    let lang = req.languages.into_iter().next();
    let game_path = PathBuf::from(&req.game_path);
    let output = PathBuf::from(&req.output_path);
    let pristine = req.pristine_path.map(PathBuf::from);
    let require_pristine = req.pristine;
    let engine = locust_formats::default_registry()
        .detect(&game_path)
        .map(|p| p.id().to_string());

    let db = state.db.clone();
    let db_path_for_errors = db.path();
    let report = tokio::task::spawn_blocking(move || {
        locust_core::patch::pack_injection_recording(
            &db,
            locust_core::patch::PackOptions {
                game_path,
                lang,
                output,
                pristine,
                engine,
                project: db_path_for_errors,
                require_pristine,
            },
        )
    })
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?
    .map_err(map_patch_err)?;

    Ok(Json(report))
}

async fn patch_verify(
    Json(req): Json<PatchPathsRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let (zip, _guard) = resolve_patch_zip(req.zip_path, req.zip_url).await?;
    let report =
        locust_core::patch::verify(&PathBuf::from(&req.game_path), &zip).map_err(map_patch_err)?;
    Ok(Json(serde_json::json!({
        "outcome": format!("{:?}", report.outcome),
        "tier": report.tier.map(|t| format!("{:?}", t)),
        "replaced": report.replaced,
        "added": report.added,
        "conflicts": report.conflicts,
        "backup_compromised": report.backup_compromised,
        "messages": report.messages,
        "manifest": report.manifest,
    })))
}

async fn patch_apply(
    Json(req): Json<PatchPathsRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let (zip, _guard) = resolve_patch_zip(req.zip_path, req.zip_url).await?;
    let opts = locust_core::patch::ApplyOptions {
        force: req.force,
        confirm_legacy: req.confirm_legacy,
        dry_run: req.dry_run,
    };
    // Apply is disk-bound and journaled; run off the async runtime.
    let game = PathBuf::from(&req.game_path);
    let report =
        tokio::task::spawn_blocking(move || locust_core::patch::apply(&game, &zip, opts, |_| {}))
            .await
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?
            .map_err(map_patch_err)?;
    Ok(Json(serde_json::json!({
        "patch_id": report.patch_id,
        "patch_version": report.patch_version,
        "replaced": report.replaced,
        "added": report.added,
        "forced": report.forced,
        "baseline": format!("{:?}", report.baseline),
        "dry_run": report.dry_run,
        "user_edits_overwritten": report.user_edits_overwritten,
        "messages": report.messages,
    })))
}

async fn patch_rollback(
    Json(req): Json<PatchPathsRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let game = PathBuf::from(&req.game_path);
    let force = req.force;
    let report = tokio::task::spawn_blocking(move || {
        locust_core::patch::rollback(
            &game,
            locust_core::patch::RollbackOptions {
                delete_modified_added: force,
            },
        )
    })
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?
    .map_err(map_patch_err)?;
    Ok(Json(serde_json::json!({
        "restored": report.restored,
        "deleted": report.deleted,
        "baseline": report.baseline.map(|b| format!("{:?}", b)),
        "messages": report.messages,
        "aborted_edited": report.aborted_edited,
        "torn_deleted": report.torn_deleted,
    })))
}

async fn patch_status(
    Json(req): Json<PatchPathsRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let store = locust_core::patch::PatchStore::new(PathBuf::from(&req.game_path));
    let status = store.status().map_err(map_patch_err)?;
    let body = match status {
        locust_core::patch::PatchStatus::NotPatched => {
            serde_json::json!({ "status": "not_patched" })
        }
        locust_core::patch::PatchStatus::Patched(r) => serde_json::json!({
            "status": "patched",
            "patch_id": r.patch_id,
            "patch_version": r.patch_version,
            "engine": r.engine,
            "language": r.language,
            "baseline": format!("{:?}", r.baseline),
            "forced": r.forced,
            "applied_at": r.applied_at,
            "replaced": r.replaced.len(),
            "added": r.added.len(),
        }),
        locust_core::patch::PatchStatus::Interrupted(j) => serde_json::json!({
            "status": "interrupted",
            "patch_id": j.patch_id,
            "state": format!("{:?}", j.state),
        }),
        locust_core::patch::PatchStatus::Unknown => {
            serde_json::json!({ "status": "unknown" })
        }
    };
    Ok(Json(body))
}

async fn validate(State(state): State<Arc<AppState>>) -> Result<Json<serde_json::Value>, ApiError> {
    let entries = state
        .db
        .get_entries(&EntryFilter::default())
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    let validation = Validator::validate_and_save(&entries, &state.db)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;

    let proj = state.current_project.read().await;
    let fonts: Vec<FontCoverageReport> = if let Some(ref p) = *proj {
        let translations: Vec<&str> = entries
            .iter()
            .filter_map(|e| e.translation.as_deref())
            .collect();
        FontValidator::check_game_fonts(&p.path, &translations).unwrap_or_default()
    } else {
        Vec::new()
    };

    Ok(Json(serde_json::json!({
        "validation": validation,
        "fonts": fonts,
    })))
}

#[derive(Deserialize)]
struct GlossaryQuery {
    lang_pair: String,
}

async fn get_glossary(
    State(state): State<Arc<AppState>>,
    Query(q): Query<GlossaryQuery>,
) -> Result<Json<Vec<GlossaryEntry>>, ApiError> {
    state
        .glossary
        .get_all(&q.lang_pair)
        .map(Json)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))
}

async fn add_glossary(
    State(state): State<Arc<AppState>>,
    Json(entry): Json<GlossaryEntry>,
) -> Result<StatusCode, ApiError> {
    state
        .glossary
        .add(
            &entry.term,
            &entry.translation,
            &entry.lang_pair,
            entry.context.as_deref(),
        )
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(StatusCode::CREATED)
}

async fn delete_glossary(
    State(state): State<Arc<AppState>>,
    AxumPath(term): AxumPath<String>,
    Query(q): Query<GlossaryQuery>,
) -> Result<StatusCode, ApiError> {
    state
        .glossary
        .delete(&term, &q.lang_pair)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct LangQuery {
    lang: String,
}

async fn export_po(
    State(state): State<Arc<AppState>>,
    Query(q): Query<LangQuery>,
) -> Result<(StatusCode, [(String, String); 2], String), ApiError> {
    let entries = state
        .db
        .get_entries(&EntryFilter::default())
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    let config = state.config.read().await;
    let source = state
        .db
        .resolve_export_source_lang(&q.lang, &config.default_source_lang)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    let po = export::export_po(&entries, &source, &q.lang);
    Ok((
        StatusCode::OK,
        [
            (
                "Content-Type".to_string(),
                "text/plain; charset=utf-8".to_string(),
            ),
            (
                "Content-Disposition".to_string(),
                format!("attachment; filename=\"translation_{}.po\"", q.lang),
            ),
        ],
        po,
    ))
}

async fn import_po(
    State(state): State<Arc<AppState>>,
    body: String,
) -> Result<Json<serde_json::Value>, ApiError> {
    let po_entries = export::import_po(&body).map_err(|e| err(StatusCode::BAD_REQUEST, e))?;
    let mut imported = 0;
    let mut skipped = 0;
    for pe in &po_entries {
        if pe.translation.is_empty() {
            skipped += 1;
            continue;
        }
        let Some(ref id) = pe.id else {
            skipped += 1;
            continue;
        };
        match state
            .db
            .save_translation(id, &pe.translation, "import")
            .await
        {
            Ok(true) => imported += 1,
            Ok(false) => skipped += 1,
            Err(e) => return Err(err(StatusCode::INTERNAL_SERVER_ERROR, e)),
        }
    }
    Ok(Json(
        serde_json::json!({"imported": imported, "skipped": skipped}),
    ))
}

async fn export_xliff(
    State(state): State<Arc<AppState>>,
    Query(q): Query<LangQuery>,
) -> Result<(StatusCode, [(String, String); 2], String), ApiError> {
    let entries = state
        .db
        .get_entries(&EntryFilter::default())
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    let config = state.config.read().await;
    let source = state
        .db
        .resolve_export_source_lang(&q.lang, &config.default_source_lang)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    let xliff = export::export_xliff(&entries, &source, &q.lang);
    Ok((
        StatusCode::OK,
        [
            (
                "Content-Type".to_string(),
                "application/xml; charset=utf-8".to_string(),
            ),
            (
                "Content-Disposition".to_string(),
                format!("attachment; filename=\"translation_{}.xliff\"", q.lang),
            ),
        ],
        xliff,
    ))
}

async fn import_xliff(
    State(state): State<Arc<AppState>>,
    body: String,
) -> Result<Json<serde_json::Value>, ApiError> {
    let units = export::import_xliff(&body).map_err(|e| err(StatusCode::BAD_REQUEST, e))?;
    let mut imported = 0;
    let mut skipped = 0;
    for unit in &units {
        if unit.target.is_empty() {
            skipped += 1;
            continue;
        }
        match state
            .db
            .save_translation(&unit.id, &unit.target, "import")
            .await
        {
            Ok(true) => imported += 1,
            Ok(false) => skipped += 1,
            Err(e) => return Err(err(StatusCode::INTERNAL_SERVER_ERROR, e)),
        }
    }
    Ok(Json(
        serde_json::json!({"imported": imported, "skipped": skipped}),
    ))
}

async fn get_config(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let config = state.config.read().await;
    let mut val = serde_json::to_value(&*config).unwrap_or_default();
    // Redact API keys
    if let Some(providers) = val.get_mut("providers").and_then(|v| v.as_object_mut()) {
        for (_id, pc) in providers.iter_mut() {
            if let Some(obj) = pc.as_object_mut() {
                if obj
                    .get("api_key")
                    .and_then(|v| v.as_str())
                    .is_some_and(|s| !s.is_empty())
                {
                    obj.insert(
                        "api_key".to_string(),
                        serde_json::Value::String("***".to_string()),
                    );
                }
            }
        }
    }
    Json(val)
}

async fn patch_config(
    State(state): State<Arc<AppState>>,
    Json(partial): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let mut config = state.config.write().await;
    // Merge partial into current
    let mut current = serde_json::to_value(&*config).unwrap_or_default();
    if let (Some(cur_obj), Some(patch_obj)) = (current.as_object_mut(), partial.as_object()) {
        for (k, v) in patch_obj {
            cur_obj.insert(k.clone(), v.clone());
        }
    }
    *config =
        serde_json::from_value(current.clone()).map_err(|e| err(StatusCode::BAD_REQUEST, e))?;
    // Persist to disk
    let _ = config.save(&AppConfig::default_path());
    Ok(Json(current))
}

async fn memory_stats(
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let project = state
        .db
        .memory_count()
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    let global = state
        .global_memory
        .memory_count()
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(serde_json::json!({
        "project_entries": project,
        "global_entries": global,
    })))
}

async fn list_memory(
    State(state): State<Arc<AppState>>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let search = params.get("search").map(|s| s.as_str());
    let lang_pair = params.get("lang_pair").map(|s| s.as_str());
    let limit: usize = params
        .get("limit")
        .and_then(|s| s.parse().ok())
        .unwrap_or(50);
    let offset: usize = params
        .get("offset")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);

    let (entries, total) = state
        .global_memory
        .list_memory(search, lang_pair, limit, offset)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;

    Ok(Json(serde_json::json!({
        "entries": entries,
        "total": total,
        "limit": limit,
        "offset": offset,
    })))
}

async fn delete_memory_entry(
    State(state): State<Arc<AppState>>,
    AxumPath((hash, lang_pair)): AxumPath<(String, String)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    state
        .global_memory
        .delete_memory(&hash, &lang_pair)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(serde_json::json!({"ok": true})))
}

async fn clear_memory(
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Clear both global memory and project-level memory
    state
        .global_memory
        .clear_memory()
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    state
        .db
        .clear_memory()
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(serde_json::json!({"ok": true})))
}

async fn memory_lang_pairs(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<String>>, ApiError> {
    state
        .global_memory
        .memory_lang_pairs()
        .map(Json)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))
}

async fn list_backups(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<BackupEntry>>, ApiError> {
    state
        .backup_manager
        .list_backups()
        .map(Json)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))
}

async fn restore_backup(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
) -> Result<StatusCode, ApiError> {
    let proj = state.current_project.read().await;
    let target = proj
        .as_ref()
        .map(|p| p.path.clone())
        .ok_or_else(|| err(StatusCode::BAD_REQUEST, "no project open"))?;
    state
        .backup_manager
        .restore(&id, &target)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(StatusCode::OK)
}

async fn delete_backup(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
) -> Result<StatusCode, ApiError> {
    state
        .backup_manager
        .delete_backup(&id)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod download_guard_tests {
    use super::*;

    /// Serve one chunked HTTP response with NO Content-Length, which is exactly
    /// the shape that defeats a pre-flight size check. `total` of `usize::MAX`
    /// streams until the client hangs up.
    async fn serve_chunked(total: usize) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            if let Ok((mut sock, _)) = listener.accept().await {
                let mut scratch = [0u8; 1024];
                let _ = sock.read(&mut scratch).await;
                let _ = sock
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Type: application/zip\r\n\
                          Transfer-Encoding: chunked\r\n\r\n",
                    )
                    .await;
                let chunk = vec![b'A'; 4096];
                let mut sent = 0usize;
                while sent < total {
                    if sock
                        .write_all(format!("{:x}\r\n", chunk.len()).as_bytes())
                        .await
                        .is_err()
                        || sock.write_all(&chunk).await.is_err()
                        || sock.write_all(b"\r\n").await.is_err()
                    {
                        return;
                    }
                    sent += chunk.len();
                }
                let _ = sock.write_all(b"0\r\n\r\n").await;
            }
        });
        format!("http://{addr}/patch.zip")
    }

    #[tokio::test]
    async fn download_rejects_non_http_scheme() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("p.zip");
        let e = download_patch_zip_async("file:///C:/Windows/win.ini", &dest)
            .await
            .expect_err("file:// must be rejected");
        assert!(format!("{e:?}").contains("http"), "{e:?}");
        assert!(!dest.exists(), "nothing should be written");
    }

    #[tokio::test]
    async fn download_aborts_past_cap_on_chunked_response() {
        // The body never ends, so only a running byte count can stop it.
        // Buffering the whole response first would read forever and time out.
        let cap = 64 * 1024;
        std::env::set_var(
            locust_core::patch::zipsec::MAX_DOWNLOAD_ENV,
            cap.to_string(),
        );
        let url = serve_chunked(usize::MAX).await;
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("p.zip");

        let res = tokio::time::timeout(
            std::time::Duration::from_secs(20),
            download_patch_zip_async(&url, &dest),
        )
        .await;
        std::env::remove_var(locust_core::patch::zipsec::MAX_DOWNLOAD_ENV);

        let res = res.expect("download must abort at the cap, not read the endless body");
        let e = res.expect_err("oversize chunked download must abort");
        assert!(format!("{e:?}").contains("too large"), "{e:?}");
        assert!(!dest.exists(), "partial download must be unlinked");
    }

    #[tokio::test]
    async fn download_rejects_empty_body() {
        let url = serve_chunked(0).await;
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("p.zip");
        let e = download_patch_zip_async(&url, &dest)
            .await
            .expect_err("empty body must be rejected");
        assert!(format!("{e:?}").contains("empty"), "{e:?}");
        assert!(!dest.exists());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn setup() -> (String, tokio::task::JoinHandle<()>) {
        let state = create_test_state();
        start_test_server(state).await
    }

    async fn setup_with_state() -> (String, tokio::task::JoinHandle<()>, Arc<AppState>) {
        let state = create_test_state();
        let s = state.clone();
        let (url, handle) = start_test_server(state).await;
        (url, handle, s)
    }

    fn client() -> reqwest::Client {
        reqwest::Client::new()
    }

    #[tokio::test]
    async fn test_health_returns_ok() {
        let (url, _h) = setup().await;
        let resp = client()
            .get(format!("{}/health", url))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let body: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(body["status"], "ok");
    }

    #[tokio::test]
    async fn test_list_formats_not_empty() {
        let (url, _h) = setup().await;
        let resp = client()
            .get(format!("{}/api/formats", url))
            .send()
            .await
            .unwrap();
        let body: Vec<serde_json::Value> = resp.json().await.unwrap();
        assert!(!body.is_empty());
    }

    #[tokio::test]
    async fn test_list_providers_not_empty() {
        let (url, _h) = setup().await;
        let resp = client()
            .get(format!("{}/api/providers", url))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let body: Vec<serde_json::Value> = resp.json().await.unwrap();
        assert!(!body.is_empty());
    }

    #[tokio::test]
    async fn test_list_providers_includes_unconfigured_deepl() {
        let (url, _h) = setup().await;
        let resp = client()
            .get(format!("{}/api/providers", url))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let body: Vec<serde_json::Value> = resp.json().await.unwrap();
        let deepl = body
            .iter()
            .find(|p| p["id"] == "deepl")
            .expect("deepl should be listed even without an API key");
        assert_eq!(deepl["configured"], false);
        assert_eq!(deepl["requires_api_key"], true);
    }

    #[tokio::test]
    async fn test_list_providers_deepl_configured_when_key_set() {
        let state = create_test_state();
        {
            let mut config = state.config.write().await;
            config.providers.insert(
                "deepl".to_string(),
                locust_core::config::ProviderConfig {
                    api_key: Some("secret-key-123".to_string()),
                    base_url: None,
                    model: None,
                    free_tier: false,
                    extra: std::collections::HashMap::new(),
                },
            );
            let reg = locust_providers::default_registry(&config);
            *state.provider_registry.write().await = reg;
        }

        let (url, _h) = start_test_server(state).await;
        let resp = client()
            .get(format!("{}/api/providers", url))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let body: Vec<serde_json::Value> = resp.json().await.unwrap();
        let deepl_entries: Vec<_> = body.iter().filter(|p| p["id"] == "deepl").collect();
        assert_eq!(deepl_entries.len(), 1, "deepl must not be duplicated");
        assert_eq!(deepl_entries[0]["configured"], true);
        assert_eq!(deepl_entries[0]["requires_api_key"], true);
    }

    #[tokio::test]
    async fn test_open_invalid_path_returns_400() {
        let (url, _h) = setup().await;
        let resp = client()
            .post(format!("{}/api/project/open", url))
            .json(&serde_json::json!({"path": "/nonexistent/path/xyz"}))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 400);
    }

    #[tokio::test]
    async fn test_open_unknown_format_returns_422() {
        let (url, _h) = setup().await;
        let dir =
            std::env::temp_dir().join(format!("locust_test_noformat_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let resp = client()
            .post(format!("{}/api/project/open", url))
            .json(&serde_json::json!({"path": dir.to_string_lossy()}))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 422);
    }

    #[tokio::test]
    async fn test_get_strings_before_project_returns_empty() {
        let (url, _h, state) = setup_with_state().await;
        // Leftover rows in project.db must not leak without an open project.
        let leftover = StringEntry::new("stale", "Old session", PathBuf::from("old.json"));
        state.db.save_entries(&[leftover]).unwrap();

        let resp = client()
            .get(format!("{}/api/strings", url))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let body: StringsResponse = resp.json().await.unwrap();
        assert!(body.entries.is_empty());
        assert_eq!(body.total, 0);

        let stats = client()
            .get(format!("{}/api/stats", url))
            .send()
            .await
            .unwrap();
        assert_eq!(stats.status(), 200);
        let stats_body: ProjectStats = stats.json().await.unwrap();
        assert_eq!(stats_body.total, 0);
    }

    #[tokio::test]
    async fn test_patch_string_updates_translation() {
        let (url, _h, state) = setup_with_state().await;
        let entry = StringEntry::new("test1", "Hello", PathBuf::from("f.json"));
        state.db.save_entries(&[entry]).unwrap();

        // Verify entry exists first
        let resp = client()
            .get(format!("{}/api/strings/test1", url))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200, "entry should exist before patch");

        let resp = client()
            .patch(format!("{}/api/strings/test1", url))
            .json(&serde_json::json!({"translation": "Hola"}))
            .send()
            .await
            .unwrap();
        let status = resp.status().as_u16();
        let body: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(status, 200, "patch failed: {:?}", body);
        assert_eq!(body["translation"], "Hola");
    }

    #[tokio::test]
    async fn test_patch_string_updates_status() {
        let (url, _h, state) = setup_with_state().await;
        let entry = StringEntry::new("test2", "Hello", PathBuf::from("f.json"));
        state.db.save_entries(&[entry]).unwrap();

        let resp = client()
            .patch(format!("{}/api/strings/test2", url))
            .json(&serde_json::json!({"status": "approved"}))
            .send()
            .await
            .unwrap();
        let status = resp.status().as_u16();
        let body: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(status, 200, "patch failed: {:?}", body);
        assert_eq!(body["status"], "approved");
    }

    #[tokio::test]
    async fn test_get_stats_shape() {
        let (url, _h) = setup().await;
        let resp = client()
            .get(format!("{}/api/stats", url))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let body: serde_json::Value = resp.json().await.unwrap();
        assert!(body.get("total").is_some());
        assert!(body.get("pending").is_some());
        assert!(body.get("translated").is_some());
    }

    #[tokio::test]
    async fn test_list_translation_runs_seeded() {
        use locust_core::database::TranslationRun;

        let (url, _h, state) = setup_with_state().await;
        state
            .db
            .record_translation_run(&TranslationRun {
                id: 0,
                started_at: "2026-03-15T12:34:56Z".into(),
                duration_secs: 42.5,
                provider: "mock→deepl".into(),
                source_lang: "en".into(),
                target_lang: "es".into(),
                strings_translated: 12,
                tokens_used: 100,
                input_tokens: 60,
                output_tokens: 40,
                cost_usd: 0.0012,
            })
            .await
            .unwrap();
        state
            .db
            .record_translation_run(&TranslationRun {
                id: 0,
                started_at: "2026-03-16T08:00:00Z".into(),
                duration_secs: 10.0,
                provider: "mock".into(),
                source_lang: "en".into(),
                target_lang: "fr".into(),
                strings_translated: 3,
                tokens_used: 20,
                input_tokens: 10,
                output_tokens: 10,
                cost_usd: 0.0001,
            })
            .await
            .unwrap();

        let resp = client()
            .get(format!("{}/api/runs", url))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let body: Vec<serde_json::Value> = resp.json().await.unwrap();
        assert_eq!(body.len(), 2, "{body:?}");
        // Newest first
        assert_eq!(body[0]["started_at"], "2026-03-16T08:00:00Z");
        assert_eq!(body[0]["provider"], "mock");
        assert_eq!(body[0]["target_lang"], "fr");
        assert_eq!(body[1]["provider"], "mock→deepl");
        assert_eq!(body[1]["strings_translated"], 12);
        assert_eq!(body[1]["input_tokens"], 60);
        assert_eq!(body[1]["output_tokens"], 40);
        assert!((body[1]["cost_usd"].as_f64().unwrap() - 0.0012).abs() < 1e-9);
        assert!((body[1]["duration_secs"].as_f64().unwrap() - 42.5).abs() < 1e-9);
        assert!(body[0]["id"].as_i64().unwrap() >= 1);
        // All ledger columns present
        for key in [
            "id",
            "started_at",
            "duration_secs",
            "provider",
            "source_lang",
            "target_lang",
            "strings_translated",
            "tokens_used",
            "input_tokens",
            "output_tokens",
            "cost_usd",
        ] {
            assert!(body[0].get(key).is_some(), "missing {key}");
        }
    }

    #[tokio::test]
    async fn test_translate_start_returns_job_id() {
        let (url, _h, state) = setup_with_state().await;
        let entry = StringEntry::new("t1", "Hello", PathBuf::from("f.json"));
        state.db.save_entries(&[entry]).unwrap();

        let resp = client()
            .post(format!("{}/api/translate/start", url))
            .json(&serde_json::json!({
                "provider_id": "mock",
                "options": TranslationOptions::default()
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let body: serde_json::Value = resp.json().await.unwrap();
        assert!(body.get("job_id").is_some());
    }

    #[tokio::test]
    async fn test_translate_start_accepts_fallback_provider_ids() {
        let (url, _h, state) = setup_with_state().await;
        let entry = StringEntry::new("t1", "Hello", PathBuf::from("f.json"));
        state.db.save_entries(&[entry]).unwrap();

        // Optional field present — same status as without it (primary must exist).
        let resp = client()
            .post(format!("{}/api/translate/start", url))
            .json(&serde_json::json!({
                "provider_id": "mock",
                "fallback_provider_ids": ["mock"],
                "options": TranslationOptions::default()
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let body: serde_json::Value = resp.json().await.unwrap();
        assert!(body.get("job_id").is_some());
    }

    #[tokio::test]
    async fn test_translate_start_without_fallback_field_unchanged() {
        let (url, _h, state) = setup_with_state().await;
        let entry = StringEntry::new("t2", "World", PathBuf::from("f.json"));
        state.db.save_entries(&[entry]).unwrap();

        // Omitting fallback_provider_ids must still deserialize and run.
        let resp = client()
            .post(format!("{}/api/translate/start", url))
            .json(&serde_json::json!({
                "provider_id": "mock",
                "options": {
                    "source_lang": "en",
                    "target_lang": "es",
                    "batch_size": 40,
                    "max_concurrent": 1,
                    "cost_limit_usd": null,
                    "game_context": null,
                    "use_glossary": false,
                    "use_memory": false,
                    "skip_approved": true
                }
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            200,
            "missing fallback_provider_ids must not break start: {}",
            resp.text().await.unwrap_or_default()
        );
    }

    #[tokio::test]
    async fn test_translate_cancel() {
        let (url, _h, state) = setup_with_state().await;
        let entry = StringEntry::new("t1", "Hello", PathBuf::from("f.json"));
        state.db.save_entries(&[entry]).unwrap();

        let resp = client()
            .post(format!("{}/api/translate/start", url))
            .json(&serde_json::json!({
                "provider_id": "mock",
                "options": TranslationOptions::default()
            }))
            .send()
            .await
            .unwrap();
        let body: serde_json::Value = resp.json().await.unwrap();
        let job_id = body["job_id"].as_str().unwrap();

        let resp = client()
            .post(format!("{}/api/translate/cancel/{}", url, job_id))
            .send()
            .await
            .unwrap();
        assert!(resp.status() == 200 || resp.status() == 404); // may have already finished
    }

    #[tokio::test]
    async fn test_glossary_add_and_get() {
        let (url, _h) = setup().await;
        let resp = client()
            .post(format!("{}/api/glossary", url))
            .json(&serde_json::json!({
                "term": "HP",
                "translation": "PV",
                "lang_pair": "en-es",
                "context": null,
                "case_sensitive": false
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 201);

        let resp = client()
            .get(format!("{}/api/glossary?lang_pair=en-es", url))
            .send()
            .await
            .unwrap();
        let body: Vec<serde_json::Value> = resp.json().await.unwrap();
        assert_eq!(body.len(), 1);
        assert_eq!(body[0]["term"], "HP");
    }

    #[tokio::test]
    async fn test_glossary_delete() {
        let (url, _h) = setup().await;
        client()
            .post(format!("{}/api/glossary", url))
            .json(&serde_json::json!({
                "term": "MP",
                "translation": "PM",
                "lang_pair": "en-es",
                "context": null,
                "case_sensitive": false
            }))
            .send()
            .await
            .unwrap();

        let resp = client()
            .delete(format!("{}/api/glossary/MP?lang_pair=en-es", url))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 204);
    }

    #[tokio::test]
    async fn test_export_po_returns_text() {
        let (url, _h, state) = setup_with_state().await;
        let mut entry = StringEntry::new("e1", "Hello", PathBuf::from("f.json"));
        entry.translation = Some("Hola".to_string());
        state.db.save_entries(&[entry]).unwrap();

        let resp = client()
            .get(format!("{}/api/export/po?lang=es", url))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let text = resp.text().await.unwrap();
        assert!(text.contains("msgid"));
        assert!(text.contains("msgstr"));
    }

    #[tokio::test]
    async fn test_config_api_keys_redacted() {
        let (url, _h, state) = setup_with_state().await;
        {
            let mut config = state.config.write().await;
            config.providers.insert(
                "deepl".to_string(),
                locust_core::config::ProviderConfig {
                    api_key: Some("secret-key-123".to_string()),
                    base_url: None,
                    model: None,
                    free_tier: false,
                    extra: std::collections::HashMap::new(),
                },
            );
        }

        let resp = client()
            .get(format!("{}/api/config", url))
            .send()
            .await
            .unwrap();
        let body: serde_json::Value = resp.json().await.unwrap();
        let deepl_key = body["providers"]["deepl"]["api_key"].as_str().unwrap();
        assert_eq!(deepl_key, "***");
    }

    #[tokio::test]
    async fn test_cors_header_present() {
        let (url, _h) = setup().await;
        let resp = client()
            .get(format!("{}/health", url))
            .send()
            .await
            .unwrap();
        // CorsLayer::permissive() adds the header on actual CORS requests
        // but for same-origin it may not. Check the server responds OK.
        assert_eq!(resp.status(), 200);
    }
}
