use std::sync::Arc;

use async_trait::async_trait;
use locust_core::error::Result;
use locust_core::model_policy::{lookup_model_policy, ModelOverrides, ModelPolicy};
use locust_core::models::{TranslationRequest, TranslationResult};
use locust_core::translation::{LangPair, TranslationProvider};

/// Snapshot the exact constructor-resolved match once, at registration.
/// No match returns the original object, including all legacy arithmetic.
pub(crate) fn configure(
    provider: Arc<dyn TranslationProvider>,
    overrides: &ModelOverrides,
) -> Arc<dyn TranslationProvider> {
    let policy = provider
        .effective_model()
        .and_then(|model| lookup_model_policy(overrides, provider.id(), model));
    match policy.cloned() {
        Some(policy) => Arc::new(ConfiguredProvider { provider, policy }),
        None => provider,
    }
}

struct ConfiguredProvider {
    provider: Arc<dyn TranslationProvider>,
    policy: ModelPolicy,
}

impl ConfiguredProvider {
    fn prices(&self) -> Option<(f64, f64)> {
        if self.provider.is_free() || !self.provider.supports_token_pricing() {
            return None;
        }
        Some((
            self.policy.input_usd_per_million?,
            self.policy.output_usd_per_million?,
        ))
    }
}

#[async_trait]
impl TranslationProvider for ConfiguredProvider {
    fn id(&self) -> &str {
        self.provider.id()
    }
    fn name(&self) -> &str {
        self.provider.name()
    }
    fn is_free(&self) -> bool {
        self.provider.is_free()
    }
    fn requires_api_key(&self) -> bool {
        self.provider.requires_api_key()
    }
    fn effective_model(&self) -> Option<&str> {
        self.provider.effective_model()
    }
    fn model_policy(&self) -> Option<&ModelPolicy> {
        Some(&self.policy)
    }
    fn supports_token_pricing(&self) -> bool {
        self.provider.supports_token_pricing()
    }
    fn supported_languages(&self) -> Vec<LangPair> {
        self.provider.supported_languages()
    }
    async fn translate(&self, requests: &[TranslationRequest]) -> Result<Vec<TranslationResult>> {
        let mut results = self.provider.translate(requests).await?;
        if let Some((input, output)) = self.prices() {
            // Do not infer usage from text or total_tokens. Preserve the adapter's
            // attribution (batch usage is carried only on its first result).
            for result in &mut results {
                result.cost_usd = result
                    .input_tokens
                    .zip(result.output_tokens)
                    .map(|(i, o)| (f64::from(i) * input + f64::from(o) * output) / 1_000_000.0);
            }
        }
        Ok(results)
    }
    async fn estimate_cost(&self, char_count: usize, target_lang: &str) -> Option<f64> {
        match self.prices() {
            Some((input, output)) => {
                // Retain the token adapters' legacy character approximation.
                let tokens = char_count as f64 * 1.3;
                Some((tokens * input + tokens * output) / 1_000_000.0)
            }
            None => self.provider.estimate_cost(char_count, target_lang).await,
        }
    }
    async fn health_check(&self) -> Result<()> {
        self.provider.health_check().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::prelude::*;
    use locust_core::config::AppConfig;
    use locust_core::models::TranslationRequest;

    fn overrides(policy: serde_json::Value) -> ModelOverrides {
        AppConfig::default()
            .patched(serde_json::json!({"model_overrides": {"openai": {"gpt-4o-mini": policy}}}))
            .unwrap()
            .model_overrides
    }

    fn prices() -> serde_json::Value {
        serde_json::json!({"input_usd_per_million": 2.0, "output_usd_per_million": 6.0})
    }

    fn requests(count: usize) -> Vec<TranslationRequest> {
        (0..count)
            .map(|i| TranslationRequest {
                entry_id: i.to_string(),
                source: "Hello".into(),
                source_lang: "en".into(),
                target_lang: "es".into(),
                context: Some("RPG".into()),
                glossary_hint: None,
            })
            .collect()
    }

    fn openai(server: &MockServer) -> Arc<dyn TranslationProvider> {
        Arc::new(crate::openai::OpenAiProvider::new(
            "fixture-key".into(),
            None,
            Some(server.base_url()),
        ))
    }

    #[test]
    fn model_policy_effective_metadata() {
        let providers: Vec<(Arc<dyn TranslationProvider>, &str)> = vec![
            (
                Arc::new(crate::openai::OpenAiProvider::new(
                    "fixture".into(),
                    None,
                    None,
                )),
                "gpt-4o-mini",
            ),
            (
                Arc::new(crate::openai::OpenAiProvider::lm_studio(None, None)),
                "qwen/qwen3-14b",
            ),
            (
                Arc::new(crate::claude::ClaudeProvider::new(
                    "fixture".into(),
                    None,
                    None,
                )),
                "claude-haiku-4-5-20251001",
            ),
            (
                Arc::new(crate::ollama::OllamaProvider::default()),
                "llama3.2",
            ),
            (
                Arc::new(crate::antigravity::AntigravityProvider::new(None)),
                "gemini-3.8-flash-low",
            ),
            (
                Arc::new(crate::xai_oauth::GrokSubscriptionProvider::new(None)),
                "grok-4.6",
            ),
        ];
        for (provider, expected) in providers {
            assert_eq!(provider.effective_model(), Some(expected));
            let policy = ModelPolicy {
                context_window: Some(1234),
                ..Default::default()
            };
            let config = ModelOverrides::from([(
                provider.id().into(),
                [(expected.into(), policy.clone())].into(),
            )]);
            let decorated = configure(provider, &config);
            assert_eq!(decorated.effective_model(), Some(expected));
            assert_eq!(decorated.model_policy(), Some(&policy));
        }
        assert_eq!(crate::mock::MockProvider.effective_model(), None);
        assert!(crate::mock::MockProvider.model_policy().is_none());
    }

    #[tokio::test]
    async fn model_policy_matching_estimate() {
        let server = MockServer::start();
        let provider = configure(openai(&server), &overrides(prices()));
        assert_eq!(provider.estimate_cost(100, "es").await, Some(0.00104));
    }

    #[tokio::test]
    async fn model_policy_matching_known_usage() {
        let server = MockServer::start();
        let wire = server.mock(|when, then| {
            when.method(POST).path("/v1/chat/completions");
            then.status(200).json_body(serde_json::json!({
                "choices": [{"message": {"content": "[\"Hola\",\"Hola\"]"}}],
                "usage": {"prompt_tokens": 50, "completion_tokens": 10, "total_tokens": 60}
            }));
        });
        let provider = configure(openai(&server), &overrides(prices()));
        let results = provider.translate(&requests(2)).await.unwrap();
        assert_eq!(results[0].cost_usd, Some(0.00016));
        assert_eq!(results[0].input_tokens, Some(50));
        assert_eq!(results[0].output_tokens, Some(10));
        assert_eq!(results[0].tokens_used, Some(60));
        assert_eq!(results[1].cost_usd, None);
        assert_eq!(results[1].input_tokens, None);
        assert_eq!(results[1].output_tokens, None);
        assert_eq!(results[1].tokens_used, None);
        wire.assert();
    }

    #[tokio::test]
    async fn model_policy_legacy_passthrough_and_exact_wire() {
        let server = MockServer::start();
        let requests = requests(1);
        let (system, user) = crate::openai::build_batch_prompt(&requests).unwrap();
        // Match the entire serialized body, not just selected prompt fragments.
        let body = format!("{{\"model\":\"gpt-4o-mini\",\"messages\":[{{\"role\":\"system\",\"content\":{}}},{{\"role\":\"user\",\"content\":{}}}]}}",
            serde_json::to_string(&system).unwrap(), serde_json::to_string(&user).unwrap());
        let wire = server.mock(|when, then| {
            when.method(POST)
                .path("/v1/chat/completions")
                .header("authorization", "Bearer fixture-key")
                .body(body);
            then.status(200).json_body(serde_json::json!({
                "choices": [{"message": {"content": "[\"Hola\"]"}}],
                "usage": {"prompt_tokens": 50, "completion_tokens": 10, "total_tokens": 60}
            }));
        });
        let baseline = openai(&server);
        let estimate = baseline.estimate_cost(100, "es").await;
        assert_eq!(estimate, Some(0.0000975));
        let results = serde_json::to_value(baseline.translate(&requests).await.unwrap()).unwrap();
        let mut wrong_model = overrides(prices());
        let policy = wrong_model
            .get_mut("openai")
            .unwrap()
            .remove("gpt-4o-mini")
            .unwrap();
        wrong_model
            .get_mut("openai")
            .unwrap()
            .insert("GPT-4o-mini".into(), policy);
        let mut wrong_provider = overrides(prices());
        let policy = wrong_provider.remove("openai").unwrap();
        wrong_provider.insert("OpenAI".into(), policy);
        for config in [
            ModelOverrides::new(),
            overrides(serde_json::json!({})),
            wrong_model,
            wrong_provider,
            overrides(serde_json::json!({"context_window": 2048})),
        ] {
            let provider = configure(baseline.clone(), &config);
            if provider.model_policy().is_none() {
                assert!(Arc::ptr_eq(&provider, &baseline));
            }
            assert_eq!(provider.estimate_cost(100, "es").await, estimate);
            assert_eq!(
                serde_json::to_value(provider.translate(&requests).await.unwrap()).unwrap(),
                results
            );
        }
        wire.assert_hits(6);
    }

    struct Probe {
        free: bool,
        token: bool,
    }

    #[async_trait]
    impl TranslationProvider for Probe {
        fn id(&self) -> &str {
            "openai"
        }
        fn name(&self) -> &str {
            "probe"
        }
        fn is_free(&self) -> bool {
            self.free
        }
        fn requires_api_key(&self) -> bool {
            true
        }
        fn effective_model(&self) -> Option<&str> {
            Some("gpt-4o-mini")
        }
        fn supports_token_pricing(&self) -> bool {
            self.token
        }
        fn supported_languages(&self) -> Vec<LangPair> {
            vec![LangPair {
                source: "en".into(),
                target: "es".into(),
            }]
        }
        async fn translate(
            &self,
            requests: &[TranslationRequest],
        ) -> Result<Vec<TranslationResult>> {
            if requests.is_empty() {
                return std::future::pending().await;
            }
            assert_eq!(
                serde_json::to_value(requests).unwrap(),
                serde_json::to_value(super::tests::requests(2)).unwrap()
            );
            Err(locust_core::LocustError::ProviderError(
                "fixture 429".into(),
            ))
        }
        async fn estimate_cost(&self, chars: usize, target: &str) -> Option<f64> {
            assert_eq!((chars, target), (17, "es"));
            Some(7.0)
        }
        async fn health_check(&self) -> Result<()> {
            Err(locust_core::LocustError::ProviderError(
                "fixture health".into(),
            ))
        }
    }

    #[tokio::test]
    async fn model_policy_delegates_capabilities_errors_and_non_token_rates() {
        for (free, token) in [(false, false), (true, true), (false, true)] {
            let provider = configure(Arc::new(Probe { free, token }), &overrides(prices()));
            assert_eq!(provider.id(), "openai");
            assert_eq!(provider.name(), "probe");
            assert_eq!(provider.is_free(), free);
            assert!(provider.requires_api_key());
            assert_eq!(provider.supports_token_pricing(), token);
            let languages = provider.supported_languages();
            assert_eq!((&*languages[0].source, &*languages[0].target), ("en", "es"));
            if free || !token {
                assert_eq!(provider.estimate_cost(17, "es").await, Some(7.0));
            }
            assert_eq!(
                provider
                    .translate(&requests(2))
                    .await
                    .unwrap_err()
                    .to_string(),
                locust_core::LocustError::ProviderError("fixture 429".into()).to_string()
            );
            assert_eq!(
                provider.health_check().await.unwrap_err().to_string(),
                locust_core::LocustError::ProviderError("fixture health".into()).to_string()
            );
            assert!(tokio::time::timeout(
                std::time::Duration::from_millis(1),
                provider.translate(&[])
            )
            .await
            .is_err());
        }
    }

    #[tokio::test]
    async fn model_policy_zero_prices_and_missing_usage() {
        for (policy, cost) in [
            (prices(), Some(0.00016)),
            (
                serde_json::json!({"input_usd_per_million": 0, "output_usd_per_million": 0}),
                Some(0.0),
            ),
        ] {
            for usage in [
                serde_json::json!(null),
                serde_json::json!({"prompt_tokens": 50, "completion_tokens": 10, "total_tokens": 60}),
            ] {
                let server = MockServer::start();
                let wire = server.mock(|when, then| {
                    when.method(POST).path("/v1/chat/completions");
                    then.status(200).json_body(serde_json::json!({
                        "choices": [{"message": {"content": "[\"Hola\"]"}}], "usage": usage
                    }));
                });
                let provider = configure(openai(&server), &overrides(policy.clone()));
                let result = provider.translate(&requests(1)).await.unwrap();
                assert_eq!(
                    result[0].cost_usd,
                    if usage.is_null() { None } else { cost }
                );
                if cost == Some(0.0) {
                    assert_eq!(provider.estimate_cost(100, "es").await, Some(0.0));
                }
                wire.assert();
            }
        }
    }
}
