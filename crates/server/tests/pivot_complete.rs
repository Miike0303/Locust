mod common;

use locust_core::database::{Database, EntryFilter};
use locust_core::models::{StringEntry, StringStatus, STALE_TRANSLATION_METADATA_KEY};
use locust_server::{
    create_router, create_test_state, create_test_state_with_db, try_project_operation,
};
use reqwest::Client;
use serde_json::{json, Value};

fn ready(id: &str) -> StringEntry {
    let mut row = StringEntry::new(id, "日本語 [name]", "script.rpy".into());
    row.translation = Some("English [name]".into());
    row.status = StringStatus::Translated;
    row
}

async fn open(state: &locust_server::AppState) {
    *state.current_project.write().await = Some(
        serde_json::from_value(
            json!({"path":"synthetic-game", "format_id":"renpy", "name":"Fixture"}),
        )
        .unwrap(),
    );
}

fn snapshot(db: &Database) -> Value {
    serde_json::to_value(db.get_entries(&EntryFilter::default()).unwrap()).unwrap()
}

#[tokio::test]
async fn strict_http_refuses_incomplete_stale_and_invalid_controls_without_output() {
    for case in [
        "missing",
        "empty",
        "blank",
        "pending",
        "error",
        "stale",
        "control",
        "physical-control",
    ] {
        let temp = tempfile::tempdir().unwrap();
        let state = create_test_state();
        open(&state).await;
        let mut invalid = ready("z");
        let reason = match case {
            "missing" => {
                invalid.translation = None;
                "missing or blank translation"
            }
            "empty" => {
                invalid.translation = Some(String::new());
                "missing or blank translation"
            }
            "blank" => {
                invalid.translation = Some("\t\u{a0}\u{2003}\u{3000}".into());
                "missing or blank translation"
            }
            "pending" => {
                invalid.status = StringStatus::Pending;
                "not ready"
            }
            "error" => {
                invalid.status = StringStatus::Error;
                "not ready"
            }
            "stale" => {
                invalid.metadata.insert(
                    STALE_TRANSLATION_METADATA_KEY.into(),
                    json!({
                        "version": 1,
                        "translated_source_sha256": "0".repeat(64),
                        "current_source_sha256": invalid.source_hash(),
                    }),
                );
                "stale translation"
            }
            "physical-control" => {
                invalid.source = "English".into();
                invalid.translation = Some("Spanish".into());
                invalid
                    .metadata
                    .insert("locust_injection_source".into(), json!("日本語 [name]"));
                "changed protected controls"
            }
            _ => {
                invalid.translation = Some("English".into());
                "changed protected controls"
            }
        };
        state.db.save_entries(&[ready("a"), invalid]).unwrap();
        let before = snapshot(&state.db);
        let project_before =
            serde_json::to_value(state.current_project.read().await.clone()).unwrap();
        let (url, server) = common::start_test_server(create_router(state.clone())).await;
        let response = Client::new()
            .post(format!("{url}/api/pivot"))
            .json(&json!({"output_path":temp.path().join("out.db"), "require_complete":true}))
            .send()
            .await
            .unwrap();
        let status = response.status();
        let text = response.text().await.unwrap();
        server.shutdown().await;
        assert_eq!(status, 400, "{case}: {text}");
        assert!(text.contains(reason), "{case}: {text}");
        assert_eq!(
            std::fs::read_dir(temp.path()).unwrap().count(),
            0,
            "{case}: no output or sidecars"
        );
        assert_eq!(snapshot(&state.db), before, "{case}: source unchanged");
        assert_eq!(
            serde_json::to_value(state.current_project.read().await.clone()).unwrap(),
            project_before
        );
    }
}

