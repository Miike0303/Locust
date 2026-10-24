use async_trait::async_trait;

use locust_core::error::{LocustError, Result};
use locust_core::models::{TranslationRequest, TranslationResult};
use locust_core::translation::TranslationProvider;

/// Google Translate provider using the free web API (no API key required).
/// Uses the undocumented but widely-used translate.googleapis.com endpoint.
pub struct GoogleTranslateProvider {
    client: reqwest::Client,
    base_url: String,
}

impl GoogleTranslateProvider {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::builder()
                .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
            base_url: "https://translate.googleapis.com".to_string(),
        }
    }

    fn chunks(requests: &[TranslationRequest]) -> Vec<&[TranslationRequest]> {
        const MAX_SOURCES: usize = 50;
        const MAX_CHARS: usize = 4500;

        let mut chunks = Vec::new();
        let mut start = 0;
        let mut chars = 0;
        for (i, request) in requests.iter().enumerate() {
            let source_chars = request.source.chars().count();
            if i > start
                && (i - start == MAX_SOURCES
                    || chars + source_chars > MAX_CHARS
                    || request.source_lang != requests[start].source_lang
                    || request.target_lang != requests[start].target_lang)
            {
                chunks.push(&requests[start..i]);
                start = i;
                chars = 0;
            }
            // A source over the character budget must travel alone, without truncation.
            chars += source_chars;
        }
        if start < requests.len() {
            chunks.push(&requests[start..]);
        }
        chunks
    }

    async fn translate_chunk(
        client: &reqwest::Client,
        base_url: &str,
        requests: &[TranslationRequest],
    ) -> Result<Vec<TranslationResult>> {
        let url = format!("{base_url}/translate_a/t");
        let mut results = Vec::with_capacity(requests.len());
        let mut pending = vec![requests];

        // Traverse the halving retry tree in input order. Each split strictly reduces
        // the chunk size, so malformed responses eventually fail at a single source.
        while let Some(chunk) = pending.pop() {
            let fields: Vec<_> = chunk.iter().map(|r| ("q", r.source.as_str())).collect();
            let resp = client
                .post(&url)
                .query(&[
                    ("client", "gtx"),
                    ("sl", chunk[0].source_lang.as_str()),
                    ("tl", chunk[0].target_lang.as_str()),
                    ("dj", "1"),
                ])
                .form(&fields)
                .send()
                .await
                .map_err(|e| {
                    LocustError::ProviderError(crate::error_sanitizer::sanitize_provider_error(
                        &format!("Google Translate request failed: {}", e),
                        &[],
                    ))
                })?;

            // Let the engine retry HTTP failures according to their status. Splitting
            // these would multiply requests during rate limits or upstream outages.
            if !resp.status().is_success() {
                return Err(LocustError::ProviderError(
                    crate::error_sanitizer::sanitize_provider_error(
                        &format!("Google Translate returned status {}", resp.status()),
                        &[],
                    ),
                ));
            }

            let parsed = resp
                .json::<serde_json::Value>()
                .await
                .map_err(|e| {
                    LocustError::ProviderError(crate::error_sanitizer::sanitize_provider_error(
                        &format!("Google Translate returned invalid JSON: {}", e),
                        &[],
                    ))
                })
                .and_then(|body| Self::parse_response(&body, chunk));

            match parsed {
                Ok(translations) => results.extend(translations),
                Err(_) if chunk.len() > 1 => {
                    let (left, right) = chunk.split_at(chunk.len() / 2);
                    pending.push(right);
                    pending.push(left);
                }
                Err(error) => return Err(error),
            }
        }
        Ok(results)
    }

    fn parse_response(
        body: &serde_json::Value,
        requests: &[TranslationRequest],
    ) -> Result<Vec<TranslationResult>> {
        let items = body.as_array().ok_or_else(|| {
            LocustError::ProviderError("Google Translate returned a non-array response".to_string())
        })?;
        if items.len() != requests.len() {
            return Err(LocustError::ProviderError(
                crate::error_sanitizer::sanitize_provider_error(
                    &format!(
                        "Google Translate returned {} items for {} sources",
                        items.len(),
                        requests.len()
                    ),
                    &[],
                ),
            ));
        }

        items
            .iter()
            .zip(requests)
            .enumerate()
            .map(|(i, (item, request))| {
                let (text, detected_source_lang) = match item {
                    serde_json::Value::String(text) => (Some(text.as_str()), None),
                    serde_json::Value::Array(parts) => (
                        parts.first().and_then(|v| v.as_str()),
                        parts.get(1).and_then(|v| v.as_str()).map(str::to_string),
                    ),
                    _ => (None, None),
                };
                let text = text.ok_or_else(|| {
                    LocustError::ProviderError(crate::error_sanitizer::sanitize_provider_error(
                        &format!(
                            "Google Translate returned an invalid translation item at index {}",
                            i
                        ),
                        &[],
                    ))
                })?;
                if text.is_empty() && !request.source.is_empty() {
                    return Err(LocustError::ProviderError(
                        "Google Translate returned empty translation".to_string(),
                    ));
                }
                Ok(TranslationResult {
                    entry_id: request.entry_id.clone(),
                    translation: text.to_string(),
                    detected_source_lang,
                    provider: "google".to_string(),
                    tokens_used: None,
                    input_tokens: None,
                    output_tokens: None,
                    cost_usd: None,
                })
            })
            .collect()
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
        const CONCURRENCY: usize = 8;

        let chunks = Self::chunks(requests);
        let mut results = vec![Vec::new(); chunks.len()];
        let mut chunks = chunks.into_iter().enumerate();
        // Dropping the set on failure or cancellation aborts the remaining tasks.
        let mut tasks = tokio::task::JoinSet::new();
        loop {
            while tasks.len() < CONCURRENCY {
                let Some((index, chunk)) = chunks.next() else {
                    break;
                };
                let client = self.client.clone();
                let base_url = self.base_url.clone();
                let chunk = chunk.to_vec();
                tasks.spawn(async move {
                    let translations = Self::translate_chunk(&client, &base_url, &chunk).await;
                    (index, translations)
                });
            }
            let Some(task) = tasks.join_next().await else {
                break;
            };
            let (index, translations) = task.map_err(|e| {
                LocustError::ProviderError(crate::error_sanitizer::sanitize_provider_error(
                    &format!("task join error: {}", e),
                    &[],
                ))
            })?;
            results[index] = translations?;
        }
        Ok(results.into_iter().flatten().collect())
    }

    async fn estimate_cost(&self, _char_count: usize, _target_lang: &str) -> Option<f64> {
        None // Free
    }

    async fn health_check(&self) -> Result<()> {
        self.translate(&[TranslationRequest {
            entry_id: "health-check".to_string(),
            source: "hello".to_string(),
            source_lang: "en".to_string(),
            target_lang: "es".to_string(),
            context: None,
            glossary_hint: None,
        }])
        .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::prelude::*;
    use locust_core::translation::is_retryable;
    use serde_json::json;
    use std::time::Duration;

    #[tokio::test]
    async fn google_transport_and_decode_errors_redact_url_queries() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(POST);
            then.status(200).body("not JSON");
        });
        for base in [format!("http://{address}"), server.base_url()] {
            let provider = GoogleTranslateProvider {
                base_url: format!("{base}/?token=google-query-sentinel"),
                ..GoogleTranslateProvider::new()
            };
            let error = provider
                .translate(&requests(&["Hello"], "en"))
                .await
                .unwrap_err()
                .to_string();
            assert!(!error.contains("google-query-sentinel"), "{error}");
        }
    }

    fn make_provider(server: &MockServer) -> GoogleTranslateProvider {
        GoogleTranslateProvider {
            base_url: server.base_url(),
            ..GoogleTranslateProvider::new()
        }
    }

    fn requests(sources: &[&str], source_lang: &str) -> Vec<TranslationRequest> {
        sources
            .iter()
            .enumerate()
            .map(|(i, source)| TranslationRequest {
                entry_id: format!("entry-{i}"),
                source: (*source).to_string(),
                source_lang: source_lang.to_string(),
                target_lang: "en".to_string(),
                context: None,
                glossary_hint: None,
            })
            .collect()
    }

    fn form_body(requests: &[TranslationRequest]) -> String {
        let fields: Vec<_> = requests.iter().map(|r| ("q", r.source.as_str())).collect();
        let request = reqwest::Client::new()
            .post("http://localhost")
            .form(&fields)
            .build()
            .unwrap();
        String::from_utf8(request.body().unwrap().as_bytes().unwrap().to_vec()).unwrap()
    }

    fn assert_results(
        results: &[TranslationResult],
        requests: &[TranslationRequest],
        texts: &[&str],
    ) {
        assert_eq!(results.len(), requests.len());
        assert_eq!(results.len(), texts.len());
        for ((result, request), text) in results.iter().zip(requests).zip(texts) {
            assert_eq!(result.entry_id, request.entry_id);
            assert_eq!(result.translation, *text);
            assert_eq!(result.provider, "google");
        }
    }

    #[tokio::test]
    async fn post_repeated_q_fields_preserve_order_and_literal_separator() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(POST)
                .path("/translate_a/t")
                .query_param("client", "gtx")
                .query_param("sl", "ja")
                .query_param("tl", "en")
                .query_param("dj", "1")
                .header("content-type", "application/x-www-form-urlencoded")
                .body("q=%E3%81%AF%E3%81%84&q=%E3%81%84%E3%81%84%E3%81%88&q=a%7C%7C%7Cb");
            then.status(200).json_body(json!(["yes", "no", "a|||b"]));
        });
        let requests = requests(&["はい", "いいえ", "a|||b"], "ja");
        let results = make_provider(&server).translate(&requests).await.unwrap();
        assert_results(&results, &requests, &["yes", "no", "a|||b"]);
        assert!(results.iter().all(|r| r.detected_source_lang.is_none()));
        mock.assert();
    }

    #[tokio::test]
    async fn auto_response_preserves_detected_language_per_entry() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(POST)
                .path("/translate_a/t")
                .query_param("sl", "auto")
                .body("q=yes&q=no");
            then.status(200)
                .json_body(json!([["yes", "ja"], ["no", "es"]]));
        });
        let requests = requests(&["yes", "no"], "auto");
        let results = make_provider(&server).translate(&requests).await.unwrap();
        assert_results(&results, &requests, &["yes", "no"]);
        assert_eq!(results[0].detected_source_lang.as_deref(), Some("ja"));
        assert_eq!(results[1].detected_source_lang.as_deref(), Some("es"));
        mock.assert();
    }

    #[tokio::test]
    async fn single_source_accepts_both_response_shapes() {
        for (sl, response, detected) in [
            ("ja", json!(["yes"]), None),
            ("auto", json!([["yes", "ja"]]), Some("ja")),
        ] {
            let server = MockServer::start();
            let mock = server.mock(|when, then| {
                when.method(POST).path("/translate_a/t").body("q=yes");
                then.status(200).json_body(response);
            });
            let requests = requests(&["yes"], sl);
            let results = make_provider(&server).translate(&requests).await.unwrap();
            assert_results(&results, &requests, &["yes"]);
            assert_eq!(results[0].detected_source_lang.as_deref(), detected);
            mock.assert();
        }
    }

    #[tokio::test]
    async fn preserves_form_characters_codes_newlines_and_whitespace() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(POST)
                .path("/translate_a/t")
                .body("q=+%5CC%5B1%5Da%5CC%5B0%5D%0A%26%3D%2B+");
            then.status(200).json_body(json!([" \\C[1]A\\C[0]\n&=+ "]));
        });
        let requests = requests(&[" \\C[1]a\\C[0]\n&=+ "], "ja");
        let results = make_provider(&server).translate(&requests).await.unwrap();
        assert_results(&results, &requests, &[" \\C[1]A\\C[0]\n&=+ "]);
        mock.assert();
    }

    #[tokio::test]
    async fn empty_request_needs_no_http_call() {
        let server = MockServer::start();
        let mock = server.mock(|_when, then| {
            then.status(500);
        });
        assert!(make_provider(&server)
            .translate(&[])
            .await
            .unwrap()
            .is_empty());
        mock.assert_hits(0);
    }

    #[tokio::test]
    async fn wrong_count_halves_recursively_without_misassigning() {
        let server = MockServer::start();
        let parent = server.mock(|when, then| {
            when.method(POST)
                .path("/translate_a/t")
                .body("q=a&q=b&q=c&q=d&q=e");
            then.status(200).json_body(json!(["WRONG"]));
        });
        let left = server.mock(|when, then| {
            when.method(POST).path("/translate_a/t").body("q=a&q=b");
            then.status(200).json_body(json!(["A", "B"]));
        });
        let right = server.mock(|when, then| {
            when.method(POST).path("/translate_a/t").body("q=c&q=d&q=e");
            then.status(200).json_body(json!(["WRONG", "COUNT"]));
        });
        let leaf = server.mock(|when, then| {
            when.method(POST).path("/translate_a/t").body("q=c");
            then.status(200).json_body(json!(["C"]));
        });
        let tail = server.mock(|when, then| {
            when.method(POST).path("/translate_a/t").body("q=d&q=e");
            then.status(200).json_body(json!(["D", "E"]));
        });
        let requests = requests(&["a", "b", "c", "d", "e"], "ja");
        let results = make_provider(&server).translate(&requests).await.unwrap();
        assert_results(&results, &requests, &["A", "B", "C", "D", "E"]);
        for mock in [parent, left, right, leaf, tail] {
            mock.assert();
        }
    }

    #[tokio::test]
    async fn malformed_chunks_retry_each_half() {
        for response in [
            json!({"text": "wrong shape"}),
            json!(["too", "many", "items"]),
            json!(["A", 7]),
            json!(["A", []]),
            json!(["A", [7, "ja"]]),
            json!(["A", ""]),
        ] {
            let server = MockServer::start();
            let parent = server.mock(|when, then| {
                when.method(POST).path("/translate_a/t").body("q=a&q=b");
                then.status(200).json_body(response);
            });
            let left = server.mock(|when, then| {
                when.method(POST).path("/translate_a/t").body("q=a");
                then.status(200).json_body(json!(["A"]));
            });
            let right = server.mock(|when, then| {
                when.method(POST).path("/translate_a/t").body("q=b");
                then.status(200).json_body(json!([["B", "ja"]]));
            });
            let requests = requests(&["a", "b"], "ja");
            let results = make_provider(&server).translate(&requests).await.unwrap();
            assert_results(&results, &requests, &["A", "B"]);
            assert_eq!(results[1].detected_source_lang.as_deref(), Some("ja"));
            for mock in [parent, left, right] {
                mock.assert();
            }
        }
    }

    #[tokio::test]
    async fn invalid_json_retries_halves() {
        let server = MockServer::start();
        let parent = server.mock(|when, then| {
            when.method(POST).path("/translate_a/t").body("q=a&q=b");
            then.status(200).body("not JSON");
        });
        let left = server.mock(|when, then| {
            when.method(POST).path("/translate_a/t").body("q=a");
            then.status(200).json_body(json!(["A"]));
        });
        let right = server.mock(|when, then| {
            when.method(POST).path("/translate_a/t").body("q=b");
            then.status(200).json_body(json!(["B"]));
        });
        let requests = requests(&["a", "b"], "ja");
        let results = make_provider(&server).translate(&requests).await.unwrap();
        assert_results(&results, &requests, &["A", "B"]);
        for mock in [parent, left, right] {
            mock.assert();
        }
    }

    #[tokio::test]
    async fn persistent_failure_returns_error_instead_of_partial_or_guessed_results() {
        let server = MockServer::start();
        let parent = server.mock(|when, then| {
            when.method(POST).path("/translate_a/t").body("q=a&q=b");
            then.status(200).json_body(json!(["WRONG"]));
        });
        let left = server.mock(|when, then| {
            when.method(POST).path("/translate_a/t").body("q=a");
            then.status(200).json_body(json!(["A"]));
        });
        let right = server.mock(|when, then| {
            when.method(POST).path("/translate_a/t").body("q=b");
            then.status(200).json_body(json!([]));
        });
        assert!(make_provider(&server)
            .translate(&requests(&["a", "b"], "ja"))
            .await
            .is_err());
        for mock in [parent, left, right] {
            mock.assert();
        }
    }

    #[tokio::test]
    async fn invalid_single_source_fails_once_without_guessing() {
        for body in [
            "null",
            "{}",
            "[]",
            "[1]",
            "[[]]",
            "[[1]]",
            "[\"\",\"extra\"]",
            "[\"\"]",
            "[[\"\",\"ja\"]]",
            "not JSON",
        ] {
            let server = MockServer::start();
            let mock = server.mock(|when, then| {
                when.method(POST).path("/translate_a/t");
                then.status(200).body(body);
            });
            assert!(
                make_provider(&server)
                    .translate(&requests(&["a"], "ja"))
                    .await
                    .is_err(),
                "accepted {body}"
            );
            mock.assert();
        }
    }

    #[tokio::test]
    async fn empty_source_may_have_empty_translation() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(POST).path("/translate_a/t").body("q=");
            then.status(200).json_body(json!([""]));
        });
        let requests = requests(&[""], "ja");
        let results = make_provider(&server).translate(&requests).await.unwrap();
        assert_results(&results, &requests, &[""]);
        mock.assert();
    }

    #[tokio::test]
    async fn chunks_at_fifty_sources_and_preserves_order_when_responses_finish_out_of_order() {
        let server = MockServer::start();
        let sources: Vec<_> = (0..523).map(|i| format!("source-{i}")).collect();
        let texts: Vec<_> = (0..523).map(|i| format!("translated-{i}")).collect();
        let source_refs: Vec<_> = sources.iter().map(String::as_str).collect();
        let requests = requests(&source_refs, "ja");
        let mut mocks = Vec::new();
        for (i, chunk) in requests.chunks(50).enumerate() {
            mocks.push(server.mock(|when, then| {
                when.method(POST)
                    .path("/translate_a/t")
                    .body(form_body(chunk));
                then.status(200)
                    .delay(Duration::from_millis(if i == 0 { 100 } else { 0 }))
                    .json_body(json!(&texts[i * 50..i * 50 + chunk.len()]));
            }));
        }
        let results = make_provider(&server).translate(&requests).await.unwrap();
        assert_results(
            &results,
            &requests,
            &texts.iter().map(String::as_str).collect::<Vec<_>>(),
        );
        for mock in mocks {
            mock.assert();
        }
    }

    #[tokio::test]
    async fn chunks_by_unicode_character_budget_and_sends_oversize_source_alone() {
        let server = MockServer::start();
        let sources = [
            "猫".repeat(2250),
            "🦀".repeat(2250),
            "a".to_string(),
            "猫".repeat(4501),
            "b".to_string(),
        ];
        let source_refs: Vec<_> = sources.iter().map(String::as_str).collect();
        let requests = requests(&source_refs, "ja");
        let texts = ["A", "B", "C", "D", "E"];
        let mut mocks = Vec::new();
        for range in [0..2, 2..3, 3..4, 4..5] {
            mocks.push(server.mock(|when, then| {
                when.method(POST)
                    .path("/translate_a/t")
                    .body(form_body(&requests[range.clone()]));
                then.status(200).json_body(json!(&texts[range]));
            }));
        }
        let results = make_provider(&server).translate(&requests).await.unwrap();
        assert_results(&results, &requests, &texts);
        for mock in mocks {
            mock.assert();
        }
    }

    #[tokio::test]
    async fn different_language_pairs_use_separate_chunks() {
        let server = MockServer::start();
        let mut requests = requests(&["a", "b", "c"], "ja");
        requests[1].source_lang = "auto".to_string();
        requests[2].source_lang = "auto".to_string();
        requests[2].target_lang = "es".to_string();
        let texts = ["A", "B", "C"];
        let mut mocks = Vec::new();
        for (request, text) in requests.iter().zip(texts) {
            mocks.push(server.mock(|when, then| {
                when.method(POST)
                    .path("/translate_a/t")
                    .query_param("sl", &request.source_lang)
                    .query_param("tl", &request.target_lang)
                    .body(form_body(std::slice::from_ref(request)));
                then.status(200).json_body(json!([text]));
            }));
        }
        let results = make_provider(&server).translate(&requests).await.unwrap();
        assert_results(&results, &requests, &texts);
        for mock in mocks {
            mock.assert();
        }
    }

    async fn assert_http_status(status: u16, reason: &str, retryable: bool) {
        let server = MockServer::start();
        let mock = server.mock(|_when, then| {
            then.status(status).body("upstream error");
        });
        let error = make_provider(&server)
            .translate(&requests(&["a", "b"], "ja"))
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains(&format!(
                "Google Translate returned status {status} {reason}"
            )),
            "{error}"
        );
        assert_eq!(is_retryable(&error), retryable, "{error}");
        // HTTP errors are left to the engine retry policy, not amplified by splitting.
        mock.assert();
    }

    #[tokio::test]
    async fn http_429_keeps_status_and_is_retryable() {
        assert_http_status(429, "Too Many Requests", true).await;
    }

    #[tokio::test]
    async fn http_500_keeps_status_and_is_retryable() {
        assert_http_status(500, "Internal Server Error", true).await;
    }

    #[tokio::test]
    async fn http_400_keeps_status_and_is_not_retryable() {
        assert_http_status(400, "Bad Request", false).await;
    }

    #[tokio::test]
    async fn health_check_uses_post_endpoint() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(POST).path("/translate_a/t").body("q=hello");
            then.status(200).json_body(json!(["hola"]));
        });
        make_provider(&server).health_check().await.unwrap();
        mock.assert();
    }

    #[tokio::test]
    #[ignore = "calls the live Google Translate endpoint"]
    async fn live_google_batch_smoke() {
        let provider = GoogleTranslateProvider::new();
        let sources = [
            "はい",
            "こんにちは、\\C[1]リディア\\C[0]さん。",
            "宿屋へようこそ！\n今日は泊まっていく？",
        ];
        for source_lang in ["ja", "auto"] {
            let requests = requests(&sources, source_lang);
            let results = provider.translate(&requests).await.unwrap();
            assert_eq!(results.len(), requests.len());
            for (request, result) in requests.iter().zip(results) {
                assert_eq!(result.entry_id, request.entry_id);
                assert!(!result.translation.is_empty());
                if source_lang == "auto" {
                    assert_eq!(result.detected_source_lang.as_deref(), Some("ja"));
                }
                println!(
                    "sl={source_lang}, {}: {:?}, detected={:?}",
                    result.entry_id, result.translation, result.detected_source_lang
                );
            }
        }
    }
}
