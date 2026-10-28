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

async fn pagination_state() -> Arc<AppState> {
    let state = seeded_state().await;
    let rows = (0..498)
        .map(|i| {
            (
                format!("page-{i:03}"),
                format!("Page source {i:03}"),
                format!("Página {i:03}"),
            )
        })
        .collect::<Vec<_>>();
    state.db.save_memory_batch(&rows, "en-es").await.unwrap();
    // Nontrivial usage counters and timestamps must survive every read.
    state
        .db
        .save_memory("hello", "Hello", "Hola", "en-es")
        .await
        .unwrap();
    state
}

async fn public_snapshot(client: &Client, base: &str) -> Value {
    json!({
        "first": get(client, base, "memory?limit=500").await,
        "tail": get(client, base, "memory?limit=500&offset=500").await,
        "stats": get(client, base, "memory/stats").await,
        "pairs": get(client, base, "memory/lang-pairs").await,
    })
}

fn expected_page(rows: &[Value], limit: usize, offset: usize) -> Value {
    json!({
        "entries": rows.iter().skip(offset).take(limit).collect::<Vec<_>>(),
        "total": rows.len(),
        "limit": limit,
        "offset": offset,
    })
}

#[tokio::test]
async fn pagination_caps_only_valid_limits_and_preserves_full_responses_and_stores() {
    let state = pagination_state().await;
    // Public repository APIs capture opaque timestamps and the complete row schema;
    // expectations below are not inferred from the handler under test.
    let rows = serde_json::to_value(state.db.list_memory(None, None, 501, 0).unwrap().0)
        .unwrap()
        .as_array()
        .unwrap()
        .clone();
    let global_before = serde_json::to_value(
        state
            .global_memory
            .list_memory(None, None, 500, 0)
            .unwrap()
            .0,
    )
    .unwrap();
    assert_eq!(rows.len(), 501);
    assert!(rows.iter().all(|row| row["source"] != "Global source"));
    assert!(rows.iter().any(|row| row["uses"] == 2));
    let (base, server) = start_test_server(state.clone()).await;
    let client = Client::builder().no_proxy().build().unwrap();
    let before = public_snapshot(&client, &base).await;
    assert_eq!(before["first"], expected_page(&rows, 500, 0));
    assert_eq!(before["tail"], expected_page(&rows, 500, 500));
    assert_eq!(
        before["stats"],
        json!({"project_entries": 501, "global_entries": 1})
    );
    assert_eq!(before["pairs"], json!(["en-es", "en-fr"]));

    for (query, limit, offset) in [
        ("", 50, 0),
        ("limit=50", 50, 0),
        ("limit=500", 500, 0),
        ("limit=501", 500, 0),
        ("limit=18446744073709551615", 500, 0),
        ("limit=18446744073709551616", 500, 0),
        (
            "limit=999999999999999999999999999999999999999999999999999999999999999999",
            500,
            0,
        ),
        ("limit=0", 0, 0),
        ("limit=000", 0, 0),
        ("limit=000501", 500, 0),
        ("limit=%2B50", 50, 0),
        ("limit=%2B18446744073709551616", 500, 0),
        ("limit=", 50, 0),
        ("limit=nope", 50, 0),
        ("limit=-1", 50, 0),
        ("limit=-0", 50, 0),
        ("limit=1.5", 50, 0),
        ("limit=%20501", 50, 0),
        ("limit=501%20", 50, 0),
        ("limit=999999999999999999999999x", 50, 0),
        ("limit=%2B", 50, 0),
        ("limit=１２", 50, 0),
        ("limit=500&offset=500", 500, 500),
        ("limit=501&offset=500", 500, 500),
        ("offset=501", 50, 501),
        ("limit=0&offset=500", 0, 500),
        ("offset=", 50, 0),
        ("offset=nope", 50, 0),
        ("offset=-1", 50, 0),
        ("offset=18446744073709551616", 50, 0),
    ] {
        assert_eq!(
            get(&client, &base, &format!("memory?{query}")).await,
            expected_page(&rows, limit, offset),
            "{query}",
        );
    }
    for (query, search, pair, limit, offset) in [
        ("search=Page&limit=501", "Page", "", 500, 0),
        ("search=Página&limit=1&offset=1", "Página", "", 1, 1),
        ("lang_pair=en-es&limit=501", "", "en-es", 500, 0),
        ("lang_pair=en-fr&limit=501", "", "en-fr", 500, 0),
        ("search=Hello&lang_pair=en-fr", "Hello", "en-fr", 50, 0),
        ("search=Mundo&lang_pair=en-fr", "Mundo", "en-fr", 50, 0),
        (
            "search=Hello&lang_pair=en-es&limit=0",
            "Hello",
            "en-es",
            0,
            0,
        ),
        ("search=&limit=501", "", "", 500, 0),
        ("lang_pair=", "", "missing", 50, 0),
    ] {
        let filtered = rows
            .iter()
            .filter(|row| {
                (pair.is_empty() || row["lang_pair"] == pair)
                    && (row["source"].as_str().unwrap().contains(search)
                        || row["translation"].as_str().unwrap().contains(search))
            })
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(
            get(&client, &base, &format!("memory?{query}")).await,
            expected_page(&filtered, limit, offset),
            "{query}",
        );
    }
    assert_eq!(public_snapshot(&client, &base).await, before);
    assert_eq!(
        serde_json::to_value(
            state
                .global_memory
                .list_memory(None, None, 500, 0)
                .unwrap()
                .0,
        )
        .unwrap(),
        global_before
    );
    server.abort();
}

