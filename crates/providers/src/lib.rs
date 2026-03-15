pub mod argos;
pub mod deepl;
pub mod openai;
pub mod claude;
pub mod ollama;
pub mod mock;
pub mod retry;

use std::sync::Arc;

use locust_core::config::AppConfig;
use locust_core::translation::ProviderRegistry;

pub fn default_registry(config: &AppConfig) -> ProviderRegistry {
    let mut reg = ProviderRegistry::new();

    // Always register mock provider
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
