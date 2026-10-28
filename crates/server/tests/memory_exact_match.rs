//! Exact memory hits must survive the popularity cap on fuzzy candidates.
use locust_core::database::sha256_hex;
use locust_core::models::StringEntry;
use locust_server::{create_test_state, start_test_server, AppState, ProjectInfo};
use reqwest::Client;
use serde_json::{json, Value};

const NEAR_MATCHES: usize = 3001;

#[derive(Clone, Copy)]
enum Store {
    Project,
    Global,
}

impl Store {
    fn origin(self) -> &'static str {
        match self {
            Self::Project => "project",
            Self::Global => "global",
        }
    }

    async fn save(self, state: &AppState, source: &str, translation: &str, pair: &str) {
        let hash = sha256_hex(source.as_bytes());
        match self {
            Self::Project => state.db.save_memory(&hash, source, translation, pair).await,
            Self::Global => {
                state
                    .global_memory
                    .save_memory(&hash, source, translation, pair)
                    .await
            }
        }
        .unwrap();
    }
}

fn snapshot(state: &AppState) -> Value {
    json!({
        "project": state.db.list_memory(None, None, NEAR_MATCHES + 10, 0).unwrap(),
        "global": state.global_memory.list_memory(None, None, NEAR_MATCHES + 10, 0).unwrap(),
        "entry": state.db.get_entry("row").unwrap(),
    })
}

async fn suggestions(client: &Client, base: &str, pair: &str) -> Vec<Value> {
    let response = client
        .get(format!("{base}/api/memory/suggestions"))
        .query(&[("entry_id", "row"), ("lang_pair", pair)])
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let body: Value = response.json().await.unwrap();
    body["suggestions"].as_array().unwrap().clone()
}

async fn exact_hit_survives_fuzzy_cap(store: Store) {
    let state = create_test_state();
    *state.current_project.write().await = Some(ProjectInfo {
        path: "memory-project".into(),
        name: "Memory project".into(),
        format_id: "html".into(),
        ..Default::default()
    });
    let mut entry = StringEntry::new("row", "Hello world here", "story.html".into());
    entry.translation = Some("Current".into());
    state.db.save_entries(std::slice::from_ref(&entry)).unwrap();

    for index in 0..NEAR_MATCHES {
        let source = format!("Hello world {index:04}");
        assert_eq!(source.chars().count(), entry.source.chars().count());
        let translation = format!("Near translation {index:04}");
        // Every near match outranks the later exact hit by usage.
        for _ in 0..2 {
            store.save(&state, &source, &translation, "en-es").await;
        }
    }
    store
        .save(&state, "Hello world 0000", "Near translation 0000", "en-es")
        .await;
    store
        .save(&state, &entry.source, "Other language", "en-fr")
        .await;

    let (base, server) = start_test_server(state.clone()).await;
    let client = Client::builder().no_proxy().build().unwrap();
    let before_fuzzy = snapshot(&state);
    let fuzzy = suggestions(&client, &base, "en-es").await;
    assert_eq!(fuzzy.len(), 5);
    assert_eq!(fuzzy[0]["uses"], 3);
    assert!(fuzzy.iter().all(|row| row["score"] == 0.75));
    assert_eq!(snapshot(&state), before_fuzzy);

    store
        .save(&state, &entry.source, "Exact alternative", "en-es")
        .await;
    let candidates = match store {
        Store::Project => state.db.memory_candidates("en-es", 16, usize::MAX),
        Store::Global => state
            .global_memory
            .memory_candidates("en-es", 16, usize::MAX),
    }
    .unwrap();
    assert_eq!(candidates.len(), 3000);
    assert!(candidates.iter().all(|row| row.uses >= 2));
    let source_hash = entry.source_hash();
    assert!(candidates.iter().all(|row| row.source_hash != source_hash));

    let before_exact = snapshot(&state);
    for _ in 0..2 {
        let rows = suggestions(&client, &base, "en-es").await;
        assert_eq!(
            rows[0],
            json!({
                "source": entry.source,
                "translation": "Exact alternative",
                "score": 1.0,
                "origin": store.origin(),
                "uses": 1,
            }),
            "{} exact hit was lost behind the fuzzy candidate cap",
            store.origin(),
        );
        assert_eq!(rows.len(), 5);
        assert_eq!(rows[1..], fuzzy[..4]);
    }
    assert!(suggestions(&client, &base, "en-de").await.is_empty());
    let other_pair = suggestions(&client, &base, "en-fr").await;
    assert_eq!(other_pair.len(), 1);
    assert_eq!(other_pair[0]["translation"], "Other language");
    assert_eq!(snapshot(&state), before_exact);

    // A rescued exact hit still disappears when it duplicates the saved text.
    entry.translation = Some("Exact alternative".into());
    state.db.save_entries(&[entry]).unwrap();
    let before_redundant = snapshot(&state);
    assert_eq!(suggestions(&client, &base, "en-es").await, fuzzy);
    assert_eq!(snapshot(&state), before_redundant);
    server.abort();
}

#[tokio::test]
async fn project_exact_match_survives_fuzzy_truncation() {
    exact_hit_survives_fuzzy_cap(Store::Project).await;
}

#[tokio::test]
async fn global_exact_match_survives_fuzzy_truncation() {
    exact_hit_survives_fuzzy_cap(Store::Global).await;
}

#[tokio::test]
async fn exact_lookup_preserves_the_existing_deduplication_tie_winner() {
    let state = create_test_state();
    *state.current_project.write().await = Some(ProjectInfo::default());
    state
        .db
        .save_entries(&[StringEntry::new("row", "Hello", "story.html".into())])
        .unwrap();
    Store::Project
        .save(&state, "Hello", "Alternative", "en-es")
        .await;
    Store::Project
        .save(&state, "HELLO", "Alternative", "en-es")
        .await;
    Store::Global
        .save(&state, "Hello", "Alternative", "en-es")
        .await;
    // Both project candidates score 1 with equal uses; the newer one wins.
    let candidates = state.db.memory_candidates("en-es", 5, 3000).unwrap();
    assert_eq!(candidates[0].source, "HELLO");
    let before = snapshot(&state);
    let (base, server) = start_test_server(state.clone()).await;
    let client = Client::builder().no_proxy().build().unwrap();
    assert_eq!(
        suggestions(&client, &base, "en-es").await,
        [json!({
            "source": "HELLO",
            "translation": "Alternative",
            "score": 1.0,
            "origin": "project",
            "uses": 1,
        })],
    );
    assert_eq!(snapshot(&state), before);
    server.abort();
}
