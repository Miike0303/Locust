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

pub fn is_retryable(e: &LocustError) -> bool {
    match e {
        LocustError::ProviderError(msg) => {
            let lower = msg.to_ascii_lowercase();
            if lower.contains("cancelled")
                || lower.contains("401")
                || lower.contains("403")
                || lower.contains("unauthorized")
                || lower.contains("forbidden")
            {
                return false;
            }
            lower.contains("429")
                || lower.contains("rate limit")
                || lower.contains("timeout")
                || lower.contains("timed out")
                || lower.contains("500")
                || lower.contains("502")
                || lower.contains("503")
                || lower.contains("504")
                || lower.contains("connection")
                || lower.contains("network")
                || lower.contains("dns")
        }
        LocustError::IoError(_) => true,
        _ => false,
    }
}

pub(super) struct ProviderCall {
    pub(super) results: Vec<TranslationResult>,
    pub(super) attempts: usize,
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
    let attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let provider = provider.clone();
    let requests = requests.to_vec();
    let attempts_for_call = attempts.clone();
    let results = with_retry(&cfg, move || {
        let provider = provider.clone();
        let requests = requests.clone();
        let limiter = limiter.clone();
        let is_retry = attempts_for_call.fetch_add(1, std::sync::atomic::Ordering::Relaxed) > 0;
        async move {
            // Every HTTP attempt consumes rate allowance, including retries.
            if is_retry {
                limiter.acquire().await;
            }
            tracing::debug!(
                provider = provider.id(),
                strings = requests.len(),
                "dispatching translation requests"
            );
            provider.translate(&requests).await
        }
    })
    .await?;
    Ok(ProviderCall {
        results,
        attempts: attempts.load(std::sync::atomic::Ordering::Relaxed),
    })
}

#[cfg(test)]
mod tests {
    use super::super::tests::MockProvider;
    use super::*;
    use std::sync::atomic::Ordering;

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
