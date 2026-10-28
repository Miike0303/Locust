use axum::extract::{Path as AxumPath, State, WebSocketUpgrade};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use locust_core::models::{ProgressEvent, StringStatus};
use locust_core::translation::{
    load_pending_entries, run_fallback_chain, unique_provider_chain, TranslationOptionsRequest,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc};

use super::jobs::{cancel_job, upgrade_job_ws};
use crate::{
    err, publish_sequenced_job_event as publish_job_event, try_project_operation, ApiError,
    AppState, JobKind, JobState, PROJECT_BUSY_MESSAGE, TRANSLATION_IN_FLIGHT_MESSAGE,
};

#[derive(Deserialize)]
pub(crate) struct TranslateStartRequest {
    provider_id: String,
    /// Optional ordered fallbacks after the primary (same chain rules as CLI `--fallback`).
    #[serde(default)]
    fallback_provider_ids: Option<Vec<String>>,
    options: TranslationOptionsRequest,
}

#[derive(Deserialize)]
pub(crate) struct TranslateEstimateRequest {
    provider_id: String,
    options: TranslationOptionsRequest,
}

#[derive(Serialize)]
pub(crate) struct TranslateEstimateResponse {
    provider_id: String,
    effective_model: Option<String>,
    context_window: Option<String>,
    approximate_input_allowance: Option<String>,
    max_batch_tokens: Option<String>,
    batch_size: usize,
    entry_count: usize,
    source_bytes: usize,
    estimated_cost_usd: Option<f64>,
    cost_known: bool,
    approximate: bool,
}

/// Read-only, source-only prospective estimate; not a dispatch or spend receipt.
pub(crate) async fn translate_estimate(
    State(state): State<Arc<AppState>>,
    Json(req): Json<TranslateEstimateRequest>,
) -> Result<Json<TranslateEstimateResponse>, ApiError> {
    let _exclusive = try_project_operation(&state).map_err(|m| err(StatusCode::CONFLICT, m))?;
    if state.current_project.read().await.is_none() {
        return Err(err(StatusCode::BAD_REQUEST, "no project open"));
    }
    let options = req.options.resolve(
        &req.provider_id,
        state.config.read().await.default_batch_size,
    );
    // Registry::get clones the Arc. Never retain its read lock across provider awaits.
    let provider = state
        .provider_registry
        .read()
        .await
        .get(&req.provider_id)
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "provider not found"))?;
    let policy = provider.model_policy();
    let allowance = policy.and_then(|p| p.approximate_input_allowance());
    let max_batch_tokens = match allowance {
        Some(n) => Some(options.max_batch_tokens.unwrap_or(n).min(n)),
        None => options.max_batch_tokens,
    };
    let entries =
        load_pending_entries(&state.db).map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    let eligible = entries.iter().filter(|entry| {
        entry.is_translatable()
            && !(options.skip_approved && entry.status == StringStatus::Approved)
    });
    let (entry_count, source_bytes) = eligible.fold((0, 0), |(count, bytes), entry| {
        (count + 1, bytes + entry.source.len())
    });
    // Match the manager's existing len()-based estimator input (UTF-8 bytes).
    let estimated_cost_usd = provider
        .estimate_cost(source_bytes, &options.target_lang)
        .await
        .filter(|cost| cost.is_finite() && *cost >= 0.0);
    Ok(Json(TranslateEstimateResponse {
        provider_id: req.provider_id,
        effective_model: provider.effective_model().map(str::to_owned),
        context_window: policy.and_then(|p| p.context_window).map(|n| n.to_string()),
        approximate_input_allowance: allowance.map(|n| n.to_string()),
        max_batch_tokens: max_batch_tokens.map(|n| n.to_string()),
        batch_size: options.batch_size,
        entry_count,
        source_bytes,
        estimated_cost_usd,
        cost_known: estimated_cost_usd.is_some(),
        approximate: true,
    }))
}

#[derive(Serialize)]
pub(crate) struct TranslateStartResponse {
    job_id: String,
}

