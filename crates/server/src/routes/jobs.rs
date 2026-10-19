use axum::extract::ws::{Message, WebSocket};
use axum::extract::WebSocketUpgrade;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast;

use crate::{err, job_event_is_terminal, publish_job_event, ApiError, AppState, JobKind};

pub(crate) fn cancel_job(state: &AppState, job_id: &str) -> Result<StatusCode, ApiError> {
    if let Some(job) = state.active_jobs.get(job_id) {
        let already_done = job
            .replay
            .lock()
            .map(|log| log.iter().any(job_event_is_terminal))
            .unwrap_or(false);
        if already_done {
            return Ok(StatusCode::OK);
        }
    }
    // Preserve the patch endpoint's existing removal contract. Patch workers
    // have their own game-folder lock and do not use the live project database.
    let is_patch = state
        .active_jobs
        .get(job_id)
        .is_some_and(|job| matches!(job.kind, JobKind::Patch));
    if is_patch {
        if let Some((_, job)) = state.active_jobs.remove(job_id) {
            publish_job_event(
                &job.progress_tx,
                &job.replay,
                serde_json::json!({"type": "error", "message": "cancelled"}),
            );
            job.cancel.cancel();
            job.abort_handle.abort();
            return Ok(StatusCode::OK);
        }
        return Err(err(StatusCode::NOT_FOUND, "job not found"));
    }
    if let Some(job) = state.active_jobs.get(job_id) {
        let terminal = match job.kind {
            JobKind::Translate => {
                serde_json::json!({"type": "failed", "entry_id": null, "error": "cancelled"})
            }
            JobKind::Patch => serde_json::json!({"type": "error", "message": "cancelled"}),
        };
        publish_job_event(&job.progress_tx, &job.replay, terminal);
        job.cancel.cancel();
        // A translation may be awaiting a SQLite spawn_blocking write, which
        // cannot be aborted. Let cooperative cancellation drain those writes
        // before its worker releases the project guard.
        Ok(StatusCode::OK)
    } else {
        Err(err(StatusCode::NOT_FOUND, "job not found"))
    }
}

pub(crate) async fn upgrade_job_ws(
    state: Arc<AppState>,
    job_id: String,
    ws: WebSocketUpgrade,
    not_found: serde_json::Value,
) -> impl IntoResponse {
    // Retry briefly to handle race condition where WS connects before job insert completes
    let mut found = None;
    for _ in 0..20 {
        if let Some(job) = state.active_jobs.get(&job_id) {
            let rx = job.progress_tx.subscribe();
            let replay = job.replay.lock().unwrap_or_else(|e| e.into_inner()).clone();
            found = Some((rx, replay));
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }

    ws.on_upgrade(move |socket| handle_job_ws(socket, found, not_found))
}

async fn handle_job_ws(
    mut socket: WebSocket,
    found: Option<(
        broadcast::Receiver<serde_json::Value>,
        Vec<serde_json::Value>,
    )>,
    not_found: serde_json::Value,
) {
    let Some((mut rx, replay)) = found else {
        let _ = socket.send(Message::Text(not_found.to_string())).await;
        close_job_ws(&mut socket).await;
        return;
    };

    let mut seen = std::collections::HashSet::new();
    for event in replay {
        let key = event.to_string();
        seen.insert(key);
        if socket.send(Message::Text(event.to_string())).await.is_err() {
            close_job_ws(&mut socket).await;
            return;
        }
        if job_event_is_terminal(&event) {
            close_job_ws(&mut socket).await;
            return;
        }
    }

    loop {
        match rx.recv().await {
            Ok(event) => {
                if !seen.insert(event.to_string()) {
                    continue;
                }
                let is_terminal = job_event_is_terminal(&event);
                if socket.send(Message::Text(event.to_string())).await.is_err() {
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
    close_job_ws(&mut socket).await;
}

async fn close_job_ws(socket: &mut WebSocket) {
    // Axum's consuming close() only sends Close. Dropping TCP with unread
    // inbound frames can reset the connection on Windows and lose the terminal
    // event at the peer. Retain the socket until the peer acknowledges Close.
    // Bound both the close write and the entire drain for unresponsive clients.
    let _ = tokio::time::timeout(Duration::from_secs(2), async {
        let _ = socket.send(Message::Close(None)).await;
        while let Some(Ok(message)) = socket.recv().await {
            if matches!(message, Message::Close(_)) {
                break;
            }
        }
    })
    .await;
}

#[cfg(test)]
#[path = "../job_ws_tests.rs"]
mod job_ws_tests;
