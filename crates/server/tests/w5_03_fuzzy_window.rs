//! Candidate length filtering must retain matches accepted by the scorer.
use locust_core::database::sha256_hex;
use locust_core::models::StringEntry;
use locust_server::{create_test_state, start_test_server, ProjectInfo};
use reqwest::Client;
use serde_json::{json, Value};

async fn suggestion_survives_window(query: &str, source: &str, global: bool) {
    let state = create_test_state();
    *state.current_project.write().await = Some(ProjectInfo::default());
    state
        .db
        .save_entries(&[StringEntry::new("row", query, "story.html".into())])
        .unwrap();
    let hash = sha256_hex(source.as_bytes());
    if global {
        state
            .global_memory
            .save_memory(&hash, source, "Hola de nuevo", "en-es")
            .await
    } else {
        state
            .db
            .save_memory(&hash, source, "Hola de nuevo", "en-es")
            .await
    }
    .unwrap();
    let (base, server) = start_test_server(state).await;
    let response = Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .get(format!("{base}/api/memory/suggestions"))
        .query(&[("entry_id", "row"), ("lang_pair", "en-es")])
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let body: Value = response.json().await.unwrap();
    assert_eq!(
        body["suggestions"],
        json!([{
            "source": source,
            "translation": "Hola de nuevo",
            "score": locust_core::fuzzy::similarity_at_threshold(query, source, 0.6).unwrap(),
            "origin": if global { "global" } else { "project" },
            "uses": 1,
        }])
    );
    server.abort();
}

#[tokio::test]
async fn project_fuzzy_window_includes_longer_suggestion() {
    // d >= |la-lb| requires la*t <= lb <= la/t. Here 17 <= 11/0.6,
    // and the score is 1 - 6/17; the former upper bound was only 16.
    suggestion_survives_window("Hello world", "Hello world again", false).await;
}

#[tokio::test]
async fn global_fuzzy_window_includes_longer_suggestion() {
    suggestion_survives_window("Hello world", "Hello world again", true).await;
}

async fn normalized_suggestions_survive_window(global: bool) {
    let prefix = "a".repeat(locust_core::fuzzy::MAX_CHARS);
    // These normalized matches can have arbitrarily different raw lengths.
    // Query bounds use the scorer's capped normalization; candidate raw bounds
    // need a fallback for whitespace, lowercase expansion, truncation and NUL.
    for (query, source) in [
        (
            format!("{}Hello world", " ".repeat(100)),
            "Hello world again".into(),
        ),
        (
            "Hello world".into(),
            format!("{}Hello world again  ", " ".repeat(100)),
        ),
        ("Hello world".into(), "Hello\t\tworld".into()),
        ("界".repeat(10), "界".repeat(16)),
        (
            "こんにちは 世界".into(),
            format!(
                "{}こんにちは{}世界",
                "\u{3000}".repeat(20),
                "\u{3000}".repeat(20)
            ),
        ),
        ("i\u{307}".repeat(10), "İ".repeat(10)),
        ("İ".repeat(10), "i\u{307}".repeat(10)),
        (prefix.clone(), format!("{prefix}{}", "x".repeat(1000))),
        (format!("{prefix}{}", "x".repeat(1000)), prefix.clone()),
        ("hello\0world".into(), "Hello\0world".into()),
        (" \t".into(), "\u{2003}".repeat(100)),
    ] {
        suggestion_survives_window(&query, &source, global).await;
    }
}

#[tokio::test]
async fn project_fuzzy_window_is_conservative_for_normalization() {
    normalized_suggestions_survive_window(false).await;
}

#[tokio::test]
async fn global_fuzzy_window_is_conservative_for_normalization() {
    normalized_suggestions_survive_window(true).await;
}
