//! Real HTTP admission/cancellation tests with local deterministic providers.
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures_util::StreamExt;
use locust_core::error::Result;
use locust_core::models::{StringEntry, TranslationRequest, TranslationResult};
use locust_core::translation::{TranslationOptions, TranslationProvider};
use locust_server::{
    create_test_state, start_test_server, AppState, ProjectExclusiveGuard, ProjectInfo,
};

async fn state() -> Arc<AppState> {
    let state = create_test_state();
    state
        .db
        .save_entries(&[StringEntry::new(
            "old-row",
            "Original text",
            PathBuf::from("story.html"),
        )])
        .unwrap();
    *state.current_project.write().await = Some(ProjectInfo {
        path: PathBuf::from("old-project"),
        format_id: "html".into(),
        name: "Old project".into(),
        extraction_warnings: vec![],
        ..Default::default()
    });
    state
}

fn options() -> TranslationOptions {
    TranslationOptions {
        use_memory: false,
        use_glossary: false,
        cost_limit_usd: Some(1.0),
        ..Default::default()
    }
}

#[tokio::test]
async fn clear_memory_refuses_project_conflict_without_clearing_either_store() {
    let state = state().await;
    state
        .db
        .save_memory("project-hash", "Project", "Proyecto", "en-es")
        .await
        .unwrap();
    state
        .global_memory
        .save_memory("global-hash", "Global", "Global", "en-es")
        .await
        .unwrap();
    let (base, server) = start_test_server(state.clone()).await;
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let held = locust_server::try_project_operation(&state).unwrap();
    let response = client
        .delete(format!("{base}/api/memory"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 409);
    assert_eq!(state.db.memory_count().unwrap(), 1);
    assert_eq!(state.global_memory.memory_count().unwrap(), 1);
    assert_eq!(
        state
            .db
            .lookup_memory("project-hash", "en-es")
            .unwrap()
            .as_deref(),
        Some("Proyecto")
    );
    assert_eq!(
        state
            .global_memory
            .lookup_memory("global-hash", "en-es")
            .unwrap()
            .as_deref(),
        Some("Global")
    );
    drop(held);
    let response = client
        .delete(format!("{base}/api/memory"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(
        response.json::<serde_json::Value>().await.unwrap(),
        serde_json::json!({"ok":true})
    );
    assert_eq!(state.db.memory_count().unwrap(), 0);
    assert_eq!(state.global_memory.memory_count().unwrap(), 0);
    server.abort();
}

async fn start(base: &str, provider: &str) -> reqwest::Response {
    reqwest::Client::new()
        .post(format!("{base}/api/translate/start"))
        .json(&serde_json::json!({"provider_id":provider,"options":options()}))
        .send()
        .await
        .unwrap()
}

#[tokio::test]
async fn translation_start_refuses_a_project_operation_before_dispatch() {
    let state = state().await;
    let (base, server) = start_test_server(state.clone()).await;
    let _held = ProjectExclusiveGuard::enter(&state.project_exclusive);
    let response = start(&base, "mock").await;
    assert_eq!(
        response.status(),
        409,
        "translation must not enter a project owned by font generation/injection/pivot"
    );
    assert!(state.active_jobs.is_empty());
    assert!(state
        .db
        .get_entry("old-row")
        .unwrap()
        .unwrap()
        .translation
        .is_none());
    server.abort();
}

#[derive(Default)]
struct BlockingGate {
    entered: AtomicBool,
    released: Mutex<bool>,
    wake: Condvar,
}
impl BlockingGate {
    fn block(&self) {
        self.entered.store(true, Ordering::SeqCst);
        let held = self.released.lock().unwrap();
        let (held, timeout) = self
            .wake
            .wait_timeout_while(held, Duration::from_secs(15), |released| !*released)
            .unwrap();
        assert!(
            *held && !timeout.timed_out(),
            "test provider was not released"
        );
    }
    fn release(&self) {
        *self.released.lock().unwrap() = true;
        self.wake.notify_all();
    }
}
struct ReleaseOnDrop(Arc<BlockingGate>);
impl Drop for ReleaseOnDrop {
    fn drop(&mut self) {
        self.0.release();
    }
}

struct BlockingEstimate(Arc<BlockingGate>);
#[async_trait]
impl TranslationProvider for BlockingEstimate {
    fn id(&self) -> &str {
        "blocking-estimate"
    }
    fn name(&self) -> &str {
        "Deterministic local estimate"
    }
    fn is_free(&self) -> bool {
        true
    }
    fn requires_api_key(&self) -> bool {
        false
    }
    async fn estimate_cost(&self, _: usize, _: &str) -> Option<f64> {
        self.0.block();
        Some(0.0)
    }
    async fn health_check(&self) -> Result<()> {
        Ok(())
    }
    async fn translate(&self, requests: &[TranslationRequest]) -> Result<Vec<TranslationResult>> {
        Ok(requests
            .iter()
            .map(|request| TranslationResult {
                entry_id: request.entry_id.clone(),
                translation: "Translated local text".into(),
                provider: self.id().into(),
                detected_source_lang: None,
                tokens_used: Some(1),
                input_tokens: Some(1),
                output_tokens: Some(0),
                cost_usd: Some(0.0),
            })
            .collect())
    }
}

async fn wait_entered(gate: &BlockingGate) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !gate.entered.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancelled_job_keeps_project_until_actual_worker_exit() {
    let state = state().await;
    let gate = Arc::new(BlockingGate::default());
    let _release = ReleaseOnDrop(gate.clone());
    state
        .provider_registry
        .write()
        .await
        .register(Arc::new(BlockingEstimate(gate.clone())));
    let (base, server) = start_test_server(state.clone()).await;
    let started = start(&base, "blocking-estimate").await;
    assert_eq!(started.status(), 200);
    let body: serde_json::Value = started.json().await.unwrap();
    let job = body["job_id"].as_str().unwrap();
    wait_entered(&gate).await;
    let cancel = tokio::time::timeout(
        Duration::from_secs(2),
        reqwest::Client::new()
            .post(format!("{base}/api/translate/cancel/{job}"))
            .send(),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        cancel.status(),
        200,
        "cancel must acknowledge without waiting for synchronous work"
    );
    assert_eq!(start(&base, "mock").await.status(), 409);
    let client = reqwest::Client::new();
    for (route, body) in [
        ("/api/pivot", serde_json::json!({"output_path":"unused.db"})),
        (
            "/api/strings/batch",
            serde_json::json!({"updates":[{"id":"old-row","translation":"wrong"}]}),
        ),
        ("/api/validate", serde_json::json!({})),
        (
            "/api/inject",
            serde_json::json!({"project_path":"unused","format_id":"html-game","languages":["en"],"direct":true}),
        ),
        (
            "/api/patch/font",
            serde_json::json!({"game_path":"unused","source_font":"unused.ttf","target_path":"font.ttf","language":"en","output_path":"unused.zip"}),
        ),
    ] {
        assert_eq!(
            client
                .post(format!("{base}{route}"))
                .json(&body)
                .send()
                .await
                .unwrap()
                .status(),
            409,
            "{route}"
        );
    }
    assert_eq!(
        client
            .patch(format!("{base}/api/strings/old-row"))
            .json(&serde_json::json!({"translation":"wrong"}))
            .send()
            .await
            .unwrap()
            .status(),
        409
    );
    for route in ["/api/strings", "/api/stats", "/api/runs"] {
        assert_eq!(
            client
                .get(format!("{base}{route}"))
                .send()
                .await
                .unwrap()
                .status(),
            200,
            "read-only {route}"
        );
    }
    let websocket_url = format!(
        "{}/api/translate/ws/{job}",
        base.replacen("http://", "ws://", 1)
    );
    let (mut websocket, _) = tokio_tungstenite::connect_async(websocket_url)
        .await
        .unwrap();
    let terminal = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let message = websocket.next().await.unwrap().unwrap();
            if let Ok(text) = message.to_text() {
                let event: serde_json::Value = serde_json::from_str(text).unwrap();
                if event["type"] == "failed" {
                    break event;
                }
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(
        terminal["error"], "cancelled",
        "late WebSocket must replay cancellation"
    );
    let other = tempfile::tempdir().unwrap();
    let game = other.path().join("other.html");
    std::fs::write(&game, "<p>Different neutral project</p>").unwrap();
    let open_request = serde_json::json!({"path":game,"format_id":"html-game"});
    let response = reqwest::Client::new()
        .post(format!("{base}/api/project/open"))
        .json(&open_request)
        .send()
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        409,
        "abort is only a request: the worker is still executing its provider estimate"
    );
    assert_eq!(state.db.path(), PathBuf::from(":memory:"));
    assert!(state.db.get_entry("old-row").unwrap().is_some());
    gate.release();
    tokio::time::timeout(Duration::from_secs(5), async {
        while state.project_exclusive.load(Ordering::SeqCst) != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(
        state.active_jobs.contains_key(job),
        "terminal cancellation remains replayable during retention"
    );
    let response = reqwest::Client::new()
        .post(format!("{base}/api/project/open"))
        .json(&open_request)
        .send()
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        200,
        "project must unlock after actual task cleanup"
    );
    server.abort();
}

#[tokio::test]
async fn admission_is_reserved_before_provider_await_and_released_on_invalid_provider() {
    let state = state().await;
    let (base, server) = start_test_server(state.clone()).await;
    let registry = state.provider_registry.write().await;
    let url = base.clone();
    let pending = tokio::spawn(async move { start(&url, "missing-provider").await });
    wait_reserved(&state).await;
    assert!(
        state.active_jobs.is_empty(),
        "reservation must precede job publication"
    );
    assert_eq!(start(&base, "mock").await.status(), 409);
    let response = reqwest::Client::new()
        .post(format!("{base}/api/project/open"))
        .json(&serde_json::json!({"path":"unused","format_id":"html-game"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 409);
    drop(registry);
    assert_eq!(pending.await.unwrap().status(), 404);
    assert_eq!(state.project_exclusive.load(Ordering::SeqCst), 0);
    assert!(state.active_jobs.is_empty());
    server.abort();
}

async fn wait_reserved(state: &AppState) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while state.project_exclusive.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn export_snapshot_blocks_reopen_across_config_await_and_releases_after_response() {
    let state = state().await;
    let (base, server) = start_test_server(state.clone()).await;
    for format in ["po", "xliff"] {
        let config = state.config.write().await;
        let url = format!("{base}/api/export/{format}?lang=en");
        let export = tokio::spawn(async move { reqwest::get(url).await.unwrap() });
        wait_reserved(&state).await;
        let response = reqwest::Client::new()
            .post(format!("{base}/api/project/open"))
            .json(&serde_json::json!({"path":"unused","format_id":"html-game"}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 409);
        drop(config);
        let response = export.await.unwrap();
        assert_eq!(response.status(), 200);
        assert!(response.text().await.unwrap().contains("Original text"));
        assert_eq!(state.project_exclusive.load(Ordering::SeqCst), 0);
    }
    server.abort();
}

#[test]
fn manual_http_write_and_disconnected_owned_task_drain_sqlite_before_unlocking() {
    // Saturating a one-thread blocking pool makes the pending SQLite write
    // deterministic without sleeping or reaching into private Database locks.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .max_blocking_threads(1)
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let state = state().await;
        let (base, server) = start_test_server(state.clone()).await;
        for direct_helper in [false, true] {
            let gate = Arc::new(BlockingGate::default());
            let _release = ReleaseOnDrop(gate.clone());
            let blocked = gate.clone();
            let blocker = tokio::task::spawn_blocking(move || blocked.block());
            wait_entered(&gate).await;
            let pending = if direct_helper {
                let state = state.clone();
                tokio::spawn(async move {
                    let guard = locust_server::try_project_operation(&state).unwrap();
                    locust_server::run_owned_project_operation(guard, async move {
                        state
                            .db
                            .save_translation("old-row", "Owned detached edit", "manual")
                            .await
                            .unwrap();
                    })
                    .await
                    .unwrap();
                })
            } else {
                let url = format!("{base}/api/strings/old-row");
                tokio::spawn(async move {
                    let response = reqwest::Client::new()
                        .patch(url)
                        .json(&serde_json::json!({"translation":"Manual edit","status":"reviewed"}))
                        .send()
                        .await
                        .unwrap();
                    assert_eq!(response.status(), 200);
                })
            };
            wait_reserved(&state).await;
            if direct_helper {
                pending.abort();
                assert!(pending.await.unwrap_err().is_cancelled());
            }
            let response = reqwest::Client::new()
                .post(format!("{base}/api/project/open"))
                .json(&serde_json::json!({"path":"unused","format_id":"html-game"}))
                .send()
                .await
                .unwrap();
            assert_eq!(
                response.status(),
                409,
                "queued blocking write must still own old DB"
            );
            gate.release();
            blocker.await.unwrap();
            tokio::time::timeout(Duration::from_secs(5), async {
                while state.project_exclusive.load(Ordering::SeqCst) != 0 {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            let entry = state.db.get_entry("old-row").unwrap().unwrap();
            assert_eq!(
                entry.translation.as_deref(),
                Some(if direct_helper {
                    "Owned detached edit"
                } else {
                    "Manual edit"
                })
            );
        }
        server.abort();
    });
}
