pub mod argos;
pub mod claude;
pub mod deepl;
pub mod google;
pub mod mock;
pub mod ollama;
pub mod openai;
pub mod retry;
pub mod xai_oauth;

use std::collections::HashSet;
use std::sync::Arc;

use locust_core::config::AppConfig;
use locust_core::translation::ProviderRegistry;
use serde::{Deserialize, Serialize};

/// Metadata for providers registered only when `config.providers[id].api_key` is set.
/// Shared by `default_registry` and `list_providers_for_api` so listing cannot drift.
pub struct KeyGatedProviderDef {
    pub id: &'static str,
    pub name: &'static str,
    pub is_free: bool,
    /// OpenAI-compatible endpoint defaults (`base_url`, `model`); `None` for native providers.
    pub compatible_defaults: Option<(&'static str, &'static str)>,
}

pub const KEY_GATED_PROVIDERS: &[KeyGatedProviderDef] = &[
    KeyGatedProviderDef {
        id: "deepl",
        name: "DeepL",
        is_free: false,
        compatible_defaults: None,
    },
    KeyGatedProviderDef {
        id: "openai",
        name: "OpenAI",
        is_free: false,
        compatible_defaults: None,
    },
    KeyGatedProviderDef {
        id: "claude",
        name: "Claude",
        is_free: false,
        compatible_defaults: None,
    },
    KeyGatedProviderDef {
        id: "deepseek",
        name: "DeepSeek",
        is_free: false,
        compatible_defaults: Some(("https://api.deepseek.com", "deepseek-v4-flash")),
    },
    KeyGatedProviderDef {
        id: "grok",
        name: "Grok (xAI)",
        is_free: false,
        compatible_defaults: Some(("https://api.x.ai", "grok-4-1-fast")),
    },
    KeyGatedProviderDef {
        id: "gemini",
        name: "Google Gemini",
        is_free: false,
        compatible_defaults: Some((
            "https://generativelanguage.googleapis.com/v1beta/openai",
            "gemini-2.5-flash",
        )),
    },
];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DiscoverableProviderInfo {
    pub id: String,
    pub name: String,
    pub is_free: bool,
    pub requires_api_key: bool,
    pub configured: bool,
}

/// Registered providers plus unconfigured key-gated catalog entries for GET /api/providers.
pub fn list_providers_for_api(reg: &ProviderRegistry) -> Vec<DiscoverableProviderInfo> {
    let mut out: Vec<DiscoverableProviderInfo> = reg
        .list()
        .into_iter()
        .map(|p| DiscoverableProviderInfo {
            id: p.id,
            name: p.name,
            is_free: p.is_free,
            requires_api_key: p.requires_api_key,
            configured: true,
        })
        .collect();

    let registered: HashSet<String> = out.iter().map(|p| p.id.clone()).collect();
    for def in KEY_GATED_PROVIDERS {
        if registered.contains(def.id) {
            continue;
        }
        out.push(DiscoverableProviderInfo {
            id: def.id.to_string(),
            name: def.name.to_string(),
            is_free: def.is_free,
            requires_api_key: true,
            configured: false,
        });
    }
    out
}

