use async_trait::async_trait;
use std::sync::Arc;
use std::time::Duration;

use locust_core::error::{LocustError, Result};
use locust_core::models::{TranslationRequest, TranslationResult};
use locust_core::translation::{rate_limiter_for, RateLimiter, TranslationProvider};

// Bound the encoded POST body as well as Unicode characters. The endpoint URL
// contains only language/options; source text is never placed in its query.
const MAX_FORM_BYTES: usize = 64 * 1024;

enum ChunkError {
    Request(LocustError),
    Malformed(LocustError),
}

fn source_form_bytes(source: &str) -> usize {
    4 + source
        .as_bytes()
        .iter()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || matches!(*byte, b' ' | b'*' | b'-' | b'.' | b'_') {
                1
            } else {
                3
            }
        })
        .sum::<usize>()
}

/// Google Translate provider using the free web API (no API key required).
/// Uses the undocumented but widely-used translate.googleapis.com endpoint.
pub struct GoogleTranslateProvider {
    client: reqwest::Client,
    base_url: String,
    limiter: Arc<RateLimiter>,
    max_sources: usize,
    max_chars: usize,
    concurrency: usize,
}

impl GoogleTranslateProvider {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::builder()
                .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
                .timeout(Duration::from_secs(20))
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
            base_url: "https://translate.googleapis.com".to_string(),
            limiter: rate_limiter_for("google"),
            // Bounded live measurements and rationale: docs/GOOGLE-FAST-MODE.md.
            max_sources: 200,
            max_chars: 9000,
            concurrency: 4,
        }
    }

    /// Explicit tuning for the opt-in bounded endpoint probe. Normal callers use
    /// the process-wide Google limiter, including configured fixed-rpm overrides.
    pub fn with_tuning(
        limiter: Arc<RateLimiter>,
        max_sources: usize,
        max_chars: usize,
        concurrency: usize,
    ) -> Self {
        Self {
            limiter,
            max_sources: max_sources.max(1),
            max_chars: max_chars.max(1),
            concurrency: concurrency.max(1),
            ..Self::new()
        }
    }

    fn chunks<'a>(&self, requests: &'a [TranslationRequest]) -> Vec<&'a [TranslationRequest]> {
        let mut chunks = Vec::new();
        let mut start = 0;
        let mut chars = 0;
        let mut form_bytes = 0;
        for (i, request) in requests.iter().enumerate() {
            let source_chars = request.source.chars().count();
            let source_bytes = source_form_bytes(&request.source);
            if i > start
                && (i - start == self.max_sources
                    || chars + source_chars > self.max_chars
                    || form_bytes + source_bytes > MAX_FORM_BYTES
                    || request.source_lang != requests[start].source_lang
                    || request.target_lang != requests[start].target_lang)
            {
                chunks.push(&requests[start..i]);
                start = i;
                chars = 0;
                form_bytes = 0;
            }
            // A source over the character budget must travel alone, without truncation.
            chars += source_chars;
            form_bytes += source_bytes;
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
        limiter: &RateLimiter,
    ) -> std::result::Result<Vec<TranslationResult>, ChunkError> {
        let url = format!("{base_url}/translate_a/t");
        let response: Result<_> = async {
            if requests.len() == 1 && source_form_bytes(&requests[0].source) > MAX_FORM_BYTES {
                return Err(LocustError::ProviderError(
                    "Google Translate source exceeds the 64 KiB POST form budget".into(),
                ));
            }
            if !limiter.acquire_http().await {
                return Err(LocustError::ProviderError(
                    "Google live probe stopped after consecutive blocked responses".into(),
                ));
            }
            let fields: Vec<_> = requests.iter().map(|r| ("q", r.source.as_str())).collect();
            let resp = client
                .post(&url)
                .query(&[
                    ("client", "gtx"),
                    ("sl", requests[0].source_lang.as_str()),
                    ("tl", requests[0].target_lang.as_str()),
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

            let status = resp.status();
            let retry_after = resp
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned);
            let html = resp
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .is_some_and(|v| v.to_ascii_lowercase().contains("text/html"));
            if status.as_u16() == 429 || status.as_u16() == 503 || html {
                limiter.record_google_response(status.as_u16(), true, retry_after.as_deref());
                return Err(Self::traffic_error(status));
            }
            let body = resp.text().await.map_err(|e| {
                LocustError::ProviderError(crate::error_sanitizer::sanitize_provider_error(
                    &format!("Google Translate response read failed: {e}"),
                    &[],
                ))
            })?;
            let lower = body.trim_start().to_ascii_lowercase();
            let decoded = serde_json::from_str::<serde_json::Value>(&body);
            let blocked = lower.starts_with("<!doctype html")
                || lower.starts_with("<html")
                || (decoded.is_err() && lower.contains("unusual traffic"))
                || (lower.starts_with('<') && lower.contains("captcha"));
            if blocked {
                limiter.record_google_response(status.as_u16(), true, retry_after.as_deref());
                return Err(Self::traffic_error(status));
            }
            // Let the engine retry HTTP failures according to their status. Splitting
            // these would multiply requests during rate limits or upstream outages.
            if !status.is_success() {
                return Err(LocustError::ProviderError(
                    crate::error_sanitizer::sanitize_provider_error(
                        &format!("Google Translate returned status {}", status),
                        &[],
                    ),
                ));
            }

            Ok((decoded, status))
        }
        .await;
        let (decoded, status) = response.map_err(ChunkError::Request)?;
        decoded
            .map_err(|e| {
                LocustError::ProviderError(crate::error_sanitizer::sanitize_provider_error(
                    &format!("Google Translate returned invalid JSON: {}", e),
                    &[],
                ))
            })
            .and_then(|body| Self::parse_response(&body, requests))
            .inspect(|_| {
                limiter.record_google_response(status.as_u16(), false, None);
            })
            .map_err(ChunkError::Malformed)
    }

    async fn collect_partial(
        &self,
        requests: &[TranslationRequest],
        successes: &mut Vec<TranslationResult>,
    ) -> Vec<(String, LocustError)> {
        let mut pending: std::collections::VecDeque<_> = self
            .chunks(requests)
            .into_iter()
            .map(<[TranslationRequest]>::to_vec)
            .collect();
        let mut failed = Vec::new();
        let mut tasks = tokio::task::JoinSet::new();
        let mut stopped = None;
        loop {
            while stopped.is_none() && tasks.len() < self.concurrency {
                let Some(chunk) = pending.pop_front() else {
                    break;
                };
                let client = self.client.clone();
                let base_url = self.base_url.clone();
                let limiter = self.limiter.clone();
                tasks.spawn(async move {
                    let result = Self::translate_chunk(&client, &base_url, &chunk, &limiter).await;
                    (chunk, result)
                });
            }
            let Some(task) = tasks.join_next().await else {
                break;
            };
            match task {
                Ok((_, Ok(results))) => successes.extend(results),
                Ok((chunk, Err(ChunkError::Malformed(_))))
                    if chunk.len() > 1 && stopped.is_none() =>
                {
                    let mid = chunk.len() / 2;
                    pending.push_front(chunk[mid..].to_vec());
                    pending.push_front(chunk[..mid].to_vec());
                }
                // A completed malformed parent drained after a transport stop
                // still needs splitting on retry, regardless of join order.
                Ok((chunk, Err(ChunkError::Malformed(_)))) if chunk.len() > 1 => {}
                Ok((chunk, Err(ChunkError::Malformed(error)))) => {
                    failed.extend(chunk.iter().map(|r| {
                        (
                            r.entry_id.clone(),
                            LocustError::ProviderError(error.to_string()),
                        )
                    }));
                }
                Ok((_, Err(ChunkError::Request(error)))) => {
                    if stopped.is_none() {
                        stopped = Some(error.to_string());
                        // Abort queued HTTP waits immediately after a block; waiting
                        // through their shared cooldown would bury known receipts.
                        // join_next still yields successful tasks already completed.
                        tasks.abort_all();
                    }
                }
                Err(error) if !error.is_cancelled() => {
                    stopped = Some(format!("Google Translate task join error: {error}"));
                    tasks.abort_all();
                }
                Err(_) => {}
            }
        }
        if let Some(error) = stopped {
            let accounted: std::collections::HashSet<_> = successes
                .iter()
                .map(|r| r.entry_id.as_str())
                .chain(failed.iter().map(|(id, _)| id.as_str()))
                .collect();
            let unsettled = requests
                .iter()
                .filter(|r| !accounted.contains(r.entry_id.as_str()))
                .map(|r| {
                    (
                        r.entry_id.clone(),
                        LocustError::ProviderError(error.clone()),
                    )
                })
                .collect::<Vec<_>>();
            failed.extend(unsettled);
        }
        // Preserve the adapter's existing ordered success API for direct callers.
        let order: std::collections::HashMap<_, _> = requests
            .iter()
            .enumerate()
            .map(|(i, r)| (r.entry_id.as_str(), i))
            .collect();
        successes.sort_by_key(|r| order[r.entry_id.as_str()]);
        failed
    }

    fn traffic_error(status: reqwest::StatusCode) -> LocustError {
        // Do not echo challenge HTML or a 403: core's generic auth-error
        // classifier must see this as a retryable traffic limit instead.
        LocustError::ProviderError(format!(
            "Google Translate rate limit (429/503 or traffic challenge; status {})",
            if status.as_u16() == 403 {
                "traffic challenge".to_string()
            } else {
                status.to_string()
            }
        ))
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
    fn supports_instructions(&self) -> bool {
        false
    }

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
        let mut results = Vec::new();
        let failed = self.collect_partial(requests, &mut results).await;
        if let Some((_, error)) = failed.into_iter().next() {
            Err(error)
        } else {
            Ok(results)
        }
    }

    async fn translate_partial(
        &self,
        requests: &[TranslationRequest],
        successes: &mut Vec<TranslationResult>,
    ) -> Option<Vec<(String, LocustError)>> {
        Some(self.collect_partial(requests, successes).await)
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

    #[tokio::test]
    async fn mt_control_repair_changes_post_source_and_fallback_only_gets_failed_id() {
        use locust_core::{
            database::Database,
            glossary::Glossary,
            models::{StringEntry, StringStatus},
            translation::{run_fallback_chain, TranslationManager, TranslationOptions},
        };
        for damaged_alternate in [false, true] {
            let server = MockServer::start_async().await;
            let initial = server
                .mock_async(|when, then| {
                    when.method(POST)
                        .path("/translate_a/t")
                        .body("q=Hello+%7BPL_0%7D&q=Goodbye+%7BPL_0%7D");
                    then.status(200).json_body(json!(["Hola", "Adios {PL_0}"]));
                })
                .await;
            let alternate = server
                .mock_async(|when, then| {
                    when.method(POST)
                        .path("/translate_a/t")
                        .body("q=Hello+%E2%9F%A6L0X0%E2%9F%A7");
                    then.status(200).json_body(json!([if damaged_alternate {
                        "Hola ⟦L0X0⟧ ⟦L0X0⟧"
                    } else {
                        "Hola ⟦L0X0⟧"
                    }]));
                })
                .await;
            let provider = Arc::new(GoogleTranslateProvider {
                base_url: server.base_url(),
                limiter: Arc::new(RateLimiter::unlimited()),
                ..GoogleTranslateProvider::new()
            });
            assert!(!provider.supports_instructions());
            let db = Arc::new(Database::open_in_memory().unwrap());
            let glossary = Arc::new(Glossary::new(db.clone()));
            let entries = vec![
                StringEntry::new("bad", "Hello [player]", "script.rpy".into())
                    .with_format_id("renpy"),
                StringEntry::new("good", "Goodbye [player]", "script.rpy".into())
                    .with_format_id("renpy"),
            ];
            db.save_entries(&entries).unwrap();
            let opts = TranslationOptions {
                source_lang: "en".into(),
                target_lang: "es".into(),
                use_memory: false,
                ..Default::default()
            };
            let (tx, _rx) = tokio::sync::mpsc::channel(100);
            TranslationManager::new(provider, db.clone(), glossary.clone())
                .translate_entries(
                    entries,
                    opts.clone(),
                    tx,
                    "mt-post".into(),
                    Default::default(),
                )
                .await
                .unwrap();
            initial.assert_hits_async(1).await;
            alternate.assert_hits_async(1).await;
            let bad = db.get_entry("bad").unwrap().unwrap();
            if damaged_alternate {
                assert_eq!(bad.status, StringStatus::Pending);
                let fallback = Arc::new(PartialFallback(Default::default()));
                let (tx, _rx) = tokio::sync::mpsc::channel(100);
                run_fallback_chain(
                    &["partial-fallback".into()],
                    &|_| Some(fallback.clone()),
                    db.clone(),
                    glossary,
                    opts,
                    tx,
                    "mt-fallback".into(),
                    Default::default(),
                )
                .await
                .unwrap();
                assert_eq!(*fallback.0.lock().unwrap(), ["bad"]);
            } else {
                assert_eq!(bad.translation.as_deref(), Some("Hola [player]"));
            }
            let good = db.get_entry("good").unwrap().unwrap();
            assert_eq!(good.translation.as_deref(), Some("Adios [player]"));
            assert_eq!(good.provider_used.as_deref(), Some("google"));
        }
    }

    // Dynamic loopback server: logs every source, including malformed parents,
    // so retry regressions cannot hide behind a total HTTP request count.
    async fn partial_server(
        mode: &'static str,
    ) -> (
        String,
        Arc<std::sync::Mutex<Vec<String>>>,
        tokio::task::JoinHandle<()>,
    ) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let log = Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen = log.clone();
        let serving = tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                let seen = seen.clone();
                tokio::spawn(async move {
                    let mut bytes = Vec::new();
                    let body = loop {
                        let mut buf = [0; 4096];
                        let read = socket.read(&mut buf).await.unwrap();
                        if read == 0 {
                            return;
                        }
                        bytes.extend_from_slice(&buf[..read]);
                        if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                            let headers =
                                String::from_utf8_lossy(&bytes[..end]).to_ascii_lowercase();
                            let len = headers
                                .lines()
                                .find_map(|line| {
                                    line.strip_prefix("content-length:")
                                        .map(|v| v.trim().parse::<usize>().unwrap())
                                })
                                .unwrap();
                            if bytes.len() >= end + 4 + len {
                                break String::from_utf8(bytes[end + 4..end + 4 + len].to_vec())
                                    .unwrap();
                            }
                        }
                    };
                    let sources: Vec<_> = body
                        .split('&')
                        .map(|q| q.strip_prefix("q=").unwrap())
                        .collect();
                    let call = {
                        let mut log = seen.lock().unwrap();
                        log.extend(sources.iter().map(|s| (*s).to_string()));
                        log.iter().filter(|s| *s == "c").count()
                    };
                    let blocked = body == "q=c&q=d"
                        && (mode == "blocked"
                            || mode == "cancel"
                            || (mode == "recover" && call == 1));
                    let malformed = mode == "malformed" && (body == "q=c&q=d" || body == "q=d");
                    if mode == "cancel-inflight" && body == "q=c&q=d" {
                        tokio::time::sleep(Duration::from_secs(60)).await;
                    }
                    let (status, response) = if blocked {
                        (
                            "503 Service Unavailable",
                            "upstream unavailable".to_string(),
                        )
                    } else if malformed {
                        ("200 OK", "[]".to_string())
                    } else {
                        (
                            "200 OK",
                            serde_json::to_string(
                                &sources.iter().map(|s| s.to_uppercase()).collect::<Vec<_>>(),
                            )
                            .unwrap(),
                        )
                    };
                    let response = format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}", response.len());
                    let _ = socket.write_all(response.as_bytes()).await;
                });
            }
        });
        (url, log, serving)
    }

    struct PartialFallback(std::sync::Mutex<Vec<String>>);

    #[async_trait]
    impl TranslationProvider for PartialFallback {
        fn id(&self) -> &str {
            "partial-fallback"
        }
        fn name(&self) -> &str {
            "Partial fallback"
        }
        fn is_free(&self) -> bool {
            true
        }
        fn requires_api_key(&self) -> bool {
            false
        }
        async fn translate(
            &self,
            requests: &[TranslationRequest],
        ) -> Result<Vec<TranslationResult>> {
            self.0
                .lock()
                .unwrap()
                .extend(requests.iter().map(|r| r.entry_id.clone()));
            Ok(requests
                .iter()
                .map(|r| TranslationResult {
                    entry_id: r.entry_id.clone(),
                    translation: format!("Fallback {}", r.source),
                    detected_source_lang: None,
                    provider: self.id().into(),
                    tokens_used: Some(3),
                    input_tokens: Some(2),
                    output_tokens: Some(1),
                    cost_usd: None,
                })
                .collect())
        }
        async fn estimate_cost(&self, _: usize, _: &str) -> Option<f64> {
            None
        }
        async fn health_check(&self) -> Result<()> {
            Ok(())
        }
    }

    async fn partial_scenario(mode: &'static str, with_fallback: bool) {
        use locust_core::{
            database::Database,
            glossary::Glossary,
            models::{ProgressEvent, StringEntry},
            translation::{
                load_pending_entries, run_fallback_chain, RetryConfig, TranslationManager,
                TranslationOptions,
            },
        };
        let (base_url, log, serving) = partial_server(mode).await;
        let provider = Arc::new(GoogleTranslateProvider {
            base_url,
            limiter: Arc::new(RateLimiter::unlimited()),
            max_sources: 2,
            concurrency: 1,
            ..GoogleTranslateProvider::new()
        });
        let db = Arc::new(Database::open_in_memory().unwrap());
        let glossary = Arc::new(Glossary::new(db.clone()));
        let entries = ["a", "b", "c", "d"]
            .iter()
            .map(|s| StringEntry::new(*s, *s, "file.json".into()))
            .collect::<Vec<_>>();
        db.save_entries(&entries).unwrap();
        let opts = TranslationOptions {
            source_lang: "en".into(),
            target_lang: "es".into(),
            use_memory: false,
            use_glossary: false,
            ..Default::default()
        };
        let (tx, mut rx) = tokio::sync::mpsc::channel(100);
        let cfg = RetryConfig {
            initial_delay_ms: if mode == "cancel" { 60_000 } else { 1 },
            max_delay_ms: 60_000,
            ..Default::default()
        };
        let cancel = cfg.cancel.clone().unwrap_or_default();
        let cancel_job = cancel.clone();
        let manager = TranslationManager::new(provider.clone(), db.clone(), glossary.clone())
            .with_retry_config(cfg);
        let job = tokio::spawn(async move {
            manager
                .translate_entries(entries, opts, tx, "partial-google".into(), cancel_job)
                .await
        });
        if mode.starts_with("cancel") {
            tokio::time::timeout(Duration::from_secs(5), async {
                while !log.lock().unwrap().iter().any(|s| s == "d") {
                    tokio::time::sleep(Duration::from_millis(1)).await;
                }
            })
            .await
            .unwrap();
            // Cancellation while core is backing off, after Google has delivered receipts.
            tokio::time::sleep(Duration::from_millis(20)).await;
            cancel.cancel();
        }
        tokio::time::timeout(Duration::from_secs(5), job)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let expected = match mode {
            "recover" => [1, 1, 2, 2],
            "blocked" => [1, 1, 3, 3],
            "malformed" => [1, 1, 2, 2],
            "cancel" | "cancel-inflight" => [1, 1, 1, 1],
            _ => unreachable!(),
        };
        for (source, expected) in ["a", "b", "c", "d"].iter().zip(expected) {
            assert_eq!(
                log.lock()
                    .unwrap()
                    .iter()
                    .filter(|s| s.as_str() == *source)
                    .count(),
                expected,
                "{mode}: {source}"
            );
        }
        let saved = match mode {
            "recover" => 4,
            "malformed" => 3,
            _ => 2,
        };
        assert_eq!(load_pending_entries(&db).unwrap().len(), 4 - saved);
        let mut writes = Vec::new();
        let mut paused = false;
        while let Some(event) = rx.recv().await {
            match event {
                ProgressEvent::StringTranslated { entry_id, .. } => writes.push(entry_id),
                ProgressEvent::Paused => paused = true,
                _ => {}
            }
        }
        assert_eq!(writes.len(), saved);
        writes.sort();
        writes.dedup();
        assert_eq!(writes.len(), saved);
        assert_eq!(paused, mode.starts_with("cancel"));
        let runs = db.get_translation_runs().unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].strings_translated, saved);
        assert_eq!(
            (
                runs[0].tokens_used,
                runs[0].cost_usd,
                runs[0].cost_is_complete
            ),
            (0, 0.0, true)
        );
        assert_eq!(runs[0].provider, "google");
        for id in ["a", "b"] {
            assert_eq!(
                db.get_entry(id).unwrap().unwrap().provider_used.as_deref(),
                Some("google")
            );
        }
        if with_fallback {
            let fallback = Arc::new(PartialFallback(Default::default()));
            let (tx, _rx) = tokio::sync::mpsc::channel(100);
            let fallback_dyn: Arc<dyn TranslationProvider> = fallback.clone();
            run_fallback_chain(
                &["partial-fallback".into()],
                &|_| Some(fallback_dyn.clone()),
                db.clone(),
                glossary,
                TranslationOptions {
                    use_memory: false,
                    use_glossary: false,
                    ..Default::default()
                },
                tx,
                "fallback".into(),
                Default::default(),
            )
            .await
            .unwrap();
            let expected_ids: Vec<_> = if mode == "malformed" {
                vec!["d"]
            } else {
                vec!["c", "d"]
            };
            assert_eq!(*fallback.0.lock().unwrap(), expected_ids);
            assert!(load_pending_entries(&db).unwrap().is_empty());
            assert_eq!(
                db.get_entry("d").unwrap().unwrap().provider_used.as_deref(),
                Some("partial-fallback")
            );
            let runs = db.get_translation_runs().unwrap();
            assert_eq!(runs.len(), 2);
            assert_eq!(runs.iter().map(|r| r.strings_translated).sum::<usize>(), 4);
            let fallback_run = runs
                .iter()
                .find(|r| r.provider == "partial-fallback")
                .unwrap();
            assert_eq!(
                (
                    fallback_run.tokens_used,
                    fallback_run.input_tokens,
                    fallback_run.output_tokens
                ),
                (
                    (4 - saved) as u64 * 3,
                    (4 - saved) as u64 * 2,
                    (4 - saved) as u64
                )
            );
        }
        serving.abort();
    }

    #[tokio::test]
    async fn partial_success_503_then_recovery_never_resends_successes() {
        partial_scenario("recover", false).await;
    }
    #[tokio::test]
    async fn partial_success_persistent_503_saves_successes_and_falls_back_only_pending() {
        partial_scenario("blocked", true).await;
    }
    #[tokio::test]
    async fn partial_success_malformed_singleton_preserves_siblings_and_fallback() {
        partial_scenario("malformed", true).await;
    }
    #[tokio::test]
    async fn partial_success_cancel_mid_retry_preserves_receipts() {
        partial_scenario("cancel", false).await;
    }
    #[tokio::test]
    async fn partial_success_cancel_inflight_preserves_completed_subchunks() {
        partial_scenario("cancel-inflight", false).await;
    }

    struct LegacyGoogle(GoogleTranslateProvider);
    #[async_trait]
    impl TranslationProvider for LegacyGoogle {
        fn id(&self) -> &str {
            "mock"
        }
        fn name(&self) -> &str {
            "Legacy Google"
        }
        fn is_free(&self) -> bool {
            true
        }
        fn requires_api_key(&self) -> bool {
            false
        }
        async fn translate(
            &self,
            requests: &[TranslationRequest],
        ) -> Result<Vec<TranslationResult>> {
            self.0.translate(requests).await
        }
        async fn estimate_cost(&self, _: usize, _: &str) -> Option<f64> {
            None
        }
        async fn health_check(&self) -> Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn partial_success_non_opt_in_provider_stays_all_or_nothing() {
        use locust_core::{
            database::Database,
            glossary::Glossary,
            models::StringEntry,
            translation::{
                load_pending_entries, RetryConfig, TranslationManager, TranslationOptions,
            },
        };
        let (base_url, log, serving) = partial_server("blocked").await;
        let provider = Arc::new(LegacyGoogle(GoogleTranslateProvider {
            base_url,
            limiter: Arc::new(RateLimiter::unlimited()),
            max_sources: 2,
            concurrency: 1,
            ..GoogleTranslateProvider::new()
        }));
        let db = Arc::new(Database::open_in_memory().unwrap());
        let entries = ["a", "b", "c", "d"]
            .iter()
            .map(|s| StringEntry::new(*s, *s, "file.json".into()))
            .collect::<Vec<_>>();
        db.save_entries(&entries).unwrap();
        let (tx, _rx) = tokio::sync::mpsc::channel(100);
        TranslationManager::new(provider, db.clone(), Arc::new(Glossary::new(db.clone())))
            .with_retry_config(RetryConfig {
                initial_delay_ms: 1,
                max_delay_ms: 1,
                ..Default::default()
            })
            .translate_entries(
                entries,
                TranslationOptions {
                    use_memory: false,
                    use_glossary: false,
                    ..Default::default()
                },
                tx,
                "legacy".into(),
                Default::default(),
            )
            .await
            .unwrap();
        for source in ["a", "b", "c", "d"] {
            assert_eq!(
                log.lock()
                    .unwrap()
                    .iter()
                    .filter(|s| s.as_str() == source)
                    .count(),
                3
            );
        }
        assert_eq!(load_pending_entries(&db).unwrap().len(), 4);
        assert!(db.get_translation_runs().unwrap().is_empty());
        serving.abort();
    }

    #[tokio::test]
    async fn partial_success_concurrent_paced_chunks_cancel_queued_retries() {
        use locust_core::{
            database::Database,
            glossary::Glossary,
            models::StringEntry,
            translation::{
                load_pending_entries, RetryConfig, TranslationManager, TranslationOptions,
            },
        };
        for rpm in [None, Some(6000)] {
            let (base_url, log, serving) = partial_server("blocked").await;
            let limiter = Arc::new(RateLimiter::google(rpm, None));
            let provider = Arc::new(GoogleTranslateProvider {
                base_url,
                limiter: limiter.clone(),
                max_sources: 2,
                concurrency: 3,
                ..GoogleTranslateProvider::new()
            });
            let db = Arc::new(Database::open_in_memory().unwrap());
            let entries = ["a", "b", "c", "d", "e", "f"]
                .iter()
                .map(|s| StringEntry::new(*s, *s, "file.json".into()))
                .collect::<Vec<_>>();
            db.save_entries(&entries).unwrap();
            let manager =
                TranslationManager::new(provider, db.clone(), Arc::new(Glossary::new(db.clone())))
                    .with_retry_config(RetryConfig {
                        initial_delay_ms: 1,
                        max_delay_ms: 1,
                        overall_deadline: Some(Duration::from_secs(5)),
                        ..Default::default()
                    });
            let (tx, _rx) = tokio::sync::mpsc::channel(100);
            let cancel = RetryConfig::default().cancel.unwrap_or_default();
            let token = cancel.clone();
            let job = tokio::spawn(async move {
                manager
                    .translate_entries(
                        entries,
                        TranslationOptions {
                            use_memory: false,
                            use_glossary: false,
                            ..Default::default()
                        },
                        tx,
                        "paced-partial".into(),
                        token,
                    )
                    .await
            });
            tokio::time::timeout(Duration::from_secs(5), async {
                while limiter.google_stats().blocked == 0 {
                    tokio::time::sleep(Duration::from_millis(1)).await;
                }
            })
            .await
            .unwrap();
            // The next attempt queues behind Google's cooldown, then cancellation
            // must release its position without another POST or lost receipts.
            tokio::time::sleep(Duration::from_millis(20)).await;
            cancel.cancel();
            tokio::time::timeout(Duration::from_secs(1), job)
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            assert_eq!(*log.lock().unwrap(), ["a", "b", "c", "d"]);
            let stats = limiter.google_stats();
            assert_eq!((stats.requests, stats.successes, stats.blocked), (2, 1, 1));
            assert_eq!(limiter.requests_per_minute(), rpm.unwrap_or(31));
            assert_eq!(load_pending_entries(&db).unwrap().len(), 4);
            let runs = db.get_translation_runs().unwrap();
            assert_eq!((runs.len(), runs[0].strings_translated), (1, 2));
            assert!(runs[0].cost_is_complete);
            serving.abort();
        }
    }

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
                limiter: Arc::new(RateLimiter::unlimited()),
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
            limiter: Arc::new(RateLimiter::unlimited()),
            // Keep the original boundary/isolation regressions at their explicit
            // 50-source/4500-character settings; larger bodies have separate tests.
            max_sources: 50,
            max_chars: 4500,
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
    async fn unicode_post_chunks_obey_encoded_body_budget() {
        let server = MockServer::start();
        let sources = ["🦀".repeat(3000), "猫".repeat(4000), "🦊".repeat(3000)];
        let source_refs: Vec<_> = sources.iter().map(String::as_str).collect();
        let requests = requests(&source_refs, "ja");
        let provider = GoogleTranslateProvider {
            max_sources: 200,
            max_chars: 9000,
            ..make_provider(&server)
        };
        let mut mocks = Vec::new();
        for request in &requests {
            let body = form_body(std::slice::from_ref(request));
            assert!(body.len() < MAX_FORM_BYTES);
            mocks.push(server.mock(|when, then| {
                when.method(POST).body(body);
                then.status(200).json_body(json!(["A"]));
            }));
        }
        assert_eq!(provider.translate(&requests).await.unwrap().len(), 3);
        for mock in mocks {
            mock.assert_hits(1);
        }
    }

    #[tokio::test]
    async fn single_source_over_body_budget_fails_without_sending_or_truncating() {
        let server = MockServer::start();
        let mock = server.mock(|_when, then| {
            then.status(200);
        });
        let source = "🦀".repeat(6000);
        let error = make_provider(&server)
            .translate(&requests(&[&source], "ja"))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("POST form budget"));
        mock.assert_hits(0);
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
            error.to_string().contains(&format!("{status} {reason}")),
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
    async fn mocked_success_grows_google_rate_and_429_halves_it() {
        let server = MockServer::start();
        let success = server.mock(|when, then| {
            when.method(POST).body("q=a");
            then.status(200).json_body(json!(["A"]));
        });
        let block = server.mock(|when, then| {
            when.method(POST).body("q=b");
            then.status(429).header("Retry-After", "75");
        });
        let limiter = Arc::new(RateLimiter::google(None, None));
        let provider = GoogleTranslateProvider {
            limiter: limiter.clone(),
            ..make_provider(&server)
        };
        provider.translate(&requests(&["a"], "en")).await.unwrap();
        assert_eq!(limiter.requests_per_minute(), 62);
        let error = provider
            .translate(&requests(&["b"], "en"))
            .await
            .unwrap_err();
        assert!(is_retryable(&error));
        assert_eq!(limiter.requests_per_minute(), 31);
        tokio::time::pause();
        let start = tokio::time::Instant::now();
        assert!(limiter.acquire_http().await);
        assert!(start.elapsed() >= Duration::from_secs(74));
        success.assert();
        block.assert();
    }

    #[tokio::test]
    async fn mocked_fixed_override_observes_retry_after_without_adaptation() {
        let server = MockServer::start();
        let block = server.mock(|_when, then| {
            then.status(503).header("Retry-After", "80");
        });
        let limiter = Arc::new(RateLimiter::google(Some(120), None));
        let provider = GoogleTranslateProvider {
            limiter: limiter.clone(),
            ..make_provider(&server)
        };
        assert!(provider
            .translate(&requests(&["a", "b"], "en"))
            .await
            .is_err());
        assert_eq!(limiter.requests_per_minute(), 120);
        assert!(!limiter.is_adaptive());
        tokio::time::pause();
        let start = tokio::time::Instant::now();
        limiter.acquire_http().await;
        assert!(start.elapsed() >= Duration::from_secs(79));
        block.assert();
    }

    #[tokio::test]
    async fn captcha_and_unusual_traffic_never_amplify_into_split_requests() {
        for (status, body) in [
            (200, "<!doctype html><html>captcha</html>"),
            (
                403,
                "<html>Our systems have detected unusual traffic</html>",
            ),
            (200, "Our systems have detected unusual traffic"),
        ] {
            let server = MockServer::start();
            let blocked = server.mock(|_when, then| {
                then.status(status).body(body);
            });
            let limiter = Arc::new(RateLimiter::google(None, None));
            let provider = GoogleTranslateProvider {
                limiter: limiter.clone(),
                ..make_provider(&server)
            };
            let error = provider
                .translate(&requests(&["a", "b", "c", "d"], "en"))
                .await
                .unwrap_err();
            assert!(is_retryable(&error), "{error}");
            assert_eq!(limiter.requests_per_minute(), 30);
            assert_eq!(limiter.google_stats().blocked, 1);
            blocked.assert_hits(1);
        }
    }

    #[tokio::test]
    async fn legitimate_traffic_dialogue_is_not_a_challenge() {
        let server = MockServer::start();
        let success = server.mock(|_when, then| {
            then.status(200)
                .json_body(json!(["There is unusual traffic near the castle."]));
        });
        let limiter = Arc::new(RateLimiter::google(None, None));
        let provider = GoogleTranslateProvider {
            limiter: limiter.clone(),
            ..make_provider(&server)
        };
        provider.translate(&requests(&["a"], "en")).await.unwrap();
        assert_eq!(limiter.google_stats().blocked, 0);
        success.assert();
    }

    #[tokio::test]
    async fn blocked_headers_take_effect_without_waiting_for_the_body() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let serving = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0; 1];
            socket.read_exact(&mut request).await.unwrap();
            socket.write_all(b"HTTP/1.1 429 Too Many Requests\r\nRetry-After: 90\r\nContent-Length: 1000000\r\n\r\n").await.unwrap();
            std::future::pending::<()>().await;
        });
        let limiter = Arc::new(RateLimiter::google(None, None));
        let provider = GoogleTranslateProvider {
            base_url: format!("http://{address}"),
            limiter: limiter.clone(),
            ..GoogleTranslateProvider::new()
        };
        let batch = requests(&["a"], "en");
        let error = tokio::time::timeout(Duration::from_secs(2), provider.translate(&batch))
            .await
            .unwrap()
            .unwrap_err();
        assert!(is_retryable(&error));
        assert_eq!(limiter.requests_per_minute(), 30);
        assert_eq!(limiter.google_stats().http_429, 1);
        serving.abort();
    }

    #[tokio::test]
    async fn cancellation_wins_during_google_cooldown_and_deadline_still_bounds_it() {
        use locust_core::translation::{with_retry, RetryConfig};
        let server = MockServer::start();
        let blocked = server.mock(|_when, then| {
            then.status(429).header("Retry-After", "120");
        });
        let limiter = Arc::new(RateLimiter::google(None, None));
        let provider = Arc::new(GoogleTranslateProvider {
            limiter,
            ..make_provider(&server)
        });
        provider
            .translate(&requests(&["a"], "en"))
            .await
            .unwrap_err();
        let cfg = RetryConfig {
            cancel: Some(Default::default()),
            ..Default::default()
        };
        cfg.cancel.as_ref().unwrap().cancel();
        let error = with_retry(&cfg, || provider.translate(&[]))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("cancelled"));

        let cfg = RetryConfig {
            cancel: Some(Default::default()),
            ..Default::default()
        };
        let cancel = cfg.cancel.clone().unwrap();
        let waiting_provider = provider.clone();
        let waiting = tokio::spawn(async move {
            let batch = requests(&["b"], "en");
            with_retry(&cfg, || waiting_provider.translate(&batch)).await
        });
        tokio::task::yield_now().await;
        cancel.cancel();
        assert!(waiting
            .await
            .unwrap()
            .unwrap_err()
            .to_string()
            .contains("cancelled"));

        tokio::time::pause();
        let cfg = RetryConfig {
            overall_deadline: Some(Duration::from_secs(2)),
            ..Default::default()
        };
        let batch = requests(&["c"], "en");
        let error = with_retry(&cfg, || provider.translate(&batch))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("deadline exceeded"));
        blocked.assert_hits(1);
    }

    #[tokio::test]
    async fn concurrent_batches_share_http_rate_without_starvation() {
        let server = MockServer::start();
        let success = server.mock(|_when, then| {
            then.status(200).json_body(json!(["A"]));
        });
        let limiter = Arc::new(RateLimiter::google(Some(6000), None));
        let provider = Arc::new(GoogleTranslateProvider {
            limiter: limiter.clone(),
            ..make_provider(&server)
        });
        let mut tasks = tokio::task::JoinSet::new();
        for _ in 0..16 {
            let provider = provider.clone();
            tasks.spawn(async move { provider.translate(&requests(&["a"], "en")).await });
        }
        tokio::time::timeout(Duration::from_secs(5), async {
            while let Some(result) = tasks.join_next().await {
                assert_eq!(result.unwrap().unwrap().len(), 1);
            }
        })
        .await
        .unwrap();
        assert_eq!(limiter.google_stats().requests, 16);
        success.assert_hits(16);
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
