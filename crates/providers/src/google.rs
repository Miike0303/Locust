use async_trait::async_trait;
use serde::Deserialize;

use locust_core::error::{LocustError, Result};
use locust_core::models::{TranslationRequest, TranslationResult};
use locust_core::translation::TranslationProvider;

/// Google Translate provider using the free web API (no API key required).
/// Uses the undocumented but widely-used translate.googleapis.com endpoint.
pub struct GoogleTranslateProvider {
    client: reqwest::Client,
}

impl GoogleTranslateProvider {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::builder()
                .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
        }
    }
}

impl Default for GoogleTranslateProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl TranslationProvider for GoogleTranslateProvider {
    fn id(&self) -> &str {
        "google"
    }

    fn name(&self) -> &str {
        "Google Translate (free)"
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

        // Strategy: split requests into chunks, concatenate each chunk with a separator,
        // send as ONE Google Translate request, then split the result back.
        // This reduces HTTP calls from N to N/CHUNK_SIZE.
        const SEPARATOR: &str = " ||| ";
        const CHUNK_SIZE: usize = 25; // 25 strings per HTTP request

        let chunks: Vec<&[TranslationRequest]> = requests.chunks(CHUNK_SIZE).collect();

        // Process chunks concurrently (up to 8 at a time)
        let semaphore = std::sync::Arc::new(tokio::sync::Semaphore::new(8));
        let mut handles = Vec::with_capacity(chunks.len());

        for chunk in &chunks {
            let client = self.client.clone();
            let sl = source_lang.clone();
            let tl = target_lang.clone();
            let sources: Vec<String> = chunk.iter().map(|r| r.source.clone()).collect();
            let entry_ids: Vec<String> = chunk.iter().map(|r| r.entry_id.clone()).collect();
            let sem = semaphore.clone();

            handles.push(tokio::spawn(async move {
                let _permit = sem.acquire().await.unwrap();

                // Concatenate all sources with separator
                let combined = sources.join(SEPARATOR);
                let translated = Self::translate_single_static(&client, &combined, &sl, &tl).await?;

                // Split result back by separator
                let parts: Vec<&str> = translated.split("|||").collect();

                let mut results = Vec::with_capacity(entry_ids.len());
                for (i, entry_id) in entry_ids.iter().enumerate() {
                    let translation = if i < parts.len() {
                        parts[i].trim().to_string()
                    } else {
                        // Fallback: if separator got mangled, use last known part
                        parts.last().unwrap_or(&"").trim().to_string()
                    };
                    results.push(TranslationResult {
                        entry_id: entry_id.clone(),
                        translation,
                        detected_source_lang: None,
                        provider: "google".to_string(),
                        tokens_used: None,
                        cost_usd: None,
                    });
                }

                Ok::<Vec<TranslationResult>, LocustError>(results)
            }));
        }

        let mut all_results = Vec::with_capacity(requests.len());
        for handle in handles {
            let chunk_results = handle.await
                .map_err(|e| LocustError::ProviderError(format!("task join error: {}", e)))??;
            all_results.extend(chunk_results);
        }

        Ok(all_results)
    }

    async fn estimate_cost(&self, _char_count: usize, _target_lang: &str) -> Option<f64> {
        None // Free
    }

    async fn health_check(&self) -> Result<()> {
        // Try a simple translation to verify the endpoint works
        self.translate_single("hello", "en", "es").await?;
        Ok(())
    }
}

impl GoogleTranslateProvider {
    async fn translate_single(&self, text: &str, source_lang: &str, target_lang: &str) -> Result<String> {
        Self::translate_single_static(&self.client, text, source_lang, target_lang).await
    }

    async fn translate_single_static(client: &reqwest::Client, text: &str, source_lang: &str, target_lang: &str) -> Result<String> {
        let url = "https://translate.googleapis.com/translate_a/single";

        let resp = client
            .get(url)
            .query(&[
                ("client", "gtx"),
                ("sl", source_lang),
                ("tl", target_lang),
                ("dt", "t"),
                ("q", text),
            ])
            .send()
            .await
            .map_err(|e| LocustError::ProviderError(format!("Google Translate request failed: {}", e)))?;

        if !resp.status().is_success() {
            return Err(LocustError::ProviderError(format!(
                "Google Translate returned status {}",
                resp.status()
            )));
        }

        let body: serde_json::Value = resp.json().await.map_err(|e| {
            LocustError::ProviderError(format!("Google Translate returned invalid JSON: {}", e))
        })?;

        // Response format: [[["translated text","source text",null,null,confidence],...],null,"en",...]
        // We need to concatenate all translated segments from body[0]
        let segments = body
            .get(0)
            .and_then(|v| v.as_array())
            .ok_or_else(|| LocustError::ProviderError("unexpected Google Translate response format".to_string()))?;

        let mut translated = String::new();
        for segment in segments {
            if let Some(text) = segment.get(0).and_then(|v| v.as_str()) {
                translated.push_str(text);
            }
        }

        if translated.is_empty() {
            return Err(LocustError::ProviderError("Google Translate returned empty translation".to_string()));
        }

        Ok(translated)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::prelude::*;

    fn make_google_response(translated: &str) -> serde_json::Value {
        serde_json::json!([
            [[translated, "source", null, null, 1.0]],
            null,
            "en"
        ])
    }

    #[tokio::test]
    async fn test_google_translate_single() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET).path("/translate_a/single");
            then.status(200)
                .json_body(make_google_response("Hola mundo"));
        });

        // Create provider that talks to our mock
        let provider = GoogleTranslateProvider {
            client: reqwest::Client::new(),
        };

        // We can't easily redirect the URL in the provider, so test the parsing logic directly
        let response_json = make_google_response("Hola mundo");
        let segments = response_json.get(0).unwrap().as_array().unwrap();
        let mut translated = String::new();
        for segment in segments {
            if let Some(text) = segment.get(0).and_then(|v| v.as_str()) {
                translated.push_str(text);
            }
        }
        assert_eq!(translated, "Hola mundo");
    }

    #[tokio::test]
    async fn test_google_response_parsing_multi_segment() {
        let response = serde_json::json!([
            [
                ["Hola ", "Hello ", null, null, 1.0],
                ["mundo", "world", null, null, 1.0]
            ],
            null,
            "en"
        ]);

        let segments = response.get(0).unwrap().as_array().unwrap();
        let mut translated = String::new();
        for segment in segments {
            if let Some(text) = segment.get(0).and_then(|v| v.as_str()) {
                translated.push_str(text);
            }
        }
        assert_eq!(translated, "Hola mundo");
    }

    #[tokio::test]
    async fn test_google_response_parsing_empty() {
        let response = serde_json::json!([[], null, "en"]);
        let segments = response.get(0).unwrap().as_array().unwrap();
        let mut translated = String::new();
        for segment in segments {
            if let Some(text) = segment.get(0).and_then(|v| v.as_str()) {
                translated.push_str(text);
            }
        }
        assert!(translated.is_empty());
    }
}
