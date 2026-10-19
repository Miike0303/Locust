use locust_core::models::{StringEntry, StringStatus};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::broadcast;

use crate::{create_test_state, start_test_server, AppState, JobKind, JobState, ProjectInfo};

pub(crate) async fn setup() -> (String, tokio::task::JoinHandle<()>) {
    let state = create_test_state();
    start_test_server(state).await
}

pub(crate) async fn setup_with_state() -> (String, tokio::task::JoinHandle<()>, Arc<AppState>) {
    let state = create_test_state();
    let s = state.clone();
    let (url, handle) = start_test_server(state).await;
    (url, handle, s)
}

pub(crate) fn client() -> reqwest::Client {
    reqwest::Client::new()
}

pub(crate) async fn mark_project_open(state: &AppState) {
    let mut proj = state.current_project.write().await;
    *proj = Some(ProjectInfo {
        path: PathBuf::from("/tmp/locust-test-game"),
        format_id: "rpgmaker-mv".into(),
        name: "test-game".into(),
        extraction_warnings: Vec::new(),
        ..Default::default()
    });
}

pub(crate) fn translated(id: &str, source: &str, file: &str, translation: &str) -> StringEntry {
    let mut e = StringEntry::new(id, source, PathBuf::from(file));
    e.translation = Some(translation.into());
    e.status = StringStatus::Translated;
    e
}

pub(crate) fn park_job(state: &AppState, id: &str, kind: JobKind, terminal: Option<&str>) {
    park_job_with_patch_key(state, id, kind, terminal, None);
}

pub(crate) fn park_job_with_patch_key(
    state: &AppState,
    id: &str,
    kind: JobKind,
    terminal: Option<&str>,
    patch_game_key: Option<String>,
) {
    let (tx, _) = broadcast::channel(8);
    let replay = Arc::new(std::sync::Mutex::new(Vec::new()));
    if let Some(ty) = terminal {
        replay
            .lock()
            .unwrap()
            .push(serde_json::json!({ "type": ty }));
    }
    state.active_jobs.insert(
        id.to_string(),
        JobState {
            abort_handle: tokio::spawn(async {}).abort_handle(),
            progress_tx: tx,
            replay,
            cancel: tokio_util::sync::CancellationToken::new(),
            kind,
            patch_game_key,
        },
    );
}

pub(crate) fn write_rpgmaker_game(dir: &Path) {
    let data = dir.join("data");
    std::fs::create_dir_all(&data).unwrap();
    std::fs::write(
        data.join("System.json"),
        r#"{"gameTitle":"Other Game","terms":{"basic":["HP"],"commands":["Fight"],"params":["HP"],"messages":{}}}"#,
    )
    .unwrap();
}
