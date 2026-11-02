use std::sync::Arc;
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use crate::error::{LocustError, Result};
use crate::models::{TranslationRequest, TranslationResult};

use super::provider::TranslationProvider;
use super::rate_limit::{rate_limiter_for, RateLimiter};

#[derive(Clone, Debug)]
pub struct RetryConfig {
    pub max_attempts: u32,
    pub initial_delay_ms: u64,
    pub max_delay_ms: u64,
    pub backoff_multiplier: f64,
    pub overall_deadline: Option<Duration>,
    pub cancel: Option<CancellationToken>,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            initial_delay_ms: 1000,
            max_delay_ms: 30000,
            backoff_multiplier: 2.0,
            overall_deadline: Some(Duration::from_secs(60)),
            cancel: None,
        }
    }
}

pub async fn with_retry<F, Fut, T>(config: &RetryConfig, operation: F) -> Result<T>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Result<T>>,
{
    let deadline = config
        .overall_deadline
        .map(|d| tokio::time::Instant::now() + d);
    let mut last_err = None;

    for attempt in 0..config.max_attempts {
        if let Some(cancel) = &config.cancel {
            if cancel.is_cancelled() {
                return Err(LocustError::ProviderError("cancelled".into()));
            }
        }
        if let Some(dl) = deadline {
            if tokio::time::Instant::now() >= dl {
                return Err(last_err.unwrap_or_else(|| {
                    LocustError::ProviderError("retry deadline exceeded".into())
                }));
            }
        }

        // Bound the in-flight request too, not just the delay between attempts.
        // Dropping the future cancels the local HTTP wait when the job is stopped.
        let outcome = tokio::select! {
            biased;
            _ = async {
                match &config.cancel {
                    Some(cancel) => cancel.cancelled().await,
                    None => std::future::pending().await,
                }
            } => return Err(LocustError::ProviderError("cancelled".into())),
            _ = async {
                match deadline {
                    Some(dl) => tokio::time::sleep_until(dl).await,
                    None => std::future::pending().await,
                }
            } => return Err(LocustError::ProviderError("provider request timeout: retry deadline exceeded".into())),
            outcome = operation() => outcome,
        };
        match outcome {
            Ok(v) => return Ok(v),
            Err(e) => {
                if is_retryable(&e) && attempt < config.max_attempts - 1 {
                    let mut delay_ms = jitter_delay(compute_delay(config, attempt));
                    if let Some(dl) = deadline {
                        let remaining = dl
                            .saturating_duration_since(tokio::time::Instant::now())
                            .as_millis() as u64;
                        if remaining == 0 {
                            return Err(e);
                        }
                        delay_ms = delay_ms.min(remaining);
                    }
                    tracing::warn!(
                        "Retryable error on attempt {}/{}: {}. Retrying in {}ms",
                        attempt + 1,
                        config.max_attempts,
                        e,
                        delay_ms
                    );
                    let sleep = tokio::time::sleep(Duration::from_millis(delay_ms));
                    if let Some(cancel) = &config.cancel {
                        tokio::select! {
                            biased;
                            _ = cancel.cancelled() => {
                                return Err(LocustError::ProviderError("cancelled".into()));
                            }
                            _ = sleep => {}
                        }
                    } else {
                        sleep.await;
                    }
                    last_err = Some(e);
                } else {
                    return Err(e);
                }
            }
        }
    }

    Err(last_err
        .unwrap_or_else(|| LocustError::ProviderError("retry exhausted with no error".to_string())))
}

fn compute_delay(config: &RetryConfig, attempt: u32) -> u64 {
    let delay = config.initial_delay_ms as f64 * config.backoff_multiplier.powi(attempt as i32);
    (delay as u64).min(config.max_delay_ms)
}

fn jitter_delay(base_ms: u64) -> u64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let factor = 0.5 + (nanos % 1000) as f64 / 1000.0;
    ((base_ms as f64) * factor) as u64
}

