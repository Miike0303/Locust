use axum::extract::{Path as AxumPath, State, WebSocketUpgrade};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use locust_core::models::ProgressEvent;
use locust_core::translation::{
    run_fallback_chain, unique_provider_chain, TranslationOptionsRequest,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc};

use super::jobs::{cancel_job, upgrade_job_ws};
use crate::{
    err, publish_job_event, try_project_operation, ApiError, AppState, JobKind, JobState,
    PROJECT_BUSY_MESSAGE, TRANSLATION_IN_FLIGHT_MESSAGE,
};

#[derive(Deserialize)]
pub(crate) struct TranslateStartRequest {
    provider_id: String,
    /// Optional ordered fallbacks after the primary (same chain rules as CLI `--fallback`).
    #[serde(default)]
    fallback_provider_ids: Option<Vec<String>>,
    options: TranslationOptionsRequest,
}

#[derive(Serialize)]
pub(crate) struct TranslateStartResponse {
    job_id: String,
}

/// Shared by HTTP `POST /api/translate/start` and the Tauri command.
pub async fn spawn_translation_job(
    state: &AppState,
    provider_id: String,
    fallback_provider_ids: Option<Vec<String>>,
    options: impl Into<TranslationOptionsRequest>,
) -> std::result::Result<String, String> {
    let exclusive = try_project_operation(state)?;
    if state.current_project.read().await.is_none() {
        return Err("no project open".into());
    }
    let options = options
        .into()
        .resolve(&provider_id, state.config.read().await.default_batch_size);
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
    let (broadcast_tx, _) = broadcast::channel::<serde_json::Value>(1000);
    let broadcast_tx_clone = broadcast_tx.clone();

    let cancel = tokio_util::sync::CancellationToken::new();
    let cancel_clone = cancel.clone();
    let job_id_clone = job_id.clone();
    let replay = Arc::new(std::sync::Mutex::new(Vec::new()));
    let replay_bridge = replay.clone();

    // Bridge mpsc → broadcast so WebSocket clients can subscribe.
    // BatchFailed and ProviderSwitched are non-terminal; only Completed / Failed end the stream.
    let jobs = state.active_jobs.clone();
    let cleanup_job_id = job_id.clone();
    tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            let is_terminal = matches!(
                event,
                ProgressEvent::Completed { .. } | ProgressEvent::Failed { .. }
            );
            let json = serde_json::to_value(&event).unwrap_or_default();
            publish_job_event(&broadcast_tx_clone, &replay_bridge, json);
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
            replay,
            cancel,
            kind: JobKind::Translate,
            patch_game_key: None,
        },
    );

    let db = state.db.clone();
    let glossary = state.glossary.clone();
    let resolve_map = Arc::new(resolve_map);
    let handle = tokio::spawn(async move {
        // Abort only requests cancellation. Keep ownership until this future
        // actually exits/drops, even while a provider is executing synchronously.
        let _exclusive = exclusive;
        let map = resolve_map;
        let resolve = |id: &str| map.get(id).cloned();
        let result = run_fallback_chain(
            &chain,
            &resolve,
            db,
            glossary,
            options,
            tx.clone(),
            job_id_clone,
            cancel_clone,
        )
        .await;
        if let Err(error) = result {
            let _ = tx
                .send(ProgressEvent::Failed {
                    entry_id: None,
                    error: error.to_string(),
                })
                .await;
        }
    });

    // Update with real abort handle
    if let Some(mut job) = state.active_jobs.get_mut(&job_id) {
        job.abort_handle = handle.abort_handle();
    }

    Ok(job_id)
}

pub(crate) async fn translate_start(
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
    .map_err(|m| {
        let status = if m == PROJECT_BUSY_MESSAGE || m == TRANSLATION_IN_FLIGHT_MESSAGE {
            StatusCode::CONFLICT
        } else if m == "no project open" {
            StatusCode::BAD_REQUEST
        } else {
            StatusCode::NOT_FOUND
        };
        err(status, m)
    })?;
    Ok(Json(TranslateStartResponse { job_id }))
}

