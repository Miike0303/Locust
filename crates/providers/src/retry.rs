use std::collections::VecDeque;
use std::future::Future;
use std::sync::Mutex;
use std::time::Duration;
// tokio's Instant, not std's: it is virtualized under `tokio::time::pause()`, which
// lets the rate-limiter tests assert on the limiter's own logic instead of on how
// loaded the host happens to be. Outside a paused runtime it reads the real clock,
// so production behavior is unchanged.
use tokio::time::Instant;

use locust_core::error::{LocustError, Result};

#[derive(Clone, Debug)]
pub struct RetryConfig {
    pub max_attempts: u32,
    pub initial_delay_ms: u64,
    pub max_delay_ms: u64,
    pub backoff_multiplier: f64,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            initial_delay_ms: 1000,
            max_delay_ms: 30000,
            backoff_multiplier: 2.0,
        }
    }
}

pub async fn with_retry<F, Fut, T>(config: &RetryConfig, operation: F) -> Result<T>
where
    F: Fn() -> Fut,
    Fut: Future<Output = Result<T>>,
{
    let mut last_err = None;

    for attempt in 0..config.max_attempts {
        match operation().await {
            Ok(v) => return Ok(v),
            Err(e) => {
                if is_retryable(&e) && attempt < config.max_attempts - 1 {
                    let delay_ms = compute_delay(config, attempt);
                    tracing::warn!(
                        "Retryable error on attempt {}/{}: {}. Retrying in {}ms",
                        attempt + 1,
                        config.max_attempts,
                        e,
                        delay_ms
                    );
                    tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                    last_err = Some(e);
                } else {
                    return Err(e);
                }
            }
        }
    }

    Err(last_err.unwrap_or_else(|| {
        LocustError::ProviderError("retry exhausted with no error".to_string())
    }))
}

fn compute_delay(config: &RetryConfig, attempt: u32) -> u64 {
    let delay = config.initial_delay_ms as f64 * config.backoff_multiplier.powi(attempt as i32);
    (delay as u64).min(config.max_delay_ms)
}

pub fn is_retryable(e: &LocustError) -> bool {
    match e {
        LocustError::ProviderError(msg) => {
            msg.contains("429")
                || msg.contains("rate limit")
                || msg.contains("503")
                || msg.contains("502")
                || msg.contains("timeout")
        }
        LocustError::IoError(_) => true,
        _ => false,
    }
}

pub struct RateLimiter {
    requests_per_minute: u32,
    last_requests: Mutex<VecDeque<Instant>>,
}

impl RateLimiter {
    pub fn new(requests_per_minute: u32) -> Self {
        Self {
            requests_per_minute,
            last_requests: Mutex::new(VecDeque::new()),
        }
    }

    pub fn unlimited() -> Self {
        Self::new(u32::MAX)
    }

    pub async fn acquire(&self) {
        loop {
            let should_wait = {
                let mut requests = self.last_requests.lock().unwrap();
                let now = Instant::now();
                let window = Duration::from_secs(60);

                // Remove requests outside the window
                while let Some(&front) = requests.front() {
                    if now.duration_since(front) > window {
                        requests.pop_front();
                    } else {
                        break;
                    }
                }

                if (requests.len() as u32) < self.requests_per_minute {
                    requests.push_back(now);
                    None
                } else {
                    // Wait until oldest request falls outside window
                    requests.front().map(|&oldest| {
                        let elapsed = now.duration_since(oldest);
                        if elapsed < window {
                            window - elapsed + Duration::from_millis(10)
                        } else {
                            Duration::from_millis(10)
                        }
                    })
                }
            };

            match should_wait {
                None => return,
                Some(dur) => tokio::time::sleep(dur).await,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    #[tokio::test]
    async fn test_retry_succeeds_first_attempt() {
        let call_count = Arc::new(AtomicU32::new(0));
        let cc = call_count.clone();

        let config = RetryConfig {
            max_attempts: 3,
            initial_delay_ms: 10,
            ..Default::default()
        };

        let result = with_retry(&config, || {
            let cc = cc.clone();
            async move {
                cc.fetch_add(1, Ordering::SeqCst);
                Ok::<_, LocustError>(42)
            }
        })
        .await;

        assert_eq!(result.unwrap(), 42);
        assert_eq!(call_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_retry_on_rate_limit() {
        let call_count = Arc::new(AtomicU32::new(0));
        let cc = call_count.clone();

        let config = RetryConfig {
            max_attempts: 3,
            initial_delay_ms: 10,
            max_delay_ms: 50,
            ..Default::default()
        };

        let result = with_retry(&config, || {
            let cc = cc.clone();
            async move {
                let n = cc.fetch_add(1, Ordering::SeqCst);
                if n < 2 {
                    Err(LocustError::ProviderError(
                        "429 Too Many Requests".to_string(),
                    ))
                } else {
                    Ok(42)
                }
            }
        })
        .await;

        assert_eq!(result.unwrap(), 42);
        assert_eq!(call_count.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn test_retry_non_retryable_fails_fast() {
        let call_count = Arc::new(AtomicU32::new(0));
        let cc = call_count.clone();

        let config = RetryConfig {
            max_attempts: 3,
            initial_delay_ms: 10,
            ..Default::default()
        };

        let result: Result<i32> = with_retry(&config, || {
            let cc = cc.clone();
            async move {
                cc.fetch_add(1, Ordering::SeqCst);
                Err(LocustError::ParseError {
                    file: "test".to_string(),
                    message: "bad parse".to_string(),
                })
            }
        })
        .await;

        assert!(result.is_err());
        assert_eq!(call_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_retry_exhausted_returns_last_error() {
        let call_count = Arc::new(AtomicU32::new(0));
        let cc = call_count.clone();

        let config = RetryConfig {
            max_attempts: 3,
            initial_delay_ms: 10,
            max_delay_ms: 20,
            ..Default::default()
        };

        let result: Result<i32> = with_retry(&config, || {
            let cc = cc.clone();
            async move {
                cc.fetch_add(1, Ordering::SeqCst);
                Err(LocustError::ProviderError("503 Service Unavailable".to_string()))
            }
        })
        .await;

        assert!(result.is_err());
        assert_eq!(call_count.load(Ordering::SeqCst), 3);
    }

    // Both rate-limiter tests run on a PAUSED clock and assert on VIRTUAL elapsed
    // time. Virtual time only moves when the limiter actually awaits a sleep, so
    // "did it throttle" becomes an exact question rather than a guess about wall
    // clock. These previously asserted on real elapsed time and failed under CPU
    // contention with nothing broken.

    #[tokio::test(start_paused = true)]
    async fn test_rate_limiter_allows_under_limit() {
        let limiter = RateLimiter::new(60);
        let start = Instant::now();
        for _ in 0..5 {
            limiter.acquire().await;
        }
        assert_eq!(
            start.elapsed(),
            Duration::ZERO,
            "5 requests against a 60/min limit must not wait at all"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn test_rate_limiter_throttles_over_limit() {
        let limiter = RateLimiter::new(3);
        for _ in 0..3 {
            limiter.acquire().await;
        }

        // The 4th request cannot proceed until the oldest of the 3 leaves the
        // 60s window, so the limiter must sleep out roughly the whole window.
        let start = Instant::now();
        limiter.acquire().await;
        let waited = start.elapsed();

        assert!(
            waited >= Duration::from_secs(60),
            "the 4th request against a 3/min limit must wait out the window, waited {waited:?}"
        );
        assert!(
            waited < Duration::from_secs(61),
            "the wait must be the window and not longer, waited {waited:?}"
        );
    }
}
