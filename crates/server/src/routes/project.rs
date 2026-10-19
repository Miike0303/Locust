use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use locust_core::database::PivotResult;
use locust_core::models::OutputMode;
use locust_core::project::{self, ProjectOpenOutcome};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::{
    err, remember_open_project, run_owned_project_operation, try_project_operation, ApiError,
    AppState, ProjectInfo,
};

#[derive(Deserialize)]
pub(crate) struct OpenProjectRequest {
    pub(crate) path: String,
    pub(crate) format_id: Option<String>,
    /// Legacy recents without `database_path` set this to prefer an existing
    /// project DB instead of extracting. Default false preserves explicit
    /// Open Folder extract/merge.
    #[serde(default)]
    pub(crate) prefer_saved: bool,
}

#[derive(Serialize)]
pub(crate) struct ProjectOpenResponse {
    format_id: String,
    format_name: String,
    total_strings: usize,
    project_path: String,
    project_name: String,
    pub(crate) supported_modes: Vec<OutputMode>,
    pub(crate) database_path: String,
    added: usize,
    updated: usize,
    stale_source_reset: usize,
    removed: usize,
    preserved_translations: usize,
    pub extraction_warnings: Vec<String>,
    pub(crate) persistence_warning: Option<String>,
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
            extraction_warnings: o.extraction_warnings,
            persistence_warning: None,
        }
    }
}

fn map_open_err(e: locust_core::error::LocustError) -> ApiError {
    match e {
        locust_core::error::LocustError::InjectionError(msg) => err(StatusCode::CONFLICT, msg),
        locust_core::error::LocustError::PatchError(msg) if msg.starts_with("game busy:") => {
            err(StatusCode::CONFLICT, msg)
        }
        locust_core::error::LocustError::ProjectNotFound(_) => {
            err(StatusCode::BAD_REQUEST, "path not found")
        }
        locust_core::error::LocustError::UnsupportedFormat(msg) => {
            err(StatusCode::UNPROCESSABLE_ENTITY, msg)
        }
        locust_core::error::LocustError::IoError(io)
            if matches!(
                io.kind(),
                std::io::ErrorKind::InvalidData | std::io::ErrorKind::InvalidInput
            ) =>
        {
            err(
                StatusCode::BAD_REQUEST,
                locust_core::error::LocustError::IoError(io),
            )
        }
        other => err(StatusCode::INTERNAL_SERVER_ERROR, other),
    }
}

#[derive(Deserialize)]
pub(crate) struct ProjectPreflightRequest {
    game_path: String,
    #[serde(alias = "format_id")]
    format: Option<String>,
}

pub(crate) async fn project_preflight(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ProjectPreflightRequest>,
) -> Result<Json<project::ProjectOpenPreflight>, ApiError> {
    project::preflight_project_open(
        &state.format_registry,
        Path::new(&req.game_path),
        req.format.as_deref(),
    )
    .map(Json)
    .map_err(map_open_err)
}

pub(crate) async fn project_resume(
    State(state): State<Arc<AppState>>,
    Json(req): Json<OpenProjectDbRequest>,
) -> Result<Json<ProjectOpenResponse>, ApiError> {
    let guard = try_project_operation(&state).map_err(|m| err(StatusCode::CONFLICT, m))?;
    run_owned_project_operation(guard, async move {
        let outcome = project::open_verified_saved_project(
            &state.db,
            &state.format_registry,
            Path::new(&req.database_path),
            Path::new(&req.game_path),
            &req.format_id,
        )
        .map_err(map_open_err)?;
        finish_project_open(&state, outcome).await
    })
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?
}

pub(crate) async fn project_open(
    State(state): State<Arc<AppState>>,
    Json(req): Json<OpenProjectRequest>,
) -> Result<Json<ProjectOpenResponse>, ApiError> {
    let guard = try_project_operation(&state).map_err(|m| err(StatusCode::CONFLICT, m))?;
    run_owned_project_operation(guard, async move { project_open_owned(state, req).await })
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?
}

async fn project_open_owned(
    state: Arc<AppState>,
    req: OpenProjectRequest,
) -> Result<Json<ProjectOpenResponse>, ApiError> {
    let raw_path = PathBuf::from(&req.path);
    let outcome = if req.prefer_saved {
        project::open_recent_project(
            &state.db,
            &state.format_registry,
            &raw_path,
            req.format_id.as_deref(),
        )
    } else {
        project::open_project(
            &state.db,
            &state.format_registry,
            &raw_path,
            req.format_id.as_deref(),
        )
    }
    .map_err(map_open_err)?;

    finish_project_open(&state, outcome).await
}

