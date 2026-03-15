use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{Path as AxumPath, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use locust_core::backup::BackupManager;
use locust_core::database::Database;
use locust_core::extraction::{FormatRegistry, MultiLangInjector, MultiLangReport};
use locust_core::models::OutputMode;

pub struct AppState {
    pub registry: Arc<FormatRegistry>,
    pub db: Arc<Database>,
    pub backup_manager: Arc<BackupManager>,
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/formats", get(list_formats))
        .route("/api/formats/{id}/modes", get(get_format_modes))
        .route("/api/inject", post(inject))
        .with_state(state)
}

async fn list_formats(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let formats = state.registry.list();
    Json(serde_json::to_value(formats).unwrap_or_default())
}

#[derive(Serialize)]
struct FormatModes {
    format_id: String,
    supported_modes: Vec<OutputMode>,
}

async fn get_format_modes(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<FormatModes>, (axum::http::StatusCode, String)> {
    let plugin = state
        .registry
        .get(&id)
        .ok_or_else(|| {
            (
                axum::http::StatusCode::NOT_FOUND,
                format!("format not found: {}", id),
            )
        })?;

    Ok(Json(FormatModes {
        format_id: id,
        supported_modes: plugin.supported_modes(),
    }))
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
) -> Result<Json<MultiLangReport>, (axum::http::StatusCode, String)> {
    let injector = MultiLangInjector::new(
        state.registry.clone(),
        state.db.clone(),
        state.backup_manager.clone(),
    );

    let (tx, mut rx) = mpsc::channel(100);

    // Drain events in background
    tokio::spawn(async move {
        while rx.recv().await.is_some() {}
    });

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
        .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(report))
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_placeholder() {
        assert_eq!(1, 1);
    }
}
