use axum::extract::{Path as AxumPath, Query, State, WebSocketUpgrade};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use locust_core::database::Database;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::broadcast;

use super::jobs::{cancel_job, upgrade_job_ws};
use crate::{
    active_patch_job_for_game, err, patch_game_key, publish_job_event, try_project_operation,
    ApiError, AppState, JobKind, JobState, PATCH_APPLY_IN_FLIGHT_MESSAGE,
};

fn apply_report_json(report: locust_core::patch::ApplyReport) -> serde_json::Value {
    serde_json::json!({
        "patch_id": report.patch_id,
        "patch_version": report.patch_version,
        "replaced": report.replaced,
        "added": report.added,
        "forced": report.forced,
        "baseline": format!("{:?}", report.baseline),
        "dry_run": report.dry_run,
        "user_edits_overwritten": report.user_edits_overwritten,
        "messages": report.messages,
    })
}

pub(crate) async fn patch_ws(
    State(state): State<Arc<AppState>>,
    AxumPath(job_id): AxumPath<String>,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    upgrade_job_ws(
        state,
        job_id,
        ws,
        serde_json::json!({"type": "error", "message": "job not found"}),
    )
    .await
}

pub(crate) async fn patch_cancel(
    State(state): State<Arc<AppState>>,
    AxumPath(job_id): AxumPath<String>,
) -> Result<StatusCode, ApiError> {
    cancel_job(&state, &job_id)
}

// ─── Patch apply / rollback / status ───────────────────────────────────────

#[derive(Deserialize)]
pub(crate) struct PatchPathsRequest {
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
    resolve_patch_zip_cancellable(zip_path, zip_url, None).await
}

async fn resolve_patch_zip_cancellable(
    zip_path: Option<String>,
    zip_url: Option<String>,
    cancel: Option<&tokio_util::sync::CancellationToken>,
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
            download_patch_zip_async(&url, &dest, cancel).await?;
            Ok((dest, Some(dir)))
        }
        (Some(_), Some(_)) => Err(err(
            StatusCode::BAD_REQUEST,
            "pass either zip_path or zip_url, not both",
        )),
        (None, None) => Err(err(StatusCode::BAD_REQUEST, "zip_path or zip_url required")),
    }
}

async fn download_patch_zip_async(
    url: &str,
    dest: &Path,
    cancel: Option<&tokio_util::sync::CancellationToken>,
) -> Result<(), ApiError> {
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
        if cancel.is_some_and(|c| c.is_cancelled()) {
            drop(file);
            let _ = std::fs::remove_file(dest);
            return Err(err(StatusCode::BAD_REQUEST, "cancelled"));
        }
        let chunk = if let Some(c) = cancel {
            tokio::select! {
                _ = c.cancelled() => {
                    drop(file);
                    let _ = std::fs::remove_file(dest);
                    return Err(err(StatusCode::BAD_REQUEST, "cancelled"));
                }
                chunk = resp.chunk() => chunk,
            }
        } else {
            resp.chunk().await
        }
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
pub(crate) struct PatchPackRequest {
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
    /// Exact BackupManager id returned by the injection being packaged.
    #[serde(default)]
    pristine_backup_id: Option<String>,
    #[serde(default)]
    rj_code: Option<String>,
    #[serde(default)]
    store_ids: Vec<String>,
    #[serde(default)]
    game_version: Option<String>,
    #[serde(default = "default_detect_id")]
    detect_id: bool,
    #[serde(default)]
    entry_path: Option<String>,
}

fn default_detect_id() -> bool {
    true
}

#[derive(Deserialize)]
pub(crate) struct PatchIdentityQuery {
    game_path: String,
}

#[derive(Serialize)]
pub(crate) struct PatchIdentityResponse {
    detected_dlsite_code: Option<String>,
}

pub(crate) async fn patch_identity(
    Query(req): Query<PatchIdentityQuery>,
) -> Result<Json<PatchIdentityResponse>, ApiError> {
    if req.game_path.trim().is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "game_path required"));
    }
    Ok(Json(PatchIdentityResponse {
        detected_dlsite_code: locust_core::patch::detect_dlsite_code(Path::new(&req.game_path)),
    }))
}

