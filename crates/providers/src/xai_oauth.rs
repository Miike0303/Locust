//! xAI OAuth device-code flow for SuperGrok / X Premium+ subscriptions.
//!
//! Lets Locust translate through the user's Grok subscription without an
//! API key. Access tokens live ~6h and are refreshed automatically using
//! the stored refresh token. Note: xAI gates this surface server-side and
//! may return 403 for some subscription tiers.

use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use locust_core::config::AppConfig;
use locust_core::error::{LocustError, Result};
use locust_core::models::{TranslationRequest, TranslationResult};
use locust_core::translation::TranslationProvider;

use crate::openai::OpenAiProvider;

const DEVICE_CODE_URL: &str = "https://auth.x.ai/oauth2/device/code";
const TOKEN_URL: &str = "https://auth.x.ai/oauth2/token";
const CLIENT_ID: &str = "b1a00492-073a-47ea-816f-4c329264a828";
const SCOPE: &str = "openid profile email offline_access grok-cli:access api:access";
const API_BASE_URL: &str = "https://api.x.ai";
const DEVICE_LOGIN_TIMEOUT: Duration = Duration::from_secs(30);
/// Refresh when less than this many seconds of validity remain.
const REFRESH_SKEW_SECS: u64 = 3600;

#[derive(Serialize, Deserialize, Clone)]
pub struct TokenStore {
    pub access_token: String,
    pub refresh_token: String,
    /// Unix seconds when the access token expires.
    pub expires_at: u64,
}

// Deliberately hand-written instead of derived: `PollOutcome` is `Debug`, so a
// derive here would print live xAI credentials into tracing output and into the
// panic message of any test that asserts on a `PollOutcome`.
impl std::fmt::Debug for TokenStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TokenStore")
            .field("access_token", &"<redacted>")
            .field("refresh_token", &"<redacted>")
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

static TOKEN_PATH_OVERRIDE: RwLock<Option<PathBuf>> = RwLock::new(None);

/// Serializes tests that touch the on-disk token file so they never clobber
/// a real `xai-oauth.json` or each other.
pub fn token_store_test_lock() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// Point `token_path()` at a temp file. Callers must hold `token_store_test_lock`.
pub fn set_token_path_override(path: Option<PathBuf>) {
    *TOKEN_PATH_OVERRIDE
        .write()
        .unwrap_or_else(|e| e.into_inner()) = path;
}

pub fn token_path() -> PathBuf {
    TOKEN_PATH_OVERRIDE
        .read()
        .ok()
        .and_then(|g| g.clone())
        .unwrap_or_else(|| AppConfig::config_dir().join("xai-oauth.json"))
}

pub fn load_tokens() -> Option<TokenStore> {
    let raw = std::fs::read_to_string(token_path()).ok()?;
    serde_json::from_str(&raw).ok()
}

fn save_tokens(tokens: &TokenStore) -> Result<()> {
    save_tokens_with(tokens, |file, bytes| file.write_all(bytes))
}

fn save_tokens_with(
    tokens: &TokenStore,
    write: impl FnOnce(&mut std::fs::File, &[u8]) -> std::io::Result<()>,
) -> Result<()> {
    let path = token_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(tokens)?;
    locust_core::export::write_file_atomically(&path, |file| {
        // Restrict the staging file before writing any secrets. Windows relies
        // on the NTFS ACLs inherited from the per-user data directory.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        }
        write(file, json.as_bytes())
    })?;
    Ok(())
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[derive(Deserialize)]
struct DeviceCodeResponse {
    device_code: String,
    user_code: String,
    verification_uri: String,
    #[serde(default)]
    verification_uri_complete: Option<String>,
    #[serde(default = "default_interval")]
    interval: u64,
    expires_in: u64,
}

fn default_interval() -> u64 {
    5
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default = "default_expires")]
    expires_in: u64,
}

fn default_expires() -> u64 {
    6 * 3600
}

#[derive(Deserialize)]
struct TokenErrorResponse {
    error: String,
}

/// One device-code grant, ready for the GUI or for `device_login` to poll.
#[derive(Debug)]
pub struct DeviceCode {
    pub device_code: String,
    pub user_code: String,
    /// `verification_uri_complete` when the IdP sent one, else `verification_uri`.
    pub verification_uri: String,
    interval: AtomicU64,
    pub expires_in: u64,
    pub expires_at: u64,
}

impl Clone for DeviceCode {
    fn clone(&self) -> Self {
        Self {
            device_code: self.device_code.clone(),
            user_code: self.user_code.clone(),
            verification_uri: self.verification_uri.clone(),
            interval: AtomicU64::new(self.interval()),
            expires_in: self.expires_in,
            expires_at: self.expires_at,
        }
    }
}

