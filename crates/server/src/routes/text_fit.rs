use std::sync::Arc;

use axum::{extract::State, http::StatusCode, Json};
use locust_core::text_fit::{estimate_entry, TextFitRequest, TextFitResponse};

use crate::{err, try_project_operation, ApiError, AppState};

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
/// Snapshot one row under project admission, then release it before font I/O.
pub async fn estimate_text_fit(
    state: &AppState,
    request: TextFitRequest,
) -> Result<TextFitResponse, String> {
    let entry = {
        let _exclusive = try_project_operation(state)?;
        if state.current_project.read().await.is_none() {
            return Err("no project open".into());
        }
        state
            .db
            .get_entry(&request.entry_id)
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
    use crate::routes::test_support::{client, mark_project_open, setup_with_state};
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