#[derive(Serialize)]
pub(crate) struct PatchPackResponse {
    #[serde(flatten)]
    report: locust_core::patch::PackReport,
    #[serde(skip_serializing_if = "Option::is_none")]
    entry_path: Option<String>,
}

pub(crate) fn map_patch_err(e: locust_core::error::LocustError) -> ApiError {
    use locust_core::error::LocustError::*;
    let status = match &e {
        PatchAlreadyApplied(_)
        | PatchDowngradeBlocked { .. }
        | PatchInterrupted(_)
        | PatchLegacyUnconfirmed(_)
        | PatchVerificationFailed(_)
        | PatchBackupIncomplete(_) => StatusCode::CONFLICT,
        PatchUnsafeEntry(_) | GameDirNotWritable(_) | PatchError(_) | BackupError(_) => {
            StatusCode::BAD_REQUEST
        }
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    err(status, e)
}

/// A project opened from its database reports the DB file as its path, so the
/// desktop sends it as `game_path`. Resolve exactly that case to the recorded
/// injection root (selected language, or the only recording). Any other path
/// keeps its existing meaning, including the different-tree refusal.
fn project_db_game_root(db: &Database, lang: Option<&str>, game_path: &Path) -> Option<PathBuf> {
    if !game_path.is_file() || !locust_core::database::paths_identical(game_path, &db.path()) {
        return None;
    }
    let recorded = match lang {
        Some(lang) => db.get_injection(Some(lang)).ok().flatten(),
        None => match db.list_recorded_langs().ok().as_deref() {
            Some([only]) => db.get_injection(only.as_deref()).ok().flatten(),
            _ => None,
        },
    };
    recorded.map(|record| record.root)
}

/// Game folder a site entry describes and protects: the recorded root for a
/// project DB path, the folder itself, or a single file's containing folder.
fn entry_game_root(game_path: &Path) -> PathBuf {
    if game_path.is_file() {
        game_path.parent().unwrap_or(game_path).to_path_buf()
    } else {
        game_path.to_path_buf()
    }
}

pub(crate) async fn patch_pack(
    State(state): State<Arc<AppState>>,
    Json(req): Json<PatchPackRequest>,
) -> Result<Json<PatchPackResponse>, ApiError> {
    let guard = try_project_operation(&state).map_err(|m| err(StatusCode::CONFLICT, m))?;
    if state.current_project.read().await.is_none() {
        return Err(err(StatusCode::BAD_REQUEST, "no project open"));
    }
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
    let requested = PathBuf::from(&req.game_path);
    let game_path =
        project_db_game_root(&state.db, lang.as_deref(), &requested).unwrap_or(requested);
    let game_root = entry_game_root(&game_path);
    let (game, _) = locust_core::patch::identity::resolve_patch_identity(
        req.rj_code.as_deref(),
        &req.store_ids,
        req.game_version.as_deref(),
        req.detect_id,
        &game_root,
    )
    .map_err(|e| err(StatusCode::BAD_REQUEST, e))?;
    let output = PathBuf::from(&req.output_path);
    let entry_path = req.entry_path;
    if entry_path
        .as_ref()
        .is_some_and(|path| path.trim().is_empty())
    {
        return Err(err(StatusCode::BAD_REQUEST, "entry_path required"));
    }
    let pristine = req.pristine_path.map(PathBuf::from);
    if req.pristine_backup_id.is_some() && (pristine.is_some() || !req.pristine) {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "select either a pristine path or an injection backup and require pristine hashes",
        ));
    }
    let pristine_backup_id = req.pristine_backup_id;
    let backups = state.backup_manager.clone();
    let require_pristine = req.pristine;
    // The recorded output may be a directory containing only a file-source
    // format, or a shared parent containing unrelated engines. The open project
    // already knows the format; detecting that parent would mislabel the ZIP.
    let engine = state
        .current_project
        .read()
        .await
        .as_ref()
        .map(|p| p.format_id.clone());

    let db = state.db.clone();
    let db_path_for_errors = db.path();
    let report = tokio::task::spawn_blocking(move || {
        let _guard = guard;
        if let Some(path) = &entry_path {
            let path = Path::new(path);
            locust_core::patch::release_entry::validate_release_entry_output(
                path,
                &game_root,
                &db_path_for_errors,
                &output,
                pristine.as_deref(),
                backups.root(),
            )
            .map_err(|e| err(StatusCode::BAD_REQUEST, e))?;
            for language in db.list_recorded_langs().map_err(map_patch_err)? {
                if let Some(record) = db
                    .get_injection(language.as_deref())
                    .map_err(map_patch_err)?
                {
                    // Every recorded game tree and its backup store stays protected.
                    locust_core::patch::ensure_pack_output_outside(path, &record.root)
                        .map_err(map_patch_err)?;
                    if let Some(storage) = record
                        .pristine_backup
                        .and_then(|backup| backup.storage_root)
                    {
                        locust_core::patch::ensure_pack_output_outside(path, &storage)
                            .map_err(map_patch_err)?;
                    }
                }
            }
        }
        let entry_engine = engine.clone().unwrap_or_else(|| "other".into());
        // Extraction can target one file, while injection recordings are
        // always rooted at its containing game directory.
        let report = locust_core::patch::pack_with_pristine_backup(
            &db,
            locust_core::patch::PackOptions {
                game: Some(game),
                game_path: game_path.clone(),
                lang,
                output,
                pristine,
                engine,
                project: db_path_for_errors,
                require_pristine,
            },
            &backups,
            pristine_backup_id.as_deref(),
            require_pristine,
        )
        .map_err(map_patch_err)?;
        if let Some(path) = &entry_path {
            locust_core::patch::release_entry::write_release_entry(
                Path::new(path),
                &game_root,
                report.recording_lang.as_deref(),
                Path::new(&report.output_path),
                &entry_engine,
            )
            .map_err(|e| {
                err(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!(
                        "Patch written to {}; entry could not be written: {e}",
                        report.output_path
                    ),
                )
            })?;
        }
        Ok::<_, ApiError>(PatchPackResponse { report, entry_path })
    })
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))??;

    Ok(Json(report))
}