impl DeviceCode {
    pub fn interval(&self) -> u64 {
        self.interval.load(Ordering::Relaxed)
    }

    fn bump_interval(&self) {
        self.interval.fetch_add(5, Ordering::Relaxed);
    }

    pub fn from_parts(
        device_code: impl Into<String>,
        user_code: impl Into<String>,
        verification_uri: impl Into<String>,
        interval: u64,
        expires_in: u64,
        expires_at: u64,
    ) -> Self {
        Self {
            device_code: device_code.into(),
            user_code: user_code.into(),
            verification_uri: verification_uri.into(),
            interval: AtomicU64::new(interval.max(1)),
            expires_in,
            expires_at,
        }
    }
}

/// One poll of the token endpoint.
#[derive(Debug)]
pub enum PollOutcome {
    Pending,
    Complete(TokenStore),
    Denied,
    Expired,
}

/// Fetch a device code from the production xAI endpoint.
pub async fn request_device_code() -> Result<DeviceCode> {
    request_device_code_at(DEVICE_CODE_URL).await
}

/// Fetch a device code from an explicit URL (tests / injected endpoints).
pub async fn request_device_code_at(url: &str) -> Result<DeviceCode> {
    request_device_code_with_timeout(url, DEVICE_LOGIN_TIMEOUT).await
}

fn device_login_timeout(operation: &str) -> LocustError {
    LocustError::ProviderError(format!("xAI {operation} timed out; retry login"))
}

fn device_login_error(
    error: reqwest::Error,
    operation: &str,
    context: &str,
    secrets: &[&str],
) -> LocustError {
    if error.is_timeout() {
        device_login_timeout(operation)
    } else {
        LocustError::ProviderError(crate::error_sanitizer::sanitize_provider_error(
            &format!("{context}: {error}"),
            secrets,
        ))
    }
}

async fn request_device_code_with_timeout(url: &str, timeout: Duration) -> Result<DeviceCode> {
    let request = async {
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .map_err(|e| {
                device_login_error(e, "device code request", "device login client failed", &[])
            })?;
        let resp = client
            .post(url)
            .form(&[("client_id", CLIENT_ID), ("scope", SCOPE)])
            .send()
            .await
            .map_err(|e| {
                device_login_error(e, "device code request", "device code request failed", &[])
            })?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = match resp.text().await {
                Ok(body) => crate::error_sanitizer::sanitize_provider_error(&body, &[]),
                Err(e) if e.is_timeout() => {
                    return Err(device_login_timeout("device code request"))
                }
                Err(_) => String::new(),
            };
            return Err(LocustError::ProviderError(
                crate::error_sanitizer::sanitize_provider_error(
                    &format!("device code request returned {}: {}", status, body),
                    &[],
                ),
            ));
        }

        let dc: DeviceCodeResponse = resp.json().await.map_err(|e| {
            device_login_error(
                e,
                "device code request",
                "malformed device code response",
                &[],
            )
        })?;

        let verification_uri = dc.verification_uri_complete.unwrap_or(dc.verification_uri);
        Ok(DeviceCode::from_parts(
            dc.device_code,
            dc.user_code,
            verification_uri,
            dc.interval,
            dc.expires_in,
            now_secs().saturating_add(dc.expires_in),
        ))
    };
    // One budget covers connection setup, headers and all response decoding.
    tokio::time::timeout(timeout, request)
        .await
        .map_err(|_| device_login_timeout("device code request"))?
}

/// One poll of the production token endpoint.
pub async fn poll_for_token(device: &DeviceCode) -> Result<PollOutcome> {
    poll_for_token_at(TOKEN_URL, device).await
}

/// One poll of an explicit token URL. Persists tokens on `Complete`.
pub async fn poll_for_token_at(url: &str, device: &DeviceCode) -> Result<PollOutcome> {
    poll_for_token_with_timeout(url, device, DEVICE_LOGIN_TIMEOUT).await
}

