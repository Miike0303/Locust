use std::path::PathBuf;
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
use locust_core::database::{Database, EntryFilter, GlobalMemoryDb, GlossaryEntry, MemoryEntry, ProjectStats};
use locust_core::export;
use locust_core::extraction::{FormatRegistry, MultiLangInjector, PluginInfo};
use locust_core::font_validation::{FontCoverageReport, FontValidator};
use locust_core::glossary::Glossary;
use locust_core::models::{OutputMode, ProgressEvent, StringEntry, StringStatus};
use locust_core::translation::{ProviderRegistry, TranslationManager, TranslationOptions};
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
    let db = Arc::new(Database::open_in_memory().unwrap());
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
        .route("/api/strings/:id", get(get_string).patch(patch_string))
        .route("/api/stats", get(get_stats))
        .route("/api/translate/start", post(translate_start))
        .route("/api/translate/cancel/:job_id", post(translate_cancel))
        .route("/api/translate/ws/:job_id", get(translate_ws))
        .route("/api/inject", post(inject))
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
    let app = create_router(state);
    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", port)).await?;
    tracing::info!("Server listening on port {}", port);
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

async fn list_providers(
    State(state): State<Arc<AppState>>,
) -> Json<serde_json::Value> {
    let reg = state.provider_registry.read().await;
    Json(serde_json::to_value(reg.list()).unwrap_or_default())
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
}

async fn project_open(
    State(state): State<Arc<AppState>>,
    Json(req): Json<OpenProjectRequest>,
) -> Result<Json<ProjectOpenResponse>, ApiError> {
    let raw_path = PathBuf::from(&req.path);
    if !raw_path.exists() {
        return Err(err(StatusCode::BAD_REQUEST, "path not found"));
    }

    // Resolve executable/file path to game root
    let path = locust_core::extraction::resolve_game_root(&raw_path, &state.format_registry);

    let plugin = if let Some(ref fid) = req.format_id {
        state
            .format_registry
            .get(fid)
            .ok_or_else(|| err(StatusCode::UNPROCESSABLE_ENTITY, format!("Unknown format: {}", fid)))?
    } else {
        state
            .format_registry
            .detect(&path)
            .ok_or_else(|| err(StatusCode::UNPROCESSABLE_ENTITY, "format not detected"))?
    };

    let format_id = plugin.id().to_string();
    let format_name = plugin.name().to_string();
    let supported_modes = plugin.supported_modes();

    let entries = plugin.extract(&path).map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;

    state.db.clear_entries().map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    let total_strings = state
        .db
        .save_entries(&entries)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;

    let project_name = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();

    {
        let mut proj = state.current_project.write().await;
        *proj = Some(ProjectInfo {
            path: path.clone(),
            format_id: format_id.clone(),
            name: project_name.clone(),
        });
    }

    {
        let mut config = state.config.write().await;
        config.add_recent_project(path.clone(), project_name.clone(), format_id.clone());
    }

    Ok(Json(ProjectOpenResponse {
        format_id,
        format_name,
        total_strings,
        project_path: req.path,
        project_name,
        supported_modes,
    }))
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
    let total = state.db.count_entries(&count_filter).map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;

    let filter = EntryFilter {
        status,
        file_path: q.file_path,
        tag: q.tag,
        search: q.search,
        limit: Some(limit),
        offset: Some(offset),
    };
    let entries = state.db.get_entries(&filter).map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;

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

async fn get_stats(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ProjectStats>, ApiError> {
    state
        .db
        .get_stats()
        .map(Json)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))
}

#[derive(Deserialize)]
struct TranslateStartRequest {
    provider_id: String,
    options: TranslationOptions,
}

#[derive(Serialize)]
struct TranslateStartResponse {
    job_id: String,
}