#[derive(Serialize)]
pub(crate) struct PatchRecordingsResponse {
    /// Named keys first; JSON `null` is the language-unspecified recording.
    languages: Vec<Option<String>>,
    /// Provenance is recorded; the payload is verified again when packing.
    pristine_languages: Vec<Option<String>>,
    /// Actual output roots, keyed by language (empty key for unspecified).
    roots: std::collections::HashMap<String, PathBuf>,
}

pub(crate) async fn patch_recordings(
    State(state): State<Arc<AppState>>,
) -> Result<Json<PatchRecordingsResponse>, ApiError> {
    // Same leak guard as get_strings: leftover recordings from another session
    // must not surface without an open project.
    if state.current_project.read().await.is_none() {
        return Ok(Json(PatchRecordingsResponse {
            languages: vec![],
            pristine_languages: vec![],
            roots: std::collections::HashMap::new(),
        }));
    }
    let languages = state
        .db
        .list_recorded_langs()
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    let mut pristine_languages = Vec::new();
    let mut roots = std::collections::HashMap::new();
    for language in &languages {
        if let Some(record) = state
            .db
            .get_injection(language.as_deref())
            .map_err(map_patch_err)?
        {
            roots.insert(language.clone().unwrap_or_default(), record.root);
            if record.pristine_backup.is_some() {
                pristine_languages.push(language.clone());
            }
        }
    }
    Ok(Json(PatchRecordingsResponse {
        languages,
        pristine_languages,
        roots,
    }))
}