async fn poll_for_token_with_timeout(
    url: &str,
    device: &DeviceCode,
    timeout: Duration,
) -> Result<PollOutcome> {
    if now_secs() > device.expires_at {
        return Ok(PollOutcome::Expired);
    }

    let request = async {
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .map_err(|e| {
                device_login_error(
                    e,
                    "token poll",
                    "device login client failed",
                    &[&device.device_code],
                )
            })?;
        let resp = client
            .post(url)
            .form(&[
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                ("device_code", device.device_code.as_str()),
                ("client_id", CLIENT_ID),
            ])
            .send()
            .await
            .map_err(|e| {
                device_login_error(e, "token poll", "token poll failed", &[&device.device_code])
            })?;

        if resp.status().is_success() {
            let tr: TokenResponse = resp.json().await.map_err(|e| {
                device_login_error(
                    e,
                    "token poll",
                    "malformed token response",
                    &[&device.device_code],
                )
            })?;
            let store = TokenStore {
                access_token: tr.access_token,
                refresh_token: tr.refresh_token.unwrap_or_default(),
                expires_at: now_secs() + tr.expires_in,
            };
            save_tokens(&store)?;
            return Ok(PollOutcome::Complete(store));
        }

        let err: TokenErrorResponse = match resp.json().await {
            Ok(error) => error,
            Err(e) if e.is_timeout() => return Err(device_login_timeout("token poll")),
            Err(_) => TokenErrorResponse {
                error: "unknown".to_string(),
            },
        };
        match err.error.as_str() {
            "authorization_pending" => Ok(PollOutcome::Pending),
            "slow_down" => {
                device.bump_interval();
                Ok(PollOutcome::Pending)
            }
            "access_denied" => Ok(PollOutcome::Denied),
            "expired_token" => Ok(PollOutcome::Expired),
            other => Err(LocustError::ProviderError(format!(
                "login failed: {}",
                crate::error_sanitizer::sanitize_provider_error(other, &[&device.device_code])
            ))),
        }
    };
    tokio::time::timeout(timeout, request)
        .await
        .map_err(|_| device_login_timeout("token poll"))?
}

/// Run the browser device-code login and persist tokens.
///
/// Thin composition of `request_device_code` + `poll_for_token` that keeps
/// the CLI (`locust auth grok`) output unchanged.
pub async fn device_login() -> Result<TokenStore> {
    let dc = request_device_code().await?;
    println!("Open this URL in your browser and approve access:");
    println!("\n  {}\n", dc.verification_uri);
    println!("Code: {}", dc.user_code);
    println!("Waiting for approval...");

    loop {
        if now_secs() > dc.expires_at {
            return Err(LocustError::ProviderError(
                "login timed out — run `locust auth grok` again".to_string(),
            ));
        }
        tokio::time::sleep(Duration::from_secs(dc.interval())).await;

        match poll_for_token(&dc).await? {
            PollOutcome::Pending => continue,
            PollOutcome::Complete(store) => return Ok(store),
            PollOutcome::Denied => {
                return Err(LocustError::ProviderError(
                    "login failed: access_denied".to_string(),
                ));
            }
            PollOutcome::Expired => {
                return Err(LocustError::ProviderError(
                    "login timed out — run `locust auth grok` again".to_string(),
                ));
            }
        }
    }
}

async fn refresh(tokens: &TokenStore) -> Result<TokenStore> {
    refresh_with_endpoint(tokens, TOKEN_URL, &reqwest::Client::new()).await
}

async fn refresh_with_endpoint(
    tokens: &TokenStore,
    url: &str,
    client: &reqwest::Client,
) -> Result<TokenStore> {
    let resp = client
        .post(url)
        .timeout(Duration::from_secs(30))
        .form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", tokens.refresh_token.as_str()),
            ("client_id", CLIENT_ID),
        ])
        .send()
        .await
        .map_err(|e| {
            LocustError::ProviderError(crate::error_sanitizer::sanitize_provider_error(
                &format!("token refresh failed: {}", e),
                &[&tokens.access_token, &tokens.refresh_token],
            ))
        })?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        // Match the core retry classifier, including HTTP 425.
        let transient = status.is_server_error() || matches!(status.as_u16(), 408 | 425 | 429);
        if matches!(status.as_u16(), 401 | 403)
            || (status.is_client_error() && !transient && body.contains("invalid_grant"))
        {
            return Err(LocustError::ProviderError(
                "xAI session expired — run `locust auth grok` to log in again".to_string(),
            ));
        }

        // Redact before truncating so echoed tokens crossing the excerpt boundary
        // cannot leak a prefix. Keep the status envelope ahead of untrusted text
        // so the retry classifier uses the actual HTTP status.
        let excerpt: String = crate::error_sanitizer::sanitize_provider_error(
            &body,
            &[&tokens.access_token, &tokens.refresh_token],
        )
        .chars()
        .take(200)
        .collect();
        return Err(LocustError::ProviderError(format!(
            "xAI token refresh returned status {status}: {excerpt}"
        )));
    }

    let tr: TokenResponse = resp.json().await.map_err(|e| {
        LocustError::ProviderError(crate::error_sanitizer::sanitize_provider_error(
            &format!("malformed refresh response: {}", e),
            &[&tokens.access_token, &tokens.refresh_token],
        ))
    })?;
    let store = TokenStore {
        access_token: tr.access_token,
        refresh_token: tr
            .refresh_token
            .unwrap_or_else(|| tokens.refresh_token.clone()),
        expires_at: now_secs() + tr.expires_in,
    };
    save_tokens(&store)?;
    Ok(store)
}

