//! Deterministic HTTP adapters: saved-data resume never extracts injected sources.
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};

use locust_core::{
    error::Result,
    extraction::{inject_direct, FormatPlugin, FormatRegistry, InjectionReport},
    models::{StringEntry, StringStatus},
    project::open_project,
};
use locust_server::{create_test_state, start_test_server, AppState, ProjectInfo};
use serde_json::{json, Value};

struct TextPlugin(Arc<AtomicUsize>);
impl FormatPlugin for TextPlugin {
    fn id(&self) -> &str {
        "resume-http"
    }
    fn name(&self) -> &str {
        "Resume HTTP"
    }
    fn supported_extensions(&self) -> &[&str] {
        &["txt"]
    }
    fn detect(&self, path: &Path) -> bool {
        path.join("story.txt").is_file()
    }
    fn extract(&self, path: &Path) -> Result<Vec<StringEntry>> {
        self.0.fetch_add(1, Ordering::SeqCst);
        let mut entry = StringEntry::new(
            "hero",
            fs::read_to_string(path.join("story.txt"))?,
            path.join("story.txt"),
        );
        entry.metadata.insert(
            "iostore_unread_containers".into(),
            json!(["extra.utoc: encrypted"]),
        );
        Ok(vec![entry])
    }
    fn inject(&self, path: &Path, entries: &[StringEntry]) -> Result<InjectionReport> {
        let file = path.join("story.txt");
        fs::write(&file, entries[0].translation.as_deref().unwrap())?;
        Ok(InjectionReport {
            files_modified: 1,
            strings_written: 1,
            strings_skipped: 0,
            files_written: vec![file],
            warnings: vec![],
            skip_reasons: Default::default(),
        })
    }
}

struct Fixture {
    _temp: tempfile::TempDir,
    game: PathBuf,
    database: PathBuf,
    state: Arc<AppState>,
    calls: Arc<AtomicUsize>,
}
impl Fixture {
    async fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let game = temp.path().join("game");
        fs::create_dir(&game).unwrap();
        fs::write(game.join("story.txt"), "勇者").unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let mut registry = FormatRegistry::new();
        registry.register(Box::new(TextPlugin(calls.clone())));
        let mut state = create_test_state();
        Arc::get_mut(&mut state).unwrap().format_registry = Arc::new(registry);
        let opened = open_project(&state.db, &state.format_registry, &game, None).unwrap();
        let mut row = state.db.get_entry("hero").unwrap().unwrap();
        row.translation = Some("Hero".into());
        row.status = StringStatus::Approved;
        row.provider_used = Some("test-provider".into());
        state.db.save_entries(&[row]).unwrap();
        inject_direct(
            &state.format_registry,
            &state.db,
            &state.backup_manager,
            &game,
            "resume-http",
            &["en".into()],
        )
        .unwrap();
        // A different live project makes a failed resume's state invariant observable.
        state
            .db
            .reopen(&temp.path().join("other.locust.db"))
            .unwrap();
        state
            .db
            .save_entries(&[StringEntry::new("keep", "Unrelated", "other.txt".into())])
            .unwrap();
        *state.current_project.write().await = Some(ProjectInfo {
            path: temp.path().join("other"),
            format_id: "resume-http".into(),
            name: "Other".into(),
            ..Default::default()
        });
        Self {
            _temp: temp,
            game,
            database: opened.database_path,
            state,
            calls,
        }
    }
    fn resume_body(&self) -> Value {
        json!({"database_path": self.database, "game_path": self.game, "format_id": "resume-http"})
    }
}

async fn post(base: &str, path: &str, body: Value) -> reqwest::Response {
    reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .post(format!("{base}/api/project/{path}"))
        .json(&body)
        .send()
        .await
        .unwrap()
}

#[tokio::test]
async fn preflight_shape_and_resume_preserve_approved_rows_without_extraction() {
    let f = Fixture::new().await;
    let (base, server) = start_test_server(f.state.clone()).await;
    let before_live = f.state.db.path();
    let preflight = post(&base, "preflight", json!({"game_path": f.game})).await;
    assert_eq!(preflight.status(), 200);
    assert_eq!(
        preflight.json::<Value>().await.unwrap(),
        json!({
            "kind": "resume_available", "database_path": f.database,
            "project_path": f.game, "format_id": "resume-http"
        })
    );
    assert_eq!(f.state.db.path(), before_live);
    assert_eq!(
        f.state.current_project.read().await.as_ref().unwrap().name,
        "Other"
    );
    assert!(f.state.config.read().await.recent_projects.is_empty());
    let response = post(&base, "resume", f.resume_body()).await;
    assert_eq!(response.status(), 200);
    let result = response.json::<Value>().await.unwrap();
    for counter in [
        "added",
        "updated",
        "stale_source_reset",
        "removed",
        "preserved_translations",
    ] {
        assert_eq!(result[counter], 0, "{counter}");
    }
    assert_eq!(result["total_strings"], 1);
    assert_eq!(
        result["extraction_warnings"],
        json!(["extra.utoc: encrypted"])
    );
    let row = f.state.db.get_entry("hero").unwrap().unwrap();
    assert_eq!(row.source, "勇者");
    assert_eq!(row.translation.as_deref(), Some("Hero"));
    assert_eq!(row.status, StringStatus::Approved);
    assert_eq!(row.provider_used.as_deref(), Some("test-provider"));
    assert_eq!(f.state.db.path(), f.database);
    assert_eq!(
        f.state.current_project.read().await.as_ref().unwrap().path,
        f.game
    );
    assert_eq!(
        f.state.config.read().await.recent_projects[0]
            .database_path
            .as_ref(),
        Some(&f.database)
    );
    assert!(f.state.config_path.is_file());
    assert_eq!(f.calls.load(Ordering::SeqCst), 1);
    server.abort();
}

