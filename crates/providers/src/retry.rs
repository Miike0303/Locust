pub use locust_core::translation::{
    is_retryable, rate_limiter_for, with_retry, RateLimiter, RetryConfig,
};

#[cfg(test)]
mod tests {
    use super::*;
    use locust_core::error::{LocustError, Result};
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::time::Instant;

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
                Err(LocustError::ProviderError(
                    "503 Service Unavailable".to_string(),
                ))
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
