use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use locust_core::error::{LocustError, Result};
use locust_core::models::{TranslationRequest, TranslationResult};
use locust_core::translation::TranslationProvider;

pub struct OpenAiProvider {
    api_key: String,
    model: String,
    base_url: String,
    client: reqwest::Client,
}

impl OpenAiProvider {
    pub fn new(api_key: String, model: Option<String>, base_url: Option<String>) -> Self {
        Self {
            api_key,
            model: model.unwrap_or_else(|| "gpt-4o-mini".to_string()),
            base_url: base_url.unwrap_or_else(|| "https://api.openai.com".to_string()),
            client: reqwest::Client::new(),
        }
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
    // Try direct parse first
    if let Ok(arr) = serde_json::from_str::<Vec<String>>(text) {
        return Ok(arr);
    }
    // Lenient: find first [ and last ]
    if let (Some(start), Some(end)) = (text.find('['), text.rfind(']')) {
        let substr = &text[start..=end];
        if let Ok(arr) = serde_json::from_str::<Vec<String>>(substr) {
            return Ok(arr);
        }
    }
    Err(format!("could not parse JSON array from response: {}", text))
}

#[derive(Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<ChatMessage>,
}

#[derive(Serialize)]
struct ChatMessage {
    role: String,
    content: String,
}

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<ChatChoice>,
    usage: Option<ChatUsage>,
}

#[derive(Deserialize)]
struct ChatChoice {
    message: ChatChoiceMessage,
}

#[derive(Deserialize)]
struct ChatChoiceMessage {
    content: String,
}

#[derive(Deserialize)]
struct ChatUsage {
    prompt_tokens: u32,
    completion_tokens: u32,
    total_tokens: u32,
}

#[async_trait]
impl TranslationProvider for OpenAiProvider {
    fn id(&self) -> &str {
        "openai"
    }

    fn name(&self) -> &str {
        "OpenAI"
    }

    fn is_free(&self) -> bool {
        false
    }

    fn requires_api_key(&self) -> bool {
        true
    }

