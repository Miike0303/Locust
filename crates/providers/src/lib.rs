pub mod argos;
pub mod deepl;
pub mod google;
pub mod openai;
pub mod claude;
pub mod ollama;
pub mod mock;
pub mod retry;
pub mod xai_oauth;

use std::sync::Arc;

use locust_core::config::AppConfig;
use locust_core::translation::ProviderRegistry;

pub fn default_registry(config: &AppConfig) -> ProviderRegistry {
    let mut reg = ProviderRegistry::new();

    // Always register Google Translate (free, no API key needed)
    reg.register(Arc::new(google::GoogleTranslateProvider::new()));

    // Mock provider for testing pipelines without hitting any service
    reg.register(Arc::new(mock::MockProvider));

    // Register Argos if configured or use defaults
    if let Some(pc) = config.get_provider_config("argos") {
        let base_url = pc.base_url.clone().unwrap_or_else(|| "http://localhost:5000".to_string());
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
    // All of these speak the same chat-completions protocol as OpenAI.
    const COMPATIBLE_APIS: [(&str, &str, &str, &str); 3] = [
        (
            "deepseek",
            "DeepSeek",
            "https://api.deepseek.com",
            "deepseek-v4-flash",
        ),
        ("grok", "Grok (xAI)", "https://api.x.ai", "grok-4-1-fast"),
        (
            "gemini",
            "Google Gemini",
            "https://generativelanguage.googleapis.com/v1beta/openai",
            "gemini-2.5-flash",
        ),
    ];
    for (id, name, default_url, default_model) in COMPATIBLE_APIS {
        if let Some(pc) = config.get_provider_config(id) {
            if let Some(ref api_key) = pc.api_key {
                reg.register(Arc::new(openai::OpenAiProvider::compatible(
                    id.to_string(),
                    name.to_string(),
                    api_key.clone(),
                    pc.base_url.clone().unwrap_or_else(|| default_url.to_string()),
                    pc.model.clone().unwrap_or_else(|| default_model.to_string()),
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
        "google", "argos", "deepl", "openai", "claude", "lmstudio", "ollama", "deepseek",
        "grok", "gemini", "grok-sub",
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
