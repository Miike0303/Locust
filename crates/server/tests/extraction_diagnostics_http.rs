mod common;

use std::{path::Path, sync::Arc};

use locust_core::{
    database::EntryFilter,
    error::Result,
    extraction::{Extraction, FormatPlugin, FormatRegistry, InjectionReport},
    models::{StringEntry, StringStatus},
    project::saved_extraction_warnings,
};
use serde_json::{json, Value};

struct DiagnosticPlugin;

impl FormatPlugin for DiagnosticPlugin {
    fn id(&self) -> &str {
        "diagnostics-http"
    }
    fn name(&self) -> &str {
        "Diagnostics HTTP"
    }
    fn supported_extensions(&self) -> &[&str] {
        &[]
    }
    fn detect(&self, _: &Path) -> bool {
        true
    }
    fn extract(&self, _: &Path) -> Result<Vec<StringEntry>> {
        panic!("HTTP opening must use the diagnostics channel")
    }
    fn extract_with_diagnostics(&self, path: &Path) -> Result<Extraction> {
        let complete = !path.join("partial").exists();
        Ok(Extraction {
            entries: std::fs::read_to_string(path.join("story.txt"))?
                .lines()
                .map(|id| StringEntry::new(id, id, path.join("story.txt")))
                .collect(),
            warnings: if complete {
                vec![]
            } else {
                vec!["timeline.dtl: unsupported prefix".into()]
            },
            complete,
        })
    }
    fn inject(&self, _: &Path, _: &[StringEntry]) -> Result<InjectionReport> {
        unreachable!("extraction-only fixture")
    }
}

#[tokio::test]
async fn http_open_current_and_saved_db_surface_zero_row_diagnostics_then_clear() {
    let temp = tempfile::tempdir().unwrap();
    let game = temp.path().join("game");
    std::fs::create_dir(&game).unwrap();
    std::fs::write(game.join("story.txt"), "hero\nnpc").unwrap();
    let mut registry = FormatRegistry::new();
    registry.register(Box::new(DiagnosticPlugin));
    let mut state = locust_server::create_test_state();
    Arc::get_mut(&mut state).unwrap().format_registry = Arc::new(registry);
    let (base, server) =
        common::start_test_server(locust_server::create_router(state.clone())).await;
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let open_body = json!({"path": game});
    let response = client
        .post(format!("{base}/api/project/open"))
        .json(&open_body)
        .send()
        .await
        .unwrap();
    let status = response.status();
    let body = response.text().await.unwrap();
    assert_eq!(status, 200, "{body}");
    let first: Value = serde_json::from_str(&body).unwrap();
    let mut rows = state.db.get_entries(&EntryFilter::default()).unwrap();
    for row in &mut rows {
        row.translation = Some("Traducción".into());
        row.status = StringStatus::Approved;
    }
    state.db.save_entries(&rows).unwrap();
    let before = serde_json::to_value(rows).unwrap();
    std::fs::write(game.join("story.txt"), "").unwrap();
    std::fs::write(game.join("partial"), "").unwrap();
    let expected = json!(["timeline.dtl: unsupported prefix"]);
    for (route, body) in [
        ("open", open_body.clone()),
        (
            "open-db",
            json!({"database_path": first["database_path"], "game_path": game, "format_id": "diagnostics-http"}),
        ),
    ] {
        let response = client
            .post(format!("{base}/api/project/{route}"))
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let result = response.json::<Value>().await.unwrap();
        assert_eq!(result["extraction_warnings"], expected);
        assert_eq!(result["total_strings"], 2);
        assert_eq!(result["removed"], 0);
        assert_eq!(
            serde_json::to_value(state.db.get_entries(&EntryFilter::default()).unwrap()).unwrap(),
            before
        );
        let current = client
            .get(format!("{base}/api/project/current"))
            .send()
            .await
            .unwrap()
            .json::<Value>()
            .await
            .unwrap();
        assert_eq!(current["extraction_warnings"], expected);
    }
    assert_eq!(
        saved_extraction_warnings(&state.db).unwrap(),
        ["timeline.dtl: unsupported prefix"]
    );
    std::fs::remove_file(game.join("partial")).unwrap();
    let response = client
        .post(format!("{base}/api/project/open"))
        .json(&open_body)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let result = response.json::<Value>().await.unwrap();
    assert_eq!(result["total_strings"], 0);
    assert_eq!(result["removed"], 2);
    assert_eq!(result["extraction_warnings"], json!([]));
    assert!(saved_extraction_warnings(&state.db).unwrap().is_empty());
    server.shutdown().await;
}
