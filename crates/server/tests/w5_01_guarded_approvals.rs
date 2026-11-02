use std::sync::Arc;

use locust_core::database::TRANSLATION_CONFLICT_MESSAGE;
use locust_core::models::StringEntry;
use locust_server::{create_test_state, start_test_server, AppState};
use reqwest::{Client, Response};
use serde_json::{json, Value};

struct Fixture {
    state: Arc<AppState>,
    client: Client,
    url: String,
    server: tokio::task::JoinHandle<()>,
    _temp: tempfile::TempDir,
}

impl Fixture {
    async fn new(translation: Option<&str>) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let state = create_test_state();
        let mut row = StringEntry::new("row", "Source", temp.path().join("story.html"));
        row.translation = translation.map(str::to_owned);
        state.db.save_entries(&[row]).unwrap();
        *state.current_project.write().await = Some(
            serde_json::from_value(json!({
                "path": temp.path(), "format_id": "html-game", "name": "Fixture"
            }))
            .unwrap(),
        );
        let (base, server) = start_test_server(state.clone()).await;
        Self {
            state,
            client: Client::new(),
            url: format!("{base}/api/strings/row"),
            server,
            _temp: temp,
        }
    }

    async fn patch(&self, body: Value) -> Response {
        self.client
            .patch(&self.url)
            .json(&body)
            .send()
            .await
            .unwrap()
    }

    async fn read(&self) -> Value {
        self.client
            .get(&self.url)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}

async fn approval_conflict(edited: bool) {
    let f = Fixture::new(Some("V1")).await;
    let loaded = f.read().await;
    assert_eq!(
        f.patch(json!({"translation":"V2", "status":"reviewed"}))
            .await
            .status(),
        200
    );
    let before = f.read().await;
    let history = serde_json::to_value(f.state.db.get_string_history("row", 50).unwrap()).unwrap();
    let mut approval = json!({"status":"approved", "expected_translation":loaded["translation"]});
    if edited {
        approval["translation"] = json!("V-review");
    }
    let response = f.patch(approval).await;
    assert_eq!(response.status(), 409);
    assert_eq!(response.text().await.unwrap(), TRANSLATION_CONFLICT_MESSAGE);
    assert_eq!(f.read().await, before);
    assert_eq!(before["translation"], "V2");
    assert_eq!(before["status"], "reviewed");
    assert_eq!(
        serde_json::to_value(f.state.db.get_string_history("row", 50).unwrap()).unwrap(),
        history
    );
}

#[tokio::test]
async fn edited_approval_rejects_a_newer_translation() {
    approval_conflict(true).await;
}

#[tokio::test]
async fn unchanged_approval_rejects_a_newer_translation() {
    approval_conflict(false).await;
}

#[tokio::test]
async fn status_guards_match_null_empty_and_text_and_preserve_provider() {
    for translation in [None, Some(""), Some("V1")] {
        let f = Fixture::new(translation).await;
        if let Some(text) = translation {
            f.state
                .db
                .save_translation("row", text, "original-provider")
                .await
                .unwrap();
        }
        let before = f.read().await;
        let wrong = if translation.is_none() {
            json!("")
        } else {
            Value::Null
        };
        assert_eq!(
            f.patch(json!({"status":"approved", "expected_translation":wrong}))
                .await
                .status(),
            409
        );
        assert_eq!(f.read().await, before);
        assert_eq!(
            f.patch(json!({"status":"approved", "expected_translation":translation}))
                .await
                .status(),
            200
        );
        let saved = f.read().await;
        assert_eq!(saved["translation"], before["translation"]);
        assert_eq!(saved["provider_used"], before["provider_used"]);
        assert_eq!(saved["translated_at"], before["translated_at"]);
        assert_eq!(saved["status"], "approved");
    }
}

#[tokio::test]
async fn status_without_guard_is_legacy_and_missing_is_not_conflict() {
    let f = Fixture::new(Some("V2")).await;
    assert_eq!(f.patch(json!({"status":"approved"})).await.status(), 200);
    assert_eq!(f.read().await["status"], "approved");
    let response = f
        .client
        .patch(f.url.replace("/row", "/missing"))
        .json(&json!({"status":"approved", "expected_translation":"V1"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 404);
}
