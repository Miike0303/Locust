use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use locust_core::error::{LocustError, Result};
use locust_core::models::{TranslationRequest, TranslationResult};
use locust_core::translation::TranslationProvider;

pub struct OllamaProvider {
    base_url: String,
    model: String,
    client: reqwest::Client,
}

impl OllamaProvider {
    pub fn new(base_url: Option<String>, model: Option<String>) -> Self {
        Self {
            base_url: base_url.unwrap_or_else(|| "http://localhost:11434".to_string()),
            model: model.unwrap_or_else(|| "llama3.2".to_string()),
            client: reqwest::Client::new(),
        }
    }
}

impl Default for OllamaProvider {
    fn default() -> Self {
        Self::new(None, None)
    }
}

fn build_system_prompt(req: &TranslationRequest) -> String {
    let mut prompt = format!(
        "You are a professional game translator. Translate the following strings from {} to {}.",
        req.source_lang, req.target_lang
    );
    if let Some(ref ctx) = req.context {
        prompt.push_str(&format!("\nGame context: {}", ctx));
    }
    if let Some(ref hint) = req.glossary_hint {
        prompt.push_str(&format!("\n{}", hint));
    }
    prompt.push_str(
        "\nRules:\n- Preserve all placeholder tokens like {PL_0}, {PL_1} exactly as-is\n- Return ONLY a JSON array of translated strings, in the same order as input\n- Do not add explanations or notes",
    );
    prompt
}

fn parse_json_array(text: &str) -> std::result::Result<Vec<String>, String> {
    if let Ok(arr) = serde_json::from_str::<Vec<String>>(text) {
        return Ok(arr);
    }
    if let (Some(start), Some(end)) = (text.find('['), text.rfind(']')) {
        let substr = &text[start..=end];
        if let Ok(arr) = serde_json::from_str::<Vec<String>>(substr) {
            return Ok(arr);
        }
    }
    Err(format!("could not parse JSON array from response: {}", text))
}

#[derive(Serialize)]
struct OllamaRequest {
    model: String,
    messages: Vec<OllamaMessage>,
    stream: bool,
}

#[derive(Serialize)]
struct OllamaMessage {
    role: String,
    content: String,
}

#[derive(Deserialize)]
struct OllamaResponse {
    message: OllamaRespMessage,
    eval_count: Option<u32>,
}

#[derive(Deserialize)]
struct OllamaRespMessage {
    content: String,
}

#[derive(Deserialize)]
struct OllamaTagsResponse {
    models: Vec<OllamaModel>,
}

#[derive(Deserialize)]
struct OllamaModel {
    name: String,
}

#[async_trait]
impl TranslationProvider for OllamaProvider {
    fn id(&self) -> &str {
        "ollama"
    }

    fn name(&self) -> &str {
        "Ollama (local LLM)"
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

        let system_prompt = build_system_prompt(&requests[0]);
        let sources: Vec<&str> = requests.iter().map(|r| r.source.as_str()).collect();
        let user_content = serde_json::to_string(&sources)
            .map_err(|e| LocustError::ProviderError(e.to_string()))?;

        let body = OllamaRequest {
            model: self.model.clone(),
            messages: vec![
                OllamaMessage {
                    role: "system".to_string(),
                    content: system_prompt,
                },
                OllamaMessage {
                    role: "user".to_string(),
                    content: user_content,
                },
            ],
            stream: false,
        };

        let resp = self
            .client
            .post(format!("{}/api/chat", self.base_url))
            .json(&body)
            .send()
            .await
            .map_err(|e| LocustError::ProviderError(format!("Ollama connection failed: {}", e)))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body_text = resp.text().await.unwrap_or_default();
            return Err(LocustError::ProviderError(format!(
                "Ollama returned status {}: {}",
                status, body_text
            )));
        }

        let ollama_resp: OllamaResponse = resp.json().await.map_err(|e| {
            LocustError::ProviderError(format!("Ollama malformed response: {}", e))
        })?;

        let translations = parse_json_array(&ollama_resp.message.content)
            .map_err(|e| LocustError::ProviderError(e))?;

        let tokens_used = ollama_resp.eval_count;

        Ok(requests
            .iter()
            .zip(translations.iter())
            .map(|(req, trans)| TranslationResult {
                entry_id: req.entry_id.clone(),
                translation: trans.clone(),
                detected_source_lang: None,
                provider: "ollama".to_string(),
                tokens_used,
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
            .get(format!("{}/api/tags", self.base_url))
            .send()
            .await
            .map_err(|e| {
                LocustError::ProviderError(format!(
                    "Ollama is not running at {}. Install from https://ollama.ai. Error: {}",
                    self.base_url, e
                ))
            })?;

        if !resp.status().is_success() {
            return Err(LocustError::ProviderError(format!(
                "Ollama health check returned status {}",
                resp.status()
            )));
        }

        let tags: OllamaTagsResponse = resp.json().await.map_err(|e| {
            LocustError::ProviderError(format!("Ollama tags malformed: {}", e))
        })?;

        let model_found = tags
            .models
            .iter()
            .any(|m| m.name == self.model || m.name.starts_with(&format!("{}:", self.model)));

        if !model_found {
            return Err(LocustError::ProviderError(format!(
                "Model '{}' not found in Ollama. Run: ollama pull {}",
                self.model, self.model
            )));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::prelude::*;

    fn make_provider(server: &MockServer) -> OllamaProvider {
        OllamaProvider::new(Some(server.base_url()), Some("llama3.2".to_string()))
    }

    #[tokio::test]
    async fn test_ollama_translate_success() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(POST).path("/api/chat");
            then.status(200).json_body(serde_json::json!({
                "message": {"role": "assistant", "content": "[\"Hola\",\"Mundo\"]"},
                "eval_count": 25
            }));
        });

        let provider = make_provider(&server);
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
        ];

        let results = provider.translate(&requests).await.unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].translation, "Hola");
        assert_eq!(results[1].translation, "Mundo");
        assert_eq!(results[0].tokens_used, Some(25));
        assert_eq!(results[0].cost_usd, None);
    }

    #[tokio::test]
    async fn test_ollama_health_check_model_found() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET).path("/api/tags");
            then.status(200).json_body(serde_json::json!({
                "models": [
                    {"name": "llama3.2:latest"},
                    {"name": "mistral:latest"}
                ]
            }));
        });

        let provider = make_provider(&server);
        assert!(provider.health_check().await.is_ok());
    }

    #[tokio::test]
    async fn test_ollama_health_check_model_missing() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET).path("/api/tags");
            then.status(200).json_body(serde_json::json!({
                "models": [{"name": "mistral:latest"}]
            }));
        });

        let provider = make_provider(&server);
        let result = provider.health_check().await;
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("ollama pull"));
    }

    #[tokio::test]
    async fn test_ollama_estimate_cost_is_none() {
        let provider = OllamaProvider::default();
        assert_eq!(provider.estimate_cost(1000, "en").await, None);
    }
}
