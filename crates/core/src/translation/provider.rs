use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::models::{TranslationRequest, TranslationResult};

use super::batching::default_max_batch_tokens;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LangPair {
    pub source: String,
    pub target: String,
}

#[async_trait]
pub trait TranslationProvider: Send + Sync {
    fn id(&self) -> &str;
    fn name(&self) -> &str;
    fn is_free(&self) -> bool;
    fn requires_api_key(&self) -> bool;
    fn supported_languages(&self) -> Vec<LangPair> {
        vec![]
    }
    async fn translate(&self, requests: &[TranslationRequest]) -> Result<Vec<TranslationResult>>;
    async fn estimate_cost(&self, char_count: usize, target_lang: &str) -> Option<f64>;
    async fn health_check(&self) -> Result<()>;
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TranslationOptions {
    pub source_lang: String,
    pub target_lang: String,
    pub batch_size: usize,
    pub max_concurrent: usize,
    pub cost_limit_usd: Option<f64>,
    pub game_context: Option<String>,
    pub use_glossary: bool,
    pub use_memory: bool,
    pub skip_approved: bool,
    /// Approximate input budget: source/context bytes divided by 3, plus overhead.
    #[serde(default = "default_max_batch_tokens")]
    pub max_batch_tokens: Option<usize>,
    /// Explicit opt-in: mechanical shortening can destroy meaning.
    #[serde(default)]
    pub allow_lossy_binary_fit: bool,
}

impl Default for TranslationOptions {
    fn default() -> Self {
        Self {
            source_lang: "ja".to_string(),
            target_lang: "en".to_string(),
            batch_size: 40,
            // Sequential by default: local GPU models degrade under parallel
            // requests; API providers can opt in via --concurrency.
            max_concurrent: 1,
            cost_limit_usd: None,
            game_context: None,
            use_glossary: true,
            use_memory: true,
            skip_approved: true,
            max_batch_tokens: default_max_batch_tokens(),
            allow_lossy_binary_fit: false,
        }
    }
}

impl TranslationOptions {
    /// Engine batch defaults, shared by CLI, HTTP and desktop jobs.
    pub fn defaults_for_provider(provider_id: &str) -> Self {
        let mut options = Self::default();
        match provider_id {
            "google" => {
                // 2026-10-12: 9,041 RPG Maker MV strings took ~4 min per JA→EN /
                // EN→ES pass with these values, versus ~28 min at batch 40; no 429s.
                options.batch_size = 200;
                options.max_concurrent = 4;
                options.max_batch_tokens = Some(40_000);
            }
            "antigravity" => {
                // Measured ~19s for 5 strings vs ~22s for 100: amortize each call.
                // 6k tokens splits batches near 40; the provider also caps prompts.
                options.batch_size = 100;
                options.max_concurrent = 4;
                options.max_batch_tokens = Some(20_000);
            }
            "grok" | "grok-sub" => {
                // Measured with Grok 4.6: small batches avoid reasoning deadlines.
                options.batch_size = 10;
                options.max_concurrent = 10;
            }
            _ => {}
        }
        options
    }
}

/// Wire options retain omissions until the primary provider and user settings
/// are known. Fully specified requests keep their existing behavior.
#[derive(Debug, Deserialize)]
pub struct TranslationOptionsRequest {
    pub source_lang: String,
    pub target_lang: String,
    pub batch_size: Option<usize>,
    pub max_concurrent: Option<usize>,
    pub cost_limit_usd: Option<f64>,
    pub game_context: Option<String>,
    pub use_glossary: bool,
    pub use_memory: bool,
    pub skip_approved: bool,
    /// Missing uses the provider budget; explicit null still disables the cap.
    #[serde(default, deserialize_with = "deserialize_batch_token_override")]
    pub max_batch_tokens: Option<Option<usize>>,
    #[serde(default)]
    pub allow_lossy_binary_fit: bool,
}

fn deserialize_batch_token_override<'de, D>(
    deserializer: D,
) -> std::result::Result<Option<Option<usize>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<usize>::deserialize(deserializer).map(Some)
}

impl TranslationOptionsRequest {
    pub fn resolve(
        self,
        provider_id: &str,
        configured_batch_size: Option<usize>,
    ) -> TranslationOptions {
        let defaults = TranslationOptions::defaults_for_provider(provider_id);
        TranslationOptions {
            source_lang: self.source_lang,
            target_lang: self.target_lang,
            batch_size: self
                .batch_size
                .or(configured_batch_size)
                .unwrap_or(defaults.batch_size),
            max_concurrent: self.max_concurrent.unwrap_or(defaults.max_concurrent),
            max_batch_tokens: self.max_batch_tokens.unwrap_or(defaults.max_batch_tokens),
            cost_limit_usd: self.cost_limit_usd,
            game_context: self.game_context,
            use_glossary: self.use_glossary,
            use_memory: self.use_memory,
            skip_approved: self.skip_approved,
            allow_lossy_binary_fit: self.allow_lossy_binary_fit,
        }
    }
}

