//! xAI OAuth device-code flow for SuperGrok / X Premium+ subscriptions.
//!
//! Lets Locust translate through the user's Grok subscription without an
//! API key. Access tokens live ~6h and are refreshed automatically using
//! the stored refresh token. Note: xAI gates this surface server-side and
//! may return 403 for some subscription tiers.

use std::path::PathBuf;
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

pub fn token_path() -> PathBuf {
    AppConfig::config_dir().join("xai-oauth.json")
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

/// Run the browser device-code login and persist tokens.
pub async fn device_login() -> Result<TokenStore> {
    let client = reqwest::Client::new();

    let resp = client
        .post(DEVICE_CODE_URL)
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

    let url = dc
        .verification_uri_complete
        .clone()
        .unwrap_or_else(|| dc.verification_uri.clone());
    println!("Open this URL in your browser and approve access:");
    println!("\n  {}\n", url);
    println!("Code: {}", dc.user_code);
    println!("Waiting for approval...");

    let deadline = now_secs() + dc.expires_in;
    let mut interval = dc.interval.max(1);

    loop {
        if now_secs() > deadline {
            return Err(LocustError::ProviderError(
                "login timed out — run `locust auth grok` again".to_string(),
            ));
        }
        tokio::time::sleep(Duration::from_secs(interval)).await;

        let resp = client
            .post(TOKEN_URL)
            .form(&[
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                ("device_code", dc.device_code.as_str()),
                ("client_id", CLIENT_ID),
            ])
            .send()
            .await
            .map_err(|e| LocustError::ProviderError(format!("token poll failed: {}", e)))?;

        if resp.status().is_success() {
            let tr: TokenResponse = resp.json().await.map_err(|e| {
                LocustError::ProviderError(format!("malformed token response: {}", e))
            })?;
            let store = TokenStore {
                access_token: tr.access_token,
                refresh_token: tr.refresh_token.unwrap_or_default(),
                expires_at: now_secs() + tr.expires_in,
            };
            save_tokens(&store)?;
            return Ok(store);
        }

        let err: TokenErrorResponse = resp.json().await.unwrap_or(TokenErrorResponse {
            error: "unknown".to_string(),
        });
        match err.error.as_str() {
            "authorization_pending" => continue,
            "slow_down" => interval += 5,
            other => {
                return Err(LocustError::ProviderError(format!(
                    "login failed: {}",
                    other
                )))
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

    let tr: TokenResponse = resp.json().await.map_err(|e| {
        LocustError::ProviderError(format!("malformed refresh response: {}", e))
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
}

impl GrokSubscriptionProvider {
    pub fn new(model: Option<String>) -> Self {
        Self {
            model: model.unwrap_or_else(|| "grok-4.3".to_string()),
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
}
