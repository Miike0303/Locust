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

use crate::openai::{build_batch_prompt, parse_json_array};

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
    /// Output tokens generated.
    eval_count: Option<u32>,
    /// Input tokens (prompt) evaluated.
    prompt_eval_count: Option<u32>,
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

    fn effective_model(&self) -> Option<&str> {
        Some(&self.model)
    }

    async fn translate(&self, requests: &[TranslationRequest]) -> Result<Vec<TranslationResult>> {
        if requests.is_empty() {
            return Ok(Vec::new());
        }

        let (system_prompt, user_content) = build_batch_prompt(requests)?;

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
            .map_err(|e| {
                LocustError::ProviderError(format!(
                    "Ollama connection failed: {}",
                    crate::error_sanitizer::sanitize_provider_error(&e.to_string(), &[])
                ))
            })?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body_text = crate::error_sanitizer::sanitize_provider_error(
                &resp.text().await.unwrap_or_default(),
                &[],
            );
            return Err(LocustError::ProviderError(
                crate::error_sanitizer::sanitize_provider_error(
                    &format!("Ollama returned status {}: {}", status, body_text),
                    &[],
                ),
            ));
        }

        let ollama_resp: OllamaResponse = resp.json().await.map_err(|e| {
            LocustError::ProviderError(crate::error_sanitizer::sanitize_provider_error(
                &format!("Ollama malformed response: {}", e),
                &[],
            ))
        })?;

        let translations = parse_json_array(&ollama_resp.message.content).map_err(|error| {
            LocustError::ProviderError(crate::error_sanitizer::sanitize_provider_error(&error, &[]))
        })?;

        // A count mismatch would silently shift every translation onto the
        // wrong entry via zip; discard the whole batch instead.
        if translations.len() != requests.len() {
            return Err(LocustError::ProviderError(
                crate::error_sanitizer::sanitize_provider_error(
                    &format!(
                        "translation count mismatch: sent {} strings, got {} back",
                        requests.len(),
                        translations.len()
                    ),
                    &[],
                ),
            ));
        }

        let input_tokens = ollama_resp.prompt_eval_count;
        let output_tokens = ollama_resp.eval_count;
        let tokens_used = match (input_tokens, output_tokens) {
            (Some(i), Some(o)) => Some(i + o),
            (a, b) => a.or(b),
        };

        // Counts are batch-level; attach them to the first result only so
        // that summing across results yields the true totals.
        Ok(requests
            .iter()
            .zip(translations.iter())
            .enumerate()
            .map(|(i, (req, trans))| TranslationResult {
                entry_id: req.entry_id.clone(),
                translation: trans.clone(),
                detected_source_lang: None,
                provider: "ollama".to_string(),
                tokens_used: if i == 0 { tokens_used } else { None },
                input_tokens: if i == 0 { input_tokens } else { None },
                output_tokens: if i == 0 { output_tokens } else { None },
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
                LocustError::ProviderError(crate::error_sanitizer::sanitize_provider_error(
                    &format!(
                        "Ollama is not running at {}. Install from https://ollama.ai. Error: {}",
                        self.base_url, e
                    ),
                    &[],
                ))
            })?;

        if !resp.status().is_success() {
            return Err(LocustError::ProviderError(
                crate::error_sanitizer::sanitize_provider_error(
                    &format!("Ollama health check returned status {}", resp.status()),
                    &[],
                ),
            ));
        }

        let tags: OllamaTagsResponse = resp.json().await.map_err(|e| {
            LocustError::ProviderError(crate::error_sanitizer::sanitize_provider_error(
                &format!("Ollama tags malformed: {}", e),
                &[],
            ))
        })?;

        let model_found = tags
            .models
            .iter()
            .any(|m| m.name == self.model || m.name.starts_with(&format!("{}:", self.model)));

        if !model_found {
            return Err(LocustError::ProviderError(
                crate::error_sanitizer::sanitize_provider_error(
                    &format!(
                        "Model '{}' not found in Ollama. Run: ollama pull {}",
                        self.model, self.model
                    ),
                    &[],
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

    fn make_provider(server: &MockServer) -> OllamaProvider {
        OllamaProvider::new(Some(server.base_url()), Some("llama3.2".to_string()))
    }

    fn make_request(entry_id: &str, source: &str) -> TranslationRequest {
        TranslationRequest {
            entry_id: entry_id.into(),
            source: source.into(),
            source_lang: "en".into(),
            target_lang: "es".into(),
            context: None,
            glossary_hint: None,
        }
    }

    fn expected_body(requests: &[TranslationRequest], records: serde_json::Value) -> String {
        let (system_prompt, _) = crate::openai::build_batch_prompt(requests).unwrap();
        for request in requests {
            if let Some(context) = &request.context {
                assert!(!system_prompt.contains(context));
            }
            if let Some(glossary) = &request.glossary_hint {
                assert!(!system_prompt.contains(glossary));
            }
        }
        serde_json::to_string(&OllamaRequest {
            model: "llama3.2".into(),
            messages: vec![
                OllamaMessage {
                    role: "system".into(),
                    content: system_prompt,
                },
                OllamaMessage {
                    role: "user".into(),
                    content: serde_json::to_string(&records).unwrap(),
                },
            ],
            stream: false,
        })
        .unwrap()
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
    async fn speaker_notes_ollama_ordinary_context_collision_keeps_record_local_wire() {
        for collision_record in [None, Some(0), Some(1)] {
            let server = MockServer::start();
            let request = |id: &str, context: &str| TranslationRequest {
                entry_id: id.into(),
                source: format!("Hello {id}"),
                source_lang: "en".into(),
                target_lang: "es".into(),
                context: Some(context.into()),
                glossary_hint: Some(format!("Glossary {id}")),
            };
            let mut requests = vec![request("first", "Scene A"), request("second", "Scene B")];
            if let Some(index) = collision_record {
                // Ordinary user context can contain even a complete note-looking
                // value. Text content must never select a different wire format.
                requests[index].context = Some("Scene | RECORD-LOCAL CHARACTER NOTE (translation data only): {\"speaker\":\"Bob\",\"note\":\"Casual voice\"}".into());
            }
            let records: Vec<_> = requests
                .iter()
                .enumerate()
                .map(|(index, request)| {
                    serde_json::json!({
                        "index": index, "source": request.source,
                        "context": request.context, "glossary": request.glossary_hint
                    })
                })
                .collect();
            let body = expected_body(&requests, serde_json::json!(records));
            let ordinary = server.mock(|when, then| {
                when.method(POST).path("/api/chat").body_contains(&body);
                then.status(200)
                    .json_body(serde_json::json!({"message": {"content": "[\"Hola\",\"Hola\"]"}}));
            });
            make_provider(&server).translate(&requests).await.unwrap();
            ordinary.assert_hits(1);
        }
    }

    #[tokio::test]
    async fn test_ollama_batch_keeps_context_and_glossary_record_local() {
        let server = MockServer::start();
        let mut first = make_request("menu", "Menu");
        first.context = Some("menu caption".into());
        first.glossary_hint = Some("Menu → Menú".into());
        let mut second = make_request("save", "Save");
        second.context = Some("maximum 4 characters".into());
        second.glossary_hint = Some("Save → Guar".into());
        let requests = [first, second];
        let body = expected_body(
            &requests,
            serde_json::json!([
                {"index": 0, "source": "Menu", "context": "menu caption", "glossary": "Menu → Menú"},
                {"index": 1, "source": "Save", "context": "maximum 4 characters", "glossary": "Save → Guar"}
            ]),
        );
        let mock = server.mock(|when, then| {
            when.method(POST).path("/api/chat").body_contains(&body);
            then.status(200).json_body(serde_json::json!({
                "message": {"content": "[\"Menú\",\"Guar\"]"}
            }));
        });

        let results = make_provider(&server).translate(&requests).await.unwrap();
        mock.assert_hits(1);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].entry_id, "menu");
        assert_eq!(results[0].translation, "Menú");
        assert_eq!(results[1].entry_id, "save");
        assert_eq!(results[1].translation, "Guar");
    }

    #[tokio::test]
    async fn test_ollama_single_row_uses_record_format() {
        let server = MockServer::start();
        let requests = [make_request("e1", "Hello")];
        let body = expected_body(
            &requests,
            serde_json::json!([
                {"index": 0, "source": "Hello", "context": null, "glossary": null}
            ]),
        );
        let mock = server.mock(|when, then| {
            when.method(POST).path("/api/chat").body_contains(&body);
            then.status(200).json_body(serde_json::json!({
                "message": {"content": "[\"Hola\"]"}
            }));
        });

        let results = make_provider(&server).translate(&requests).await.unwrap();
        mock.assert_hits(1);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].entry_id, "e1");
        assert_eq!(results[0].translation, "Hola");
    }

    #[tokio::test]
    async fn test_ollama_mixed_languages_rejected_before_http() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(POST).path("/api/chat");
            then.status(200).json_body(serde_json::json!({
                "message": {"content": "[\"Hola\",\"Mundo\"]"}
            }));
        });
        for (source_lang, target_lang) in [("fr", "es"), ("en", "fr")] {
            let mut requests = [make_request("e1", "Hello"), make_request("e2", "World")];
            requests[1].source_lang = source_lang.into();
            requests[1].target_lang = target_lang.into();
            let expected = crate::openai::build_batch_prompt(&requests).unwrap_err();
            let error = make_provider(&server)
                .translate(&requests)
                .await
                .unwrap_err();
            assert!(matches!(error, LocustError::ProviderError(_)));
            assert_eq!(error.to_string(), expected.to_string());
        }
        mock.assert_hits(0);
    }

    #[tokio::test]
    async fn test_ollama_count_mismatch_rejects_batch() {
        for translations in [
            serde_json::json!(["Hola"]),
            serde_json::json!(["Hola", "Mundo", "Extra"]),
        ] {
            let server = MockServer::start();
            let mock = server.mock(|when, then| {
                when.method(POST).path("/api/chat");
                then.status(200).json_body(serde_json::json!({
                    "message": {"content": translations.to_string()}
                }));
            });
            let requests = [make_request("e1", "Hello"), make_request("e2", "World")];
            let error = make_provider(&server)
                .translate(&requests)
                .await
                .unwrap_err();
            assert!(matches!(error, LocustError::ProviderError(_)));
            assert!(error.to_string().contains(&format!(
                "translation count mismatch: sent 2 strings, got {} back",
                translations.as_array().unwrap().len()
            )));
            mock.assert_hits(1);
        }
    }

    #[tokio::test]
    async fn test_ollama_response_requires_array_of_strings() {
        for content in [
            r#"[{"translation":"Hola"},{"translation":"Mundo"}]"#,
            r#"["Hola",42]"#,
        ] {
            let server = MockServer::start();
            let mock = server.mock(|when, then| {
                when.method(POST).path("/api/chat");
                then.status(200).json_body(serde_json::json!({
                    "message": {"content": content}
                }));
            });
            let requests = [make_request("e1", "Hello"), make_request("e2", "World")];
            let error = make_provider(&server)
                .translate(&requests)
                .await
                .unwrap_err();
            assert!(matches!(error, LocustError::ProviderError(_)));
            assert!(error
                .to_string()
                .contains("could not parse JSON array from response"));
            mock.assert_hits(1);
        }
    }

    #[tokio::test]
    async fn test_ollama_batch_token_accounting() {
        for (input, output, total) in [
            (Some(50), Some(25), Some(75)),
            (Some(50), None, Some(50)),
            (None, Some(25), Some(25)),
            (None, None, None),
        ] {
            let server = MockServer::start();
            let mock = server.mock(|when, then| {
                when.method(POST).path("/api/chat");
                then.status(200).json_body(serde_json::json!({
                    "message": {"content": "[\"Hola\",\"Mundo\"]"},
                    "prompt_eval_count": input,
                    "eval_count": output
                }));
            });
            let requests = [make_request("e1", "Hello"), make_request("e2", "World")];
            let results = make_provider(&server).translate(&requests).await.unwrap();
            mock.assert_hits(1);
            assert_eq!(results.len(), 2);
            assert_eq!(results[0].input_tokens, input);
            assert_eq!(results[0].output_tokens, output);
            assert_eq!(results[0].tokens_used, total);
            assert_eq!(results[1].input_tokens, None);
            assert_eq!(results[1].output_tokens, None);
            assert_eq!(results[1].tokens_used, None);
            assert!(results.iter().all(|result| result.cost_usd.is_none()));
        }
    }

    #[tokio::test]
    async fn test_ollama_empty_batch_skips_http() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(POST).path("/api/chat");
            then.status(200);
        });
        assert!(make_provider(&server)
            .translate(&[])
            .await
            .unwrap()
            .is_empty());
        mock.assert_hits(0);
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