// Provider translation and health-check errors share this envelope. Read only
// the first matching status so upstream body text cannot override the HTTP code.
fn returned_status(message: &str) -> Option<u16> {
    message.split("returned status ").skip(1).find_map(|tail| {
        let code = tail.get(..3)?;
        if code.bytes().all(|byte| byte.is_ascii_digit()) {
            code.parse().ok()
        } else {
            None
        }
    })
}

fn bare_status_codes(message: &str) -> impl Iterator<Item = u16> + '_ {
    let mut after_comparison = false;
    message.split_whitespace().filter_map(move |token| {
        let token = token.trim_matches(|c| {
            matches!(
                c,
                '(' | ')' | '[' | ']' | '{' | '}' | ':' | ',' | ';' | '.' | '!' | '?' | '\"' | '\''
            )
        });
        let skip = after_comparison;
        after_comparison = token.ends_with(['<', '>', '=']);
        // Preserve bare HTTP codes, but exclude IDs and numeric constraints such
        // as "max_tokens <= 500", whether or not the operator has a space.
        if skip || token.len() != 3 {
            return None;
        }
        token.parse().ok()
    })
}

pub fn is_retryable(e: &LocustError) -> bool {
    match e {
        LocustError::ProviderError(msg) => {
            let lower = msg.to_ascii_lowercase();
            if let Some(status) = returned_status(&lower) {
                return matches!(status, 408 | 425 | 429 | 500..=599);
            }
            if lower.contains("cancelled")
                || lower.contains("unauthorized")
                || lower.contains("forbidden")
            {
                return false;
            }
            let mut retryable_status = false;
            for status in bare_status_codes(&lower) {
                if matches!(status, 401 | 403) {
                    return false;
                }
                retryable_status |= matches!(status, 429 | 500 | 502 | 503 | 504);
            }
            retryable_status
                || lower.contains("rate limit")
                || lower.contains("timeout")
                || lower.contains("timed out")
                || lower.contains("connection")
                || lower.contains("network")
                || lower.contains("dns")
        }
        LocustError::ProviderErrorWithUsage { error, .. } => is_retryable(error),
        LocustError::IoError(_) => true,
        _ => false,
    }
}

pub(super) struct ProviderCall {
    pub(super) results: Vec<TranslationResult>,
    pub(super) attempts: usize,
    pub(super) failed_ids: Vec<String>,
    pub(super) partial: bool,
    pub(super) rejected_results: Vec<TranslationResult>,
    pub(super) error: Option<String>,
}

impl ProviderCall {
    pub(super) fn reported_usage(&self) -> impl Iterator<Item = &TranslationResult> {
        self.results.iter().chain(&self.rejected_results)
    }
}

fn valid_partition(
    requests: &[TranslationRequest],
    successes: &[TranslationResult],
    failed: impl Iterator<Item = String>,
) -> bool {
    let expected: std::collections::HashSet<_> =
        requests.iter().map(|r| r.entry_id.as_str()).collect();
    let mut seen = std::collections::HashSet::new();
    expected.len() == requests.len()
        && successes
            .iter()
            .map(|r| r.entry_id.clone())
            .chain(failed)
            .all(|id| expected.contains(id.as_str()) && seen.insert(id))
        && seen.len() == expected.len()
}

pub(super) async fn call_provider(
    provider: Arc<dyn TranslationProvider>,
    requests: &[TranslationRequest],
    retry: &RetryConfig,
    cancel: &CancellationToken,
) -> Result<ProviderCall> {
    let limiter = rate_limiter_for(provider.id());
    call_provider_with_limiter(provider, requests, retry, cancel, limiter).await
}

