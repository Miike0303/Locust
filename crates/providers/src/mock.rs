use async_trait::async_trait;

use locust_core::error::Result;
use locust_core::models::{TranslationRequest, TranslationResult};
use locust_core::translation::{LangPair, TranslationProvider};

pub struct MockProvider;

#[async_trait]
impl TranslationProvider for MockProvider {
    fn id(&self) -> &str {
        "mock"
    }

    fn name(&self) -> &str {
        "Mock (testing)"
    }

    fn is_free(&self) -> bool {
        true
    }

    fn requires_api_key(&self) -> bool {
        false
    }

    async fn translate(&self, requests: &[TranslationRequest]) -> Result<Vec<TranslationResult>> {
        Ok(requests
            .iter()
            .map(|r| TranslationResult {
                entry_id: r.entry_id.clone(),
                translation: format!("[MOCK:{}] {}", r.target_lang, r.source),
                detected_source_lang: None,
                provider: "mock".to_string(),
                tokens_used: None,
                cost_usd: None,
            })
            .collect())
    }

    async fn estimate_cost(&self, _char_count: usize, _target_lang: &str) -> Option<f64> {
        None
    }

    async fn health_check(&self) -> Result<()> {
        Ok(())
    }
}
