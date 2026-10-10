use std::sync::Arc;

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

#[tokio::test]
async fn stale_save_preserves_other_clients_translation_and_status() {
    let f = Fixture::new(Some("V0")).await;
    let a = f.read().await;
    let b = f.read().await;
    assert_eq!(a["translation"], b["translation"]);
    assert_eq!(f.patch(json!({"translation":"V1", "expected_translation":b["translation"], "status":"approved"})).await.status(), 200);
    let before = serde_json::to_value(f.state.db.get_entry("row").unwrap().unwrap()).unwrap();
    let response = f.patch(json!({"translation":"A draft", "expected_translation":a["translation"], "status":"pending"})).await;
    assert_eq!(response.status(), 409);
    assert_eq!(
        response.text().await.unwrap(),
        "translation changed since it was loaded"
    );
    assert_eq!(
        serde_json::to_value(f.state.db.get_entry("row").unwrap().unwrap()).unwrap(),
        before
    );
    assert_eq!(
        f.patch(
            json!({"translation":"A resolved", "expected_translation":"V1", "status":"reviewed"})
        )
        .await
        .status(),
        200
    );
    let saved = f.read().await;
    assert_eq!(saved["translation"], "A resolved");
    assert_eq!(saved["status"], "reviewed");
}

#[tokio::test]
async fn omitted_guard_and_status_only_requests_keep_legacy_behavior() {
    let f = Fixture::new(Some("V1")).await;
    assert_eq!(f.patch(json!({"translation":"legacy"})).await.status(), 200);
    assert_eq!(f.patch(json!({"status":"approved"})).await.status(), 200);
    let saved = f.read().await;
    assert_eq!(saved["translation"], "legacy");
    assert_eq!(saved["status"], "approved");
}

#[tokio::test]
async fn expected_null_is_a_guard_and_is_distinct_from_empty_string() {
    let f = Fixture::new(None).await;
    assert_eq!(
        f.patch(json!({"translation":"", "expected_translation":null}))
            .await
            .status(),
        200
    );
    assert_eq!(
        f.patch(json!({"translation":"stale empty", "expected_translation":null}))
            .await
            .status(),
        409
    );
    assert_eq!(f.read().await["translation"], "");
    assert_eq!(
        f.patch(json!({"translation":"filled", "expected_translation":""}))
            .await
            .status(),
        200
    );
    assert_eq!(
        f.patch(json!({"translation":"stale", "expected_translation":null, "status":"pending"}))
            .await
            .status(),
        409
    );
    assert_eq!(f.read().await["translation"], "filled");
}

#[tokio::test]
async fn guarded_unknown_entry_is_not_a_conflict() {
    let f = Fixture::new(None).await;
    let response = f
        .client
        .patch(f.url.replace("/row", "/missing"))
        .json(&json!({"translation":"draft", "expected_translation":null}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 404);
}

#[tokio::test]
async fn separate_database_connections_cannot_both_win_the_same_baseline() {
    use locust_core::database::{Database, TranslationSaveOutcome};
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("project.db");
    let a = Database::open(&path).unwrap();
    let b = Database::open(&path).unwrap();
    let mut row = StringEntry::new("row", "Source", temp.path().join("story.html"));
    row.translation = Some("V0".into());
    a.save_entries(&[row]).unwrap();
    let (left, right) = tokio::join!(
        a.save_translation_if_unchanged("row", "A", "manual", Some(Some("V0".into()))),
        b.save_translation_if_unchanged("row", "B", "manual", Some(Some("V0".into())))
    );
    let outcomes = [left.unwrap(), right.unwrap()];
    assert_eq!(
        outcomes
            .iter()
            .filter(|&&o| o == TranslationSaveOutcome::Updated)
            .count(),
        1
    );
    assert_eq!(
        outcomes
            .iter()
            .filter(|&&o| o == TranslationSaveOutcome::Conflict)
            .count(),
        1
    );
    let winner = if outcomes[0] == TranslationSaveOutcome::Updated {
        "A"
    } else {
        "B"
    };
    assert_eq!(
        a.get_entry("row").unwrap().unwrap().translation.as_deref(),
        Some(winner)
    );
}