/// Shared by HTTP `POST /api/translate/start` and the Tauri command.
pub async fn spawn_translation_job(
    state: &AppState,
    provider_id: String,
    fallback_provider_ids: Option<Vec<String>>,
    options: impl Into<TranslationOptionsRequest>,
) -> std::result::Result<String, String> {
    let exclusive = try_project_operation(state)?;
    if state.current_project.read().await.is_none() {
        return Err("no project open".into());
    }
    let options = options
        .into()
        .resolve(&provider_id, state.config.read().await.default_batch_size);
    let reg = state.provider_registry.read().await;
    if reg.get(&provider_id).is_none() {
        return Err("provider not found".into());
    }

    let chain = unique_provider_chain(provider_id.as_str(), fallback_provider_ids.as_deref());

    let mut resolve_map: std::collections::HashMap<
        String,
        Arc<dyn locust_core::translation::TranslationProvider>,
    > = std::collections::HashMap::new();
    for id in &chain {
        if let Some(p) = reg.get(id) {
            resolve_map.insert(id.clone(), p);
        }
    }
    drop(reg);

    let job_id = uuid::Uuid::new_v4().to_string();
    let (tx, mut rx) = mpsc::channel::<ProgressEvent>(1000);
    let (broadcast_tx, _) = broadcast::channel::<serde_json::Value>(1000);
    let broadcast_tx_clone = broadcast_tx.clone();

    let cancel = tokio_util::sync::CancellationToken::new();
    let cancel_clone = cancel.clone();
    let job_id_clone = job_id.clone();
    let replay = Arc::new(std::sync::Mutex::new(Vec::new()));
    let replay_bridge = replay.clone();

    // Bridge mpsc → broadcast so WebSocket clients can subscribe.
    // BatchFailed and ProviderSwitched are non-terminal; only Completed / Failed end the stream.
    let jobs = state.active_jobs.clone();
    let cleanup_job_id = job_id.clone();
    tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            let is_terminal = matches!(
                event,
                ProgressEvent::Completed { .. } | ProgressEvent::Failed { .. }
            );
            let json = serde_json::to_value(&event).unwrap_or_default();
            publish_job_event(&broadcast_tx_clone, &replay_bridge, json);
            if is_terminal {
                break;
            }
        }
        // Delay cleanup so WebSocket clients have time to connect and receive final events
        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        jobs.remove(&cleanup_job_id);
    });

    // Insert job BEFORE spawning so WebSocket can find it immediately
    state.active_jobs.insert(
        job_id.clone(),
        JobState {
            abort_handle: tokio::spawn(async {}).abort_handle(), // placeholder
            progress_tx: broadcast_tx,
            replay,
            cancel,
            kind: JobKind::Translate,
            patch_game_key: None,
        },
    );

    let db = state.db.clone();
    let glossary = state.glossary.clone();
    let resolve_map = Arc::new(resolve_map);
    let handle = tokio::spawn(async move {
        // Abort only requests cancellation. Keep ownership until this future
        // actually exits/drops, even while a provider is executing synchronously.
        let _exclusive = exclusive;
        let map = resolve_map;
        let resolve = |id: &str| map.get(id).cloned();
        let result = run_fallback_chain(
            &chain,
            &resolve,
            db,
            glossary,
            options,
            tx.clone(),
            job_id_clone,
            cancel_clone,
        )
        .await;
        if let Err(error) = result {
            let _ = tx
                .send(ProgressEvent::Failed {
                    entry_id: None,
                    error: error.to_string(),
                })
                .await;
        }
    });

    // Update with real abort handle
    if let Some(mut job) = state.active_jobs.get_mut(&job_id) {
        job.abort_handle = handle.abort_handle();
    }

    Ok(job_id)
}

pub(crate) async fn translate_start(
    State(state): State<Arc<AppState>>,
    Json(req): Json<TranslateStartRequest>,
) -> Result<Json<TranslateStartResponse>, ApiError> {
    let job_id = spawn_translation_job(
        &state,
        req.provider_id,
        req.fallback_provider_ids,
        req.options,
    )
    .await
    .map_err(|m| {
        let status = if m == PROJECT_BUSY_MESSAGE || m == TRANSLATION_IN_FLIGHT_MESSAGE {
            StatusCode::CONFLICT
        } else if m == "no project open" {
            StatusCode::BAD_REQUEST
        } else {
            StatusCode::NOT_FOUND
        };
        err(status, m)
    })?;
    Ok(Json(TranslateStartResponse { job_id }))
}

