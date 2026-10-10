use locust_core::models::{StringEntry, StringStatus};
use locust_server::{create_test_state, start_test_server};
use serde_json::json;

#[tokio::test]
async fn batch_reports_conflicts_without_409_and_preserves_intervening_edits() {
    let state = create_test_state();
    let mut rows = Vec::new();
    for id in ["changed", "matching", "legacy", "null", "empty"] {
        let mut row = StringEntry::new(id, "Source", "story.html".into());
        row.translation = match id {
            "null" => None,
            "empty" => Some(String::new()),
            _ => Some("old text".into()),
        };
        rows.push(row);
    }
    state.db.save_entries(&rows).unwrap();
    *state.current_project.write().await = Some(
        serde_json::from_value(
            json!({"path":"/fixture", "format_id":"html-game", "name":"Fixture"}),
        )
        .unwrap(),
    );
    let (base, server) = start_test_server(state.clone()).await;
    let client = reqwest::Client::new();
    let loaded: serde_json::Value = client
        .get(format!("{base}/api/strings/changed"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let saved = client
        .patch(format!("{base}/api/strings/changed"))
        .json(&json!({"translation":"desktop edit", "status":"approved"}))
        .send()
        .await
        .unwrap();
    assert_eq!(saved.status(), 200);
    let before = serde_json::to_value(state.db.get_entry("changed").unwrap().unwrap()).unwrap();
    let response = client.post(format!("{base}/api/strings/batch"))
        .json(&json!({"provider":"search-replace", "updates":[
            {"id":"changed", "translation":"new text", "expected_translation":loaded["translation"]},
            {"id":"matching", "translation":"new text", "expected_translation":"old text"},
            {"id":"legacy", "translation":"legacy write"},
            {"id":"null", "translation":"filled", "expected_translation":null},
            {"id":"empty", "translation":"must not fill", "expected_translation":null},
            {"id":"missing", "translation":"unknown", "expected_translation":null}
        ]})).send().await.unwrap();
    assert_eq!(
        response.status(),
        200,
        "row conflicts are a successful partial batch, not HTTP 409"
    );
    let report: serde_json::Value = response.json().await.unwrap();
    server.abort();
    assert_eq!(
        serde_json::to_value(state.db.get_entry("changed").unwrap().unwrap()).unwrap(),
        before
    );
    assert_eq!(
        report,
        json!({"requested":6, "applied":3, "skipped":3, "conflicts":["changed", "empty"]})
    );
    for (id, expected) in [
        ("matching", "new text"),
        ("legacy", "legacy write"),
        ("null", "filled"),
    ] {
        let row = state.db.get_entry(id).unwrap().unwrap();
        assert_eq!(row.translation.as_deref(), Some(expected));
        assert_eq!(row.status, StringStatus::Translated);
        assert_eq!(row.provider_used.as_deref(), Some("search-replace"));
    }
    assert_eq!(
        state
            .db
            .get_entry("empty")
            .unwrap()
            .unwrap()
            .translation
            .as_deref(),
        Some("")
    );
}

#[tokio::test]
async fn legacy_and_empty_batches_keep_counts_and_add_empty_conflicts() {
    let state = create_test_state();
    state
        .db
        .save_entries(&[StringEntry::new("row", "Source", "story.html".into())])
        .unwrap();
    *state.current_project.write().await = Some(
        serde_json::from_value(
            json!({"path":"/fixture", "format_id":"html-game", "name":"Fixture"}),
        )
        .unwrap(),
    );
    let (base, server) = start_test_server(state.clone()).await;
    let client = reqwest::Client::new();
    for (updates, requested, applied) in [
        (
            json!([{"id":"row", "translation":"legacy"}, {"id":"missing", "translation":"unknown"}]),
            2,
            1,
        ),
        (json!([]), 0, 0),
    ] {
        let response = client
            .post(format!("{base}/api/strings/batch"))
            .json(&json!({"updates":updates}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let report: serde_json::Value = response.json().await.unwrap();
        assert_eq!(
            report,
            json!({"requested":requested, "applied":applied, "skipped":requested-applied, "conflicts":[]})
        );
    }
    server.abort();
}