pub(crate) async fn patch_verify(
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

pub(crate) async fn patch_apply(
    State(state): State<Arc<AppState>>,
    Json(req): Json<PatchPathsRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    match (&req.zip_path, &req.zip_url) {
        (Some(_), Some(_)) => {
            return Err(err(
                StatusCode::BAD_REQUEST,
                "pass either zip_path or zip_url, not both",
            ));
        }
        (None, None) => {
            return Err(err(StatusCode::BAD_REQUEST, "zip_path or zip_url required"));
        }
        (Some(p), None) if !Path::new(p).is_file() => {
            return Err(err(
                StatusCode::BAD_REQUEST,
                format!("zip_path not found: {p}"),
            ));
        }
        _ => {}
    }

    if req.game_path.trim().is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "game_path required"));
    }
    if active_patch_job_for_game(&state, &req.game_path).is_some() {
        return Err(err(StatusCode::CONFLICT, PATCH_APPLY_IN_FLIGHT_MESSAGE));
    }

    let job_id = uuid::Uuid::new_v4().to_string();
    let (broadcast_tx, _) = broadcast::channel::<serde_json::Value>(1000);
    let replay = Arc::new(std::sync::Mutex::new(Vec::new()));
    let cancel = tokio_util::sync::CancellationToken::new();
    let game_key = patch_game_key(&req.game_path);

    state.active_jobs.insert(
        job_id.clone(),
        JobState {
            abort_handle: tokio::spawn(async {}).abort_handle(),
            progress_tx: broadcast_tx.clone(),
            replay: replay.clone(),
            cancel: cancel.clone(),
            kind: JobKind::Patch,
            patch_game_key: Some(game_key),
        },
    );

    let jobs = state.active_jobs.clone();
    let cleanup_job_id = job_id.clone();
    let worker_tx = broadcast_tx;
    let worker_replay = replay;
    let worker_cancel = cancel;
    let handle = tokio::spawn(async move {
        run_patch_apply_job(req, worker_tx, worker_replay, worker_cancel).await;
        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        jobs.remove(&cleanup_job_id);
    });

    if let Some(mut job) = state.active_jobs.get_mut(&job_id) {
        job.abort_handle = handle.abort_handle();
    }

    Ok((
        StatusCode::ACCEPTED,
        Json(serde_json::json!({ "job_id": job_id })),
    ))
}

