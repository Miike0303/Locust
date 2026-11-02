use axum::extract::ws::{Message, WebSocket};
use axum::extract::WebSocketUpgrade;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast;

use crate::{
    err, job_event_is_terminal, publish_sequenced_job_event as publish_job_event, ApiError,
    AppState, JobKind,
};

struct JobSubscription {
    rx: broadcast::Receiver<serde_json::Value>,
    snapshot: Vec<serde_json::Value>,
    replay: Arc<std::sync::Mutex<Vec<serde_json::Value>>>,
    include_sequence: bool,
}

impl JobSubscription {
    fn subscribe(
        tx: &broadcast::Sender<serde_json::Value>,
        replay: &Arc<std::sync::Mutex<Vec<serde_json::Value>>>,
        kind: JobKind,
    ) -> Self {
        // Subscribe before snapshotting; sequence watermarks remove overlap.
        let rx = tx.subscribe();
        let snapshot = replay.lock().unwrap_or_else(|e| e.into_inner()).clone();
        Self {
            rx,
            snapshot,
            replay: replay.clone(),
            include_sequence: matches!(kind, JobKind::Translate),
        }
    }
}

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
    if let Some(job) = state.active_jobs.get(job_id) {
        if matches!(job.kind, JobKind::Translate) {
            publish_job_event(
                &job.progress_tx,
                &job.replay,
                serde_json::json!({"type": "failed", "entry_id": null, "error": "cancelled"}),
            );
        }
        // Patch workers publish their actual result after cooperative cancellation
        // drains the blocking apply. Retain the supervisor and replay entry.
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
            found = Some(JobSubscription::subscribe(
                &job.progress_tx,
                &job.replay,
                job.kind,
            ));
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }

    ws.on_upgrade(move |socket| handle_job_ws(socket, found, not_found))
}

async fn handle_job_ws(
    mut socket: WebSocket,
    found: Option<JobSubscription>,
    not_found: serde_json::Value,
) {
    let Some(JobSubscription {
        mut rx,
        snapshot,
        replay,
        include_sequence,
    }) = found
    else {
        send_job_ws(&mut socket, Message::Text(not_found.to_string())).await;
        close_job_ws(&mut socket).await;
        return;
    };

    let mut last_sent = 0;
    let mut pending = snapshot;
    loop {
        for mut event in pending.drain(..) {
            // Unsequenced frames only occur in legacy lifecycle test fixtures.
            if let Some(seq) = event.get("seq").and_then(|seq| seq.as_u64()) {
                if seq <= last_sent {
                    continue;
                }
                last_sent = seq;
            }
            // Translation sessions use stable JSON identity across reconnects.
            // Patch consumers need only internal dedupe; preserve their schema.
            if !include_sequence {
                if let Some(fields) = event.as_object_mut() {
                    fields.remove("seq");
                }
            }
            let sent = send_job_ws(&mut socket, Message::Text(event.to_string())).await;
            if !sent || job_event_is_terminal(&event) {
                close_job_ws(&mut socket).await;
                return;
            }
        }
        tokio::select! {
            message = socket.recv() => match message {
                Some(Ok(Message::Close(_))) => {
                    // Receiving Close queues Axum's reply. Drive that reply to
                    // completion before dropping TCP, without waiting on the job.
                    close_job_ws(&mut socket).await;
                    return;
                }
                Some(Ok(Message::Ping(data))) => {
                    // Replace Axum's queued automatic Pong with the same payload
                    // and flush it under our send deadline (no duplicate Pong).
                    if !send_job_ws(&mut socket, Message::Pong(data)).await {
                        close_job_ws(&mut socket).await;
                        return;
                    }
                }
                Some(Ok(_)) => {} // Client text/binary/Pong carries no job commands.
                Some(Err(error)) => {
                    tracing::debug!(%error, "Job WebSocket receive failed");
                    return;
                }
                None => return,
            },
            event = rx.recv() => match event {
                Ok(event) => pending.push(event),
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!("WS client lagged by {} messages", n);
                    // A live ring can lose informational frames or a terminal.
                    // Retain the replay Arc even after active_jobs cleanup, and
                    // resume from compact state without redelivering the prefix.
                    pending = replay.lock().unwrap_or_else(|e| e.into_inner()).clone();
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        };
    }
    close_job_ws(&mut socket).await;
}

async fn send_job_ws(socket: &mut WebSocket, message: Message) -> bool {
    // Disconnect only this subscriber when its peer stops reading. The worker
    // and retained replay continue independently of the socket's lifetime.
    match tokio::time::timeout(Duration::from_secs(2), socket.send(message)).await {
        Ok(Ok(())) => true,
        Ok(Err(error)) => {
            tracing::warn!(%error, "Job WebSocket send failed");
            false
        }
        Err(_) => {
            tracing::warn!("Job WebSocket send timed out");
            false
        }
    }
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