/// Translates through the user's Grok subscription. Each call delegates to
/// an OpenAI-compatible client built with a fresh OAuth bearer token.
pub struct GrokSubscriptionProvider {
    model: String,
    client: reqwest::Client,
}

impl GrokSubscriptionProvider {
    pub fn new(model: Option<String>) -> Self {
        Self {
            model: model.unwrap_or_else(|| "grok-4.6".to_string()),
            client: reqwest::Client::new(),
        }
    }

    async fn delegate(&self) -> Result<OpenAiProvider> {
        // Concurrent batches must not rotate the same refresh token at once.
        // Re-read after acquiring the gate so later batches see the new token.
        static REFRESH_GATE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
        let guard = REFRESH_GATE.lock().await;
        let tokens = load_tokens().ok_or_else(|| {
            LocustError::ProviderError(
                "not logged in to xAI — run `locust auth grok` first".to_string(),
            )
        })?;
        let tokens = if tokens.expires_at.saturating_sub(now_secs()) < REFRESH_SKEW_SECS {
            refresh(&tokens).await?
        } else {
            tokens
        };
        drop(guard);
        Ok(OpenAiProvider::compatible(
            "grok-sub".to_string(),
            "Grok (subscription)".to_string(),
            tokens.access_token,
            API_BASE_URL.to_string(),
            self.model.clone(),
        )
        .with_client(self.client.clone()))
    }
}

#[async_trait]
impl TranslationProvider for GrokSubscriptionProvider {
    fn id(&self) -> &str {
        "grok-sub"
    }

    fn name(&self) -> &str {
        "Grok (SuperGrok/Premium+ subscription)"
    }

    fn is_free(&self) -> bool {
        false
    }

    fn requires_api_key(&self) -> bool {
        false
    }

    fn effective_model(&self) -> Option<&str> {
        Some(&self.model)
    }

    fn supports_token_pricing(&self) -> bool {
        true
    }

    async fn translate(&self, requests: &[TranslationRequest]) -> Result<Vec<TranslationResult>> {
        let mut results = self.delegate().await?.translate(requests).await?;
        for r in &mut results {
            r.provider = "grok-sub".to_string();
        }
        Ok(results)
    }

    async fn estimate_cost(&self, _char_count: usize, _target_lang: &str) -> Option<f64> {
        None
    }

    async fn health_check(&self) -> Result<()> {
        self.delegate().await?.health_check().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::prelude::*;

    #[test]
    fn test_provider_identity() {
        let p = GrokSubscriptionProvider::new(None);
        assert_eq!(p.id(), "grok-sub");
        assert!(!p.is_free());
        assert!(!p.requires_api_key());
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn test_translate_without_login_errors() {
        let _lock = token_store_test_lock();
        let token_file = std::env::temp_dir().join(format!(
            "locust_xai_no_login_{}.json",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        struct TokenPathRestore;
        impl Drop for TokenPathRestore {
            fn drop(&mut self) {
                set_token_path_override(None);
            }
        }
        set_token_path_override(Some(token_file));
        let _restore = TokenPathRestore;
        let p = GrokSubscriptionProvider::new(None);
        let result = p.health_check().await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("locust auth grok"));
    }

    fn mock_device_payload() -> serde_json::Value {
        serde_json::json!({
            "device_code": "dev-abc",
            "user_code": "WDJB-MJHT",
            "verification_uri": "https://auth.x.ai/device",
            "verification_uri_complete": "https://auth.x.ai/device?user_code=WDJB-MJHT",
            "interval": 5,
            "expires_in": 900
        })
    }

    #[tokio::test]
    async fn request_device_code_returns_user_code_and_prefers_complete_uri() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(POST).path("/oauth2/device/code");
            then.status(200).json_body(mock_device_payload());
        });
        let url = format!("{}/oauth2/device/code", server.base_url());
        let dc = request_device_code_at(&url).await.expect("device code");
        assert_eq!(dc.user_code, "WDJB-MJHT");
        assert_eq!(dc.device_code, "dev-abc");
        assert_eq!(
            dc.verification_uri,
            "https://auth.x.ai/device?user_code=WDJB-MJHT"
        );
        assert_eq!(dc.interval(), 5);
        assert_eq!(dc.expires_in, 900);
        assert!(dc.expires_at >= now_secs());
    }

