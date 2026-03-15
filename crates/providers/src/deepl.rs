use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use locust_core::error::{LocustError, Result};
use locust_core::models::{TranslationRequest, TranslationResult};
use locust_core::translation::{LangPair, TranslationProvider};

pub struct DeepLProvider {
    api_key: String,
    free_tier: bool,
    base_url: String,
    client: reqwest::Client,
}

impl DeepLProvider {
    pub fn new(api_key: String, free_tier: bool) -> Self {
        let base_url = if free_tier {
            "https://api-free.deepl.com".to_string()
        } else {
            "https://api.deepl.com".to_string()
        };
        Self {
            api_key,
            free_tier,
            base_url,
            client: reqwest::Client::new(),
        }
    }

    /// Create with custom base URL (for testing)
    pub fn with_base_url(api_key: String, free_tier: bool, base_url: String) -> Self {
        Self {
            api_key,
            free_tier,
            base_url,
            client: reqwest::Client::new(),
        }
    }
}

#[derive(Serialize)]
struct DeepLRequest {
    text: Vec<String>,
    source_lang: String,
    target_lang: String,
}

#[derive(Deserialize)]
struct DeepLResponse {
    translations: Vec<DeepLTranslation>,
}

#[derive(Deserialize)]
struct DeepLTranslation {
    text: String,
    detected_source_language: Option<String>,
}

#[async_trait]
impl TranslationProvider for DeepLProvider {
    fn id(&self) -> &str {
        "deepl"
    }

    fn name(&self) -> &str {
        "DeepL"
    }

    fn is_free(&self) -> bool {
        false
    }

    fn requires_api_key(&self) -> bool {
        true
    }

    fn supported_languages(&self) -> Vec<LangPair> {
        let sources = ["BG","CS","DA","DE","EL","EN","ES","ET","FI","FR","HU","ID","IT","JA","KO","LT","LV","NB","NL","PL","PT","RO","RU","SK","SL","SV","TR","UK","ZH"];
        let targets = ["BG","CS","DA","DE","EL","EN-GB","EN-US","ES","ET","FI","FR","HU","ID","IT","JA","KO","LT","LV","NB","NL","PL","PT-BR","PT-PT","RO","RU","SK","SL","SV","TR","UK","ZH-HANS","ZH-HANT"];
        let mut pairs = Vec::new();
        for &s in &sources {
            for &t in &targets {
                if !t.starts_with(s) {
                    pairs.push(LangPair {
                        source: s.to_lowercase(),
                        target: t.to_lowercase(),
                    });
                }
            }
        }
        pairs
    }

    async fn translate(&self, requests: &[TranslationRequest]) -> Result<Vec<TranslationResult>> {
        if requests.is_empty() {
            return Ok(Vec::new());
        }

        let source_lang = requests[0].source_lang.to_uppercase();
        let target_lang = requests[0].target_lang.to_uppercase();
        let texts: Vec<String> = requests.iter().map(|r| r.source.clone()).collect();

        let body = DeepLRequest {
            text: texts,
            source_lang,
            target_lang,
        };

        let resp = self
            .client
            .post(format!("{}/v2/translate", self.base_url))
            .header("Authorization", format!("DeepL-Auth-Key {}", self.api_key))
            .json(&body)
            .send()
            .await
            .map_err(|e| LocustError::ProviderError(format!("DeepL connection failed: {}", e)))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body_text = resp.text().await.unwrap_or_default();
            return Err(LocustError::ProviderError(format!(
                "DeepL returned status {}: {}",
                status, body_text
            )));
        }

        let deepl_resp: DeepLResponse = resp.json().await.map_err(|e| {
            LocustError::ProviderError(format!("DeepL returned malformed response: {}", e))
        })?;

        let char_count: usize = requests.iter().map(|r| r.source.len()).sum();

        Ok(requests
            .iter()
            .zip(deepl_resp.translations.iter())
            .map(|(req, trans)| TranslationResult {
                entry_id: req.entry_id.clone(),
                translation: trans.text.clone(),
                detected_source_lang: trans.detected_source_language.clone(),
                provider: "deepl".to_string(),
                tokens_used: None,
                cost_usd: if !self.free_tier {
                    Some(req.source.len() as f64 * 0.00002)
                } else {
                    None
                },
            })
            .collect())
    }

    async fn estimate_cost(&self, char_count: usize, _target_lang: &str) -> Option<f64> {
        if self.free_tier {
            None
        } else {
            Some(char_count as f64 * 0.00002)
        }
    }

    async fn health_check(&self) -> Result<()> {
        let resp = self
            .client
            .get(format!("{}/v2/usage", self.base_url))
            .header("Authorization", format!("DeepL-Auth-Key {}", self.api_key))
            .send()
            .await
            .map_err(|e| LocustError::ProviderError(format!("DeepL health check failed: {}", e)))?;

        if !resp.status().is_success() {
            return Err(LocustError::ProviderError(format!(
                "DeepL health check returned status {}",
                resp.status()
            )));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::prelude::*;

    fn make_provider(server: &MockServer) -> DeepLProvider {
        DeepLProvider::with_base_url("test-key".to_string(), false, server.base_url())
    }

    #[tokio::test]
    async fn test_deepl_translate_success() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(POST).path("/v2/translate");
            then.status(200).json_body(serde_json::json!({
                "translations": [
                    {"text": "Hola", "detected_source_language": "EN"}
                ]
            }));
        });

        let provider = make_provider(&server);
        let requests = vec![TranslationRequest {
            entry_id: "e1".to_string(),
            source: "Hello".to_string(),
            source_lang: "en".to_string(),
            target_lang: "es".to_string(),
            context: None,
            glossary_hint: None,
        }];

        let results = provider.translate(&requests).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].translation, "Hola");
        assert_eq!(results[0].detected_source_lang, Some("EN".to_string()));
    }

    #[tokio::test]
    async fn test_deepl_translate_sends_uppercase_lang() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(POST)
                .path("/v2/translate")
                .json_body_partial(r#"{"source_lang":"EN","target_lang":"ES"}"#);
            then.status(200).json_body(serde_json::json!({
                "translations": [{"text": "Hola"}]
            }));
        });

        let provider = make_provider(&server);
        let requests = vec![TranslationRequest {
            entry_id: "e1".to_string(),
            source: "Hello".to_string(),
            source_lang: "en".to_string(),
            target_lang: "es".to_string(),
            context: None,
            glossary_hint: None,
        }];

        provider.translate(&requests).await.unwrap();
        mock.assert();
    }

    #[tokio::test]
    async fn test_deepl_health_check_ok() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET).path("/v2/usage");
            then.status(200).json_body(serde_json::json!({
                "character_count": 100,
                "character_limit": 500000
            }));
        });

        let provider = make_provider(&server);
        assert!(provider.health_check().await.is_ok());
    }

    #[tokio::test]
    async fn test_deepl_health_check_unauthorized() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET).path("/v2/usage");
            then.status(403).body("Forbidden");
        });

        let provider = make_provider(&server);
        let result = provider.health_check().await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("403"));
    }

    #[tokio::test]
    async fn test_deepl_cost_estimate_pro() {
        let provider = DeepLProvider::new("key".to_string(), false);
        let cost = provider.estimate_cost(1000, "es").await;
        assert_eq!(cost, Some(0.02));
    }

    #[tokio::test]
    async fn test_deepl_free_tier_url() {
        let provider = DeepLProvider::new("key".to_string(), true);
        assert!(provider.base_url.contains("api-free.deepl.com"));

        let cost = provider.estimate_cost(1000, "es").await;
        assert_eq!(cost, None);
    }
}