async fn finish_project_open(
    state: &AppState,
    outcome: ProjectOpenOutcome,
) -> Result<Json<ProjectOpenResponse>, ApiError> {
    let persistence_warning = remember_open_project(state, &outcome, true).await;
    {
        let mut proj = state.current_project.write().await;
        *proj = Some(ProjectInfo {
            path: outcome.project_path.clone(),
            format_id: outcome.format_id.clone(),
            name: outcome.project_name.clone(),
            extraction_warnings: outcome.extraction_warnings.clone(),
            database_path: Some(outcome.database_path.clone()),
            supported_modes: outcome.supported_modes.clone(),
            persistence_warning: persistence_warning.clone(),
        });
    }

    let mut response = ProjectOpenResponse::from(outcome);
    response.persistence_warning = persistence_warning;
    Ok(Json(response))
}

#[derive(Deserialize)]
pub(crate) struct OpenProjectDbRequest {
    database_path: String,
    game_path: String,
    format_id: String,
}

fn map_open_db_err(e: locust_core::error::LocustError) -> ApiError {
    let status = match &e {
        locust_core::error::LocustError::InjectionError(_) => StatusCode::CONFLICT,
        locust_core::error::LocustError::PatchError(msg) if msg.starts_with("game busy:") => {
            StatusCode::CONFLICT
        }
        locust_core::error::LocustError::UnsupportedFormat(_) => StatusCode::UNPROCESSABLE_ENTITY,
        locust_core::error::LocustError::ProjectNotFound(_)
        | locust_core::error::LocustError::IoError(_)
        | locust_core::error::LocustError::DatabaseError(_) => StatusCode::BAD_REQUEST,
        _ => StatusCode::BAD_REQUEST,
    };
    err(status, e)
}

pub(crate) async fn project_open_db(
    State(state): State<Arc<AppState>>,
    Json(req): Json<OpenProjectDbRequest>,
) -> Result<Json<ProjectOpenResponse>, ApiError> {
    let guard = try_project_operation(&state).map_err(|m| err(StatusCode::CONFLICT, m))?;
    run_owned_project_operation(
        guard,
        async move { project_open_db_owned(state, req).await },
    )
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?
}

async fn project_open_db_owned(
    state: Arc<AppState>,
    req: OpenProjectDbRequest,
) -> Result<Json<ProjectOpenResponse>, ApiError> {
    let outcome = project::open_project_db(
        &state.db,
        &state.format_registry,
        Path::new(&req.database_path),
        Path::new(&req.game_path),
        &req.format_id,
    )
    .map_err(map_open_db_err)?;

    finish_project_open(&state, outcome).await
}