pub(crate) async fn translate_cancel(
    State(state): State<Arc<AppState>>,
    AxumPath(job_id): AxumPath<String>,
) -> Result<StatusCode, ApiError> {
    cancel_job(&state, &job_id)
}

/// Desktop cancellation uses the same token and terminal replay as HTTP.
/// Acknowledgement is immediate; the worker still owns its project reservation.
pub fn cancel_translation_job(state: &AppState, job_id: &str) -> Result<(), String> {
    cancel_job(state, job_id)
        .map(|_| ())
        .map_err(|(_, message)| message)
}

pub(crate) async fn translate_ws(
    State(state): State<Arc<AppState>>,
    AxumPath(job_id): AxumPath<String>,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    upgrade_job_ws(
        state,
        job_id,
        ws,
        serde_json::json!({"type": "failed", "error": "job not found"}),
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routes::test_support::*;
    use locust_core::models::StringEntry;
    use locust_core::translation::TranslationOptions;
    use std::path::PathBuf;

    #[tokio::test]
    async fn translation_start_without_project_keeps_stale_rows_untouched() {
        let (url, _h, state) = setup_with_state().await;
        state
            .db
            .save_entries(&[StringEntry::new(
                "stale",
                "Old project text",
                PathBuf::from("story.html"),
            )])
            .unwrap();

        let response = client()
            .post(format!("{url}/api/translate/start"))
            .json(&serde_json::json!({
                "provider_id": "mock",
                "options": TranslationOptions::default()
            }))
            .send()
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(response.text().await.unwrap(), "no project open");
        assert!(state.active_jobs.is_empty());
        assert!(state
            .db
            .get_entry("stale")
            .unwrap()
            .unwrap()
            .translation
            .is_none());
    }

    #[tokio::test]
    async fn test_translate_start_returns_job_id() {
        let (url, _h, state) = setup_with_state().await;
        mark_project_open(&state).await;
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

    #[test]
    fn provider_translation_defaults_wire_options() {
        for (provider, expected) in [
            ("google", (200, 4, Some(40_000))),
            ("antigravity", (100, 4, Some(20_000))),
            ("grok", (10, 10, Some(6000))),
            ("grok-sub", (10, 10, Some(6000))),
            ("mock", (40, 1, Some(6000))),
        ] {
            for explicit in [false, true] {
                let mut options = serde_json::json!({
                    "source_lang": "ja", "target_lang": "en",
                    "use_glossary": false, "use_memory": false, "skip_approved": true
                });
                if explicit {
                    options["batch_size"] = 7.into();
                    options["max_concurrent"] = 2.into();
                    options["max_batch_tokens"] = 900.into();
                }
                let params: TranslateStartRequest = serde_json::from_value(
                    serde_json::json!({"provider_id": provider, "options": options}),
                )
                .unwrap();
                let resolved = params.options.resolve(&params.provider_id, None);
                assert_eq!(
                    (
                        resolved.batch_size,
                        resolved.max_concurrent,
                        resolved.max_batch_tokens
                    ),
                    if explicit {
                        (7, 2, Some(900))
                    } else {
                        expected
                    },
                    "{provider}"
                );
            }
        }
    }
    #[tokio::test]
    async fn provider_translation_defaults_accept_omitted_request_fields() {
        let (url, _h, state) = setup_with_state().await;
        mark_project_open(&state).await;
        let response = client()
            .post(format!("{url}/api/translate/start"))
            .json(&serde_json::json!({
                "provider_id": "mock",
                "options": {
                    "source_lang": "ja", "target_lang": "en",
                    "use_glossary": true, "use_memory": true, "skip_approved": true
                }
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200, "{}", response.text().await.unwrap());
    }

    #[tokio::test]
    async fn test_translate_start_accepts_fallback_provider_ids() {
        let (url, _h, state) = setup_with_state().await;
        mark_project_open(&state).await;
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
        mark_project_open(&state).await;
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
        mark_project_open(&state).await;
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
}