async fn run_patch_apply_job(
    req: PatchPathsRequest,
    broadcast_tx: broadcast::Sender<serde_json::Value>,
    replay: Arc<std::sync::Mutex<Vec<serde_json::Value>>>,
    cancel: tokio_util::sync::CancellationToken,
) {
    let publish = |event: serde_json::Value| publish_job_event(&broadcast_tx, &replay, event);

    if cancel.is_cancelled() {
        publish(serde_json::json!({"type": "error", "message": "cancelled"}));
        return;
    }

    let (zip, guard) =
        match resolve_patch_zip_cancellable(req.zip_path, req.zip_url, Some(&cancel)).await {
            Ok(v) => v,
            Err((_, msg)) => {
                if cancel.is_cancelled() || msg == "cancelled" {
                    publish(serde_json::json!({"type": "error", "message": "cancelled"}));
                } else {
                    publish(serde_json::json!({"type": "error", "message": msg}));
                }
                return;
            }
        };

    if cancel.is_cancelled() {
        publish(serde_json::json!({"type": "error", "message": "cancelled"}));
        return;
    }

    let opts = locust_core::patch::ApplyOptions {
        force: req.force,
        confirm_legacy: req.confirm_legacy,
        dry_run: req.dry_run,
    };
    let game = PathBuf::from(&req.game_path);
    let (progress_tx, progress_rx) =
        tokio::sync::mpsc::channel::<locust_core::patch::PatchProgress>(1);
    let apply_cancel = cancel.clone();

    let apply_task = tokio::task::spawn_blocking(move || {
        locust_core::patch::apply_cancellable(&game, &zip, opts, |p| {
            if apply_cancel.is_cancelled() || progress_tx.blocking_send(p).is_err() {
                return Err(locust_core::error::LocustError::PatchInterrupted(
                    "cancelled".into(),
                ));
            }
            Ok(())
        })
    });
    tokio::pin!(apply_task);
    let mut progress_rx = Some(progress_rx);

    let outcome = loop {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                // Drop the receiver so the next blocking_send fails and apply unwinds.
                progress_rx.take();
                let _ = apply_task.await;
                break Err("cancelled".to_string());
            }
            ev = async {
                match progress_rx.as_mut() {
                    Some(rx) => rx.recv().await,
                    None => None,
                }
            } => {
                match ev {
                    Some(p) => {
                        publish(serde_json::json!({
                            "type": "progress",
                            "current": p.current as u64,
                            "total": p.total as u64,
                            "path": p.path,
                            "phase": p.phase,
                        }));
                    }
                    None => {
                        break match apply_task.await {
                            Ok(Ok(report)) => Ok(apply_report_json(report)),
                            Ok(Err(e)) => Err(e.to_string()),
                            Err(e) if e.is_cancelled() || cancel.is_cancelled() => {
                                Err("cancelled".into())
                            }
                            Err(e) => Err(format!("apply task failed: {e}")),
                        };
                    }
                }
            }
            result = &mut apply_task => {
                break match result {
                    Ok(Ok(report)) => {
                        if let Some(rx) = progress_rx.as_mut() {
                            while let Ok(p) = rx.try_recv() {
                                publish(serde_json::json!({
                                    "type": "progress",
                                    "current": p.current as u64,
                                    "total": p.total as u64,
                                    "path": p.path,
                                    "phase": p.phase,
                                }));
                            }
                        }
                        Ok(apply_report_json(report))
                    }
                    Ok(Err(e)) => Err(e.to_string()),
                    Err(e) if e.is_cancelled() || cancel.is_cancelled() => Err("cancelled".into()),
                    Err(e) => Err(format!("apply task failed: {e}")),
                };
            }
        }
    };

    drop(guard);

    if cancel.is_cancelled() {
        publish(serde_json::json!({"type": "error", "message": "cancelled"}));
        return;
    }

    match outcome {
        Ok(report) => publish(serde_json::json!({"type": "done", "report": report})),
        Err(message) => publish(serde_json::json!({"type": "error", "message": message})),
    }
}

pub(crate) async fn patch_rollback(
    Json(req): Json<PatchPathsRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let game = PathBuf::from(&req.game_path);
    let force = req.force;
    let report = tokio::task::spawn_blocking(move || {
        let opts = locust_core::patch::RollbackOptions {
            delete_modified_added: force,
        };
        if req.dry_run {
            locust_core::patch::preview_rollback(&game, opts)
        } else {
            locust_core::patch::rollback(&game, opts)
        }
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
        "dry_run": req.dry_run,
    })))
}

pub(crate) async fn patch_status(
    Json(req): Json<PatchPathsRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let store = locust_core::patch::PatchStore::new(PathBuf::from(&req.game_path));
    let status = store.status().map_err(map_patch_err)?;
    let injections = store.injection_status().map_err(map_patch_err)?;
    let mut body = match status {
        locust_core::patch::PatchStatus::NotPatched => {
            serde_json::json!({ "status": "not_patched" })
        }
        locust_core::patch::PatchStatus::Patched(r) => {
            // Status is read-only: no GameLock. The journal check above gives
            // an in-flight apply precedence as Interrupted.
            let (r, report) = tokio::task::spawn_blocking(move || {
                locust_core::patch::verify_receipt_files(
                    &store,
                    &r,
                    locust_core::patch::ReceiptVerificationMode::Quick,
                )
                .map(|report| (r, report))
            })
            .await
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?
            .map_err(map_patch_err)?;
            let mut body = serde_json::json!({
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
            });
            if !report.changed.is_empty() || !report.missing.is_empty() {
                body["status"] = serde_json::json!("unknown");
                body["drift"] = serde_json::json!({
                    "changed": report.changed,
                    "missing": report.missing,
                });
            }
            body
        }
        locust_core::patch::PatchStatus::Interrupted(j) => serde_json::json!({
            "status": "interrupted",
            "patch_id": j.patch_id,
            "state": format!("{:?}", j.state),
        }),
        locust_core::patch::PatchStatus::Unknown => {
            serde_json::json!({ "status": "unknown" })
        }
    };
    body["injections"] = serde_json::json!(injections.injections);
    body["injection_pending"] = serde_json::json!(injections.injection_pending);
    Ok(Json(body))
}