async fn call_provider_with_limiter(
    provider: Arc<dyn TranslationProvider>,
    requests: &[TranslationRequest],
    retry: &RetryConfig,
    cancel: &CancellationToken,
    limiter: Arc<RateLimiter>,
) -> Result<ProviderCall> {
    if cancel.is_cancelled() {
        return Err(LocustError::ProviderError("cancelled".into()));
    }
    // Queueing for the first allowance is not HTTP latency. Slow provider
    // quotas may legitimately require more than the request deadline.
    tokio::select! {
        biased;
        _ = cancel.cancelled() => return Err(LocustError::ProviderError("cancelled".into())),
        _ = limiter.acquire() => {}
    }
    let mut cfg = retry.clone();
    cfg.cancel = Some(cancel.clone());
    #[derive(Default)]
    struct State {
        pending: Vec<TranslationRequest>,
        results: Vec<TranslationResult>,
        receipts: Vec<TranslationResult>,
        attempts: usize,
        partial: bool,
        rejected_results: Vec<TranslationResult>,
        error: Option<String>,
    }
    let state = Arc::new(tokio::sync::Mutex::new(State {
        pending: requests.to_vec(),
        ..Default::default()
    }));
    let outcome = with_retry(&cfg, || {
        let provider = provider.clone();
        let limiter = limiter.clone();
        let state = state.clone();
        async move {
            let mut state = state.lock().await;
            let is_retry = state.attempts > 0;
            state.attempts += 1;
            // Every HTTP attempt consumes rate allowance, including retries.
            if is_retry {
                limiter.acquire().await;
            }
            tracing::debug!(
                provider = provider.id(),
                strings = state.pending.len(),
                "dispatching translation requests"
            );
            let pending = state.pending.clone();
            match provider
                .translate_partial(&pending, &mut state.receipts)
                .await
            {
                None if !state.partial && state.receipts.is_empty() => {
                    state.results = provider.translate(&pending).await?;
                    Ok(())
                }
                Some(failed)
                    if valid_partition(
                        &pending,
                        &state.receipts,
                        failed.iter().map(|(id, _)| id.clone()),
                    ) =>
                {
                    state.partial = true;
                    let receipts = std::mem::take(&mut state.receipts);
                    state.results.extend(receipts);
                    let retry_ids: std::collections::HashSet<_> = failed
                        .iter()
                        .filter(|(_, error)| is_retryable(error))
                        .map(|(id, _)| id.as_str())
                        .collect();
                    state
                        .pending
                        .retain(|r| retry_ids.contains(r.entry_id.as_str()));
                    match failed.into_iter().find(|(_, error)| is_retryable(error)) {
                        Some((_, error)) => Err(error),
                        None => Ok(()),
                    }
                }
                _ => {
                    // Invalid IDs prevent storage, not accounting for reported work.
                    state.partial = true;
                    let receipts = std::mem::take(&mut state.receipts);
                    state.rejected_results.extend(receipts);
                    let error =
                        "provider returned mismatched or duplicate entry IDs in partial outcome";
                    state.error = Some(error.into());
                    Err(LocustError::ProviderError(error.into()))
                }
            }
        }
    })
    .await;
    let mut state = state.lock().await;
    // The interrupted future may already have published complete subchunks.
    // Synthesize failures only on interruption; normal omissions remain invalid.
    if !state.receipts.is_empty() {
        let completed: std::collections::HashSet<_> =
            state.receipts.iter().map(|r| r.entry_id.as_str()).collect();
        let missing = state
            .pending
            .iter()
            .filter(|r| !completed.contains(r.entry_id.as_str()))
            .map(|r| r.entry_id.clone());
        if valid_partition(&state.pending, &state.receipts, missing) {
            state.partial = true;
            let receipts = std::mem::take(&mut state.receipts);
            state.results.extend(receipts);
        } else {
            state.partial = true;
            let receipts = std::mem::take(&mut state.receipts);
            state.rejected_results.extend(receipts);
            state.error = Some("provider returned mismatched or duplicate entry IDs in interrupted partial outcome".into());
        }
    }
    if !state.partial {
        outcome?;
    }
    let failed_ids = if state.partial {
        let completed: std::collections::HashSet<_> =
            state.results.iter().map(|r| r.entry_id.as_str()).collect();
        requests
            .iter()
            .filter(|r| !completed.contains(r.entry_id.as_str()))
            .map(|r| r.entry_id.clone())
            .collect()
    } else {
        Vec::new()
    };
    Ok(ProviderCall {
        results: std::mem::take(&mut state.results),
        attempts: state.attempts,
        failed_ids,
        partial: state.partial,
        rejected_results: std::mem::take(&mut state.rejected_results),
        error: state.error.take(),
    })
}