impl From<TranslationOptions> for TranslationOptionsRequest {
    fn from(options: TranslationOptions) -> Self {
        Self {
            source_lang: options.source_lang,
            target_lang: options.target_lang,
            batch_size: Some(options.batch_size),
            max_concurrent: Some(options.max_concurrent),
            max_batch_tokens: Some(options.max_batch_tokens),
            cost_limit_usd: options.cost_limit_usd,
            game_context: options.game_context,
            use_glossary: options.use_glossary,
            use_memory: options.use_memory,
            skip_approved: options.skip_approved,
            allow_lossy_binary_fit: options.allow_lossy_binary_fit,
        }
    }
}

pub struct ProviderRegistry {
    providers: Vec<Arc<dyn TranslationProvider>>,
}

impl ProviderRegistry {
    pub fn new() -> Self {
        Self {
            providers: Vec::new(),
        }
    }

    pub fn register(&mut self, provider: Arc<dyn TranslationProvider>) {
        self.providers.push(provider);
    }

    pub fn get(&self, id: &str) -> Option<Arc<dyn TranslationProvider>> {
        self.providers.iter().find(|p| p.id() == id).cloned()
    }

    pub fn list(&self) -> Vec<ProviderInfo> {
        self.providers
            .iter()
            .map(|p| ProviderInfo {
                id: p.id().to_string(),
                name: p.name().to_string(),
                is_free: p.is_free(),
                requires_api_key: p.requires_api_key(),
            })
            .collect()
    }
}

impl Default for ProviderRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ProviderInfo {
    pub id: String,
    pub name: String,
    pub is_free: bool,
    pub requires_api_key: bool,
}

#[cfg(test)]
mod tests {
    use super::super::tests::MockProvider;
    use super::*;

    #[test]
    fn provider_translation_defaults_request_overrides_are_independent() {
        let base = serde_json::json!({
            "source_lang": "en", "target_lang": "es", "cost_limit_usd": 2.5,
            "game_context": "Game", "use_glossary": false, "use_memory": false,
            "skip_approved": false, "allow_lossy_binary_fit": true
        });
        for provider in [
            "google",
            "antigravity",
            "grok",
            "grok-sub",
            "ollama",
            "unknown",
        ] {
            for (patch, setting) in [
                (serde_json::json!({}), None),
                (serde_json::json!({}), Some(40)),
                (serde_json::json!({"batch_size": 7}), Some(40)),
                (serde_json::json!({"max_concurrent": 2}), None),
                (serde_json::json!({"max_batch_tokens": 999}), None),
                (serde_json::json!({"max_batch_tokens": null}), None),
                (
                    serde_json::json!({"batch_size": 0, "max_concurrent": 0, "max_batch_tokens": 0}),
                    None,
                ),
            ] {
                let mut input = base.clone();
                input
                    .as_object_mut()
                    .unwrap()
                    .extend(patch.as_object().unwrap().clone());
                let request: TranslationOptionsRequest = serde_json::from_value(input).unwrap();
                let mut expected = TranslationOptions::defaults_for_provider(provider);
                expected.batch_size = setting.unwrap_or(expected.batch_size);
                let mut expected = serde_json::to_value(expected).unwrap();
                expected
                    .as_object_mut()
                    .unwrap()
                    .extend(base.as_object().unwrap().clone());
                expected
                    .as_object_mut()
                    .unwrap()
                    .extend(patch.as_object().unwrap().clone());
                assert_eq!(
                    serde_json::to_value(request.resolve(provider, setting)).unwrap(),
                    expected,
                    "{provider}, {patch}"
                );
            }
        }
    }

    #[test]
    fn provider_translation_defaults_resolved_options_stay_explicit() {
        let options = TranslationOptions {
            max_batch_tokens: None,
            ..Default::default()
        };
        let request = TranslationOptionsRequest::from(options.clone());
        assert_eq!(
            serde_json::to_value(request.resolve("google", Some(123))).unwrap(),
            serde_json::to_value(options).unwrap()
        );
    }

    #[test]
    fn provider_translation_defaults_table() {
        for (id, batch, concurrency, tokens) in [
            ("google", 200, 4, 40_000),
            ("antigravity", 100, 4, 20_000),
            ("grok", 10, 10, 6000),
            ("grok-sub", 10, 10, 6000),
            ("ollama", 40, 1, 6000),
            ("deepl", 40, 1, 6000),
            ("mock", 40, 1, 6000),
            ("unknown", 40, 1, 6000),
        ] {
            let opts = TranslationOptions::defaults_for_provider(id);
            let expected = TranslationOptions {
                batch_size: batch,
                max_concurrent: concurrency,
                max_batch_tokens: Some(tokens),
                ..Default::default()
            };
            assert_eq!(
                serde_json::to_value(opts).unwrap(),
                serde_json::to_value(expected).unwrap(),
                "{id}"
            );
        }
    }

    #[test]
    fn test_provider_registry_register_and_get() {
        let mut reg = ProviderRegistry::new();
        let provider = Arc::new(MockProvider::new());
        reg.register(provider);
        assert!(reg.get("mock").is_some());
        assert_eq!(reg.get("mock").unwrap().id(), "mock");
        assert!(reg.get("nonexistent").is_none());
        assert_eq!(reg.list().len(), 1);
    }
}