#[tokio::test]
async fn drift_after_preflight_refuses_without_changing_live_or_saved_project() {
    let f = Fixture::new().await;
    let (base, server) = start_test_server(f.state.clone()).await;
    let preflight = post(
        &base,
        "preflight",
        json!({"game_path": f.game, "format": "resume-http"}),
    )
    .await;
    assert_eq!(preflight.status(), 200);
    assert_eq!(
        preflight.json::<Value>().await.unwrap()["kind"],
        "resume_available"
    );
    let before_path = f.state.db.path();
    let before_project =
        serde_json::to_value(f.state.current_project.read().await.clone()).unwrap();
    let before_config = serde_json::to_value(f.state.config.read().await.clone()).unwrap();
    let saved = fs::read(&f.database).unwrap();
    fs::write(f.game.join("story.txt"), "Drift").unwrap();
    let response = post(&base, "resume", f.resume_body()).await;
    assert_eq!(response.status(), 409);
    assert!(!response.text().await.unwrap().is_empty());
    assert_eq!(f.state.db.path(), before_path);
    assert_eq!(
        f.state.db.get_entry("keep").unwrap().unwrap().source,
        "Unrelated"
    );
    assert!(f.state.db.get_entry("hero").unwrap().is_none());
    assert_eq!(
        serde_json::to_value(f.state.current_project.read().await.clone()).unwrap(),
        before_project
    );
    assert_eq!(
        serde_json::to_value(f.state.config.read().await.clone()).unwrap(),
        before_config
    );
    assert_eq!(fs::read(&f.database).unwrap(), saved);
    assert_eq!(f.calls.load(Ordering::SeqCst), 1);
    let attention = post(&base, "preflight", json!({"game_path": f.game})).await;
    let attention = attention.json::<Value>().await.unwrap();
    assert_eq!(attention["kind"], "needs_attention");
    assert!(!attention["reason"].as_str().unwrap().is_empty());
    server.abort();
}

#[tokio::test]
async fn plain_preflight_keeps_backward_compatible_folder_extraction() {
    let f = Fixture::new().await;
    let plain = f._temp.path().join("plain");
    fs::create_dir(&plain).unwrap();
    fs::write(plain.join("story.txt"), "Plain source").unwrap();
    let (base, server) = start_test_server(f.state.clone()).await;
    let preflight = post(&base, "preflight", json!({"game_path": plain})).await;
    assert_eq!(preflight.status(), 200);
    assert_eq!(
        preflight.json::<Value>().await.unwrap(),
        json!({"kind": "extract"})
    );
    assert!(!plain.with_extension("locust.db").exists());
    let response = post(&base, "open", json!({"path": plain})).await;
    assert_eq!(response.status(), 200);
    assert_eq!(response.json::<Value>().await.unwrap()["added"], 1);
    assert_eq!(
        f.state.db.get_entry("hero").unwrap().unwrap().source,
        "Plain source"
    );
    assert_eq!(f.calls.load(Ordering::SeqCst), 2);
    server.abort();
}

#[tokio::test]
async fn resume_refuses_project_admission_conflicts_and_explicit_refresh_remains_available() {
    let f = Fixture::new().await;
    let (base, server) = start_test_server(f.state.clone()).await;
    let guard = locust_server::try_project_operation(&f.state).unwrap();
    let response = post(&base, "resume", f.resume_body()).await;
    assert_eq!(response.status(), 409);
    assert_eq!(f.calls.load(Ordering::SeqCst), 1);
    drop(guard);
    let incompatible = post(
        &base,
        "preflight",
        json!({"game_path": f.game, "format": "wrong"}),
    )
    .await;
    assert_eq!(
        incompatible.json::<Value>().await.unwrap()["kind"],
        "needs_attention"
    );
    let response = post(
        &base,
        "open",
        json!({"path": f.game, "format_id": "resume-http"}),
    )
    .await;
    assert_eq!(response.status(), 200);
    assert_eq!(
        response.json::<Value>().await.unwrap()["stale_source_reset"],
        1
    );
    assert_eq!(
        f.state.db.get_entry("hero").unwrap().unwrap().source,
        "Hero"
    );
    assert_eq!(
        f.state.db.get_entry("hero").unwrap().unwrap().status,
        StringStatus::Pending
    );
    server.abort();
}