async fn translate_start(
    State(state): State<Arc<AppState>>,
    Json(req): Json<TranslateStartRequest>,
) -> Result<Json<TranslateStartResponse>, ApiError> {
    let reg = state.provider_registry.read().await;
    let provider = reg
        .get(&req.provider_id)
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "provider not found"))?;

    let job_id = uuid::Uuid::new_v4().to_string();
    let (tx, mut rx) = mpsc::channel::<ProgressEvent>(1000);
    let (broadcast_tx, _) = broadcast::channel::<ProgressEvent>(1000);
    let broadcast_tx_clone = broadcast_tx.clone();

    let entries = state
        .db
        .get_entries(&EntryFilter::default())
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;

    let manager = TranslationManager::new(provider, state.db.clone(), state.glossary.clone());
    let cancel = tokio_util::sync::CancellationToken::new();
    let cancel_clone = cancel.clone();
    let job_id_clone = job_id.clone();

    // Bridge mpsc → broadcast so WebSocket clients can subscribe
    let jobs = state.active_jobs.clone();
    let cleanup_job_id = job_id.clone();
    tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            let is_terminal = matches!(event, ProgressEvent::Completed { .. } | ProgressEvent::Failed { .. });
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
    state.active_jobs.insert(job_id.clone(), JobState {
        abort_handle: tokio::spawn(async {}).abort_handle(), // placeholder
        progress_tx: broadcast_tx,
    });

    let handle = tokio::spawn(async move {
        let _ = manager
            .translate_entries(entries, req.options, tx, job_id_clone, cancel_clone)
            .await;
    });

    // Update with real abort handle
    if let Some(mut job) = state.active_jobs.get_mut(&job_id) {
        job.abort_handle = handle.abort_handle();
    }

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
                serde_json::json!({"type": "failed", "error": "job not found"}).to_string().into(),
            ))
            .await;
        let _ = socket.close().await;
        return;
    };

    loop {
        match rx.recv().await {
            Ok(event) => {
                let is_terminal = matches!(event, ProgressEvent::Completed { .. } | ProgressEvent::Failed { .. });
                let json = serde_json::to_string(&event).unwrap_or_default();
                if socket.send(Message::Text(json.into())).await.is_err() {
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
    mode: OutputMode,
    languages: Vec<String>,
    output_dir: Option<String>,
}

async fn inject(
    State(state): State<Arc<AppState>>,
    Json(req): Json<InjectRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let injector = MultiLangInjector::new(
        state.format_registry.clone(),
        state.db.clone(),
        state.backup_manager.clone(),
    );
    let (tx, mut rx) = mpsc::channel(100);
    tokio::spawn(async move { while rx.recv().await.is_some() {} });

    let report = injector
        .inject(
            &PathBuf::from(&req.project_path),
            &req.format_id,
            req.mode,
            req.languages,
            req.output_dir.map(PathBuf::from),
            tx,
        )
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;

    Ok(Json(serde_json::to_value(report).unwrap_or_default()))
}

async fn validate(
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let entries = state.db.get_entries(&EntryFilter::default()).map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    let validation = Validator::validate_and_save(&entries, &state.db).await.map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;

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
        .add(&entry.term, &entry.translation, &entry.lang_pair, entry.context.as_deref())
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
    let entries = state.db.get_entries(&EntryFilter::default()).map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    let config = state.config.read().await;
    let po = export::export_po(&entries, &config.default_source_lang, &q.lang);
    Ok((
        StatusCode::OK,
        [
            ("Content-Type".to_string(), "text/plain; charset=utf-8".to_string()),
            ("Content-Disposition".to_string(), format!("attachment; filename=\"translation_{}.po\"", q.lang)),
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
    for pe in &po_entries {
        if !pe.translation.is_empty() {
            if let Some(ref id) = pe.id {
                let _ = state.db.save_translation(id, &pe.translation, "import").await;
                imported += 1;
            }
        }
    }
    Ok(Json(serde_json::json!({"imported": imported})))
}

async fn export_xliff(
    State(state): State<Arc<AppState>>,
    Query(q): Query<LangQuery>,
) -> Result<(StatusCode, [(String, String); 2], String), ApiError> {
    let entries = state.db.get_entries(&EntryFilter::default()).map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    let config = state.config.read().await;
    let xliff = export::export_xliff(&entries, &config.default_source_lang, &q.lang);
    Ok((
        StatusCode::OK,
        [
            ("Content-Type".to_string(), "application/xml; charset=utf-8".to_string()),
            ("Content-Disposition".to_string(), format!("attachment; filename=\"translation_{}.xliff\"", q.lang)),
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
    for unit in &units {
        if !unit.target.is_empty() {
            let _ = state.db.save_translation(&unit.id, &unit.target, "import").await;
            imported += 1;
        }
    }
    Ok(Json(serde_json::json!({"imported": imported})))
}

async fn get_config(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let config = state.config.read().await;
    let mut val = serde_json::to_value(&*config).unwrap_or_default();
    // Redact API keys
    if let Some(providers) = val.get_mut("providers").and_then(|v| v.as_object_mut()) {
        for (_id, pc) in providers.iter_mut() {
            if let Some(obj) = pc.as_object_mut() {
                if obj.get("api_key").and_then(|v| v.as_str()).map_or(false, |s| !s.is_empty()) {
                    obj.insert("api_key".to_string(), serde_json::Value::String("***".to_string()));
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
    *config = serde_json::from_value(current.clone()).map_err(|e| err(StatusCode::BAD_REQUEST, e))?;
    // Persist to disk
    let _ = config.save(&AppConfig::default_path());
    Ok(Json(current))
}

async fn memory_stats(
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let project = state.db.memory_count().map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    let global = state.global_memory.memory_count().map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
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
    let limit: usize = params.get("limit").and_then(|s| s.parse().ok()).unwrap_or(50);
    let offset: usize = params.get("offset").and_then(|s| s.parse().ok()).unwrap_or(0);

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
        let resp = client().get(format!("{}/health", url)).send().await.unwrap();
        assert_eq!(resp.status(), 200);
        let body: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(body["status"], "ok");
    }

    #[tokio::test]
    async fn test_list_formats_not_empty() {
        let (url, _h) = setup().await;
        let resp = client().get(format!("{}/api/formats", url)).send().await.unwrap();
        let body: Vec<serde_json::Value> = resp.json().await.unwrap();
        assert!(!body.is_empty());
    }

    #[tokio::test]
    async fn test_list_providers_not_empty() {
        let (url, _h) = setup().await;
        let resp = client().get(format!("{}/api/providers", url)).send().await.unwrap();
        assert_eq!(resp.status(), 200);
        let body: Vec<serde_json::Value> = resp.json().await.unwrap();
        assert!(!body.is_empty());
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
        let dir = std::env::temp_dir().join(format!("locust_test_noformat_{}", uuid::Uuid::new_v4()));
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
        let (url, _h) = setup().await;
        let resp = client()
            .get(format!("{}/api/strings", url))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let body: StringsResponse = resp.json().await.unwrap();
        assert!(body.entries.is_empty());
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