#[cfg(test)]
mod tests {
    use super::super::tests::MockProvider;
    use super::*;
    use std::sync::atomic::Ordering;

    #[test]
    fn invalid_request_status_is_not_a_retryable_body_number() {
        let error = LocustError::ProviderError(
            "OpenAI returned status 400 Bad Request: max_tokens must be <= 500".into(),
        );
        assert!(!is_retryable(&error));
    }

    #[test]
    fn transient_status_is_not_overridden_by_an_auth_number_in_the_body() {
        let error = LocustError::ProviderError(
            "OpenAI returned status 503 Service Unavailable: request_id=401abc".into(),
        );
        assert!(is_retryable(&error));
    }

    #[test]
    fn status_envelope_classifies_every_http_status_independently_of_the_body() {
        for status in 100..=599 {
            let expected = matches!(status, 408 | 425 | 429 | 500..=599);
            for body in [
                "max_tokens must be <= 500; timeout; connection; network; dns; rate limit",
                "request_id=401abc; 403 unauthorized forbidden cancelled; returned status 400",
            ] {
                let message = format!("Provider returned status {status}: {body}");
                assert_eq!(
                    is_retryable(&LocustError::ProviderError(message.clone())),
                    expected,
                    "{message}"
                );
            }
        }
    }

    #[test]
    fn authentication_statuses_do_not_retry_real_error_bodies() {
        for message in [
            r#"OpenAI returned status 401 Unauthorized: {"error":{"message":"Incorrect API key provided","code":"invalid_api_key"},"request_id":"500abc"}"#,
            r#"Claude returned status 403 Forbidden: {"error":{"type":"permission_error","message":"Your API key does not have permission; rate limit 429"}}"#,
        ] {
            assert!(
                !is_retryable(&LocustError::ProviderError(message.into())),
                "{message}"
            );
        }
    }

    #[test]
    fn health_check_status_uses_the_same_policy() {
        for status in 100..=599 {
            let expected = matches!(status, 408 | 425 | 429 | 500..=599);
            for suffix in ["", ": timeout 500; request_id=401abc; cancelled"] {
                let message = format!("OpenAI health check returned status {status}{suffix}");
                assert_eq!(
                    is_retryable(&LocustError::ProviderError(message.clone())),
                    expected,
                    "{message}"
                );
            }
        }
    }

    #[test]
    fn first_status_envelope_wins() {
        for (message, expected) in [
            (
                "Provider returned status 400: upstream returned status 503",
                false,
            ),
            (
                "Provider returned status 503: upstream returned status 401",
                true,
            ),
            ("PROVIDER RETURNED STATUS 503: UNAUTHORIZED CANCELLED", true),
            ("timeout 503: Provider returned status 400", false),
        ] {
            assert_eq!(
                is_retryable(&LocustError::ProviderError(message.into())),
                expected,
                "{message}"
            );
        }
    }

    #[test]
    fn status_code_must_immediately_follow_the_envelope_marker() {
        for message in [
            "Provider returned status unavailable 400: timeout",
            "Provider returned status  400: timeout",
            "Provider returned status 40: timeout",
        ] {
            assert!(
                is_retryable(&LocustError::ProviderError(message.into())),
                "{message}"
            );
        }
    }