    async fn translate(&self, requests: &[TranslationRequest]) -> Result<Vec<TranslationResult>> {
        if requests.is_empty() {
            return Ok(Vec::new());
        }

        let system_prompt = build_system_prompt(&requests[0]);
        let sources: Vec<&str> = requests.iter().map(|r| r.source.as_str()).collect();
        let user_content = serde_json::to_string(&sources)
            .map_err(|e| LocustError::ProviderError(e.to_string()))?;

        let body = ChatRequest {
            model: self.model.clone(),
            messages: vec![
                ChatMessage {
                    role: "system".to_string(),
                    content: system_prompt,
                },
                ChatMessage {
                    role: "user".to_string(),
                    content: user_content,
                },
            ],
        };

        let resp = self
            .client
            .post(format!("{}/v1/chat/completions", self.base_url))
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&body)
            .send()
            .await
            .map_err(|e| LocustError::ProviderError(format!("OpenAI connection failed: {}", e)))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body_text = resp.text().await.unwrap_or_default();
            return Err(LocustError::ProviderError(format!(
                "OpenAI returned status {}: {}",
                status, body_text
            )));
        }

        let chat_resp: ChatResponse = resp.json().await.map_err(|e| {
            LocustError::ProviderError(format!("OpenAI malformed response: {}", e))
        })?;

        let content = chat_resp
            .choices
            .first()
            .map(|c| c.message.content.as_str())
            .unwrap_or("[]");

        let translations = parse_json_array(content)
            .map_err(|e| LocustError::ProviderError(e))?;

        let usage = chat_resp.usage.as_ref();
        let tokens_used = usage.map(|u| u.total_tokens);
        let cost_usd = usage.map(|u| {
            (u.prompt_tokens as f64 * 0.00015 + u.completion_tokens as f64 * 0.0006) / 1000.0
        });

        Ok(requests
            .iter()
            .zip(translations.iter())
            .map(|(req, trans)| TranslationResult {
                entry_id: req.entry_id.clone(),
                translation: trans.clone(),
                detected_source_lang: None,
                provider: "openai".to_string(),
                tokens_used,
                cost_usd,
            })
            .collect())
    }

    async fn estimate_cost(&self, char_count: usize, _target_lang: &str) -> Option<f64> {
        let input_tokens = char_count as f64 * 1.3;
        let output_tokens = char_count as f64 * 1.3;
        Some((input_tokens * 0.00015 + output_tokens * 0.0006) / 1000.0)
    }

    async fn health_check(&self) -> Result<()> {
        let resp = self
            .client
            .get(format!("{}/v1/models", self.base_url))
            .header("Authorization", format!("Bearer {}", self.api_key))
            .send()
            .await
            .map_err(|e| LocustError::ProviderError(format!("OpenAI health check failed: {}", e)))?;

        if !resp.status().is_success() {
            return Err(LocustError::ProviderError(format!(
                "OpenAI health check returned status {}",
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

    fn make_provider(server: &MockServer) -> OpenAiProvider {
        OpenAiProvider::new(
            "test-key".to_string(),
            None,
            Some(server.base_url()),
        )
    }

    fn make_request(ctx: Option<&str>, hint: Option<&str>) -> TranslationRequest {
        TranslationRequest {
            entry_id: "e1".to_string(),
            source: "Hello".to_string(),
            source_lang: "ja".to_string(),
            target_lang: "en".to_string(),
            context: ctx.map(|s| s.to_string()),
            glossary_hint: hint.map(|s| s.to_string()),
        }
    }

    #[tokio::test]
    async fn test_openai_translate_returns_array() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(POST).path("/v1/chat/completions");
            then.status(200).json_body(serde_json::json!({
                "choices": [{"message": {"content": "[\"Hola\",\"Mundo\"]"}}],
                "usage": {"prompt_tokens": 50, "completion_tokens": 10, "total_tokens": 60}
            }));
        });

        let provider = make_provider(&server);
        let requests = vec![
            make_request(None, None),
            TranslationRequest {
                entry_id: "e2".to_string(),
                source: "World".to_string(),
                source_lang: "ja".to_string(),
                target_lang: "en".to_string(),
                context: None,
                glossary_hint: None,
            },
        ];

        let results = provider.translate(&requests).await.unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].entry_id, "e1");
        assert_eq!(results[0].translation, "Hola");
        assert_eq!(results[1].translation, "Mundo");
        assert_eq!(results[0].tokens_used, Some(60));
    }

    #[tokio::test]
    async fn test_openai_system_prompt_includes_context() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(POST)
                .path("/v1/chat/completions")
                .body_contains("RPG battle system");
            then.status(200).json_body(serde_json::json!({
                "choices": [{"message": {"content": "[\"Hola\"]"}}],
                "usage": {"prompt_tokens": 50, "completion_tokens": 10, "total_tokens": 60}
            }));
        });

        let provider = make_provider(&server);
        let requests = vec![make_request(Some("RPG battle system"), None)];
        provider.translate(&requests).await.unwrap();
        mock.assert();
    }

    #[tokio::test]
    async fn test_openai_glossary_in_prompt() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(POST)
                .path("/v1/chat/completions")
                .body_contains("HP = Health Points");
            then.status(200).json_body(serde_json::json!({
                "choices": [{"message": {"content": "[\"Hola\"]"}}],
                "usage": {"prompt_tokens": 50, "completion_tokens": 10, "total_tokens": 60}
            }));
        });

        let provider = make_provider(&server);
        let requests = vec![make_request(None, Some("HP = Health Points"))];
        provider.translate(&requests).await.unwrap();
        mock.assert();
    }

    #[tokio::test]
    async fn test_openai_lenient_parse() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(POST).path("/v1/chat/completions");
            then.status(200).json_body(serde_json::json!({
                "choices": [{"message": {"content": "```json\n[\"Hola\"]\n```"}}],
                "usage": {"prompt_tokens": 50, "completion_tokens": 10, "total_tokens": 60}
            }));
        });

        let provider = make_provider(&server);
        let requests = vec![make_request(None, None)];
        let results = provider.translate(&requests).await.unwrap();
        assert_eq!(results[0].translation, "Hola");
    }

    #[tokio::test]
    async fn test_openai_cost_estimate_nonzero() {
        let provider = OpenAiProvider::new("key".to_string(), None, None);
        let cost = provider.estimate_cost(1000, "en").await;
        assert!(cost.is_some());
        assert!(cost.unwrap() > 0.0);
    }

    #[tokio::test]
    async fn test_openai_health_check_ok() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET).path("/v1/models");
            then.status(200).json_body(serde_json::json!({"data": []}));
        });

        let provider = make_provider(&server);
        assert!(provider.health_check().await.is_ok());
    }

    #[tokio::test]
    async fn test_openai_health_check_fails() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET).path("/v1/models");
            then.status(401).body("Unauthorized");
        });

        let provider = make_provider(&server);
        assert!(provider.health_check().await.is_err());
    }
}
