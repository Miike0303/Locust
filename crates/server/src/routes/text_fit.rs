use std::{path::Path, sync::Arc};

use axum::{extract::State, http::StatusCode, Json};
use locust_core::text_fit::{estimate_entry, TextFitRequest, TextFitResponse};

use crate::{err, ApiError, AppState};

pub(crate) async fn text_fit(
    State(state): State<Arc<AppState>>,
    Json(request): Json<TextFitRequest>,
) -> Result<Json<TextFitResponse>, ApiError> {
    estimate_text_fit(&state, request)
        .await
        .map(Json)
        .map_err(|message| err(StatusCode::BAD_REQUEST, message))
}

/// Shared read-only implementation for HTTP and the direct Tauri command.
/// Snapshot one project-bound row without job admission; release both project
/// and connection reads before font I/O.
pub async fn estimate_text_fit(
    state: &AppState,
    request: TextFitRequest,
) -> Result<TextFitResponse, String> {
    let entry = {
        let project_guard = state.current_project.read().await;
        let project = project_guard
            .as_ref()
            .ok_or_else(|| "no project open".to_owned())?;
        // Opens swap the connection before publishing ProjectInfo. Verify its
        // path under the same DB mutex as the row read, so that gap is an error.
        // Unpersisted/in-memory projects must never accept a disk connection.
        let database_path = project
            .database_path
            .as_deref()
            .unwrap_or(Path::new(":memory:"));
        state
            .db
            .get_entry_for_project(database_path, &request.entry_id)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "entry not found".to_owned())?
    };
    let cache = state.text_fit_cache.clone();
    tokio::task::spawn_blocking(move || {
        let mut cache = cache.lock().map_err(|error| error.to_string())?;
        estimate_entry(&entry, &request, &mut cache)
    })
    .await
    .map_err(|error| format!("text estimate task failed: {error}"))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routes::test_support::{client, mark_project_open, park_job, setup_with_state};
    use crate::{create_test_state_with_db, try_project_operation, JobKind, ProjectInfo};
    use locust_core::database::Database;
    use locust_core::models::{StringEntry, StringStatus};
    use serde_json::{json, Value};

    fn row() -> StringEntry {
        let mut entry = StringEntry::new(
            "owned#event_1#page_0#cmd_1#msg",
            "あ\nHello\n",
            "arbitrary.asset".into(),
        )
        .with_format_id("rpgmaker-mv")
        .with_tags(vec!["dialogue".into()]);
        entry.translation = Some("Saved".into());
        entry.status = StringStatus::Reviewed;
        entry
    }

    fn request(id: &str) -> TextFitRequest {
        TextFitRequest {
            entry_id: id.into(),
            font_path: std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../cli/tests/fixtures/synthetic-ascii.ttf"),
            font_size_px: 28.0,
            usable_width_px: 56.0,
            draft_text: Some("Hola\n WWW ".into()),
        }
    }

    fn snapshot(state: &AppState, id: &str) -> Value {
        json!({
            "row": state.db.get_entry(id).unwrap(),
            "history": state.db.get_string_history(id, 50).unwrap(),
            "stats": state.db.get_stats().unwrap(),
        })
    }

    #[tokio::test]
    async fn http_matches_core_and_preserves_database_and_text() {
        let (base, server, state) = setup_with_state().await;
        mark_project_open(&state).await;
        let entry = row();
        state.db.save_entries(std::slice::from_ref(&entry)).unwrap();
        let before = snapshot(&state, &entry.id);
        let request = request(&entry.id);
        let expected = estimate_entry(&entry, &request, &mut Default::default()).unwrap();
        let response = client()
            .post(format!("{base}/api/text-fit"))
            .json(&request)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let body: Value = response.json().await.unwrap();
        assert_eq!(body, serde_json::to_value(expected).unwrap());
        assert_eq!(body["source"][0]["status"], "unavailable");
        assert_eq!(body["draft"][0]["advance_px"], 56.0);
        assert_eq!(body["draft"][1]["status"], "exceeds_width");
        assert_eq!(snapshot(&state, &entry.id), before);
        server.abort();
    }

    #[tokio::test]
    async fn http_and_shared_estimates_succeed_during_translation_reservation() {
        let (base, server, state) = setup_with_state().await;
        mark_project_open(&state).await;
        let entry = row();
        state.db.save_entries(std::slice::from_ref(&entry)).unwrap();
        let before = snapshot(&state, &entry.id);
        let request = request(&entry.id);
        let expected = estimate_entry(&entry, &request, &mut Default::default()).unwrap();
        let _operation = try_project_operation(&state).unwrap();
        park_job(&state, "translation", JobKind::Translate, None);

        assert_eq!(
            estimate_text_fit(&state, request.clone()).await.unwrap(),
            expected
        );
        let response = client()
            .post(format!("{base}/api/text-fit"))
            .json(&request)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        assert_eq!(response.json::<Value>().await.unwrap(), json!(expected));
        assert_eq!(snapshot(&state, &entry.id), before);
        server.abort();
    }

    async fn project_pair() -> (tempfile::TempDir, Arc<AppState>, ProjectInfo, StringEntry) {
        let dir = tempfile::tempdir().unwrap();
        let path_a = dir.path().join("a.db");
        let path_b = dir.path().join("b.db");
        let state = create_test_state_with_db(&path_a);
        let mut entry = row();
        entry.source = "iii".into();
        state.db.save_entries(std::slice::from_ref(&entry)).unwrap();
        *state.current_project.write().await = Some(ProjectInfo {
            path: dir.path().join("game-a"),
            database_path: Some(path_a),
            ..Default::default()
        });
        let mut other = entry.clone();
        other.source = "WWWWWWW".into();
        other.translation = Some("Other project".into());
        Database::open(&path_b)
            .unwrap()
            .save_entries(&[other])
            .unwrap();
        let project_b = ProjectInfo {
            path: dir.path().join("game-b"),
            database_path: Some(path_b),
            ..Default::default()
        };
        (dir, state, project_b, entry)
    }

    #[tokio::test]
    async fn http_and_shared_reject_connection_swapped_before_project_publication() {
        let (_dir, state, project_b, entry) = project_pair().await;
        let (base, server) = crate::start_test_server(state.clone()).await;
        // Force the real open ordering: replace the live connection, then publish
        // ProjectInfo later. Both databases deliberately contain the same row ID.
        let _operation = try_project_operation(&state).unwrap();
        state
            .db
            .reopen(project_b.database_path.as_ref().unwrap())
            .unwrap();
        let request = request(&entry.id);
        let error = estimate_text_fit(&state, request.clone())
            .await
            .unwrap_err();
        assert_eq!(error, "project changed while reading; retry");
        let response = client()
            .post(format!("{base}/api/text-fit"))
            .json(&request)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 400);
        assert_eq!(response.text().await.unwrap(), error);
        *state.current_project.write().await = Some(project_b);
        assert!(estimate_text_fit(&state, request).await.is_ok());
        server.abort();
    }

    #[tokio::test]
    async fn snapshot_survives_project_switch_while_font_work_waits() {
        use std::future::Future;
        use std::task::Poll;
        use std::time::Duration;

        let (_dir, state, project_b, entry) = project_pair().await;
        let request = request(&entry.id);
        let expected = estimate_entry(&entry, &request, &mut Default::default()).unwrap();
        let cache = state.text_fit_cache.clone();
        let (locked_tx, locked_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let font_blocker = tokio::task::spawn_blocking(move || {
            let _guard = cache.lock().unwrap();
            locked_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        });
        locked_rx.await.unwrap();
        let mut estimate = std::pin::pin!(estimate_text_fit(&state, request));
        // An uncontended project read completes in this poll. The font worker
        // then waits on the cache, keeping the estimate pending after its snapshot.
        std::future::poll_fn(|cx| {
            assert!(estimate.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;

        let switching = state.clone();
        tokio::time::timeout(
            Duration::from_secs(5),
            tokio::task::spawn_blocking(move || {
                let _operation = try_project_operation(&switching).unwrap();
                switching
                    .db
                    .reopen(project_b.database_path.as_ref().unwrap())
                    .unwrap();
                *switching.current_project.blocking_write() = Some(project_b);
            }),
        )
        .await
        .expect("font work must not retain connection or project locks")
        .unwrap();
        assert_ne!(
            state.db.get_entry(&entry.id).unwrap().unwrap().source,
            entry.source
        );
        release_tx.send(()).unwrap();
        font_blocker.await.unwrap();
        assert_eq!(estimate.await.unwrap(), expected);
    }

    #[tokio::test]
    async fn http_rejects_closed_missing_and_ineligible_rows() {
        let (base, server, state) = setup_with_state().await;
        let entry = row();
        state.db.save_entries(std::slice::from_ref(&entry)).unwrap();
        let endpoint = format!("{base}/api/text-fit");
        let response = client()
            .post(&endpoint)
            .json(&request(&entry.id))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 400);
        assert_eq!(response.text().await.unwrap(), "no project open");
        mark_project_open(&state).await;
        let response = client()
            .post(&endpoint)
            .json(&request("missing"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 400);
        assert_eq!(response.text().await.unwrap(), "entry not found");
        for variant in 0..4 {
            let mut excluded = row();
            match variant {
                0 => excluded.metadata.clear(),
                1 => {
                    excluded.tags = vec!["scroll_text".into()];
                }
                2 => {
                    excluded.id = "owned#1#message1".into();
                }
                _ => {
                    excluded = excluded.with_format_id("rpgmaker-vxa");
                }
            }
            state
                .db
                .save_entries(std::slice::from_ref(&excluded))
                .unwrap();
            let response = client()
                .post(&endpoint)
                .json(&request(&excluded.id))
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), 400);
        }
        server.abort();
    }

    #[tokio::test]
    async fn http_unavailable_font_and_input_limit_are_read_only() {
        let (base, server, state) = setup_with_state().await;
        mark_project_open(&state).await;
        let entry = row();
        state.db.save_entries(std::slice::from_ref(&entry)).unwrap();
        let before = snapshot(&state, &entry.id);
        let mut request = request(&entry.id);
        request.font_path = "".into();
        let response = client()
            .post(format!("{base}/api/text-fit"))
            .json(&request)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let body: Value = response.json().await.unwrap();
        assert!(body["draft"]
            .as_array()
            .unwrap()
            .iter()
            .all(|line| line["status"] == "unavailable" && line["advance_px"].is_null()));
        request.draft_text = Some("A".repeat(16_385));
        let response = client()
            .post(format!("{base}/api/text-fit"))
            .json(&request)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 400);
        assert_eq!(snapshot(&state, &entry.id), before);
        server.abort();
    }
}