pub(crate) async fn project_current(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ProjectInfo>, ApiError> {
    let proj = state.current_project.read().await;
    match proj.as_ref() {
        Some(p) => Ok(Json(p.clone())),
        None => Err(err(StatusCode::NOT_FOUND, "no project open")),
    }
}

#[derive(Deserialize)]
pub(crate) struct PivotRequest {
    output_path: String,
    #[serde(default)]
    require_complete: bool,
}

pub(crate) async fn pivot_project(
    State(state): State<Arc<AppState>>,
    Json(req): Json<PivotRequest>,
) -> Result<Json<PivotResult>, ApiError> {
    let _exclusive = try_project_operation(&state).map_err(|m| err(StatusCode::CONFLICT, m))?;
    if state.current_project.read().await.is_none() {
        return Err(err(StatusCode::BAD_REQUEST, "no project open"));
    }
    // Pivot reads every translated row from the live DB — open must not reopen under it.
    let output = PathBuf::from(&req.output_path);
    let result = if req.require_complete {
        state.db.pivot_to_complete(&output)
    } else {
        state.db.pivot_to(&output)
    };
    result.map(Json).map_err(|e| {
        let msg = e.to_string();
        let status = match &e {
            locust_core::error::LocustError::IoError(io)
                if io.kind() == std::io::ErrorKind::AlreadyExists =>
            {
                StatusCode::CONFLICT
            }
            _ if msg.to_lowercase().contains("no translated") => StatusCode::UNPROCESSABLE_ENTITY,
            _ => StatusCode::BAD_REQUEST,
        };
        err(status, e)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routes::test_support::*;
    use crate::*;
    use locust_core::config::AppConfig;
    use locust_core::database::{Database, EntryFilter};
    use locust_core::models::{StringEntry, StringStatus};

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
        let dir_guard = tempfile::Builder::new()
            .prefix("locust_test_noformat_")
            .tempdir()
            .unwrap();
        let dir = dir_guard.path().to_path_buf();
        let resp = client()
            .post(format!("{}/api/project/open", url))
            .json(&serde_json::json!({"path": dir.to_string_lossy()}))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 422);
    }

    fn write_html_game(dir: &Path, body: &str) {
        std::fs::write(dir.join("story.html"), format!("<p>{body}</p>")).unwrap();
    }

    #[tokio::test]
    async fn prefer_saved_open_skips_extract_after_source_change() {
        let fixture = tempfile::tempdir().unwrap();
        let game = tempfile::tempdir_in(fixture.path()).unwrap();
        write_html_game(game.path(), "Hello");
        let state = create_test_state();
        let (url, server) = test_server::start_test_server(create_router(state.clone())).await;
        let first = client()
            .post(format!("{url}/api/project/open"))
            .json(&serde_json::json!({
                "path": game.path(),
                "format_id": "html-game"
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(first.status(), 200);
        let first_body: serde_json::Value = first.json().await.unwrap();
        let db_path = first_body["database_path"].as_str().unwrap().to_string();
        let hero = state
            .db
            .get_entries(&EntryFilter::default())
            .unwrap()
            .into_iter()
            .find(|e| e.source == "Hello")
            .unwrap();
        assert!(state
            .db
            .save_translation(&hero.id, "Hola", "mock")
            .await
            .unwrap());
        state
            .db
            .update_entry_status(&hero.id, StringStatus::Approved)
            .await
            .unwrap();

        write_html_game(game.path(), "Hello, traveler");
        let saved = client()
            .post(format!("{url}/api/project/open"))
            .json(&serde_json::json!({
                "path": game.path(),
                "format_id": "html-game",
                "prefer_saved": true
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(saved.status(), 200);
        let saved_body: serde_json::Value = saved.json().await.unwrap();
        assert_eq!(saved_body["stale_source_reset"], 0);
        assert_eq!(saved_body["added"], 0);
        assert_eq!(saved_body["database_path"], db_path);
        let kept = state.db.get_entry(&hero.id).unwrap().unwrap();
        assert_eq!(kept.source, "Hello");
        assert_eq!(kept.translation.as_deref(), Some("Hola"));
        assert_eq!(kept.status, StringStatus::Approved);

        let explicit = client()
            .post(format!("{url}/api/project/open"))
            .json(&serde_json::json!({
                "path": game.path(),
                "format_id": "html-game"
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(explicit.status(), 200);
        let explicit_body: serde_json::Value = explicit.json().await.unwrap();
        assert_eq!(explicit_body["stale_source_reset"], 1);
        let reset = state.db.get_entry(&hero.id).unwrap().unwrap();
        assert_eq!(reset.source, "Hello, traveler");
        assert_eq!(reset.translation.as_deref(), Some("Hola"));
        assert_eq!(reset.status, StringStatus::Pending);
        server.shutdown().await;
    }

    #[tokio::test]
    async fn prefer_saved_missing_db_extracts() {
        let fixture = tempfile::tempdir().unwrap();
        let missing = tempfile::tempdir_in(fixture.path()).unwrap();
        write_html_game(missing.path(), "Only extract");
        let state = create_test_state();
        let (url, server) = test_server::start_test_server(create_router(state.clone())).await;
        let extracted = client()
            .post(format!("{url}/api/project/open"))
            .json(&serde_json::json!({
                "path": missing.path(),
                "format_id": "html-game",
                "prefer_saved": true
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(extracted.status(), 200);
        let extracted_body: serde_json::Value = extracted.json().await.unwrap();
        assert!(extracted_body["added"].as_u64().unwrap() >= 1);
        server.shutdown().await;
    }

    #[tokio::test]
    async fn prefer_saved_invalid_db_returns_400_without_extract() {
        let invalid = tempfile::tempdir().unwrap();
        write_html_game(invalid.path(), "Must not extract");
        let stem = locust_core::project::sanitize_project_stem(
            invalid.path().file_name().unwrap().to_str().unwrap(),
        );
        let adjacent = invalid
            .path()
            .parent()
            .unwrap()
            .join(format!("{stem}.locust.db"));
        std::fs::write(&adjacent, b"not-a-sqlite-database").unwrap();
        let (url, _h, state) = setup_with_state().await;
        let resp = client()
            .post(format!("{url}/api/project/open"))
            .json(&serde_json::json!({
                "path": invalid.path(),
                "format_id": "html-game",
                "prefer_saved": true
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 400);
        let text = resp.text().await.unwrap().to_lowercase();
        assert!(text.contains("not a locust project database"), "{text}");
        assert_eq!(state.db.path(), PathBuf::from(":memory:"));
        assert!(state
            .db
            .get_entries(&EntryFilter::default())
            .unwrap()
            .iter()
            .all(|e| e.source != "Must not extract"));
        let _ = std::fs::remove_file(&adjacent);
    }

    #[tokio::test]
    async fn test_pivot_without_project_is_clean_error() {
        let (url, _h) = setup().await;
        let resp = client()
            .post(format!("{}/api/pivot", url))
            .json(&serde_json::json!({"output_path": "/tmp/x.locust.db"}))
            .send()
            .await
            .unwrap();
        assert!(resp.status().is_client_error(), "{}", resp.status());
        assert_ne!(resp.status(), 500);
        let text = resp.text().await.unwrap();
        assert!(
            text.to_lowercase().contains("no project"),
            "clean error: {text}"
        );
    }

    #[tokio::test]
    async fn test_pivot_creates_new_db_skips_pending_leaves_source_untouched() {
        let (url, _h, state) = setup_with_state().await;
        mark_project_open(&state).await;
        state
            .db
            .save_entries(&[
                translated("a", "Hello", "data/Actors.json", "Hola"),
                StringEntry::new("b", "World", PathBuf::from("data/Actors.json")),
            ])
            .unwrap();

        let dir_guard = tempfile::Builder::new()
            .prefix("locust_pivot_")
            .tempdir()
            .unwrap();
        let dir = dir_guard.path().to_path_buf();
        state.test_temp_dirs.lock().unwrap().push(dir_guard);
        let out = dir.join("new.locust.db");

        let resp = client()
            .post(format!("{}/api/pivot", url))
            .json(&serde_json::json!({"output_path": out.to_string_lossy()}))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200, "{}", resp.status());
        let body: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(body["entries"], 1);
        assert_eq!(body["database_path"], out.to_string_lossy().as_ref());

        let cur = client()
            .get(format!("{}/api/project/current", url))
            .send()
            .await
            .unwrap();
        assert_eq!(cur.status(), 200);
        let cur_body: serde_json::Value = cur.json().await.unwrap();
        assert_eq!(cur_body["name"], "test-game");

        let src_entries = state.db.get_entries(&EntryFilter::default()).unwrap();
        assert_eq!(src_entries.len(), 2);
        assert_eq!(
            src_entries.iter().find(|e| e.id == "a").unwrap().source,
            "Hello"
        );

        let new_db = Database::open(&out).unwrap();
        let pivoted = new_db.get_entries(&EntryFilter::default()).unwrap();
        assert_eq!(pivoted.len(), 1);
        assert_eq!(pivoted[0].source, "Hola");
        assert!(pivoted[0].translation.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_pivot_refuses_existing_output_path() {
        let (url, _h, state) = setup_with_state().await;
        mark_project_open(&state).await;
        state
            .db
            .save_entries(&[translated("a", "Hello", "f.json", "Hola")])
            .unwrap();

        let dir_guard = tempfile::Builder::new()
            .prefix("locust_pivot_ex_")
            .tempdir()
            .unwrap();
        let dir = dir_guard.path().to_path_buf();
        state.test_temp_dirs.lock().unwrap().push(dir_guard);
        let out = dir.join("exists.locust.db");
        std::fs::write(&out, b"keep-me").unwrap();

        let resp = client()
            .post(format!("{}/api/pivot", url))
            .json(&serde_json::json!({"output_path": out.to_string_lossy()}))
            .send()
            .await
            .unwrap();
        assert!(resp.status().is_client_error(), "{}", resp.status());
        assert_ne!(resp.status(), 500);
        let text = resp.text().await.unwrap();
        assert!(
            text.to_lowercase().contains("exist") || text.to_lowercase().contains("overwrite"),
            "{text}"
        );
        assert_eq!(std::fs::read(&out).unwrap(), b"keep-me");
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn write_opendb_locust(dir: &Path, name: &str, rows: &[StringEntry]) -> PathBuf {
        let path = dir.join(name);
        let db = Database::open(&path).unwrap();
        db.save_entries(rows).unwrap();
        drop(db);
        path
    }

    #[tokio::test]
    async fn failed_open_db_keeps_current_project_and_patch_destination() {
        let fixture = tempfile::tempdir().unwrap();
        let path_a = write_opendb_locust(
            fixture.path(),
            "previous.locust.db",
            &[translated(
                "hero",
                "Previous",
                "f.json",
                "Previous translation",
            )],
        );
        let path_b = write_opendb_locust(
            fixture.path(),
            "candidate.locust.db",
            &[translated(
                "hero",
                "Candidate",
                "f.json",
                "Candidate translation",
            )],
        );
        let candidate = Database::open(&path_b).unwrap();
        candidate
            .set_project_metadata("extraction_warnings", &serde_json::json!(42))
            .unwrap();
        let candidate_row = serde_json::to_value(candidate.get_entry("hero").unwrap()).unwrap();
        drop(candidate);
        let candidate_bytes = std::fs::read(&path_b).unwrap();
        let state = create_test_state();
        let (url, server) = test_server::start_test_server(create_router(state.clone())).await;
        let http = client();
        let opened = http
            .post(format!("{url}/api/project/open-db"))
            .json(&serde_json::json!({
                "database_path": path_a,
                "game_path": fixture.path().join("moved-a"),
                "format_id": "rpgmaker-mv"
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(opened.status(), 200, "{}", opened.text().await.unwrap());
        let before: serde_json::Value = http
            .get(format!("{url}/api/project/current"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let failed = http
            .post(format!("{url}/api/project/open-db"))
            .json(&serde_json::json!({
                "database_path": path_b,
                "game_path": fixture.path().join("moved-b"),
                "format_id": "rpgmaker-mv"
            }))
            .send()
            .await
            .unwrap();
        assert!(!failed.status().is_success());
        let error = failed.text().await.unwrap();
        assert!(error.contains("expected a sequence"), "{error}");
        let current: serde_json::Value = http
            .get(format!("{url}/api/project/current"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(current, before);
        let patched = http
            .patch(format!("{url}/api/strings/hero"))
            .json(&serde_json::json!({"translation": "Edited after failure"}))
            .send()
            .await
            .unwrap();
        assert_eq!(patched.status(), 200, "{}", patched.text().await.unwrap());
        let previous = Database::open(&path_a).unwrap();
        assert_eq!(
            previous
                .get_entry("hero")
                .unwrap()
                .unwrap()
                .translation
                .as_deref(),
            Some("Edited after failure")
        );
        assert_eq!(state.db.path(), path_a);
        let candidate = Database::open(&path_b).unwrap();
        assert_eq!(
            serde_json::to_value(candidate.get_entry("hero").unwrap()).unwrap(),
            candidate_row
        );
        drop(candidate);
        assert_eq!(std::fs::read(&path_b).unwrap(), candidate_bytes);
        server.shutdown().await;
    }

    #[tokio::test]
    async fn failed_first_open_db_keeps_edits_unavailable() {
        let fixture = tempfile::tempdir().unwrap();
        let path = write_opendb_locust(
            fixture.path(),
            "candidate.locust.db",
            &[translated(
                "hero",
                "Candidate",
                "f.json",
                "Candidate translation",
            )],
        );
        let candidate = Database::open(&path).unwrap();
        candidate
            .set_project_metadata("extraction_warnings", &serde_json::json!(42))
            .unwrap();
        drop(candidate);
        let candidate_bytes = std::fs::read(&path).unwrap();
        let (url, server, state) = setup_with_state().await;
        let live_path = state.db.path();
        let http = client();
        let failed = http
            .post(format!("{url}/api/project/open-db"))
            .json(&serde_json::json!({
                "database_path": path,
                "game_path": fixture.path().join("moved"),
                "format_id": "rpgmaker-mv"
            }))
            .send()
            .await
            .unwrap();
        assert!(!failed.status().is_success());
        let error = failed.text().await.unwrap();
        assert!(error.contains("expected a sequence"), "{error}");
        let current = http
            .get(format!("{url}/api/project/current"))
            .send()
            .await
            .unwrap();
        assert_eq!(current.status(), 404);
        let patched = http
            .patch(format!("{url}/api/strings/hero"))
            .json(&serde_json::json!({"translation": "Wrong project"}))
            .send()
            .await
            .unwrap();
        assert_eq!(patched.status(), 400);
        assert_eq!(patched.text().await.unwrap(), "no project open");
        assert_eq!(state.db.path(), live_path);
        assert_eq!(state.db.get_stats().unwrap().total, 0);
        assert_eq!(std::fs::read(&path).unwrap(), candidate_bytes);
        server.abort();
    }

    #[tokio::test]
    async fn test_open_db_switches_without_extract_or_touching_source() {
        let dir_guard = tempfile::Builder::new()
            .prefix("locust_opendb_ok_")
            .tempdir()
            .unwrap();
        let dir = dir_guard.path().to_path_buf();
        let source_path = write_opendb_locust(
            &dir,
            "source.locust.db",
            &[
                translated("a", "Hello", "data/Actors.json", "Hola"),
                StringEntry::new("b", "World", PathBuf::from("data/Actors.json")),
            ],
        );
        let state = create_test_state_with_db(&source_path);
        state.test_temp_dirs.lock().unwrap().push(dir_guard);
        mark_project_open(&state).await;
        let (url, _h) = start_test_server(state.clone()).await;

        let pivot = dir.join("pivoted.locust.db");
        let pivoted = state.db.pivot_to(&pivot).unwrap();
        assert_eq!(pivoted.entries, 1);

        let game = dir.join("SomeGame");
        std::fs::create_dir_all(&game).unwrap();

        let resp = client()
            .post(format!("{}/api/project/open-db", url))
            .json(&serde_json::json!({
                "database_path": pivot.to_string_lossy(),
                "game_path": game.to_string_lossy(),
                "format_id": "rpgmaker-mv"
            }))
            .send()
            .await
            .unwrap();
        let status = resp.status();
        let body: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(status, 200, "{body}");
        assert_eq!(body["format_id"], "rpgmaker-mv");
        assert_eq!(body["format_name"], "RPG Maker MV/MZ");
        assert_eq!(body["total_strings"], 1);
        assert_eq!(body["added"], 0);
        assert_eq!(body["updated"], 0);
        assert_eq!(body["stale_source_reset"], 0);
        assert_eq!(body["removed"], 0);
        assert_eq!(body["preserved_translations"], 0);
        assert_eq!(body["project_name"], "SomeGame");
        assert_eq!(body["database_path"], pivot.to_string_lossy().as_ref());
        assert_eq!(body["project_path"], game.to_string_lossy().as_ref());
        assert!(body["supported_modes"]
            .as_array()
            .is_some_and(|m| !m.is_empty()));

        let cur = client()
            .get(format!("{}/api/project/current", url))
            .send()
            .await
            .unwrap();
        assert_eq!(cur.status(), 200);
        let cur_body: serde_json::Value = cur.json().await.unwrap();
        assert_eq!(cur_body["name"], "SomeGame");
        assert_eq!(cur_body["format_id"], "rpgmaker-mv");
        assert_eq!(cur_body["database_path"], body["database_path"]);
        assert_eq!(cur_body["supported_modes"], body["supported_modes"]);
        assert!(body["persistence_warning"].is_null());
        let saved = AppConfig::load(&state.config_path).unwrap();
        assert_eq!(
            saved.recent_projects[0].database_path.as_ref(),
            Some(&pivot)
        );

        let live = state.db.get_entries(&EntryFilter::default()).unwrap();
        assert_eq!(live.len(), 1);
        assert_eq!(live[0].id, "a");
        assert_eq!(live[0].source, "Hola");
        assert!(live[0].translation.is_none());

        let src = Database::open(&source_path).unwrap();
        let src_rows = src.get_entries(&EntryFilter::default()).unwrap();
        assert_eq!(src_rows.len(), 2);
        assert_eq!(
            src_rows.iter().find(|e| e.id == "a").unwrap().source,
            "Hello"
        );
        assert_eq!(
            src_rows
                .iter()
                .find(|e| e.id == "a")
                .unwrap()
                .translation
                .as_deref(),
            Some("Hola")
        );
        drop(src);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_open_db_rejects_non_locust_and_keeps_current() {
        let (url, _h, state) = setup_with_state().await;
        mark_project_open(&state).await;
        state
            .db
            .save_entries(&[translated("keep", "Hello", "f.json", "Hola")])
            .unwrap();

        let dir_guard = tempfile::Builder::new()
            .prefix("locust_opendb_bad_")
            .tempdir()
            .unwrap();
        let dir = dir_guard.path().to_path_buf();
        state.test_temp_dirs.lock().unwrap().push(dir_guard);
        let other = dir.join("not-locust.db");
        std::fs::write(&other, b"not-a-sqlite-database").unwrap();

        let resp = client()
            .post(format!("{}/api/project/open-db", url))
            .json(&serde_json::json!({
                "database_path": other.to_string_lossy(),
                "game_path": dir.to_string_lossy(),
                "format_id": "rpgmaker-mv"
            }))
            .send()
            .await
            .unwrap();
        assert!(resp.status().is_client_error(), "{}", resp.status());
        assert_ne!(resp.status(), 500);
        let text = resp.text().await.unwrap();
        assert!(
            text.to_lowercase()
                .contains("not a locust project database"),
            "clean error: {text}"
        );

        let cur = client()
            .get(format!("{}/api/project/current", url))
            .send()
            .await
            .unwrap();
        assert_eq!(cur.status(), 200);
        let cur_body: serde_json::Value = cur.json().await.unwrap();
        assert_eq!(cur_body["name"], "test-game");
        assert_eq!(cur_body["format_id"], "rpgmaker-mv");

        let keep = state.db.get_entry("keep").unwrap().unwrap();
        assert_eq!(keep.source, "Hello");
        assert_eq!(keep.translation.as_deref(), Some("Hola"));
        assert_eq!(std::fs::read(&other).unwrap(), b"not-a-sqlite-database");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_open_db_rejects_missing_file() {
        let (url, _h, state) = setup_with_state().await;
        mark_project_open(&state).await;
        state
            .db
            .save_entries(&[translated("keep", "Hello", "f.json", "Hola")])
            .unwrap();

        let missing = std::env::temp_dir().join(format!(
            "locust_opendb_missing_{}.locust.db",
            uuid::Uuid::new_v4()
        ));
        let resp = client()
            .post(format!("{}/api/project/open-db", url))
            .json(&serde_json::json!({
                "database_path": missing.to_string_lossy(),
                "game_path": "/tmp/game",
                "format_id": "rpgmaker-mv"
            }))
            .send()
            .await
            .unwrap();
        assert!(resp.status().is_client_error(), "{}", resp.status());
        assert_ne!(resp.status(), 500);

        let cur = client()
            .get(format!("{}/api/project/current", url))
            .send()
            .await
            .unwrap();
        assert_eq!(cur.status(), 200);
        let cur_body: serde_json::Value = cur.json().await.unwrap();
        assert_eq!(cur_body["name"], "test-game");
        assert!(state.db.get_entry("keep").unwrap().is_some());
    }

    #[tokio::test]
    async fn test_open_db_rejects_unknown_format() {
        let dir_guard = tempfile::Builder::new()
            .prefix("locust_opendb_fmt_")
            .tempdir()
            .unwrap();
        let dir = dir_guard.path().to_path_buf();
        let db_path = write_opendb_locust(
            &dir,
            "ok.locust.db",
            &[translated("a", "Hello", "f.json", "Hola")],
        );

        let (url, _h, state) = setup_with_state().await;
        state.test_temp_dirs.lock().unwrap().push(dir_guard);
        mark_project_open(&state).await;
        state
            .db
            .save_entries(&[translated("keep", "Stay", "f.json", "Queda")])
            .unwrap();

        let resp = client()
            .post(format!("{}/api/project/open-db", url))
            .json(&serde_json::json!({
                "database_path": db_path.to_string_lossy(),
                "game_path": dir.to_string_lossy(),
                "format_id": "not-a-format"
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 422);
        let text = resp.text().await.unwrap();
        assert!(text.contains("not-a-format"), "{text}");

        let cur = client()
            .get(format!("{}/api/project/current", url))
            .send()
            .await
            .unwrap();
        assert_eq!(cur.status(), 200);
        let cur_body: serde_json::Value = cur.json().await.unwrap();
        assert_eq!(cur_body["name"], "test-game");
        assert_eq!(state.db.path(), PathBuf::from(":memory:"));
        assert!(state.db.get_entry("keep").unwrap().is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn open_returns_409_while_translation_in_flight_and_keeps_current_project() {
        let (url, _h, state) = setup_with_state().await;
        mark_project_open(&state).await;
        state
            .db
            .save_entries(&[
                translated("keep-a", "Hello", "f.json", "Hola"),
                StringEntry::new("keep-b", "World", PathBuf::from("f.json")),
            ])
            .unwrap();
        park_job(&state, "job-translate", JobKind::Translate, None);

        let dir_guard = tempfile::Builder::new()
            .prefix("locust_open_busy_")
            .tempdir()
            .unwrap();
        let dir = dir_guard.path().to_path_buf();
        state.test_temp_dirs.lock().unwrap().push(dir_guard);
        let game = dir.join("OtherGame");
        write_rpgmaker_game(&game);

        let resp = client()
            .post(format!("{}/api/project/open", url))
            .json(&serde_json::json!({"path": game.to_string_lossy()}))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 409);
        let text = resp.text().await.unwrap();
        let lower = text.to_lowercase();
        assert!(
            lower.contains("translation") && (lower.contains("cancel") || lower.contains("finish")),
            "user-facing way out: {text}"
        );

        let cur = client()
            .get(format!("{}/api/project/current", url))
            .send()
            .await
            .unwrap();
        assert_eq!(cur.status(), 200);
        let cur_body: serde_json::Value = cur.json().await.unwrap();
        assert_eq!(cur_body["name"], "test-game");
        assert_eq!(cur_body["format_id"], "rpgmaker-mv");

        assert_eq!(state.db.path(), PathBuf::from(":memory:"));
        let rows = state.db.get_entries(&EntryFilter::default()).unwrap();
        assert_eq!(rows.len(), 2);
        assert!(state.db.get_entry("keep-a").unwrap().is_some());
        assert!(state
            .config
            .read()
            .await
            .recent_projects
            .iter()
            .all(|p| p.name != "OtherGame"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn open_returns_409_while_project_exclusive_and_keeps_current_project() {
        // Covers inject, pivot, and validate (all take ProjectExclusiveGuard).
        let (url, _h, state) = setup_with_state().await;
        mark_project_open(&state).await;
        state
            .db
            .save_entries(&[
                translated("keep-a", "Hello", "f.json", "Hola"),
                StringEntry::new("keep-b", "World", PathBuf::from("f.json")),
            ])
            .unwrap();
        let _hold = ProjectExclusiveGuard::enter(&state.project_exclusive);

        let dir_guard = tempfile::Builder::new()
            .prefix("locust_open_excl_")
            .tempdir()
            .unwrap();
        let dir = dir_guard.path().to_path_buf();
        state.test_temp_dirs.lock().unwrap().push(dir_guard);
        let game = dir.join("OtherGame");
        write_rpgmaker_game(&game);

        let resp = client()
            .post(format!("{}/api/project/open", url))
            .json(&serde_json::json!({"path": game.to_string_lossy()}))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 409);
        let text = resp.text().await.unwrap();
        assert_eq!(text, PROJECT_BUSY_MESSAGE);
        let lower = text.to_lowercase();
        assert!(
            lower.contains("project") && lower.contains("finish"),
            "user-facing way out: {text}"
        );

        let cur = client()
            .get(format!("{}/api/project/current", url))
            .send()
            .await
            .unwrap();
        assert_eq!(cur.status(), 200);
        let cur_body: serde_json::Value = cur.json().await.unwrap();
        assert_eq!(cur_body["name"], "test-game");

        assert_eq!(state.db.path(), PathBuf::from(":memory:"));
        let rows = state.db.get_entries(&EntryFilter::default()).unwrap();
        assert_eq!(rows.len(), 2);
        assert!(state.db.get_entry("keep-a").unwrap().is_some());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn open_db_returns_409_while_translation_in_flight_and_keeps_current_project() {
        let (url, _h, state) = setup_with_state().await;
        mark_project_open(&state).await;
        state
            .db
            .save_entries(&[
                translated("keep-a", "Hello", "f.json", "Hola"),
                StringEntry::new("keep-b", "World", PathBuf::from("f.json")),
            ])
            .unwrap();
        park_job(&state, "job-translate", JobKind::Translate, None);

        let dir_guard = tempfile::Builder::new()
            .prefix("locust_opendb_busy_")
            .tempdir()
            .unwrap();
        let dir = dir_guard.path().to_path_buf();
        state.test_temp_dirs.lock().unwrap().push(dir_guard);
        let other = write_opendb_locust(
            &dir,
            "other.locust.db",
            &[translated("other", "Bye", "g.json", "Adiós")],
        );
        let game = dir.join("OtherGame");
        std::fs::create_dir_all(&game).unwrap();

        let resp = client()
            .post(format!("{}/api/project/open-db", url))
            .json(&serde_json::json!({
                "database_path": other.to_string_lossy(),
                "game_path": game.to_string_lossy(),
                "format_id": "rpgmaker-mv"
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 409);
        let text = resp.text().await.unwrap();
        let lower = text.to_lowercase();
        assert!(
            lower.contains("translation") && (lower.contains("cancel") || lower.contains("finish")),
            "user-facing way out: {text}"
        );

        let cur = client()
            .get(format!("{}/api/project/current", url))
            .send()
            .await
            .unwrap();
        assert_eq!(cur.status(), 200);
        let cur_body: serde_json::Value = cur.json().await.unwrap();
        assert_eq!(cur_body["name"], "test-game");

        assert_eq!(state.db.path(), PathBuf::from(":memory:"));
        let rows = state.db.get_entries(&EntryFilter::default()).unwrap();
        assert_eq!(rows.len(), 2);
        assert!(state.db.get_entry("keep-a").unwrap().is_some());
        assert!(state.db.get_entry("other").unwrap().is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn finished_translation_in_retention_window_does_not_block_open() {
        let (url, _h, state) = setup_with_state().await;
        mark_project_open(&state).await;
        park_job(&state, "job-done", JobKind::Translate, Some("completed"));

        let dir_guard = tempfile::Builder::new()
            .prefix("locust_open_done_")
            .tempdir()
            .unwrap();
        let dir = dir_guard.path().to_path_buf();
        state.test_temp_dirs.lock().unwrap().push(dir_guard);
        let game = dir.join("OtherGame");
        write_rpgmaker_game(&game);

        let resp = client()
            .post(format!("{}/api/project/open", url))
            .json(&serde_json::json!({"path": game.to_string_lossy()}))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200, "{}", resp.text().await.unwrap());

        let cur = client()
            .get(format!("{}/api/project/current", url))
            .send()
            .await
            .unwrap();
        assert_eq!(cur.status(), 200);
        let cur_body: serde_json::Value = cur.json().await.unwrap();
        assert_eq!(cur_body["name"], "OtherGame");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn in_flight_patch_job_does_not_block_open() {
        let (url, _h, state) = setup_with_state().await;
        mark_project_open(&state).await;
        park_job(&state, "job-patch", JobKind::Patch, None);

        let dir_guard = tempfile::Builder::new()
            .prefix("locust_open_patch_")
            .tempdir()
            .unwrap();
        let dir = dir_guard.path().to_path_buf();
        state.test_temp_dirs.lock().unwrap().push(dir_guard);
        let game = dir.join("OtherGame");
        write_rpgmaker_game(&game);

        let resp = client()
            .post(format!("{}/api/project/open", url))
            .json(&serde_json::json!({"path": game.to_string_lossy()}))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200, "{}", resp.text().await.unwrap());

        let cur = client()
            .get(format!("{}/api/project/current", url))
            .send()
            .await
            .unwrap();
        assert_eq!(cur.status(), 200);
        let cur_body: serde_json::Value = cur.json().await.unwrap();
        assert_eq!(cur_body["name"], "OtherGame");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