#[tokio::test]
async fn strict_http_preserves_every_ready_row_and_physical_provenance() {
    let temp = tempfile::tempdir().unwrap();
    let state = create_test_state();
    open(&state).await;
    let mut slot = ready("a");
    slot.source = "English".into();
    slot.translation = Some("Spanish".into());
    slot.metadata.insert("binary_slot".into(), json!("utf8"));
    slot.metadata
        .insert("locust_injection_source".into(), json!("日本語文"));
    slot.metadata.insert(
        "locust_injection_capacity".into(),
        json!({"encoding":"utf8", "bytes":12}),
    );
    slot.context = Some("speaker".into());
    slot.tags = vec!["dialogue".into()];
    let mut reviewed = ready("b");
    reviewed.status = StringStatus::Reviewed;
    let mut approved = ready("c");
    approved.status = StringStatus::Approved;
    state.db.save_entries(&[slot, reviewed, approved]).unwrap();
    let before = snapshot(&state.db);
    let output = temp.path().join("complete.db");
    let (url, server) = common::start_test_server(create_router(state.clone())).await;
    let response = Client::new()
        .post(format!("{url}/api/pivot"))
        .json(&json!({"output_path":output, "require_complete":true}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let body: Value = response.json().await.unwrap();
    server.shutdown().await;
    assert_eq!(body, json!({"database_path":output, "entries":3}));
    let pivot = Database::open(&output).unwrap();
    for row in pivot.get_entries(&EntryFilter::default()).unwrap() {
        let original = state.db.get_entry(&row.id).unwrap().unwrap();
        assert_eq!(Some(row.source.as_str()), original.translation.as_deref());
        assert_eq!(
            row.injection_source().unwrap(),
            original.injection_source().unwrap()
        );
        assert_eq!(
            row.injection_capacity().unwrap(),
            original.injection_capacity().unwrap()
        );
        assert!(row.translation.is_none());
        assert_eq!(row.status, StringStatus::Pending);
        assert_eq!(row.context, original.context);
        assert_eq!(row.tags, original.tags);
    }
    assert_eq!(snapshot(&state.db), before);
}

#[tokio::test]
async fn legacy_http_absent_and_false_keep_partial_compatibility() {
    for strict in [None, Some(false)] {
        let temp = tempfile::tempdir().unwrap();
        let state = create_test_state();
        open(&state).await;
        let mut retained = ready("retained");
        retained.status = StringStatus::Pending;
        let mut blank = ready("blank");
        blank.translation = Some("  ".into());
        state
            .db
            .save_entries(&[
                ready("ready"),
                retained,
                blank,
                StringEntry::new("missing", "Source", "script.rpy".into()),
            ])
            .unwrap();
        let before = snapshot(&state.db);
        let mut body = json!({"output_path":temp.path().join("partial.db")});
        if let Some(value) = strict {
            body["require_complete"] = json!(value);
        }
        let (url, server) = common::start_test_server(create_router(state.clone())).await;
        let response = Client::new()
            .post(format!("{url}/api/pivot"))
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let result: Value = response.json().await.unwrap();
        server.shutdown().await;
        assert_eq!(
            result["entries"], 2,
            "legacy retains nonempty pending translations"
        );
        assert_eq!(snapshot(&state.db), before);
    }
}

#[tokio::test]
async fn strict_http_keeps_persisted_source_bytes_and_refuses_source_as_output() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source.db");
    let output_dir = tempfile::tempdir().unwrap();
    // Close the seed writer so its WAL is checkpointed before measuring bytes.
    let seed = Database::open(&source).unwrap();
    seed.save_entries(&[
        ready("a"),
        StringEntry::new("z", "Missing", "script.rpy".into()),
    ])
    .unwrap();
    drop(seed);
    let state = create_test_state_with_db(&source);
    open(&state).await;
    let before = std::fs::read(&source).unwrap();
    let rows_before = snapshot(&state.db);
    let (url, server) = common::start_test_server(create_router(state.clone())).await;
    let client = Client::new();
    for output in [output_dir.path().join("refused.db"), source.clone()] {
        let response = client
            .post(format!("{url}/api/pivot"))
            .json(&json!({"output_path":output, "require_complete":true}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), if output == source { 409 } else { 400 });
    }
    server.shutdown().await;
    assert_eq!(std::fs::read_dir(output_dir.path()).unwrap().count(), 0);
    assert_eq!(snapshot(&state.db), rows_before);
    assert_eq!(std::fs::read(&source).unwrap(), before);
    drop(state);
    assert_eq!(std::fs::read(&source).unwrap(), before);
}

#[tokio::test]
async fn strict_http_preserves_admission_and_overwrite_guards() {
    let temp = tempfile::tempdir().unwrap();
    let state = create_test_state();
    state.db.save_entries(&[ready("a")]).unwrap();
    let (url, server) = common::start_test_server(create_router(state.clone())).await;
    let client = Client::new();
    let output = temp.path().join("out.db");
    let body = json!({"output_path":output, "require_complete":true});
    let response = client
        .post(format!("{url}/api/pivot"))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 400);
    assert_eq!(response.text().await.unwrap(), "no project open");
    open(&state).await;
    let guard = try_project_operation(&state).unwrap();
    let response = client
        .post(format!("{url}/api/pivot"))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 409);
    assert_eq!(
        response.text().await.unwrap(),
        locust_server::PROJECT_BUSY_MESSAGE
    );
    drop(guard);
    assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 0);
    std::fs::write(&output, b"keep output").unwrap();
    let response = client
        .post(format!("{url}/api/pivot"))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 409);
    assert!(response
        .text()
        .await
        .unwrap()
        .contains("refusing to overwrite"));
    assert_eq!(std::fs::read(&output).unwrap(), b"keep output");
    assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 1);
    server.shutdown().await;
}