#[tokio::test]
async fn capped_reads_remain_concurrent_and_no_project_errors_leave_public_state_unchanged() {
    let state = pagination_state().await;
    let (base, server) = start_test_server(state.clone()).await;
    let client = Client::builder().no_proxy().build().unwrap();
    let before = public_snapshot(&client, &base).await;
    let held = locust_server::try_project_operation(&state).unwrap();
    let (page, pairs, stats) = tokio::join!(
        get(&client, &base, "memory?limit=501"),
        get(&client, &base, "memory/lang-pairs"),
        get(&client, &base, "memory/stats"),
    );
    assert_eq!(page, before["first"]);
    assert_eq!(pairs, before["pairs"]);
    assert_eq!(stats, before["stats"]);
    drop(held);
    assert_eq!(public_snapshot(&client, &base).await, before);

    let project = state.current_project.write().await.take();
    for query in [
        "",
        "limit=501",
        "limit=18446744073709551616",
        "limit=0",
        "limit=nope&offset=500",
    ] {
        let response = client
            .get(format!("{base}/api/memory?{query}"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 400, "{query}");
        assert_eq!(response.text().await.unwrap(), "no project open");
    }
    assert_eq!(get(&client, &base, "memory/stats").await, before["stats"]);
    *state.current_project.write().await = project;
    assert_eq!(public_snapshot(&client, &base).await, before);
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
async fn page_reads_succeed_while_mutations_respect_the_project_guard() {
    let state = seeded_state().await;
    let held = locust_server::try_project_operation(&state).unwrap();
    let (base, server) = start_test_server(state.clone()).await;
    let client = Client::builder().no_proxy().build().unwrap();
    for (method, path) in project_routes() {
        let is_read = method == Method::GET;
        let response = client
            .request(method, format!("{base}/api/{path}"))
            .send()
            .await
            .unwrap();
        if is_read {
            assert_eq!(response.status(), 200, "{path}");
            let body: Value = response.json().await.unwrap();
            if path == "memory/lang-pairs" {
                assert_eq!(body, json!(["en-es", "en-fr"]));
            } else {
                assert_eq!(body["total"], 1);
                assert_eq!(body["entries"][0]["translation"], "Hola");
            }
        } else {
            assert_eq!(response.status(), 409, "{path}");
            assert_eq!(
                response.text().await.unwrap(),
                locust_server::PROJECT_BUSY_MESSAGE
            );
        }
    }
    assert_eq!(state.db.memory_count().unwrap(), 3);
    assert_eq!(state.global_memory.memory_count().unwrap(), 1);
    drop(held);
    assert_eq!(get(&client, &base, "memory").await["total"], 3);
    server.abort();
}