#[cfg(test)]
mod tests {
    use crate::routes::test_support::*;

    #[tokio::test]
    async fn patch_recordings_lists_langs_and_hides_them_without_a_project() {
        let (url, _h, state) = setup_with_state().await;
        let dir_guard = tempfile::Builder::new()
            .prefix("locust_rec_")
            .tempdir()
            .unwrap();
        let dir = dir_guard.path().to_path_buf();
        state.test_temp_dirs.lock().unwrap().push(dir_guard);
        let root = dir.join("game");
        let file = root.join("a.txt");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(&file, b"x").unwrap();
        state
            .db
            .record_injection(Some("es"), &root, std::slice::from_ref(&file))
            .unwrap();

        let hidden = client()
            .get(format!("{}/api/patch/recordings", url))
            .send()
            .await
            .unwrap();
        assert_eq!(hidden.status(), 200);
        let hidden_body: serde_json::Value = hidden.json().await.unwrap();
        assert_eq!(
            hidden_body["languages"],
            serde_json::json!([]),
            "recordings must not leak without an open project"
        );

        mark_project_open(&state).await;
        let shown = client()
            .get(format!("{}/api/patch/recordings", url))
            .send()
            .await
            .unwrap();
        assert_eq!(shown.status(), 200);
        let shown_body: serde_json::Value = shown.json().await.unwrap();
        assert_eq!(shown_body["languages"], serde_json::json!(["es"]));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn patch_pack_requires_an_open_project() {
        let (url, _h, state) = setup_with_state().await;
        let game = tempfile::tempdir().unwrap();
        let file = game.path().join("story.html");
        let original = b"<p>Hola</p>";
        std::fs::write(&file, original).unwrap();
        state
            .db
            .save_entries(&[translated("leftover", "Hello", "story.html", "Hola")])
            .unwrap();
        state
            .db
            .record_injection(Some("es"), game.path(), std::slice::from_ref(&file))
            .unwrap();
        let before = serde_json::to_value(state.db.get_entry("leftover").unwrap()).unwrap();
        let recordings_before = state.db.list_recorded_langs().unwrap();
        let output = game
            .path()
            .parent()
            .unwrap()
            .join(format!("locust_pack_{}.zip", uuid::Uuid::new_v4()));

        let response = client()
            .post(format!("{url}/api/patch/pack"))
            .json(&serde_json::json!({
                "game_path": game.path(),
                "output_path": output,
                "languages": ["es"]
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 400);
        assert_eq!(response.text().await.unwrap(), "no project open");
        assert_eq!(
            serde_json::to_value(state.db.get_entry("leftover").unwrap()).unwrap(),
            before
        );
        assert_eq!(state.db.list_recorded_langs().unwrap(), recordings_before);
        assert_eq!(std::fs::read(&file).unwrap(), original);
        assert!(!output.exists());
    }
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
        let e = download_patch_zip_async("file:///C:/Windows/win.ini", &dest, None)
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
            download_patch_zip_async(&url, &dest, None),
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
        let e = download_patch_zip_async(&url, &dest, None)
            .await
            .expect_err("empty body must be rejected");
        assert!(format!("{e:?}").contains("empty"), "{e:?}");
        assert!(!dest.exists());
    }
}