pub fn default_registry(config: &AppConfig) -> ProviderRegistry {
    let mut reg = ProviderRegistry::new();

    // Always register Google Translate (free, no API key needed)
    reg.register(Arc::new(google::GoogleTranslateProvider::new()));

    // Mock provider for testing pipelines without hitting any service
    reg.register(Arc::new(mock::MockProvider));

    // Register Argos if configured or use defaults
    if let Some(pc) = config.get_provider_config("argos") {
        let base_url = pc
            .base_url
            .clone()
            .unwrap_or_else(|| "http://localhost:5000".to_string());
        reg.register(Arc::new(argos::ArgosProvider::new(base_url)));
    } else {
        reg.register(Arc::new(argos::ArgosProvider::default()));
    }

    // Register DeepL if API key is configured
    if let Some(pc) = config.get_provider_config("deepl") {
        if let Some(ref api_key) = pc.api_key {
            reg.register(Arc::new(deepl::DeepLProvider::new(
                api_key.clone(),
                pc.free_tier,
            )));
        }
    }

    // Register OpenAI if API key is configured
    if let Some(pc) = config.get_provider_config("openai") {
        if let Some(ref api_key) = pc.api_key {
            reg.register(Arc::new(openai::OpenAiProvider::new(
                api_key.clone(),
                pc.model.clone(),
                pc.base_url.clone(),
            )));
        }
    }

    // Register Claude if API key is configured
    if let Some(pc) = config.get_provider_config("claude") {
        if let Some(ref api_key) = pc.api_key {
            reg.register(Arc::new(claude::ClaudeProvider::new(
                api_key.clone(),
                pc.model.clone(),
                pc.base_url.clone(),
            )));
        }
    }

    // Register OpenAI-compatible API providers when an API key is configured.
    for def in KEY_GATED_PROVIDERS {
        let Some((default_url, default_model)) = def.compatible_defaults else {
            continue;
        };
        if let Some(pc) = config.get_provider_config(def.id) {
            if let Some(ref api_key) = pc.api_key {
                reg.register(Arc::new(openai::OpenAiProvider::compatible(
                    def.id.to_string(),
                    def.name.to_string(),
                    api_key.clone(),
                    pc.base_url
                        .clone()
                        .unwrap_or_else(|| default_url.to_string()),
                    pc.model
                        .clone()
                        .unwrap_or_else(|| default_model.to_string()),
                )));
            }
        }
    }

    // Grok via SuperGrok/Premium+ subscription — registered once the user
    // has logged in with `locust auth grok`.
    if xai_oauth::load_tokens().is_some() {
        let model = config
            .get_provider_config("grok-sub")
            .and_then(|pc| pc.model.clone());
        reg.register(Arc::new(xai_oauth::GrokSubscriptionProvider::new(model)));
    }

    // Any other config entry with a base_url is treated as a custom
    // OpenAI-compatible endpoint (vLLM, OpenRouter, self-hosted, ...).
    const KNOWN_IDS: [&str; 11] = [
        "google", "argos", "deepl", "openai", "claude", "lmstudio", "ollama", "deepseek", "grok",
        "gemini", "grok-sub",
    ];
    for (id, pc) in &config.providers {
        if KNOWN_IDS.contains(&id.as_str()) {
            continue;
        }
        if let Some(ref base_url) = pc.base_url {
            reg.register(Arc::new(openai::OpenAiProvider::compatible(
                id.clone(),
                format!("{} (custom endpoint)", id),
                pc.api_key.clone().unwrap_or_else(|| "none".to_string()),
                base_url.clone(),
                pc.model.clone().unwrap_or_else(|| "default".to_string()),
            )));
        }
    }

    // Register LM Studio if configured or use defaults (OpenAI-compatible local API)
    if let Some(pc) = config.get_provider_config("lmstudio") {
        reg.register(Arc::new(openai::OpenAiProvider::lm_studio(
            pc.base_url.clone(),
            pc.model.clone(),
        )));
    } else {
        reg.register(Arc::new(openai::OpenAiProvider::lm_studio(None, None)));
    }

    // Register Ollama if configured or use defaults
    if let Some(pc) = config.get_provider_config("ollama") {
        reg.register(Arc::new(ollama::OllamaProvider::new(
            pc.base_url.clone(),
            pc.model.clone(),
        )));
    } else {
        reg.register(Arc::new(ollama::OllamaProvider::default()));
    }

    reg
}

#[cfg(test)]
mod tests {
    use super::*;
    use locust_core::config::ProviderConfig;

    #[test]
    fn list_providers_for_api_includes_unconfigured_key_gated_catalog() {
        let reg = default_registry(&AppConfig::default());
        let list = list_providers_for_api(&reg);

        let deepl = list
            .iter()
            .find(|p| p.id == "deepl")
            .expect("deepl should be discoverable without a key");
        assert_eq!(
            deepl,
            &DiscoverableProviderInfo {
                id: "deepl".to_string(),
                name: "DeepL".to_string(),
                is_free: false,
                requires_api_key: true,
                configured: false,
            }
        );

        assert!(
            list.iter().any(|p| p.id == "mock" && p.configured),
            "registered providers stay configured=true"
        );
    }

    #[test]
    fn list_providers_for_api_marks_key_gated_provider_configured_when_registered() {
        let mut config = AppConfig::default();
        config.providers.insert(
            "deepl".to_string(),
            ProviderConfig {
                api_key: Some("test-key".to_string()),
                base_url: None,
                model: None,
                free_tier: false,
                extra: std::collections::HashMap::new(),
            },
        );
        let reg = default_registry(&config);
        let list = list_providers_for_api(&reg);

        let deepl_entries: Vec<_> = list.iter().filter(|p| p.id == "deepl").collect();
        assert_eq!(deepl_entries.len(), 1, "deepl must not be duplicated");
        assert!(deepl_entries[0].configured);
        assert!(deepl_entries[0].requires_api_key);
    }
}
