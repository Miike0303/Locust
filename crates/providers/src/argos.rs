use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use locust_core::error::{LocustError, Result};
use locust_core::models::{TranslationRequest, TranslationResult};
use locust_core::translation::{LangPair, TranslationProvider};

pub struct ArgosProvider {
    base_url: String,
    client: reqwest::Client,
}

impl ArgosProvider {
    pub fn new(base_url: String) -> Self {
        Self {
            base_url,
            client: reqwest::Client::new(),
        }
    }
}

impl Default for ArgosProvider {
    fn default() -> Self {
        Self::new("http://localhost:5000".to_string())
    }
}

#[derive(Serialize)]
struct ArgosRequest {
    q: Vec<String>,
    source: String,
    target: String,
}

#[derive(Deserialize)]
struct ArgosResponse {
    #[serde(rename = "translatedText")]
    translated_text: Vec<String>,
}

#[async_trait]
impl TranslationProvider for ArgosProvider {
    fn id(&self) -> &str {
        "argos"
    }

    fn name(&self) -> &str {
        "Argos Translate (offline & free)"
    }

    fn is_free(&self) -> bool {
        true
    }

    fn requires_api_key(&self) -> bool {
        false
    }

    async fn translate(&self, requests: &[TranslationRequest]) -> Result<Vec<TranslationResult>> {
        if requests.is_empty() {
            return Ok(Vec::new());
        }

        let source_lang = &requests[0].source_lang;
        let target_lang = &requests[0].target_lang;

        let sources: Vec<String> = requests.iter().map(|r| r.source.clone()).collect();

        let body = ArgosRequest {
            q: sources,
            source: source_lang.clone(),
            target: target_lang.clone(),
        };

        let resp = self
            .client
            .post(format!("{}/translate", self.base_url))
            .json(&body)
            .send()
            .await
            .map_err(|e| LocustError::ProviderError(format!("Argos connection failed: {}", e)))?;

        if !resp.status().is_success() {
            return Err(LocustError::ProviderError(format!(
                "Argos returned status {}",
                resp.status()
            )));
        }

        let argos_resp: ArgosResponse = resp.json().await.map_err(|e| {
            LocustError::ProviderError(format!("Argos returned malformed response: {}", e))
        })?;

        if argos_resp.translated_text.len() != requests.len() {
            return Err(LocustError::ProviderError(format!(
                "Argos returned {} translations for {} requests",
                argos_resp.translated_text.len(),
                requests.len()
            )));
        }

        Ok(requests
            .iter()
            .zip(argos_resp.translated_text.iter())
            .map(|(req, text)| TranslationResult {
                entry_id: req.entry_id.clone(),
                translation: text.clone(),
                detected_source_lang: None,
                provider: "argos".to_string(),
                tokens_used: None,
                cost_usd: None,
            })
            .collect())
    }

    async fn estimate_cost(&self, _char_count: usize, _target_lang: &str) -> Option<f64> {
        None
    }

    async fn health_check(&self) -> Result<()> {
        let resp = self
            .client
            .get(format!("{}/languages", self.base_url))
            .send()
            .await
            .map_err(|e| {
                LocustError::ProviderError(format!(
                    "Argos is not running at {}. Install with: pip install argostranslate && argos-translate-server. Error: {}",
                    self.base_url, e
                ))
            })?;

        if !resp.status().is_success() {
            return Err(LocustError::ProviderError(format!(
                "Argos health check failed with status {}",
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

    #[tokio::test]
    async fn test_argos_translate_success() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(POST).path("/translate");
            then.status(200)
                .json_body(serde_json::json!({
                    "translatedText": ["Hola"]
                }));
        });

        let provider = ArgosProvider::new(server.base_url());
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
        assert_eq!(results[0].provider, "argos");
    }

    #[tokio::test]
    async fn test_argos_translate_batch() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(POST).path("/translate");
            then.status(200)
                .json_body(serde_json::json!({
                    "translatedText": ["Hola", "Mundo", "Prueba"]
                }));
        });

        let provider = ArgosProvider::new(server.base_url());
        let requests = vec![
            TranslationRequest {
                entry_id: "e1".to_string(),
                source: "Hello".to_string(),
                source_lang: "en".to_string(),
                target_lang: "es".to_string(),
                context: None,
                glossary_hint: None,
            },
            TranslationRequest {
                entry_id: "e2".to_string(),
                source: "World".to_string(),
                source_lang: "en".to_string(),
                target_lang: "es".to_string(),
                context: None,
                glossary_hint: None,
            },
            TranslationRequest {
                entry_id: "e3".to_string(),
                source: "Test".to_string(),
                source_lang: "en".to_string(),
                target_lang: "es".to_string(),
                context: None,
                glossary_hint: None,
            },
        ];

        let results = provider.translate(&requests).await.unwrap();
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].translation, "Hola");
        assert_eq!(results[1].translation, "Mundo");
        assert_eq!(results[2].translation, "Prueba");
    }

    #[tokio::test]
    async fn test_argos_health_check_ok() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET).path("/languages");
            then.status(200).json_body(serde_json::json!([]));
        });

        let provider = ArgosProvider::new(server.base_url());
        assert!(provider.health_check().await.is_ok());
    }

    #[tokio::test]
    async fn test_argos_health_check_down() {
        // Use a port that nothing listens on
        let provider = ArgosProvider::new("http://127.0.0.1:59999".to_string());
        let result = provider.health_check().await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("argostranslate"));
    }

    #[tokio::test]
    async fn test_argos_wrong_response_shape() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(POST).path("/translate");
            then.status(200).body("{\"wrong\": \"shape\"}");
        });

        let provider = ArgosProvider::new(server.base_url());
        let requests = vec![TranslationRequest {
            entry_id: "e1".to_string(),
            source: "Hello".to_string(),
            source_lang: "en".to_string(),
            target_lang: "es".to_string(),
            context: None,
            glossary_hint: None,
        }];

        let result = provider.translate(&requests).await;
        assert!(result.is_err());
    }
}
