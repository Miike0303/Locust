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
    #[serde(skip_serializing_if = "Option::is_none")]
    source_lang: Option<String>,
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
        self.free_tier
    }

    fn requires_api_key(&self) -> bool {
        true
    }

    fn supported_languages(&self) -> Vec<LangPair> {
        let sources = [
            "BG", "CS", "DA", "DE", "EL", "EN", "ES", "ET", "FI", "FR", "HU", "ID", "IT", "JA",
            "KO", "LT", "LV", "NB", "NL", "PL", "PT", "RO", "RU", "SK", "SL", "SV", "TR", "UK",
            "ZH",
        ];
        let targets = [
            "BG", "CS", "DA", "DE", "EL", "EN-GB", "EN-US", "ES", "ET", "FI", "FR", "HU", "ID",
            "IT", "JA", "KO", "LT", "LV", "NB", "NL", "PL", "PT-BR", "PT-PT", "RO", "RU", "SK",
            "SL", "SV", "TR", "UK", "ZH-HANS", "ZH-HANT",
        ];
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

        let source = requests[0].source_lang.trim();
        let source_lang = if source.is_empty() || source.eq_ignore_ascii_case("auto") {
            None
        } else {
            Some(requests[0].source_lang.to_uppercase())
        };
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
            .map_err(|e| {
                LocustError::ProviderError(format!(
                    "DeepL connection failed: {}",
                    crate::error_sanitizer::sanitize_provider_error(
                        &e.to_string(),
                        &[&self.api_key]
                    )
                ))
            })?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body_text = crate::error_sanitizer::sanitize_provider_error(
                &resp.text().await.unwrap_or_default(),
                &[&self.api_key],
            );
            return Err(LocustError::ProviderError(
                crate::error_sanitizer::sanitize_provider_error(
                    &format!("DeepL returned status {}: {}", status, body_text),
                    &[&self.api_key],
                ),
            ));
        }

        let deepl_resp: DeepLResponse = resp.json().await.map_err(|e| {
            LocustError::ProviderError(crate::error_sanitizer::sanitize_provider_error(
                &format!("DeepL returned malformed response: {}", e),
                &[&self.api_key],
            ))
        })?;

        // DeepL responses are positional; a count mismatch can misattribute text.
        if deepl_resp.translations.len() != requests.len() {
            return Err(LocustError::ProviderError(
                crate::error_sanitizer::sanitize_provider_error(
                    &format!(
                        "DeepL returned mismatched translation count: expected {}, received {}",
                        requests.len(),
                        deepl_resp.translations.len()
                    ),
                    &[&self.api_key],
                ),
            ));
        }

        Ok(requests
            .iter()
            .zip(deepl_resp.translations.iter())
            .map(|(req, trans)| TranslationResult {
                entry_id: req.entry_id.clone(),
                translation: trans.text.clone(),
                detected_source_lang: trans.detected_source_language.clone(),
                provider: "deepl".to_string(),
                tokens_used: None,
                input_tokens: None,
                output_tokens: None,
                cost_usd: if self.free_tier {
                    Some(0.0)
                } else {
                    Some(req.source.len() as f64 * 0.00002)
                },
            })
            .collect())
    }

    async fn estimate_cost(&self, char_count: usize, _target_lang: &str) -> Option<f64> {
        if self.free_tier {
            Some(0.0)
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
            .map_err(|e| {
                LocustError::ProviderError(format!(
                    "DeepL health check failed: {}",
                    crate::error_sanitizer::sanitize_provider_error(
                        &e.to_string(),
                        &[&self.api_key]
                    )
                ))
            })?;

        if !resp.status().is_success() {
            return Err(LocustError::ProviderError(
                crate::error_sanitizer::sanitize_provider_error(
                    &format!("DeepL health check returned status {}", resp.status()),
                    &[&self.api_key],
                ),
            ));
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

    fn batch_requests() -> Vec<TranslationRequest> {
        [("e2", "Hello"), ("e1", "Goodbye")]
            .into_iter()
            .map(|(entry_id, source)| TranslationRequest {
                entry_id: entry_id.into(),
                source: source.into(),
                source_lang: "en".into(),
                target_lang: "es".into(),
                context: None,
                glossary_hint: None,
            })
            .collect()
    }

    async fn assert_count_mismatch(translations: &[&str]) {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(POST)
                .path("/v2/translate")
                .body_contains(r#""text":["Hello","Goodbye"]"#);
            then.status(200).json_body(serde_json::json!({
                "translations": translations.iter().map(|text| {
                    serde_json::json!({"text": text})
                }).collect::<Vec<_>>()
            }));
        });

        let outcome = make_provider(&server).translate(&batch_requests()).await;
        mock.assert_hits(1);
        let LocustError::ProviderError(message) = outcome.unwrap_err() else {
            panic!("count mismatch must reject the entire batch with a provider error");
        };
        assert!(message.contains("expected 2"), "{message}");
        assert!(
            message.contains(&format!("received {}", translations.len())),
            "{message}"
        );
        for sensitive in ["test-key", "Hello", "Goodbye", "EXTRA", "Hola", "Adiós"] {
            assert!(!message.contains(sensitive), "{message}");
        }
    }

    #[tokio::test]
    async fn test_deepl_translate_rejects_excess_prefix_response() {
        assert_count_mismatch(&["EXTRA", "Hola", "Adiós"]).await;
    }

    #[tokio::test]
    async fn test_deepl_translate_rejects_short_response() {
        assert_count_mismatch(&["Hola"]).await;
    }

    #[tokio::test]
    async fn test_deepl_translate_rejects_empty_response() {
        assert_count_mismatch(&[]).await;
    }

    #[tokio::test]
    async fn test_deepl_count_mismatch_error_uses_sanitizer() {
        let server = MockServer::start();
        let api_key = "expected 2";
        let mock = server.mock(|when, then| {
            when.method(POST)
                .path("/v2/translate")
                .header("Authorization", format!("DeepL-Auth-Key {api_key}"))
                .body_contains(r#""text":["Hello","Goodbye"]"#);
            then.status(200)
                .json_body(serde_json::json!({"translations": []}));
        });
        let provider = DeepLProvider::with_base_url(api_key.into(), false, server.base_url());
        let error = provider
            .translate(&batch_requests())
            .await
            .unwrap_err()
            .to_string();
        mock.assert_hits(1);
        assert!(!error.contains(api_key), "{error}");
        assert!(error.contains("[REDACTED]"), "{error}");
    }

    #[tokio::test]
    async fn test_deepl_exact_count_preserves_order_and_tier_receipts() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(POST)
                .path("/v2/translate")
                .body_contains(r#""text":["Hello","Goodbye"]"#);
            then.status(200).json_body(serde_json::json!({
                "translations": [
                    {"text": "Hola", "detected_source_language": "EN"},
                    {"text": "Adiós", "detected_source_language": "DE"}
                ]
            }));
        });
        let requests = batch_requests();
        for free_tier in [false, true] {
            let provider =
                DeepLProvider::with_base_url("test-key".into(), free_tier, server.base_url());
            let results = provider.translate(&requests).await.unwrap();
            assert_eq!(results.len(), 2);
            for (index, (translation, detected)) in
                [("Hola", "EN"), ("Adiós", "DE")].into_iter().enumerate()
            {
                let result = &results[index];
                assert_eq!(result.entry_id, requests[index].entry_id);
                assert_eq!(result.translation, translation);
                assert_eq!(result.detected_source_lang.as_deref(), Some(detected));
                assert_eq!(result.provider, "deepl");
                assert_eq!(
                    result.cost_usd,
                    Some(if free_tier {
                        0.0
                    } else {
                        requests[index].source.len() as f64 * 0.00002
                    })
                );
            }
        }
        mock.assert_hits(2);
    }

    #[test]
    fn test_deepl_tier_classification_keeps_api_key_requirement() {
        for free_tier in [false, true] {
            let provider = DeepLProvider::new("test-key".into(), free_tier);
            assert_eq!(provider.is_free(), free_tier);
            assert!(provider.requires_api_key());
        }
    }

    #[tokio::test]
    async fn test_deepl_auth_and_quota_errors_remain_sanitized_provider_errors() {
        for free_tier in [false, true] {
            for status in [403, 456] {
                let server = MockServer::start();
                let mock = server.mock(|when, then| {
                    when.method(POST)
                        .path("/v2/translate")
                        .body_contains(r#""text":["Hello","Goodbye"]"#);
                    then.status(status)
                        .body("DeepL credential test-key: auth or quota failure");
                });
                let provider =
                    DeepLProvider::with_base_url("test-key".into(), free_tier, server.base_url());
                let outcome = provider.translate(&batch_requests()).await;
                mock.assert_hits(1);
                let LocustError::ProviderError(message) = outcome.unwrap_err() else {
                    panic!("auth/quota failure must remain a provider error");
                };
                assert!(message.contains(&status.to_string()), "{message}");
                assert!(message.contains("auth or quota failure"), "{message}");
                assert!(!message.contains("test-key"), "{message}");
                assert!(message.contains("[REDACTED]"), "{message}");
            }
        }
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

    async fn assert_source_lang_request(source_lang: &str, expected_source_lang: Option<&str>) {
        let server = MockServer::start();
        let mut expected_body = serde_json::json!({
            "text": ["Hello"],
            "target_lang": "ES"
        });
        if let Some(source_lang) = expected_source_lang {
            expected_body["source_lang"] = serde_json::json!(source_lang);
        }
        let mock = server.mock(|when, then| {
            when.method(POST)
                .path("/v2/translate")
                .json_body(expected_body);
            then.status(200).json_body(serde_json::json!({
                "translations": [{"text": "Hola"}]
            }));
        });
        let provider = make_provider(&server);
        let requests = [TranslationRequest {
            entry_id: "e1".to_string(),
            source: "Hello".to_string(),
            source_lang: source_lang.to_string(),
            target_lang: "es".to_string(),
            context: None,
            glossary_hint: None,
        }];

        let results = provider.translate(&requests).await;
        mock.assert();
        assert_eq!(results.unwrap()[0].translation, "Hola");
    }

    #[tokio::test]
    async fn auto_source_omits_source_lang() {
        assert_source_lang_request("auto", None).await;
    }

    #[tokio::test]
    async fn uppercase_auto_source_omits_source_lang() {
        assert_source_lang_request("AUTO", None).await;
    }

    #[tokio::test]
    async fn empty_source_omits_source_lang() {
        assert_source_lang_request("", None).await;
    }

    #[tokio::test]
    async fn trimmed_auto_source_omits_source_lang() {
        assert_source_lang_request(" \tAuTo\n", None).await;
    }

    #[tokio::test]
    async fn whitespace_source_omits_source_lang() {
        assert_source_lang_request(" \t\n", None).await;
    }

    #[tokio::test]
    async fn explicit_ja_source_sends_uppercase_lang() {
        assert_source_lang_request("ja", Some("JA")).await;
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
        assert_eq!(cost, Some(0.0));
    }
}