pub(crate) async fn translate_cancel(
    State(state): State<Arc<AppState>>,
    AxumPath(job_id): AxumPath<String>,
) -> Result<StatusCode, ApiError> {
    cancel_job(&state, &job_id)
}

/// Desktop cancellation uses the same token and terminal replay as HTTP.
/// Acknowledgement is immediate; the worker still owns its project reservation.
pub fn cancel_translation_job(state: &AppState, job_id: &str) -> Result<(), String> {
    cancel_job(state, job_id)
        .map(|_| ())
        .map_err(|(_, message)| message)
}

pub(crate) async fn translate_ws(
    State(state): State<Arc<AppState>>,
    AxumPath(job_id): AxumPath<String>,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    upgrade_job_ws(
        state,
        job_id,
        ws,
        serde_json::json!({"type": "failed", "error": "job not found"}),
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routes::test_support::*;
    use locust_core::models::StringEntry;
    use locust_core::translation::TranslationOptions;
    use std::path::PathBuf;

    struct EstimateProvider {
        model: Option<String>,
        policy: Option<locust_core::model_policy::ModelPolicy>,
        cost: Option<f64>,
        calls: std::sync::Mutex<Vec<(usize, String)>>,
        gate: Option<(tokio::sync::Notify, tokio::sync::Notify)>,
    }

    #[async_trait::async_trait]
    impl locust_core::translation::TranslationProvider for EstimateProvider {
        fn id(&self) -> &str {
            "estimate-fixture"
        }
        fn name(&self) -> &str {
            "Estimate fixture"
        }
        fn is_free(&self) -> bool {
            false
        }
        fn requires_api_key(&self) -> bool {
            false
        }
        fn effective_model(&self) -> Option<&str> {
            self.model.as_deref()
        }
        fn model_policy(&self) -> Option<&locust_core::model_policy::ModelPolicy> {
            self.policy.as_ref()
        }
        async fn translate(
            &self,
            _: &[locust_core::models::TranslationRequest],
        ) -> locust_core::Result<Vec<locust_core::models::TranslationResult>> {
            panic!("estimate must not translate")
        }
        async fn health_check(&self) -> locust_core::Result<()> {
            panic!("estimate must not check health")
        }
        async fn estimate_cost(&self, bytes: usize, target: &str) -> Option<f64> {
            self.calls.lock().unwrap().push((bytes, target.into()));
            if let Some((entered, release)) = &self.gate {
                entered.notify_one();
                release.notified().await;
            }
            self.cost
        }
    }

    fn estimate_options() -> serde_json::Value {
        serde_json::json!({"source_lang":"ja", "target_lang":"es", "use_glossary":true,
            "use_memory":true, "skip_approved":true})
    }

    async fn estimate_post(url: &str, options: serde_json::Value) -> reqwest::Response {
        client()
            .post(format!("{url}/api/translate/estimate"))
            .json(&serde_json::json!({"provider_id":"estimate-fixture", "options":options}))
            .send()
            .await
            .unwrap()
    }

    async fn estimate_snapshot(url: &str, state: &AppState) -> Vec<serde_json::Value> {
        let mut values = Vec::new();
        for path in [
            "/api/project/current",
            "/api/strings",
            "/api/runs",
            "/api/stats",
        ] {
            let response = client().get(format!("{url}{path}")).send().await.unwrap();
            assert_eq!(response.status(), StatusCode::OK, "{path}");
            values.push(response.json().await.unwrap());
        }
        assert!(state.active_jobs.is_empty());
        assert!(crate::active_translation_job(state).is_none());
        values
    }

    #[tokio::test]
    async fn estimate_http_policy_cost_and_lossless_caps() {
        use locust_core::model_policy::{lookup_model_policy, ModelOverrides, ModelPolicy};
        use locust_core::models::StringStatus;
        let (url, _server, state) = setup_with_state().await;
        mark_project_open(&state).await;
        let mut entries: Vec<_> = ["日本語", "é", "  ", "approved", "translated", "error"]
            .iter()
            .enumerate()
            .map(|(i, source)| StringEntry::new(i.to_string(), *source, "f.json".into()))
            .collect();
        entries[3].status = StringStatus::Approved;
        entries[4].status = StringStatus::Translated;
        entries[5].status = StringStatus::Error;
        state.db.save_entries(&entries).unwrap();
        state.config.write().await.default_batch_size = Some(17);
        let before = estimate_snapshot(&url, &state).await;
        // Each case exercises the actual HTTP boundary; costs are the provider's existing estimate.
        let mut missing = Vec::new();
        for (label, model, window, matched, cost, cap, expected_cap, batch) in [
            (
                "absent",
                None,
                None,
                true,
                Some(0.25),
                None,
                Some("6000"),
                17,
            ),
            (
                "unmatched",
                Some("other"),
                Some(800),
                false,
                Some(0.25),
                None,
                Some("6000"),
                17,
            ),
            (
                "context-only",
                Some("model"),
                Some(800),
                true,
                Some(0.25),
                None,
                Some("600"),
                17,
            ),
            (
                "priced",
                Some("model"),
                None,
                true,
                Some(1.5),
                None,
                Some("6000"),
                17,
            ),
            (
                "unknown",
                Some("model"),
                None,
                true,
                None,
                None,
                Some("6000"),
                17,
            ),
            (
                "zero",
                Some("model"),
                None,
                true,
                Some(0.0),
                None,
                Some("6000"),
                17,
            ),
            (
                "nan",
                None,
                None,
                true,
                Some(f64::NAN),
                None,
                Some("6000"),
                17,
            ),
            (
                "infinity",
                None,
                None,
                true,
                Some(f64::INFINITY),
                None,
                Some("6000"),
                17,
            ),
            (
                "negative",
                None,
                None,
                true,
                Some(-1.0),
                None,
                Some("6000"),
                17,
            ),
            (
                "null",
                Some("model"),
                None,
                true,
                Some(0.25),
                Some(serde_json::Value::Null),
                None,
                17,
            ),
            (
                "null-model",
                Some("model"),
                Some(800),
                true,
                Some(0.25),
                Some(serde_json::Value::Null),
                Some("600"),
                17,
            ),
            (
                "clamp",
                Some("model"),
                Some(800),
                true,
                Some(0.25),
                Some(300.into()),
                Some("300"),
                7,
            ),
            (
                "no-room",
                Some("model"),
                Some(1),
                true,
                Some(0.25),
                None,
                Some("0"),
                17,
            ),
            (
                "u64-max",
                Some("model"),
                Some(u64::MAX),
                true,
                Some(0.25),
                Some(serde_json::Value::Null),
                Some("13835058055282163712"),
                17,
            ),
        ] {
            let policy = ModelPolicy {
                context_window: window,
                input_usd_per_million: (label == "priced").then_some(2.0),
                output_usd_per_million: (label == "priced").then_some(6.0),
            };
            let overrides = ModelOverrides::from([(
                "estimate-fixture".into(),
                std::collections::HashMap::from([("model".into(), policy)]),
            )]);
            let provider = Arc::new(EstimateProvider {
                model: model.map(str::to_owned),
                policy: lookup_model_policy(
                    &overrides,
                    "estimate-fixture",
                    if matched { "model" } else { "other" },
                )
                .cloned(),
                cost,
                calls: Default::default(),
                gate: None,
            });
            let mut registry = locust_core::translation::ProviderRegistry::new();
            registry.register(provider.clone());
            *state.provider_registry.write().await = registry;
            let mut options = estimate_options();
            if let Some(cap) = cap {
                options["max_batch_tokens"] = cap;
            }
            if batch == 7 {
                options["batch_size"] = 7.into();
            }
            let response = estimate_post(&url, options).await;
            if response.status() != StatusCode::OK {
                missing.push(format!("{label}: expected 200, got {}", response.status()));
                continue;
            }
            let body: serde_json::Value = response.json().await.unwrap();
            let known = cost.filter(|c| c.is_finite() && *c >= 0.0);
            assert_eq!(
                body,
                serde_json::json!({
                    "provider_id":"estimate-fixture", "effective_model":model,
                    "context_window": provider.policy.as_ref().and_then(|p| p.context_window).map(|n| n.to_string()),
                    "approximate_input_allowance": provider.policy.as_ref().and_then(ModelPolicy::approximate_input_allowance).map(|n| n.to_string()),
                    "max_batch_tokens":expected_cap, "batch_size":batch, "entry_count":2,
                    "source_bytes":11, "estimated_cost_usd":known, "cost_known":known.is_some(), "approximate":true
                }),
                "{label}"
            );
            assert_eq!(
                *provider.calls.lock().unwrap(),
                [(11, "es".into())],
                "{label}"
            );
            assert_eq!(estimate_snapshot(&url, &state).await, before, "{label}");
            assert!(
                try_project_operation(&state).is_ok(),
                "reservation released: {label}"
            );
        }
        assert!(missing.is_empty(), "{}", missing.join("\n"));
    }

    #[tokio::test]
    async fn estimate_http_errors_do_not_mutate() {
        let (url, _server, state) = setup_with_state().await;
        state
            .db
            .save_entries(&[StringEntry::new("stale", "Untouched", "f.json".into())])
            .unwrap();
        let absent = estimate_post(&url, estimate_options()).await.status();
        mark_project_open(&state).await;
        let before = estimate_snapshot(&url, &state).await;
        assert_eq!(before[1]["entries"][0]["source"], "Untouched");
        let unknown = estimate_post(&url, estimate_options()).await.status();
        let guard = try_project_operation(&state).unwrap();
        let busy = estimate_post(&url, estimate_options()).await.status();
        drop(guard);
        assert_eq!(estimate_snapshot(&url, &state).await, before);
        assert_eq!(
            (absent, unknown, busy),
            (
                StatusCode::BAD_REQUEST,
                StatusCode::NOT_FOUND,
                StatusCode::CONFLICT
            )
        );
    }

    #[tokio::test]
    async fn memory_http_after_project_open_and_dialog_reads_including_pending_estimate() {
        let (url, _server, state) = setup_with_state().await;
        let root = tempfile::tempdir().unwrap();
        let game = root.path().join("project");
        std::fs::create_dir_all(game.join("game")).unwrap();
        std::fs::write(
            game.join("game/script.rpy"),
            "label start:\n    \"Hello\"\n    return\n",
        )
        .unwrap();
        let response = client()
            .post(format!("{url}/api/project/open"))
            .json(&serde_json::json!({"path": game, "format_id": "renpy"}))
            .send()
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "{}",
            response.text().await.unwrap()
        );
        assert!(
            try_project_operation(&state).is_ok(),
            "project open releases its reservation"
        );
        state
            .db
            .save_memory("hello", "Hello", "Hola", "en-es")
            .await
            .unwrap();

        // Inject uses cached stats; recovery/status and Patch's initialization
        // reads must also leave the project available after their response.
        for path in ["/api/stats", "/api/patch/recordings", "/api/config"] {
            assert_eq!(
                client()
                    .get(format!("{url}{path}"))
                    .send()
                    .await
                    .unwrap()
                    .status(),
                StatusCode::OK,
                "{path}"
            );
            assert!(try_project_operation(&state).is_ok(), "{path}");
        }
        for path in ["/api/inject/status", "/api/patch/status"] {
            assert_eq!(
                client()
                    .post(format!("{url}{path}"))
                    .json(&serde_json::json!({"game_path": game}))
                    .send()
                    .await
                    .unwrap()
                    .status(),
                StatusCode::OK,
                "{path}"
            );
            assert!(try_project_operation(&state).is_ok(), "{path}");
        }

        let provider = Arc::new(EstimateProvider {
            model: None,
            policy: None,
            cost: None,
            calls: Default::default(),
            gate: Some(Default::default()),
        });
        state
            .provider_registry
            .write()
            .await
            .register(provider.clone());
        let request_url = url.clone();
        let request =
            tokio::spawn(async move { estimate_post(&request_url, estimate_options()).await });
        let (entered, release) = provider.gate.as_ref().unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), entered.notified())
            .await
            .unwrap();
        assert!(
            try_project_operation(&state).is_err(),
            "estimate owns the reservation while awaiting provider cost"
        );
        // Closing a dialog does not guarantee its HTTP estimate has finished.
        // Memory must remain readable both during and after that request.
        let response = client()
            .get(format!("{url}/api/memory?limit=50"))
            .send()
            .await
            .unwrap();
        release.notify_one();
        assert_eq!(request.await.unwrap().status(), StatusCode::OK);
        assert!(
            try_project_operation(&state).is_ok(),
            "estimate releases its reservation"
        );
        assert!(
            state.active_jobs.is_empty(),
            "dialog reads do not start background jobs"
        );
        assert_eq!(response.status(), StatusCode::OK);
        let body: serde_json::Value = response.json().await.unwrap();
        assert_eq!(body["total"], 1);
        let (entries, stats, pairs) = tokio::join!(
            client().get(format!("{url}/api/memory?limit=50")).send(),
            client().get(format!("{url}/api/memory/stats")).send(),
            client().get(format!("{url}/api/memory/lang-pairs")).send(),
        );
        for response in [entries, stats, pairs] {
            assert_eq!(response.unwrap().status(), StatusCode::OK);
        }
        assert!(try_project_operation(&state).is_ok());
    }

    #[tokio::test]
    async fn estimate_http_temporary_reservation_and_empty_project() {
        let (url, _server, state) = setup_with_state().await;
        mark_project_open(&state).await;
        let before = estimate_snapshot(&url, &state).await;
        let provider = Arc::new(EstimateProvider {
            model: None,
            policy: None,
            cost: None,
            calls: Default::default(),
            gate: Some(Default::default()),
        });
        state
            .provider_registry
            .write()
            .await
            .register(provider.clone());
        let request_url = url.clone();
        let request =
            tokio::spawn(async move { estimate_post(&request_url, estimate_options()).await });
        let (entered, release) = provider.gate.as_ref().unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), entered.notified())
            .await
            .unwrap();
        // The async estimator holds only the project reservation, never the registry lock.
        let registry = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            state.provider_registry.write(),
        )
        .await
        .unwrap();
        drop(registry);
        let blocked = client()
            .post(format!("{url}/api/translate/start"))
            .json(&serde_json::json!({"provider_id":"mock", "options":estimate_options()}))
            .send()
            .await
            .unwrap();
        assert_eq!(blocked.status(), StatusCode::CONFLICT);
        assert_eq!(estimate_snapshot(&url, &state).await, before);
        release.notify_one();
        let response = request.await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body: serde_json::Value = response.json().await.unwrap();
        assert_eq!(body["entry_count"], 0);
        assert_eq!(body["source_bytes"], 0);
        assert_eq!(*provider.calls.lock().unwrap(), [(0, "es".into())]);
        assert_eq!(estimate_snapshot(&url, &state).await, before);
        assert!(try_project_operation(&state).is_ok());
    }

    #[tokio::test]
    async fn translation_start_without_project_keeps_stale_rows_untouched() {
        let (url, _h, state) = setup_with_state().await;
        state
            .db
            .save_entries(&[StringEntry::new(
                "stale",
                "Old project text",
                PathBuf::from("story.html"),
            )])
            .unwrap();

        let response = client()
            .post(format!("{url}/api/translate/start"))
            .json(&serde_json::json!({
                "provider_id": "mock",
                "options": TranslationOptions::default()
            }))
            .send()
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(response.text().await.unwrap(), "no project open");
        assert!(state.active_jobs.is_empty());
        assert!(state
            .db
            .get_entry("stale")
            .unwrap()
            .unwrap()
            .translation
            .is_none());
    }

    #[tokio::test]
    async fn test_translate_start_returns_job_id() {
        let (url, _h, state) = setup_with_state().await;
        mark_project_open(&state).await;
        let entry = StringEntry::new("t1", "Hello", PathBuf::from("f.json"));
        state.db.save_entries(&[entry]).unwrap();

        let resp = client()
            .post(format!("{}/api/translate/start", url))
            .json(&serde_json::json!({
                "provider_id": "mock",
                "options": TranslationOptions::default()
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let body: serde_json::Value = resp.json().await.unwrap();
        assert!(body.get("job_id").is_some());
    }

    #[test]
    fn provider_translation_defaults_wire_options() {
        for (provider, expected) in [
            ("google", (200, 4, Some(40_000))),
            ("antigravity", (100, 4, Some(20_000))),
            ("grok", (10, 10, Some(6000))),
            ("grok-sub", (10, 10, Some(6000))),
            ("mock", (40, 1, Some(6000))),
        ] {
            for explicit in [false, true] {
                let mut options = serde_json::json!({
                    "source_lang": "ja", "target_lang": "en",
                    "use_glossary": false, "use_memory": false, "skip_approved": true
                });
                if explicit {
                    options["batch_size"] = 7.into();
                    options["max_concurrent"] = 2.into();
                    options["max_batch_tokens"] = 900.into();
                }
                let params: TranslateStartRequest = serde_json::from_value(
                    serde_json::json!({"provider_id": provider, "options": options}),
                )
                .unwrap();
                let resolved = params.options.resolve(&params.provider_id, None);
                assert_eq!(
                    (
                        resolved.batch_size,
                        resolved.max_concurrent,
                        resolved.max_batch_tokens
                    ),
                    if explicit {
                        (7, 2, Some(900))
                    } else {
                        expected
                    },
                    "{provider}"
                );
            }
        }
    }
    #[tokio::test]
    async fn provider_translation_defaults_accept_omitted_request_fields() {
        let (url, _h, state) = setup_with_state().await;
        mark_project_open(&state).await;
        let response = client()
            .post(format!("{url}/api/translate/start"))
            .json(&serde_json::json!({
                "provider_id": "mock",
                "options": {
                    "source_lang": "ja", "target_lang": "en",
                    "use_glossary": true, "use_memory": true, "skip_approved": true
                }
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200, "{}", response.text().await.unwrap());
    }

    #[tokio::test]
    async fn test_translate_start_accepts_fallback_provider_ids() {
        let (url, _h, state) = setup_with_state().await;
        mark_project_open(&state).await;
        let entry = StringEntry::new("t1", "Hello", PathBuf::from("f.json"));
        state.db.save_entries(&[entry]).unwrap();

        // Optional field present — same status as without it (primary must exist).
        let resp = client()
            .post(format!("{}/api/translate/start", url))
            .json(&serde_json::json!({
                "provider_id": "mock",
                "fallback_provider_ids": ["mock"],
                "options": TranslationOptions::default()
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let body: serde_json::Value = resp.json().await.unwrap();
        assert!(body.get("job_id").is_some());
    }

    #[tokio::test]
    async fn test_translate_start_without_fallback_field_unchanged() {
        let (url, _h, state) = setup_with_state().await;
        mark_project_open(&state).await;
        let entry = StringEntry::new("t2", "World", PathBuf::from("f.json"));
        state.db.save_entries(&[entry]).unwrap();

        // Omitting fallback_provider_ids must still deserialize and run.
        let resp = client()
            .post(format!("{}/api/translate/start", url))
            .json(&serde_json::json!({
                "provider_id": "mock",
                "options": {
                    "source_lang": "en",
                    "target_lang": "es",
                    "batch_size": 40,
                    "max_concurrent": 1,
                    "cost_limit_usd": null,
                    "game_context": null,
                    "use_glossary": false,
                    "use_memory": false,
                    "skip_approved": true
                }
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            200,
            "missing fallback_provider_ids must not break start: {}",
            resp.text().await.unwrap_or_default()
        );
    }

    #[tokio::test]
    async fn test_translate_cancel() {
        let (url, _h, state) = setup_with_state().await;
        mark_project_open(&state).await;
        let entry = StringEntry::new("t1", "Hello", PathBuf::from("f.json"));
        state.db.save_entries(&[entry]).unwrap();

        let resp = client()
            .post(format!("{}/api/translate/start", url))
            .json(&serde_json::json!({
                "provider_id": "mock",
                "options": TranslationOptions::default()
            }))
            .send()
            .await
            .unwrap();
        let body: serde_json::Value = resp.json().await.unwrap();
        let job_id = body["job_id"].as_str().unwrap();

        let resp = client()
            .post(format!("{}/api/translate/cancel/{}", url, job_id))
            .send()
            .await
            .unwrap();
        assert!(resp.status() == 200 || resp.status() == 404); // may have already finished
    }
}
