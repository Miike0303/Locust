use axum::extract::{Path as AxumPath, State};
use axum::http::StatusCode;
use axum::Json;
use locust_core::extraction::PluginInfo;
use locust_core::models::OutputMode;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::{config_persistence, err, update_app_config, ApiError, AppState, XaiPendingAuth};

pub(crate) async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION")
    }))
}

pub(crate) async fn list_formats(State(state): State<Arc<AppState>>) -> Json<Vec<PluginInfo>> {
    Json(state.format_registry.list())
}

#[derive(Serialize)]
pub(crate) struct FormatModes {
    format_id: String,
    supported_modes: Vec<OutputMode>,
}

pub(crate) async fn get_format_modes(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<FormatModes>, ApiError> {
    let plugin = state
        .format_registry
        .get(&id)
        .ok_or_else(|| err(StatusCode::NOT_FOUND, format!("format not found: {}", id)))?;
    Ok(Json(FormatModes {
        format_id: id,
        supported_modes: plugin.supported_modes(),
    }))
}

pub(crate) async fn list_providers(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let reg = state.provider_registry.read().await;
    Json(serde_json::to_value(locust_providers::list_providers_for_api(&reg)).unwrap_or_default())
}

pub(crate) async fn provider_health(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
) -> Json<serde_json::Value> {
    // Registry::get clones the Arc; no registry guard may cross the network await.
    let provider = match state.provider_registry.read().await.get(&id) {
        Some(p) => p,
        None => {
            return Json(serde_json::json!({"ok": false, "message": "provider not found"}));
        }
    };
    // Match the HTTP adapters' existing 30-second health-check limit, including
    // adapters whose implementation has no transport-level timeout.
    match tokio::time::timeout(Duration::from_secs(30), provider.health_check()).await {
        Ok(Ok(())) => Json(serde_json::json!({"ok": true, "message": "healthy"})),
        Ok(Err(e)) => Json(serde_json::json!({"ok": false, "message": e.to_string()})),
        Err(_) => Json(
            serde_json::json!({"ok": false, "message": "Provider health check timed out", "message_key": "providers.healthCheckTimedOut"}),
        ),
    }
}

pub(crate) async fn get_config(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    Json(config_persistence::public_app_config(&state).await)
}

pub(crate) async fn patch_config(
    State(state): State<Arc<AppState>>,
    Json(partial): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    update_app_config(&state, partial).await.map(Json)
}

/// Rebuild the in-process provider registry from the current config.
/// Shared by `PATCH /api/config` and xAI device-login completion so
/// `grok-sub` appears without restarting the process.
pub async fn rebuild_provider_registry(state: &AppState) {
    let (config, mut registry) = config_persistence::lock_config_and_registry(state).await;
    *registry = locust_providers::default_registry(&config);
}

// ─── xAI device-code login ─────────────────────────────────────────────────

const XAI_HANDLE_NOT_FOUND: &str = "handle not found";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct XaiAuthStartResponse {
    pub handle: String,
    pub user_code: String,
    pub verification_uri: String,
    pub expires_in_secs: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum XaiAuthStatus {
    Pending,
    Complete,
    Denied,
    Expired,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct XaiAuthPollResponse {
    pub status: XaiAuthStatus,
}

#[derive(Debug, Deserialize)]
pub struct XaiAuthPollRequest {
    pub handle: String,
}

fn unix_now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn sweep_expired_xai_pending(state: &AppState) {
    let now = unix_now_secs();
    state
        .xai_pending
        .retain(|_, session| session.device.expires_at >= now);
}

/// Start an xAI device-code grant. Shared by HTTP and the Tauri command.
pub async fn start_xai_device_login(state: &AppState) -> Result<XaiAuthStartResponse, String> {
    sweep_expired_xai_pending(state);
    let device_code_url = state.xai_device_code_url.read().await.clone();
    let device = match device_code_url.as_ref() {
        Some(url) => locust_providers::xai_oauth::request_device_code_at(url).await,
        None => locust_providers::xai_oauth::request_device_code().await,
    }
    .map_err(|e| e.to_string())?;

    let handle = uuid::Uuid::new_v4().to_string();
    let response = XaiAuthStartResponse {
        handle: handle.clone(),
        user_code: device.user_code.clone(),
        verification_uri: device.verification_uri.clone(),
        expires_in_secs: device.expires_in,
    };
    state.xai_pending.insert(
        handle,
        XaiPendingAuth {
            device: Arc::new(device),
            last_poll: None,
        },
    );
    Ok(response)
}

/// Poll an in-flight xAI grant. Shared by HTTP and the Tauri command.
pub async fn poll_xai_device_login(
    state: &AppState,
    handle: &str,
) -> Result<XaiAuthPollResponse, String> {
    if handle.is_empty() {
        return Err(XAI_HANDLE_NOT_FOUND.into());
    }

    let device = {
        let Some(mut session) = state.xai_pending.get_mut(handle) else {
            return Err(XAI_HANDLE_NOT_FOUND.into());
        };
        if unix_now_secs() > session.device.expires_at {
            drop(session);
            state.xai_pending.remove(handle);
            return Ok(XaiAuthPollResponse {
                status: XaiAuthStatus::Expired,
            });
        }
        if let Some(last) = session.last_poll {
            if last.elapsed() < Duration::from_secs(session.device.interval()) {
                return Ok(XaiAuthPollResponse {
                    status: XaiAuthStatus::Pending,
                });
            }
        }
        session.last_poll = Some(Instant::now());
        session.device.clone()
    };

    let token_url = state.xai_token_url.read().await.clone();
    let outcome = match token_url.as_ref() {
        Some(url) => locust_providers::xai_oauth::poll_for_token_at(url, &device).await,
        None => locust_providers::xai_oauth::poll_for_token(&device).await,
    }
    .map_err(|e| e.to_string())?;

    let status = match outcome {
        locust_providers::xai_oauth::PollOutcome::Pending => XaiAuthStatus::Pending,
        locust_providers::xai_oauth::PollOutcome::Complete(_) => {
            state.xai_pending.remove(handle);
            rebuild_provider_registry(state).await;
            XaiAuthStatus::Complete
        }
        locust_providers::xai_oauth::PollOutcome::Denied => {
            state.xai_pending.remove(handle);
            XaiAuthStatus::Denied
        }
        locust_providers::xai_oauth::PollOutcome::Expired => {
            state.xai_pending.remove(handle);
            XaiAuthStatus::Expired
        }
    };
    Ok(XaiAuthPollResponse { status })
}

pub(crate) async fn auth_xai_start(
    State(state): State<Arc<AppState>>,
) -> Result<Json<XaiAuthStartResponse>, ApiError> {
    start_xai_device_login(&state)
        .await
        .map(Json)
        .map_err(|e| err(StatusCode::BAD_GATEWAY, e))
}

pub(crate) async fn auth_xai_poll(
    State(state): State<Arc<AppState>>,
    Json(req): Json<XaiAuthPollRequest>,
) -> Result<Json<XaiAuthPollResponse>, ApiError> {
    match poll_xai_device_login(&state, &req.handle).await {
        Ok(body) => Ok(Json(body)),
        Err(e) if e == XAI_HANDLE_NOT_FOUND => Err(err(StatusCode::NOT_FOUND, e)),
        Err(e) => Err(err(StatusCode::BAD_GATEWAY, e)),
    }
}

#[cfg(test)]
mod tests {
    use super::provider_health;
    use crate::routes::test_support::*;
    use crate::*;
    use axum::extract::{Path, State};

    struct PendingHealth(tokio::sync::Notify);

    #[async_trait::async_trait]
    impl locust_core::translation::TranslationProvider for PendingHealth {
        fn id(&self) -> &str {
            "pending-health"
        }
        fn name(&self) -> &str {
            "Pending health"
        }
        fn is_free(&self) -> bool {
            true
        }
        fn requires_api_key(&self) -> bool {
            false
        }
        async fn translate(
            &self,
            _: &[locust_core::models::TranslationRequest],
        ) -> locust_core::error::Result<Vec<locust_core::models::TranslationResult>> {
            unreachable!()
        }
        async fn estimate_cost(&self, _: usize, _: &str) -> Option<f64> {
            None
        }
        async fn health_check(&self) -> locust_core::error::Result<()> {
            self.0.notify_one();
            std::future::pending().await
        }
    }

    #[tokio::test(start_paused = true)]
    async fn pending_health_does_not_block_settings_or_registry_updates() {
        let state = create_test_state();
        let provider = Arc::new(PendingHealth(tokio::sync::Notify::new()));
        state
            .provider_registry
            .write()
            .await
            .register(provider.clone());
        let health = tokio::spawn(provider_health(
            State(state.clone()),
            Path("pending-health".into()),
        ));
        provider.0.notified().await;
        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            update_app_config(&state, serde_json::json!({"default_target_lang": "fr"})),
        )
        .await
        .expect("pending network call blocked settings update")
        .unwrap();
        assert_eq!(state.config.read().await.default_target_lang, "fr");
        health.abort();
    }

    #[tokio::test(start_paused = true)]
    async fn pending_health_times_out() {
        let state = create_test_state();
        state
            .provider_registry
            .write()
            .await
            .register(Arc::new(PendingHealth(tokio::sync::Notify::new())));
        let response = tokio::time::timeout(
            std::time::Duration::from_secs(31),
            provider_health(State(state.clone()), Path("pending-health".into())),
        )
        .await
        .expect("health check was not bounded")
        .0;
        assert_eq!(response["ok"], false);
        assert!(response["message"].as_str().unwrap().contains("timed out"));
        assert!(state.provider_registry.try_write().is_ok());
    }

    #[derive(Clone, Default)]
    struct CapturedLogs(Arc<std::sync::Mutex<Vec<u8>>>);

    impl std::io::Write for CapturedLogs {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for CapturedLogs {
        type Writer = Self;
        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    #[tokio::test]
    async fn provider_secrets_never_reach_batch_failed_events_or_logs() {
        use httpmock::prelude::*;
        use locust_core::models::{ProgressEvent, StringEntry};
        use locust_core::translation::{RetryConfig, TranslationManager, TranslationOptions};

        let server = MockServer::start();
        let secret = "configured-job-secret-sentinel";
        let bearer = "echoed-bearer-secret-sentinel";
        let mock = server.mock(|when, then| {
            when.method(POST)
                .path("/v1/chat/completions")
                .body_contains("Hello");
            then.status(401).body(format!(
                "Rejected {secret}; Authorization: Bearer {bearer}\n{}",
                "x".repeat(1000)
            ));
        });
        let logs = CapturedLogs::default();
        let subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_max_level(tracing::Level::TRACE)
            .with_writer(logs.clone())
            .finish();
        let _subscriber = tracing::subscriber::set_default(subscriber);
        let state = create_test_state();
        let provider = Arc::new(locust_providers::openai::OpenAiProvider::new(
            secret.into(),
            None,
            Some(server.base_url()),
        ));
        let manager = TranslationManager::new(provider, state.db.clone(), state.glossary.clone())
            .with_retry_config(RetryConfig {
                max_attempts: 1,
                ..Default::default()
            });
        let entry = StringEntry::new("secret-test", "Hello", "story.txt".into());
        state.db.save_entries(std::slice::from_ref(&entry)).unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::channel(100);
        manager
            .translate_entries(
                vec![entry],
                TranslationOptions {
                    use_memory: false,
                    use_glossary: false,
                    ..Default::default()
                },
                tx,
                "secret-job".into(),
                tokio_util::sync::CancellationToken::new(),
            )
            .await
            .unwrap();
        let mut failed = false;
        while let Some(event) = rx.recv().await {
            let serialized = serde_json::to_string(&event).unwrap();
            assert!(!serialized.contains(secret));
            assert!(!serialized.contains(bearer));
            if let ProgressEvent::BatchFailed { error, .. } = event {
                failed = true;
                assert!(error.contains("401"));
                assert!(error.contains("[REDACTED]"));
                assert!(error.chars().count() <= 560);
            }
        }
        assert!(failed, "expected a real BatchFailed event");
        let logged = String::from_utf8(logs.0.lock().unwrap().clone()).unwrap();
        assert!(
            logged.contains("Batch translation failed"),
            "expected manager error log"
        );
        assert!(!logged.contains(secret));
        assert!(!logged.contains(bearer));
        mock.assert();
    }

    #[tokio::test]
    async fn test_health_returns_ok() {
        let (url, _h) = setup().await;
        let resp = client()
            .get(format!("{}/health", url))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let body: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(body["status"], "ok");
    }

    #[tokio::test]
    async fn test_list_formats_not_empty() {
        let (url, _h) = setup().await;
        let resp = client()
            .get(format!("{}/api/formats", url))
            .send()
            .await
            .unwrap();
        let body: Vec<serde_json::Value> = resp.json().await.unwrap();
        assert!(!body.is_empty());
    }

    #[tokio::test]
    async fn test_list_providers_not_empty() {
        let (url, _h) = setup().await;
        let resp = client()
            .get(format!("{}/api/providers", url))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let body: Vec<serde_json::Value> = resp.json().await.unwrap();
        assert!(!body.is_empty());
    }

    #[tokio::test]
    async fn test_list_providers_includes_unconfigured_deepl() {
        let (url, _h) = setup().await;
        let resp = client()
            .get(format!("{}/api/providers", url))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let body: Vec<serde_json::Value> = resp.json().await.unwrap();
        let deepl = body
            .iter()
            .find(|p| p["id"] == "deepl")
            .expect("deepl should be listed even without an API key");
        assert_eq!(deepl["configured"], false);
        assert_eq!(deepl["requires_api_key"], true);
    }

    #[tokio::test]
    async fn test_list_providers_deepl_configured_when_key_set() {
        let state = create_test_state();
        {
            let mut config = state.config.write().await;
            config.providers.insert(
                "deepl".to_string(),
                locust_core::config::ProviderConfig {
                    api_key: Some("secret-key-123".to_string()),
                    base_url: None,
                    model: None,
                    free_tier: false,
                    extra: std::collections::HashMap::new(),
                },
            );
            let reg = locust_providers::default_registry(&config);
            *state.provider_registry.write().await = reg;
        }

        let (url, _h) = start_test_server(state).await;
        let resp = client()
            .get(format!("{}/api/providers", url))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let body: Vec<serde_json::Value> = resp.json().await.unwrap();
        let deepl_entries: Vec<_> = body.iter().filter(|p| p["id"] == "deepl").collect();
        assert_eq!(deepl_entries.len(), 1, "deepl must not be duplicated");
        assert_eq!(deepl_entries[0]["configured"], true);
        assert_eq!(deepl_entries[0]["requires_api_key"], true);
    }

    #[tokio::test]
    async fn test_config_api_keys_redacted() {
        let (url, _h, state) = setup_with_state().await;
        {
            let mut config = state.config.write().await;
            config.providers.insert(
                "deepl".to_string(),
                locust_core::config::ProviderConfig {
                    api_key: Some("secret-key-123".to_string()),
                    base_url: None,
                    model: None,
                    free_tier: false,
                    extra: std::collections::HashMap::new(),
                },
            );
        }

        let resp = client()
            .get(format!("{}/api/config", url))
            .send()
            .await
            .unwrap();
        let body: serde_json::Value = resp.json().await.unwrap();
        let deepl_key = body["providers"]["deepl"]["api_key"].as_str().unwrap();
        assert_eq!(deepl_key, "***");
    }
}

#[cfg(test)]
mod xai_auth_tests {
    use super::*;
    use crate::{create_test_state, start_test_server};
    use httpmock::prelude::*;
    use locust_providers::xai_oauth::{set_token_path_override, token_store_test_lock, DeviceCode};

    #[tokio::test]
    async fn xai_network_waits_release_endpoint_override_locks() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let state = create_test_state();
        *state.xai_device_code_url.write().await = Some(url.clone());
        *state.xai_token_url.write().await = Some(url);
        state.xai_pending.insert(
            "pending".into(),
            XaiPendingAuth {
                device: Arc::new(DeviceCode::from_parts(
                    "device-fixture",
                    "ABCD",
                    "https://example.test",
                    1,
                    900,
                    u64::MAX,
                )),
                last_poll: None,
            },
        );
        let start_state = state.clone();
        let start = tokio::spawn(async move { start_xai_device_login(&start_state).await });
        let poll_state = state.clone();
        let poll = tokio::spawn(async move { poll_xai_device_login(&poll_state, "pending").await });
        // Accept both connections without replying, so each helper stays in its
        // network await while the endpoint override locks are inspected.
        let first = tokio::time::timeout(Duration::from_secs(2), listener.accept())
            .await
            .unwrap()
            .unwrap();
        let second = tokio::time::timeout(Duration::from_secs(2), listener.accept())
            .await
            .unwrap()
            .unwrap();
        let start_unlocked = state.xai_device_code_url.try_write().is_ok();
        let poll_unlocked = state.xai_token_url.try_write().is_ok();
        start.abort();
        poll.abort();
        drop((first, second));
        assert!(start_unlocked, "device request retained an endpoint guard");
        assert!(poll_unlocked, "token poll retained an endpoint guard");
    }

    fn assert_no_secrets(body: &serde_json::Value) {
        let dumped = body.to_string();
        assert!(
            !dumped.contains("device_code"),
            "must not echo device_code: {dumped}"
        );
        assert!(
            !dumped.contains("access_token"),
            "must not echo access_token: {dumped}"
        );
        assert!(
            !dumped.contains("refresh_token"),
            "must not echo refresh_token: {dumped}"
        );
    }

    fn mock_device_payload() -> serde_json::Value {
        serde_json::json!({
            "device_code": "dev-abc-must-not-leak",
            "user_code": "WDJB-MJHT",
            "verification_uri": "https://auth.x.ai/device",
            "verification_uri_complete": "https://auth.x.ai/device?user_code=WDJB-MJHT",
            "interval": 5,
            "expires_in": 900
        })
    }

    async fn state_pointing_at(mock: &MockServer) -> Arc<AppState> {
        let state = create_test_state();
        *state.xai_device_code_url.write().await =
            Some(format!("{}/oauth2/device/code", mock.base_url()));
        *state.xai_token_url.write().await = Some(format!("{}/oauth2/token", mock.base_url()));
        state
    }

    fn client() -> reqwest::Client {
        reqwest::Client::new()
    }

    #[tokio::test]
    async fn start_returns_handle_user_code_and_verification_uri() {
        let mock = MockServer::start();
        mock.mock(|when, then| {
            when.method(POST).path("/oauth2/device/code");
            then.status(200).json_body(mock_device_payload());
        });
        let state = state_pointing_at(&mock).await;
        let (url, _h) = start_test_server(state).await;

        let resp = client()
            .post(format!("{}/api/auth/xai/start", url))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let body: serde_json::Value = resp.json().await.unwrap();
        assert_no_secrets(&body);
        let handle = body["handle"].as_str().expect("handle");
        assert!(!handle.is_empty());
        assert_ne!(handle, "dev-abc-must-not-leak");
        assert_eq!(body["user_code"], "WDJB-MJHT");
        assert_eq!(
            body["verification_uri"],
            "https://auth.x.ai/device?user_code=WDJB-MJHT"
        );
        assert_eq!(body["expires_in_secs"], 900);
    }

    #[tokio::test]
    async fn poll_unknown_handle_is_404() {
        let state = create_test_state();
        let (url, _h) = start_test_server(state).await;
        let resp = client()
            .post(format!("{}/api/auth/xai/poll", url))
            .json(&serde_json::json!({"handle": "no-such-handle"}))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 404, "{}", resp.status());
        assert_ne!(resp.status(), 500);
        let text = resp.text().await.unwrap();
        assert!(
            text.to_lowercase().contains("not found"),
            "clean error body: {text}"
        );
    }

    #[tokio::test]
    async fn poll_faster_than_interval_short_circuits_without_upstream() {
        let mock = MockServer::start();
        mock.mock(|when, then| {
            when.method(POST).path("/oauth2/device/code");
            then.status(200).json_body(mock_device_payload());
        });
        let token = mock.mock(|when, then| {
            when.method(POST)
                .path("/oauth2/token")
                .body_contains("device_code=dev-abc-must-not-leak");
            then.status(400)
                .json_body(serde_json::json!({"error": "authorization_pending"}));
        });
        let state = state_pointing_at(&mock).await;
        let (url, _h) = start_test_server(state).await;

        let start: serde_json::Value = client()
            .post(format!("{}/api/auth/xai/start", url))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let handle = start["handle"].as_str().unwrap();

        let first = client()
            .post(format!("{}/api/auth/xai/poll", url))
            .json(&serde_json::json!({"handle": handle}))
            .send()
            .await
            .unwrap();
        assert_eq!(first.status(), 200);
        let first_body: serde_json::Value = first.json().await.unwrap();
        assert_eq!(first_body["status"], "pending");
        assert_eq!(token.hits(), 1);

        let second = client()
            .post(format!("{}/api/auth/xai/poll", url))
            .json(&serde_json::json!({"handle": handle}))
            .send()
            .await
            .unwrap();
        assert_eq!(second.status(), 200);
        let second_body: serde_json::Value = second.json().await.unwrap();
        assert_eq!(second_body["status"], "pending");
        assert_eq!(
            token.hits(),
            1,
            "second poll must not hit the token endpoint"
        );
        assert_no_secrets(&second_body);
    }

    #[tokio::test]
    async fn expired_entry_reports_expired_and_is_removed() {
        let state = create_test_state();
        let handle = uuid::Uuid::new_v4().to_string();
        state.xai_pending.insert(
            handle.clone(),
            XaiPendingAuth {
                device: Arc::new(DeviceCode::from_parts(
                    "raw-device-must-not-leak",
                    "ABCD-EFGH",
                    "https://auth.x.ai/device",
                    5,
                    1,
                    1,
                )),
                last_poll: None,
            },
        );
        let (url, _h) = start_test_server(state.clone()).await;

        let resp = client()
            .post(format!("{}/api/auth/xai/poll", url))
            .json(&serde_json::json!({"handle": handle}))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let body: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(body["status"], "expired");
        assert_no_secrets(&body);
        assert!(
            state.xai_pending.get(&handle).is_none(),
            "expired handle must be dropped"
        );

        let again = client()
            .post(format!("{}/api/auth/xai/poll", url))
            .json(&serde_json::json!({"handle": handle}))
            .send()
            .await
            .unwrap();
        assert_eq!(again.status(), 404);
    }

    // The guard must span the whole test: it serializes the tests that swap the
    // on-disk token file, and `cargo test` runs them on parallel threads. Each
    // `#[tokio::test]` gets its own current-thread runtime, so no other task in
    // this runtime can be waiting on the same lock — the deadlock this lint
    // guards against cannot happen here.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn complete_rebuilds_registry_with_grok_sub() {
        let _lock = token_store_test_lock();
        let token_file_guard = tempfile::Builder::new()
            .prefix("locust_xai_http_test_")
            .tempdir()
            .unwrap();
        let token_file = token_file_guard.path().join("fixture.json");
        struct TokenPathGuard(std::path::PathBuf);
        impl Drop for TokenPathGuard {
            fn drop(&mut self) {
                set_token_path_override(None);
                let _ = std::fs::remove_file(&self.0);
            }
        }
        set_token_path_override(Some(token_file.clone()));
        let _guard = TokenPathGuard(token_file);

        let mock = MockServer::start();
        mock.mock(|when, then| {
            when.method(POST).path("/oauth2/device/code");
            then.status(200).json_body(mock_device_payload());
        });
        mock.mock(|when, then| {
            when.method(POST)
                .path("/oauth2/token")
                .body_contains("device_code=dev-abc-must-not-leak");
            then.status(200).json_body(serde_json::json!({
                "access_token": "access-xyz-must-not-leak",
                "refresh_token": "refresh-secret-must-not-leak",
                "expires_in": 3600
            }));
        });

        let state = state_pointing_at(&mock).await;
        rebuild_provider_registry(&state).await;
        {
            let reg = state.provider_registry.read().await;
            let list = locust_providers::list_providers_for_api(&reg);
            assert!(
                list.iter().all(|p| p.id != "grok-sub"),
                "grok-sub must be absent before login"
            );
        }

        let (url, _h) = start_test_server(state.clone()).await;
        let start: serde_json::Value = client()
            .post(format!("{}/api/auth/xai/start", url))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let handle = start["handle"].as_str().unwrap().to_string();

        let poll = client()
            .post(format!("{}/api/auth/xai/poll", url))
            .json(&serde_json::json!({"handle": handle}))
            .send()
            .await
            .unwrap();
        assert_eq!(poll.status(), 200);
        let body: serde_json::Value = poll.json().await.unwrap();
        assert_eq!(body["status"], "complete");
        assert_no_secrets(&body);
        assert!(state.xai_pending.get(&handle).is_none());

        let providers = client()
            .get(format!("{}/api/providers", url))
            .send()
            .await
            .unwrap()
            .json::<Vec<serde_json::Value>>()
            .await
            .unwrap();
        assert!(
            providers.iter().any(|p| p["id"] == "grok-sub"),
            "grok-sub must be registered after complete: {providers:?}"
        );
    }
}