    #[test]
    fn free_text_signals_keep_their_existing_priority() {
        for (message, expected) in [
            ("request TIMEOUT", true),
            ("request timed out", true),
            ("connection reset", true),
            ("network unavailable", true),
            ("DNS lookup failed", true),
            ("rate limit exceeded", true),
            ("cancelled", false),
            ("cancelled after timeout", false),
            ("unauthorized network request", false),
            ("forbidden: rate limit exceeded", false),
            ("400 request timeout", true),
            ("malformed response", false),
        ] {
            assert_eq!(
                is_retryable(&LocustError::ProviderError(message.into())),
                expected,
                "{message}"
            );
        }
    }

    #[test]
    fn free_text_status_numbers_must_be_whole_tokens() {
        for (message, expected) in [
            ("429 Too Many Requests", true),
            ("500 Internal Server Error", true),
            ("HTTP (502): Bad Gateway", true),
            ("503 Service Unavailable", true),
            ("504 Gateway Timeout", true),
            ("401: network failure", false),
            ("HTTP (403): timeout", false),
            ("401 Unauthorized: timeout 503", false),
            ("403 Forbidden: network 502", false),
            ("request_id=401abc: connection reset", true),
            ("request_id=403abc: 503 Service Unavailable", true),
            ("request_id=503abc", false),
            ("request_id=15003", false),
            ("request_503_id", false),
            ("request-503-id", false),
        ] {
            assert_eq!(
                is_retryable(&LocustError::ProviderError(message.into())),
                expected,
                "{message}"
            );
        }
    }

    #[test]
    fn free_text_numeric_limits_are_not_status_codes() {
        for comparison in ["<=", ">=", "<", ">", "=", "==", "!="] {
            for spacing in ["", " "] {
                let message = format!("max_tokens must be {comparison}{spacing}500");
                assert!(
                    !is_retryable(&LocustError::ProviderError(message.clone())),
                    "{message}"
                );
                let message = format!("request_id {comparison}{spacing}401: network unavailable");
                assert!(
                    is_retryable(&LocustError::ProviderError(message.clone())),
                    "{message}"
                );
            }
        }
    }

    #[test]
    fn io_errors_remain_retryable() {
        assert!(is_retryable(&LocustError::IoError(std::io::Error::other(
            "cancelled: invalid value 401"
        ))));
    }

    #[tokio::test(start_paused = true)]
    async fn retry_attempts_follow_the_envelope_status() {
        for (message, expected_attempts) in [
            (
                "OpenAI returned status 400 Bad Request: max_tokens must be <= 500",
                1,
            ),
            (
                "OpenAI returned status 503 Service Unavailable: request_id=401abc",
                3,
            ),
        ] {
            let attempts = std::sync::atomic::AtomicU32::new(0);
            let result: Result<()> = with_retry(&RetryConfig::default(), || {
                attempts.fetch_add(1, Ordering::SeqCst);
                async { Err(LocustError::ProviderError(message.into())) }
            })
            .await;
            assert!(result.is_err());
            assert_eq!(
                attempts.load(Ordering::SeqCst),
                expected_attempts,
                "{message}"
            );
        }
    }

    #[tokio::test(start_paused = true)]
    async fn initial_rate_limit_queue_does_not_consume_http_deadline() {
        let provider = Arc::new(MockProvider::new());
        let limiter = Arc::new(RateLimiter::new(1));
        let request = TranslationRequest {
            entry_id: "rate".into(),
            source: "Hello".into(),
            source_lang: "en".into(),
            target_lang: "es".into(),
            context: None,
            glossary_hint: None,
        };
        let cancel = CancellationToken::new();
        for _ in 0..2 {
            call_provider_with_limiter(
                provider.clone(),
                std::slice::from_ref(&request),
                &RetryConfig::default(),
                &cancel,
                limiter.clone(),
            )
            .await
            .unwrap();
        }
        assert_eq!(provider.call_count.load(Ordering::SeqCst), 2);
    }
}
