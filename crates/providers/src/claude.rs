use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use locust_core::error::{LocustError, Result};
use locust_core::models::{TranslationRequest, TranslationResult};
use locust_core::translation::TranslationProvider;

pub struct ClaudeProvider {
    api_key: String,
    model: String,
    base_url: String,
    client: reqwest::Client,
}

impl ClaudeProvider {
    pub fn new(api_key: String, model: Option<String>, base_url: Option<String>) -> Self {
        Self {
            api_key,
            model: model.unwrap_or_else(|| "claude-haiku-4-5-20251001".to_string()),
            base_url: base_url.unwrap_or_else(|| "https://api.anthropic.com".to_string()),
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
struct ClaudeRequest {
    model: String,
    max_tokens: u32,
    system: String,
    messages: Vec<ClaudeMessage>,
}

#[derive(Serialize)]
struct ClaudeMessage {
    role: String,
    content: String,
}

#[derive(Deserialize)]
struct ClaudeResponse {
    content: Vec<ClaudeContent>,
    usage: Option<ClaudeUsage>,
}

#[derive(Deserialize)]
struct ClaudeContent {
    text: String,
}

#[derive(Deserialize)]
struct ClaudeUsage {
    input_tokens: u32,
    output_tokens: u32,
}

#[async_trait]
impl TranslationProvider for ClaudeProvider {
    fn id(&self) -> &str {
        "claude"
    }

    fn name(&self) -> &str {
        "Claude (Anthropic)"
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

        let body = ClaudeRequest {
            model: self.model.clone(),
            max_tokens: 4096,
            system: system_prompt,
            messages: vec![ClaudeMessage {
                role: "user".to_string(),
                content: user_content,
            }],
        };

        let resp = self
            .client
            .post(format!("{}/v1/messages", self.base_url))
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| LocustError::ProviderError(format!("Claude connection failed: {}", e)))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body_text = resp.text().await.unwrap_or_default();
            return Err(LocustError::ProviderError(format!(
                "Claude returned status {}: {}",
                status, body_text
            )));
        }

        let claude_resp: ClaudeResponse = resp.json().await.map_err(|e| {
            LocustError::ProviderError(format!("Claude malformed response: {}", e))
        })?;

        let content = claude_resp
            .content
            .first()
            .map(|c| c.text.as_str())
            .unwrap_or("[]");

        let translations =
            parse_json_array(content).map_err(|e| LocustError::ProviderError(e))?;

        let usage = claude_resp.usage.as_ref();
        let tokens_used = usage.map(|u| u.input_tokens + u.output_tokens);
        let cost_usd = usage.map(|u| {
            (u.input_tokens as f64 * 0.00025 + u.output_tokens as f64 * 0.00125) / 1000.0
        });

        Ok(requests
            .iter()
            .zip(translations.iter())
            .map(|(req, trans)| TranslationResult {
                entry_id: req.entry_id.clone(),
                translation: trans.clone(),
                detected_source_lang: None,
                provider: "claude".to_string(),
                tokens_used,
                cost_usd,
            })
            .collect())
    }

    async fn estimate_cost(&self, char_count: usize, _target_lang: &str) -> Option<f64> {
        let input_tokens = char_count as f64 * 1.3;
        let output_tokens = char_count as f64 * 1.3;
        Some((input_tokens * 0.00025 + output_tokens * 0.00125) / 1000.0)
    }

    async fn health_check(&self) -> Result<()> {
        let body = ClaudeRequest {
            model: self.model.clone(),
            max_tokens: 1,
            system: String::new(),
            messages: vec![ClaudeMessage {
                role: "user".to_string(),
                content: "ping".to_string(),
            }],
        };

        let resp = self
            .client
            .post(format!("{}/v1/messages", self.base_url))
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                LocustError::ProviderError(format!("Claude health check failed: {}", e))
            })?;

        if !resp.status().is_success() {
            return Err(LocustError::ProviderError(format!(
                "Claude health check returned status {}",
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

    fn make_provider(server: &MockServer) -> ClaudeProvider {
        ClaudeProvider::new(
            "test-key".to_string(),
            None,
            Some(server.base_url()),
        )
    }

    #[tokio::test]
    async fn test_claude_translate_success() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(POST).path("/v1/messages");
            then.status(200).json_body(serde_json::json!({
                "content": [{"text": "[\"Hola\",\"Mundo\"]"}],
                "usage": {"input_tokens": 40, "output_tokens": 10}
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
        assert_eq!(results[0].provider, "claude");
        assert_eq!(results[0].tokens_used, Some(50));
    }

    #[tokio::test]
    async fn test_claude_correct_headers() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(POST)
                .path("/v1/messages")
                .header("x-api-key", "test-key")
                .header("anthropic-version", "2023-06-01");
            then.status(200).json_body(serde_json::json!({
                "content": [{"text": "[\"Hola\"]"}],
                "usage": {"input_tokens": 10, "output_tokens": 5}
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
    async fn test_claude_health_check_ok() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(POST).path("/v1/messages");
            then.status(200).json_body(serde_json::json!({
                "content": [{"text": "pong"}],
                "usage": {"input_tokens": 1, "output_tokens": 1}
            }));
        });

        let provider = make_provider(&server);
        assert!(provider.health_check().await.is_ok());
    }

    #[tokio::test]
    async fn test_claude_cost_estimate() {
        let provider = ClaudeProvider::new("key".to_string(), None, None);
        let cost = provider.estimate_cost(1000, "en").await;
        assert!(cost.is_some());
        let c = cost.unwrap();
        // 1300 input tokens * 0.00025/1k + 1300 output tokens * 0.00125/1k
        let expected = (1300.0 * 0.00025 + 1300.0 * 0.00125) / 1000.0;
        assert!((c - expected).abs() < 0.0001, "cost={}, expected={}", c, expected);
    }
}
