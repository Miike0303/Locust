//! xAI OAuth device-code flow for SuperGrok / X Premium+ subscriptions.
//!
//! Lets Locust translate through the user's Grok subscription without an
//! API key. Access tokens live ~6h and are refreshed automatically using
//! the stored refresh token. Note: xAI gates this surface server-side and
//! may return 403 for some subscription tiers.

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
    let path = token_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, serde_json::to_string_pretty(tokens)?)?;
    // Tokens are secrets — restrict to the owner on Unix (Windows relies on the
    // NTFS ACLs already applied under the per-user data dir).
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    }
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
    let client = reqwest::Client::new();
    let resp = client
        .post(url)
        .form(&[("client_id", CLIENT_ID), ("scope", SCOPE)])
        .send()
        .await
        .map_err(|e| LocustError::ProviderError(format!("device code request failed: {}", e)))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(LocustError::ProviderError(format!(
            "device code request returned {}: {}",
            status, body
        )));
    }

    let dc: DeviceCodeResponse = resp.json().await.map_err(|e| {
        LocustError::ProviderError(format!("malformed device code response: {}", e))
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
}

/// One poll of the production token endpoint.
pub async fn poll_for_token(device: &DeviceCode) -> Result<PollOutcome> {
    poll_for_token_at(TOKEN_URL, device).await
}

/// One poll of an explicit token URL. Persists tokens on `Complete`.
pub async fn poll_for_token_at(url: &str, device: &DeviceCode) -> Result<PollOutcome> {
    if now_secs() > device.expires_at {
        return Ok(PollOutcome::Expired);
    }

    let client = reqwest::Client::new();
    let resp = client
        .post(url)
        .form(&[
            ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
            ("device_code", device.device_code.as_str()),
            ("client_id", CLIENT_ID),
        ])
        .send()
        .await
        .map_err(|e| LocustError::ProviderError(format!("token poll failed: {}", e)))?;

    if resp.status().is_success() {
        let tr: TokenResponse = resp
            .json()
            .await
            .map_err(|e| LocustError::ProviderError(format!("malformed token response: {}", e)))?;
        let store = TokenStore {
            access_token: tr.access_token,
            refresh_token: tr.refresh_token.unwrap_or_default(),
            expires_at: now_secs() + tr.expires_in,
        };
        save_tokens(&store)?;
        return Ok(PollOutcome::Complete(store));
    }

    let err: TokenErrorResponse = resp.json().await.unwrap_or(TokenErrorResponse {
        error: "unknown".to_string(),
    });
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
            other
        ))),
    }
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
    let client = reqwest::Client::new();
    let resp = client
        .post(TOKEN_URL)
        .form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", tokens.refresh_token.as_str()),
            ("client_id", CLIENT_ID),
        ])
        .send()
        .await
        .map_err(|e| LocustError::ProviderError(format!("token refresh failed: {}", e)))?;

    if !resp.status().is_success() {
        return Err(LocustError::ProviderError(
            "xAI session expired — run `locust auth grok` to log in again".to_string(),
        ));
    }

    let tr: TokenResponse = resp
        .json()
        .await
        .map_err(|e| LocustError::ProviderError(format!("malformed refresh response: {}", e)))?;
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
}

impl GrokSubscriptionProvider {
    pub fn new(model: Option<String>) -> Self {
        Self {
            // Non-reasoning variant: ~8x faster and ~3.5x fewer tokens than the
            // reasoning models for translation, with equivalent quality (Grok's
            // chain-of-thought is wasted effort on straight translation).
            model: model.unwrap_or_else(|| "grok-4.20-0309-non-reasoning".to_string()),
        }
    }

    async fn delegate(&self) -> Result<OpenAiProvider> {
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
        // ponytail: builds a client per batch; batches take seconds, this is noise
        Ok(OpenAiProvider::compatible(
            "grok-sub".to_string(),
            "Grok (subscription)".to_string(),
            tokens.access_token,
            API_BASE_URL.to_string(),
            self.model.clone(),
        ))
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
    async fn test_translate_without_login_errors() {
        // Only meaningful when no token file exists on the machine; skip otherwise.
        if load_tokens().is_some() {
            return;
        }
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
        let token_file = std::env::temp_dir().join(format!(
            "locust_xai_oauth_test_{}.json",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
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
}