    #[tokio::test]
    async fn poll_for_token_pending_when_authorization_pending() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(POST).path("/oauth2/token");
            then.status(400)
                .json_body(serde_json::json!({"error": "authorization_pending"}));
        });
        let dc = DeviceCode::from_parts(
            "dev-abc",
            "WDJB-MJHT",
            "https://auth.x.ai/device",
            5,
            900,
            now_secs() + 900,
        );
        let url = format!("{}/oauth2/token", server.base_url());
        let outcome = poll_for_token_at(&url, &dc).await.unwrap();
        assert!(matches!(outcome, PollOutcome::Pending));
    }

    // See `token_store_test_lock`: the guard has to span the whole test so
    // parallel tests cannot swap the token file under each other. Every
    // `#[tokio::test]` runs on its own current-thread runtime, so nothing else
    // in this runtime can be blocked waiting on the lock.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn poll_for_token_denied_and_expired_and_complete() {
        let _lock = token_store_test_lock();
        let token_file_guard = tempfile::Builder::new()
            .prefix("locust_xai_oauth_test_")
            .tempdir()
            .unwrap();
        let token_file = token_file_guard.path().join("fixture.json");
        set_token_path_override(Some(token_file.clone()));

        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(POST)
                .path("/oauth2/token")
                .body_contains("device_code=denied-code");
            then.status(400)
                .json_body(serde_json::json!({"error": "access_denied"}));
        });
        server.mock(|when, then| {
            when.method(POST)
                .path("/oauth2/token")
                .body_contains("device_code=expired-code");
            then.status(400)
                .json_body(serde_json::json!({"error": "expired_token"}));
        });
        server.mock(|when, then| {
            when.method(POST)
                .path("/oauth2/token")
                .body_contains("device_code=ok-code");
            then.status(200).json_body(serde_json::json!({
                "access_token": "access-xyz",
                "refresh_token": "refresh-secret-must-not-leak",
                "expires_in": 3600
            }));
        });
        let token_url = format!("{}/oauth2/token", server.base_url());

        let denied = DeviceCode::from_parts(
            "denied-code",
            "AAAA",
            "https://example",
            5,
            900,
            now_secs() + 900,
        );
        assert!(matches!(
            poll_for_token_at(&token_url, &denied).await.unwrap(),
            PollOutcome::Denied
        ));

        let expired = DeviceCode::from_parts(
            "expired-code",
            "BBBB",
            "https://example",
            5,
            900,
            now_secs() + 900,
        );
        assert!(matches!(
            poll_for_token_at(&token_url, &expired).await.unwrap(),
            PollOutcome::Expired
        ));

        let past = DeviceCode::from_parts(
            "ok-code",
            "CCCC",
            "https://example",
            5,
            1,
            now_secs().saturating_sub(5),
        );
        assert!(matches!(
            poll_for_token_at(&token_url, &past).await.unwrap(),
            PollOutcome::Expired
        ));

        let ok = DeviceCode::from_parts(
            "ok-code",
            "DDDD",
            "https://example",
            5,
            900,
            now_secs() + 900,
        );
        match poll_for_token_at(&token_url, &ok).await.unwrap() {
            PollOutcome::Complete(store) => {
                assert_eq!(store.access_token, "access-xyz");
                assert_eq!(store.refresh_token, "refresh-secret-must-not-leak");
            }
            other => panic!("expected Complete, got {other:?}"),
        }
        let loaded = load_tokens().expect("tokens persisted");
        assert_eq!(loaded.access_token, "access-xyz");
        set_token_path_override(None);
    }

    struct RefreshFixture {
        tokens: TokenStore,
        path: PathBuf,
        _directory: tempfile::TempDir,
        _lock: MutexGuard<'static, ()>,
    }

    impl RefreshFixture {
        fn new() -> Self {
            let lock = token_store_test_lock();
            let directory = tempfile::Builder::new()
                .prefix("locust_xai_refresh_test_")
                .tempdir()
                .unwrap();
            let path = directory.path().join("fixture.json");
            set_token_path_override(Some(path.clone()));
            let fixture = Self {
                tokens: TokenStore {
                    access_token: "access-sentinel-keep-secret".to_string(),
                    refresh_token: "refresh-sentinel-keep-secret".to_string(),
                    expires_at: now_secs().saturating_sub(60),
                },
                path,
                _directory: directory,
                _lock: lock,
            };
            save_tokens(&fixture.tokens).unwrap();
            fixture
        }
    }

    impl Drop for RefreshFixture {
        fn drop(&mut self) {
            set_token_path_override(None);
        }
    }

    async fn assert_device_login_deadline(poll: bool, status: Option<u16>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let fixture = RefreshFixture::new();
        let original = std::fs::read(&fixture.path).unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/oauth2/stalled", listener.local_addr().unwrap());
        let (stalled_tx, stalled_rx) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buffer = [0; 4096];
            let mut request = Vec::new();
            loop {
                let count = stream.read(&mut buffer).await.unwrap();
                assert_ne!(count, 0);
                request.extend_from_slice(&buffer[..count]);
                if request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                    break;
                }
            }
            if let Some(status) = status {
                let headers = format!(
                    "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: 4096\r\n\r\n{{"
                );
                stream.write_all(headers.as_bytes()).await.unwrap();
            }
            stalled_tx.send(()).unwrap();
            // Keep the connection open without completing headers or the body.
            while stream.read(&mut buffer).await.unwrap_or(0) != 0 {}
        });
        let device = DeviceCode::from_parts(
            "dev-abc",
            "CODE",
            "https://example",
            5,
            900,
            now_secs() + 900,
        );
        // Generous bounds: slow CI runners must still read the request
        // before the client deadline fires, or the stall is never reached.
        let result = tokio::time::timeout(Duration::from_secs(10), async {
            if poll {
                poll_for_token_with_timeout(&url, &device, Duration::from_millis(1500))
                    .await
                    .map(|_| ())
            } else {
                request_device_code_with_timeout(&url, Duration::from_millis(1500))
                    .await
                    .map(|_| ())
            }
        })
        .await;
        server.abort();
        stalled_rx
            .await
            .expect("server reached the requested stall");
        let error = result
            .expect("device-login operation exceeded its deadline")
            .unwrap_err();
        let expected = if poll {
            "xAI token poll timed out; retry login"
        } else {
            "xAI device code request timed out; retry login"
        };
        assert!(matches!(&error, LocustError::ProviderError(message) if message == expected));
        assert!(locust_core::translation::is_retryable(&error), "{error}");
        assert_eq!(std::fs::read(&fixture.path).unwrap(), original);

        // A timed-out attempt releases all state needed by a fresh login.
        let server = MockServer::start();
        let start = server.mock(|when, then| {
            when.method(POST).path("/oauth2/device/code");
            then.status(200).json_body(mock_device_payload());
        });
        let complete = server.mock(|when, then| {
            when.method(POST).path("/oauth2/token");
            then.status(200).json_body(serde_json::json!({
                "access_token": "replacement-access", "refresh_token": "replacement-refresh"
            }));
        });
        let device = request_device_code_at(&server.url("/oauth2/device/code"))
            .await
            .unwrap();
        assert_eq!(std::fs::read(&fixture.path).unwrap(), original);
        assert!(matches!(
            poll_for_token_at(&server.url("/oauth2/token"), &device)
                .await
                .unwrap(),
            PollOutcome::Complete(_)
        ));
        assert_eq!(load_tokens().unwrap().access_token, "replacement-access");
        start.assert();
        complete.assert();
    }

    #[tokio::test]
    async fn device_login_deadline_request_stalled_headers() {
        assert_device_login_deadline(false, None).await;
    }

    #[tokio::test]
    async fn device_login_deadline_request_stalled_body() {
        for status in [200, 400, 503] {
            assert_device_login_deadline(false, Some(status)).await;
        }
    }

    #[tokio::test]
    async fn device_login_deadline_poll_stalled_headers() {
        assert_device_login_deadline(true, None).await;
    }

    #[tokio::test]
    async fn device_login_deadline_poll_stalled_body() {
        for status in [200, 400, 503] {
            assert_device_login_deadline(true, Some(status)).await;
        }
    }

    #[test]
    fn save_tokens_partial_write_failure_preserves_readable_login_and_cleans_staging() {
        let fixture = RefreshFixture::new();
        let original = std::fs::read(&fixture.path).unwrap();
        let replacement = TokenStore {
            access_token: "new-access".into(),
            refresh_token: "new-refresh".into(),
            expires_at: now_secs() + 7200,
        };
        let error = save_tokens_with(&replacement, |file, bytes| {
            file.write_all(&bytes[..bytes.len() / 2])?;
            Err(std::io::Error::other("injected token write failure"))
        })
        .unwrap_err();
        assert!(error.to_string().contains("injected token write failure"));
        assert_eq!(std::fs::read(&fixture.path).unwrap(), original);
        let loaded = load_tokens().unwrap();
        assert_eq!(loaded.access_token, fixture.tokens.access_token);
        assert_eq!(loaded.refresh_token, fixture.tokens.refresh_token);
        assert_eq!(loaded.expires_at, fixture.tokens.expires_at);
        assert_eq!(
            std::fs::read_dir(fixture.path.parent().unwrap())
                .unwrap()
                .count(),
            1
        );
    }

    #[test]
    fn save_tokens_success_replaces_complete_login() {
        let fixture = RefreshFixture::new();
        let replacement = TokenStore {
            access_token: "new-access-日本語".into(),
            refresh_token: "new-refresh".into(),
            expires_at: now_secs() + 7200,
        };
        save_tokens(&replacement).unwrap();
        assert_eq!(
            std::fs::read_to_string(&fixture.path).unwrap(),
            serde_json::to_string_pretty(&replacement).unwrap()
        );
        let loaded = load_tokens().unwrap();
        assert_eq!(loaded.access_token, replacement.access_token);
        assert_eq!(loaded.refresh_token, replacement.refresh_token);
        assert_eq!(loaded.expires_at, replacement.expires_at);
        assert_eq!(
            std::fs::read_dir(fixture.path.parent().unwrap())
                .unwrap()
                .count(),
            1
        );
    }

    #[cfg(unix)]
    #[test]
    fn save_tokens_restricts_staging_permissions_before_writing_secrets() {
        use std::os::unix::fs::PermissionsExt;

        let fixture = RefreshFixture::new();
        save_tokens_with(&fixture.tokens, |file, bytes| {
            assert_eq!(file.metadata()?.permissions().mode() & 0o777, 0o600);
            assert_eq!(file.metadata()?.len(), 0);
            file.write_all(bytes)
        })
        .unwrap();
        assert_eq!(
            std::fs::metadata(&fixture.path)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }

    #[cfg(windows)]
    #[test]
    fn save_tokens_locked_destination_preserves_login_and_cleans_staging() {
        use std::os::windows::fs::OpenOptionsExt;

        let fixture = RefreshFixture::new();
        let original = std::fs::read(&fixture.path).unwrap();
        let held = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(3)
            .open(&fixture.path)
            .unwrap();
        let replacement = TokenStore {
            access_token: "new-access".into(),
            refresh_token: "new-refresh".into(),
            expires_at: now_secs() + 7200,
        };
        let error = save_tokens(&replacement).unwrap_err().to_string();
        assert!(
            error.contains("close any application using the destination and retry"),
            "{error}"
        );
        assert_eq!(std::fs::read(&fixture.path).unwrap(), original);
        let loaded = load_tokens().unwrap();
        assert_eq!(loaded.access_token, fixture.tokens.access_token);
        assert_eq!(loaded.refresh_token, fixture.tokens.refresh_token);
        assert_eq!(loaded.expires_at, fixture.tokens.expires_at);
        drop(held);
        assert_eq!(
            std::fs::read_dir(fixture.path.parent().unwrap())
                .unwrap()
                .count(),
            1
        );
    }

    async fn assert_refresh_failure(
        status: u16,
        body: &str,
        retryable: bool,
        requires_login: bool,
    ) -> String {
        let fixture = RefreshFixture::new();
        let original = std::fs::read(&fixture.path).unwrap();
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(POST)
                .path("/oauth2/token")
                .body_contains("grant_type=refresh_token")
                .body_contains(format!("refresh_token={}", fixture.tokens.refresh_token))
                .body_contains(format!("client_id={CLIENT_ID}"));
            then.status(status).body(body);
        });
        let error = refresh_with_endpoint(
            &fixture.tokens,
            &server.url("/oauth2/token"),
            &reqwest::Client::new(),
        )
        .await
        .unwrap_err();
        mock.assert();
        assert_eq!(std::fs::read(&fixture.path).unwrap(), original);
        let LocustError::ProviderError(message) = &error else {
            panic!("expected provider error, got {error:?}");
        };
        assert_eq!(
            locust_core::translation::is_retryable(&error),
            retryable,
            "status {status}: {message}"
        );
        for token in [&fixture.tokens.access_token, &fixture.tokens.refresh_token] {
            assert!(!message.contains(token), "{message}");
        }
        assert_eq!(
            message.contains("locust auth grok"),
            requires_login,
            "{message}"
        );
        if requires_login {
            assert!(message.contains("xAI session expired"), "{message}");
        } else {
            assert!(
                message.contains(&format!("token refresh returned status {status}")),
                "{message}"
            );
            assert!(!message.contains("log in again"), "{message}");
        }
        message.clone()
    }

    #[tokio::test]
    async fn refresh_transient_statuses_preserve_tokens_and_are_retryable() {
        for status in [503, 500, 502, 504, 429, 408, 425, 501, 599] {
            for body in [
                "upstream unavailable; unauthorized 401; returned status 400",
                r#"{"error":"invalid_grant"}"#,
            ] {
                assert_refresh_failure(status, body, true, false).await;
            }
        }
    }

    #[tokio::test]
    async fn refresh_auth_failures_require_login_and_are_not_retryable() {
        for (status, body) in [
            (400, r#"{"error":"invalid_grant"}"#),
            (401, "timeout; returned status 503"),
            (403, "rate limit 429"),
        ] {
            assert_refresh_failure(status, body, false, true).await;
        }
    }

    #[tokio::test]
    async fn refresh_other_client_errors_are_permanent_without_login_guidance() {
        for status in [400, 404, 405, 409, 422, 451] {
            assert_refresh_failure(
                status,
                "invalid_request: max_tokens <= 500; network timeout; returned status 503",
                false,
                false,
            )
            .await;
        }
    }

    #[tokio::test]
    async fn refresh_invalid_grant_in_other_client_error_requires_login() {
        // The auth marker is outside the diagnostic excerpt.
        let body = format!("{} invalid_grant", "x".repeat(600));
        assert_refresh_failure(422, &body, false, true).await;
    }

    #[tokio::test]
    async fn refresh_error_body_redacts_both_tokens_and_caps_unicode_excerpt() {
        let body = format!(
            "echo access-sentinel-keep-secret refresh-sentinel-keep-secret {} tail-marker",
            "界".repeat(600)
        );
        for status in [503, 400] {
            let message = assert_refresh_failure(status, &body, status == 503, false).await;
            let excerpt = message.split_once(": ").unwrap().1;
            assert_eq!(message.matches("[REDACTED]").count(), 2, "{message}");
            assert!(excerpt.contains('界'), "{message}");
            assert_eq!(excerpt.chars().count(), 200, "{message}");
            assert!(!message.contains("tail-marker"), "{message}");
        }
    }

    #[tokio::test]
    async fn refresh_redacts_tokens_crossing_excerpt_boundary() {
        for secret in [
            "access-sentinel-keep-secret",
            "refresh-sentinel-keep-secret",
        ] {
            let padding = "x".repeat(190);
            let body = format!("{padding}{secret}{}", "z".repeat(400));
            let message = assert_refresh_failure(503, &body, true, false).await;
            let excerpt = message.split_once(": ").unwrap().1;
            assert_eq!(excerpt, format!("{padding}[REDACTED]"));
        }
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn refresh_recovers_after_503_and_persists_rotated_tokens() {
        let fixture = RefreshFixture::new();
        let original = std::fs::read(&fixture.path).unwrap();
        let server = MockServer::start();
        let mut unavailable = server.mock(|when, then| {
            when.method(POST).path("/oauth2/token");
            then.status(503).body("temporarily unavailable");
        });
        let client = reqwest::Client::new();
        let url = server.url("/oauth2/token");
        let error = refresh_with_endpoint(&fixture.tokens, &url, &client)
            .await
            .unwrap_err();
        unavailable.assert();
        assert_eq!(std::fs::read(&fixture.path).unwrap(), original);
        assert!(locust_core::translation::is_retryable(&error), "{error}");
        unavailable.delete();

        let success = server.mock(|when, then| {
            when.method(POST)
                .path("/oauth2/token")
                .body_contains(format!("refresh_token={}", fixture.tokens.refresh_token));
            then.status(200).json_body(serde_json::json!({
                "access_token": "new-access-sentinel",
                "refresh_token": "new-refresh-sentinel",
                "expires_in": 7200
            }));
        });
        let before = now_secs();
        let refreshed = refresh_with_endpoint(&fixture.tokens, &url, &client)
            .await
            .unwrap();
        success.assert();
        assert_eq!(refreshed.access_token, "new-access-sentinel");
        assert_eq!(refreshed.refresh_token, "new-refresh-sentinel");
        assert!((before + 7200..=now_secs() + 7200).contains(&refreshed.expires_at));
        let saved = load_tokens().expect("refreshed tokens persisted");
        assert_eq!(saved.access_token, refreshed.access_token);
        assert_eq!(saved.refresh_token, refreshed.refresh_token);
        assert_eq!(saved.expires_at, refreshed.expires_at);
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn refresh_keeps_existing_refresh_token_when_not_rotated() {
        let fixture = RefreshFixture::new();
        let server = MockServer::start();
        let success = server.mock(|when, then| {
            when.method(POST).path("/oauth2/token");
            then.status(200).json_body(serde_json::json!({
                "access_token": "new-access-sentinel"
            }));
        });
        let before = now_secs();
        let refreshed = refresh_with_endpoint(
            &fixture.tokens,
            &server.url("/oauth2/token"),
            &reqwest::Client::new(),
        )
        .await
        .unwrap();
        success.assert();
        assert_eq!(refreshed.access_token, "new-access-sentinel");
        assert_eq!(refreshed.refresh_token, fixture.tokens.refresh_token);
        assert!(
            (before + default_expires()..=now_secs() + default_expires())
                .contains(&refreshed.expires_at)
        );
        let saved = load_tokens().expect("refreshed tokens persisted");
        assert_eq!(saved.access_token, refreshed.access_token);
        assert_eq!(saved.refresh_token, fixture.tokens.refresh_token);
        assert_eq!(saved.expires_at, refreshed.expires_at);
    }
}
