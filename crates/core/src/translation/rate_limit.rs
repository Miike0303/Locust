use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

pub struct RateLimiter {
    requests_per_minute: u32,
    last_requests: Mutex<VecDeque<tokio::time::Instant>>,
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

    pub fn requests_per_minute(&self) -> u32 {
        self.requests_per_minute
    }

    pub fn is_unlimited(&self) -> bool {
        self.requests_per_minute == u32::MAX
    }

    pub async fn acquire(&self) {
        loop {
            let should_wait = {
                let mut requests = self.last_requests.lock().unwrap_or_else(|e| e.into_inner());
                let now = tokio::time::Instant::now();
                let window = Duration::from_secs(60);

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

/// Built-in requests/minute when `ProviderConfig.extra` has no override.
/// Local engines stay unlimited; the unofficial Google web endpoint is tight.
pub fn default_requests_per_minute(provider_id: &str) -> u32 {
    match provider_id {
        "ollama" | "lmstudio" | "argos" | "mock" => u32::MAX,
        // Free translate.googleapis.com endpoint — conservative to avoid blocks.
        "google" => 8,
        "deepl" => 120,
        "antigravity" => 60,
        "openai" | "claude" | "grok" | "grok-sub" | "deepseek" | "gemini" => 90,
        _ => 60,
    }
}

fn rpm_from_provider_extra(extra: &HashMap<String, String>) -> Option<u32> {
    extra
        .get("requests_per_minute")
        .and_then(|s| s.parse().ok())
        .filter(|&n| n > 0)
}

/// Resolve rpm from existing `ProviderConfig.extra["requests_per_minute"]`,
/// else the built-in table. No new config field.
pub fn requests_per_minute_for(provider_id: &str, extra: Option<&HashMap<String, String>>) -> u32 {
    extra
        .and_then(rpm_from_provider_extra)
        .unwrap_or_else(|| default_requests_per_minute(provider_id))
}

/// Per-provider-id limiter so one chatty engine does not stall the others.
pub fn rate_limiter_for(provider_id: &str) -> Arc<RateLimiter> {
    static LIMITERS: OnceLock<Mutex<HashMap<String, Arc<RateLimiter>>>> = OnceLock::new();
    let map = LIMITERS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = map.lock().unwrap_or_else(|e| e.into_inner());
    guard
        .entry(provider_id.to_string())
        .or_insert_with(|| {
            let extra = crate::config::AppConfig::load(&crate::config::AppConfig::default_path())
                .ok()
                .and_then(|cfg| cfg.get_provider_config(provider_id).cloned())
                .map(|pc| pc.extra);
            let rpm = requests_per_minute_for(provider_id, extra.as_ref());
            Arc::new(RateLimiter::new(rpm))
        })
        .clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::Instant;

    #[tokio::test]
    async fn rate_limiter_acquire_recovers_poisoned_mutex() {
        let limiter = Arc::new(RateLimiter::new(2));
        limiter.acquire().await;
        let poisoned = limiter.clone();
        assert!(std::thread::spawn(move || {
            let _guard = poisoned.last_requests.lock().unwrap();
            panic!("poison the request window");
        })
        .join()
        .is_err());
        assert!(limiter.last_requests.is_poisoned());

        limiter.acquire().await;
        assert_eq!(
            limiter
                .last_requests
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .len(),
            2
        );
    }

    #[test]
    fn google_rate_limit_is_finite_and_local_is_unlimited() {
        let google = rate_limiter_for("google");
        assert!(
            google.requests_per_minute() < u32::MAX,
            "google must have a real rpm, got {}",
            google.requests_per_minute()
        );
        assert!(google.requests_per_minute() > 0);
        assert!(!google.is_unlimited());

        for id in ["ollama", "lmstudio", "argos", "mock"] {
            let lim = rate_limiter_for(id);
            assert!(
                lim.is_unlimited(),
                "{id} should be unlimited, got {}",
                lim.requests_per_minute()
            );
        }
    }

    #[test]
    fn extra_requests_per_minute_overrides_table() {
        let mut extra = HashMap::new();
        extra.insert("requests_per_minute".into(), "12".into());
        assert_eq!(requests_per_minute_for("google", Some(&extra)), 12);
        assert_eq!(requests_per_minute_for("ollama", None), u32::MAX);
        extra.insert("requests_per_minute".into(), "0".into());
        assert_eq!(
            requests_per_minute_for("google", Some(&extra)),
            default_requests_per_minute("google")
        );
    }

    #[tokio::test(start_paused = true)]
    async fn local_rate_limiter_never_waits() {
        let limiter = rate_limiter_for("mock");
        let start = Instant::now();
        for _ in 0..40 {
            limiter.acquire().await;
        }
        assert_eq!(
            start.elapsed(),
            Duration::ZERO,
            "local/unlimited limiter must not sleep"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn google_default_rpm_throttles_on_fresh_limiter() {
        let rpm = default_requests_per_minute("google");
        assert!(rpm < u32::MAX);
        let limiter = RateLimiter::new(rpm);
        for _ in 0..rpm {
            limiter.acquire().await;
        }
        let start = Instant::now();
        limiter.acquire().await;
        let waited = start.elapsed();
        assert!(
            waited >= Duration::from_secs(60),
            "google default must wait out the window, waited {waited:?}"
        );
        assert!(
            waited < Duration::from_secs(61),
            "wait must be the window, waited {waited:?}"
        );
    }
}
