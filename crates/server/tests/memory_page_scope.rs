//! The memory page must inspect the same project cache used by the engine.
use std::sync::Arc;

use locust_server::{create_test_state, start_test_server, AppState, ProjectInfo};
use reqwest::{Client, Method};
use serde_json::{json, Value};

async fn seeded_state() -> Arc<AppState> {
    let state = create_test_state();
    *state.current_project.write().await = Some(ProjectInfo {
        path: "memory-project".into(),
        name: "Memory project".into(),
        format_id: "html".into(),
        ..Default::default()
    });
    // The engine writes batches to the project DB, never to global memory.
    state
        .db
        .save_memory_batch(
            &[
                ("hello".into(), "Hello".into(), "Hola".into()),
                ("world".into(), "World".into(), "Mundo".into()),
            ],
            "en-es",
        )
        .await
        .unwrap();
    state
        .db
        .save_memory_batch(
            &[("hello".into(), "Hello".into(), "Bonjour".into())],
            "en-fr",
        )
        .await
        .unwrap();
    state
        .global_memory
        .save_memory("hello", "Global source", "Global translation", "en-es")
        .await
        .unwrap();
    state
}

async fn get(client: &Client, base: &str, path: &str) -> Value {
    let response = client
        .get(format!("{base}/api/{path}"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    response.json().await.unwrap()
}

#[tokio::test]
async fn list_search_filter_and_stats_report_engine_memory() {
    let state = seeded_state().await;
    let (base, server) = start_test_server(state).await;
    let client = Client::builder().no_proxy().build().unwrap();
    let all = get(&client, &base, "memory").await;
    assert_eq!(all["total"], 3);
    assert_eq!(all["entries"].as_array().unwrap().len(), 3);
    assert!(all["entries"]
        .as_array()
        .unwrap()
        .iter()
        .all(|row| row["source"] != "Global source"));
    for (query, expected) in [
        ("search=Hello", 2),
        ("search=Mundo", 1),
        ("lang_pair=en-es", 2),
        ("search=Hello&lang_pair=en-es", 1),
        ("search=Mundo&lang_pair=en-fr", 0),
    ] {
        let result = get(&client, &base, &format!("memory?{query}")).await;
        assert_eq!(result["total"], expected, "{query}");
        assert_eq!(
            result["entries"].as_array().unwrap().len(),
            expected as usize
        );
    }
    let page = get(&client, &base, "memory?lang_pair=en-es&limit=1&offset=1").await;
    assert_eq!(page["total"], 2);
    assert_eq!(page["entries"].as_array().unwrap().len(), 1);
    assert_eq!(page["limit"], 1);
    assert_eq!(page["offset"], 1);
    assert_eq!(
        get(&client, &base, "memory/lang-pairs").await,
        json!(["en-es", "en-fr"])
    );
    assert_eq!(
        get(&client, &base, "memory/stats").await,
        json!({"project_entries": 3, "global_entries": 1})
    );
    server.abort();
}

#[tokio::test]
async fn deleting_one_page_row_removes_engine_hit_only_for_that_pair() {
    let state = seeded_state().await;
    let (base, server) = start_test_server(state.clone()).await;
    let client = Client::builder().no_proxy().build().unwrap();
    let response = client
        .delete(format!("{base}/api/memory/hello/en-es"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let hits = state
        .db
        .lookup_memory_batch(&["hello".into(), "world".into()], "en-es")
        .unwrap();
    assert!(!hits.contains_key("hello"));
    assert_eq!(hits.get("world").map(String::as_str), Some("Mundo"));
    assert_eq!(
        state.db.lookup_memory("hello", "en-fr").unwrap().as_deref(),
        Some("Bonjour")
    );
    assert_eq!(state.global_memory.memory_count().unwrap(), 1);
    assert_eq!(get(&client, &base, "memory").await["total"], 2);
    // Removing the final row for a language updates the language filter too.
    assert_eq!(
        client
            .delete(format!("{base}/api/memory/hello/en-fr"))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    assert_eq!(
        get(&client, &base, "memory/lang-pairs").await,
        json!(["en-es"])
    );
    server.abort();
}

#[tokio::test]
async fn clear_uses_the_same_project_store_as_list_and_clears_legacy_global_memory() {
    let state = seeded_state().await;
    let (base, server) = start_test_server(state.clone()).await;
    let client = Client::builder().no_proxy().build().unwrap();
    assert_eq!(
        client
            .delete(format!("{base}/api/memory"))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    assert_eq!(state.db.memory_count().unwrap(), 0);
    assert!(state
        .db
        .lookup_memory_batch(&["hello".into(), "world".into()], "en-es")
        .unwrap()
        .is_empty());
    assert_eq!(state.global_memory.memory_count().unwrap(), 0);
    assert_eq!(get(&client, &base, "memory").await["total"], 0);
    assert_eq!(get(&client, &base, "memory/lang-pairs").await, json!([]));
    assert_eq!(
        get(&client, &base, "memory/stats").await,
        json!({"project_entries": 0, "global_entries": 0})
    );
    server.abort();
}

fn project_routes() -> [(Method, &'static str); 4] {
    [
        (Method::GET, "memory?search=Hello&lang_pair=en-es"),
        (Method::GET, "memory/lang-pairs"),
        (Method::DELETE, "memory/hello/en-es"),
        (Method::DELETE, "memory"),
    ]
}

#[tokio::test]
async fn page_operations_require_an_open_project_without_touching_either_store() {
    let state = seeded_state().await;
    *state.current_project.write().await = None;
    let (base, server) = start_test_server(state.clone()).await;
    let client = Client::builder().no_proxy().build().unwrap();
    for (method, path) in project_routes() {
        let response = client
            .request(method, format!("{base}/api/{path}"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 400, "{path}");
        assert_eq!(response.text().await.unwrap(), "no project open");
    }
    assert_eq!(state.db.memory_count().unwrap(), 3);
    assert_eq!(state.global_memory.memory_count().unwrap(), 1);
    server.abort();
}

#[tokio::test]
async fn page_operations_hold_the_existing_project_guard() {
    let state = seeded_state().await;
    let held = locust_server::try_project_operation(&state).unwrap();
    let (base, server) = start_test_server(state.clone()).await;
    let client = Client::builder().no_proxy().build().unwrap();
    for (method, path) in project_routes() {
        let response = client
            .request(method, format!("{base}/api/{path}"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 409, "{path}");
        assert_eq!(
            response.text().await.unwrap(),
            locust_server::PROJECT_BUSY_MESSAGE
        );
    }
    assert_eq!(state.db.memory_count().unwrap(), 3);
    assert_eq!(state.global_memory.memory_count().unwrap(), 1);
    drop(held);
    assert_eq!(get(&client, &base, "memory").await["total"], 3);
    server.abort();
}
