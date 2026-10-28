use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

pub struct RateLimiter {
    requests_per_minute: u32,
    last_requests: Mutex<VecDeque<tokio::time::Instant>>,
    google: Option<GoogleLimiter>,
}

/// A paced Google HTTP budget shared by every batch and retry in this process.
struct GoogleLimiter {
    state: Mutex<GoogleState>,
    queue: tokio::sync::Mutex<()>,
    changed: tokio::sync::Notify,
    adaptive: bool,
    stop_after_blocks: Option<u32>,
}

struct GoogleState {
    rpm: u32,
    next: tokio::time::Instant,
    cooldown: tokio::time::Instant,
    started: tokio::time::Instant,
    stats: GoogleRateStats,
}

#[derive(Clone, Debug, Default)]
pub struct GoogleRateStats {
    pub requests: u32,
    pub successes: u32,
    pub blocked: u32,
    pub http_429: u32,
    pub consecutive_blocks: u32,
    pub circuit_open: bool,
    pub first_429: Option<Duration>,
    pub first_block: Option<Duration>,
}

pub const GOOGLE_INITIAL_RPM: u32 = 60;
pub const GOOGLE_MAX_RPM: u32 = 140;
pub const GOOGLE_MIN_RPM: u32 = 8;
const GOOGLE_COOLDOWN: Duration = Duration::from_secs(30);

impl RateLimiter {
    pub fn new(requests_per_minute: u32) -> Self {
        Self {
            requests_per_minute,
            last_requests: Mutex::new(VecDeque::new()),
            google: None,
        }
    }

    /// `Some(rpm)` selects a fixed paced HTTP rate; `None` enables AIMD.
    /// The optional circuit breaker is used only by the bounded live probe.
    pub fn google(rpm: Option<u32>, stop_after_blocks: Option<u32>) -> Self {
        let now = tokio::time::Instant::now();
        let rpm_value = rpm.unwrap_or(GOOGLE_INITIAL_RPM).max(1);
        Self {
            requests_per_minute: rpm_value,
            last_requests: Mutex::new(VecDeque::new()),
            google: Some(GoogleLimiter {
                state: Mutex::new(GoogleState {
                    rpm: rpm_value,
                    next: now,
                    cooldown: now,
                    started: now,
                    stats: GoogleRateStats::default(),
                }),
                queue: tokio::sync::Mutex::new(()),
                changed: tokio::sync::Notify::new(),
                adaptive: rpm.is_none(),
                stop_after_blocks,
            }),
        }
    }

    pub fn is_adaptive(&self) -> bool {
        self.google.as_ref().is_some_and(|g| g.adaptive)
    }

    pub fn google_stats(&self) -> GoogleRateStats {
        self.google
            .as_ref()
            .map_or_else(GoogleRateStats::default, |g| {
                g.state
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .stats
                    .clone()
            })
    }

    /// Wake the live probe supervisor as soon as its circuit opens, even if
    /// provider retries or in-flight HTTP calls are still waiting.
    pub async fn probe_stopped(&self) {
        let Some(google) = &self.google else {
            return std::future::pending().await;
        };
        loop {
            let changed = google.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if self.google_stats().circuit_open {
                return;
            }
            changed.await;
        }
    }

    /// Google calls this immediately before *each* POST, including split fallbacks.
    /// Dropping a queued future releases its FIFO position without reserving time.
    /// False means the live probe's circuit breaker has opened.
    pub async fn acquire_http(&self) -> bool {
        if self.google.is_some() {
            self.acquire_google(true).await
        } else {
            self.acquire().await;
            true
        }
    }

    async fn acquire_google(&self, consume: bool) -> bool {
        let google = self.google.as_ref().expect("Google HTTP limiter");
        let _turn = google.queue.lock().await;
        loop {
            // Register before inspecting state: feedback cannot be lost between
            // computing the wake-up and starting the wait.
            let changed = google.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            let wake = {
                let mut state = google.state.lock().unwrap_or_else(|e| e.into_inner());
                if state.stats.circuit_open {
                    return false;
                }
                let now = tokio::time::Instant::now();
                let wake = state.next.max(state.cooldown);
                if now >= wake {
                    if consume {
                        state.next = now + Duration::from_secs_f64(60.0 / f64::from(state.rpm));
                        state.stats.requests += 1;
                    }
                    return true;
                }
                wake
            };
            tokio::select! {
                biased;
                _ = &mut changed => {},
                _ = tokio::time::sleep_until(wake) => {},
            }
        }
    }

    /// Report a validated response, or a 429/503/HTML traffic challenge.
    /// Fixed rates still observe server cooldowns, but never change their rpm.
    pub fn record_google_response(&self, status: u16, blocked: bool, retry_after: Option<&str>) {
        let Some(google) = &self.google else { return };
        let mut state = google.state.lock().unwrap_or_else(|e| e.into_inner());
        let now = tokio::time::Instant::now();
        if blocked {
            state.stats.blocked += 1;
            state.stats.consecutive_blocks += 1;
            state.stats.circuit_open |= google
                .stop_after_blocks
                .is_some_and(|n| state.stats.consecutive_blocks >= n);
            let elapsed = now.saturating_duration_since(state.started);
            state.stats.first_block.get_or_insert(elapsed);
            if status == 429 {
                state.stats.http_429 += 1;
                state.stats.first_429.get_or_insert(elapsed);
            }
            if google.adaptive {
                state.rpm = (state.rpm / 2).max(GOOGLE_MIN_RPM);
            }
            let multiplier = 1u32 << state.stats.consecutive_blocks.saturating_sub(1).min(3);
            let delay = retry_after
                .and_then(parse_retry_after)
                .unwrap_or_default()
                .max(GOOGLE_COOLDOWN * multiplier);
            state.cooldown = state.cooldown.max(now + delay);
        } else if (200..300).contains(&status) {
            state.stats.successes += 1;
            state.stats.consecutive_blocks = 0;
            // In-flight successes after a block must not undo its reduction.
            if now >= state.cooldown && google.adaptive {
                state.rpm = (state.rpm + 2).min(GOOGLE_MAX_RPM);
            }
        }
        drop(state);
        google.changed.notify_waiters();
    }

    pub fn unlimited() -> Self {
        Self::new(u32::MAX)
    }

    pub fn requests_per_minute(&self) -> u32 {
        self.google.as_ref().map_or(self.requests_per_minute, |g| {
            g.state.lock().unwrap_or_else(|e| e.into_inner()).rpm
        })
    }

    pub fn is_unlimited(&self) -> bool {
        self.requests_per_minute == u32::MAX
    }

    pub async fn acquire(&self) {
        if self.google.is_some() {
            // Core's first admission stays outside the HTTP deadline. The provider
            // consumes the actual HTTP allowance, so a batch is not charged twice.
            self.acquire_google(false).await;
            return;
        }
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

fn parse_retry_after(value: &str) -> Option<Duration> {
    if let Ok(seconds) = value.trim().parse::<u32>() {
        return Some(Duration::from_secs(u64::from(seconds)));
    }
    chrono::DateTime::parse_from_rfc2822(value.trim())
        .ok()
        .map(|date| {
            (date.with_timezone(&chrono::Utc) - chrono::Utc::now())
                .to_std()
                .unwrap_or_default()
        })
}

/// Built-in requests/minute when `ProviderConfig.extra` has no override.
/// Local engines stay unlimited; Google uses a separate paced HTTP budget.
pub fn default_requests_per_minute(provider_id: &str) -> u32 {
    match provider_id {
        "ollama" | "lmstudio" | "argos" | "mock" => u32::MAX,
        "google" => GOOGLE_INITIAL_RPM,
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

fn limiter_for_settings(provider_id: &str, extra: Option<&HashMap<String, String>>) -> RateLimiter {
    if provider_id == "google" {
        RateLimiter::google(extra.and_then(rpm_from_provider_extra), None)
    } else {
        RateLimiter::new(requests_per_minute_for(provider_id, extra))
    }
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
            Arc::new(limiter_for_settings(provider_id, extra.as_ref()))
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
        let google = limiter_for_settings("google", Some(&extra));
        assert_eq!(google.requests_per_minute(), 12);
        assert!(!google.is_adaptive());
        google.record_google_response(429, true, None);
        assert_eq!(google.requests_per_minute(), 12);
        assert!(limiter_for_settings("google", None).is_adaptive());
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

    #[tokio::test(start_paused = true)]
    async fn google_aimd_is_bounded_and_stale_success_does_not_undo_backoff() {
        let limiter = RateLimiter::google(None, None);
        assert_eq!(limiter.requests_per_minute(), GOOGLE_INITIAL_RPM);
        limiter.record_google_response(200, false, None);
        assert_eq!(limiter.requests_per_minute(), GOOGLE_INITIAL_RPM + 2);
        for _ in 0..200 {
            limiter.record_google_response(200, false, None);
        }
        assert_eq!(limiter.requests_per_minute(), GOOGLE_MAX_RPM);
        limiter.record_google_response(429, true, None);
        assert_eq!(limiter.requests_per_minute(), GOOGLE_MAX_RPM / 2);
        limiter.record_google_response(200, false, None);
        assert_eq!(limiter.requests_per_minute(), GOOGLE_MAX_RPM / 2);
        for _ in 0..10 {
            limiter.record_google_response(503, true, None);
        }
        assert_eq!(limiter.requests_per_minute(), GOOGLE_MIN_RPM);
        limiter.acquire_http().await;
        limiter.record_google_response(200, false, None);
        assert_eq!(limiter.requests_per_minute(), GOOGLE_MIN_RPM + 2);
    }

    #[tokio::test(start_paused = true)]
    async fn fixed_google_override_keeps_rate_and_honours_retry_after() {
        let limiter = RateLimiter::google(Some(123), None);
        limiter.record_google_response(200, false, None);
        limiter.record_google_response(429, true, Some("75"));
        assert!(!limiter.is_adaptive());
        assert_eq!(limiter.requests_per_minute(), 123);
        let start = Instant::now();
        assert!(limiter.acquire_http().await);
        assert_eq!(start.elapsed(), Duration::from_secs(75));
    }

    #[test]
    fn retry_after_parses_http_date_and_rejects_garbage() {
        let date = (chrono::Utc::now() + chrono::Duration::seconds(95))
            .format("%a, %d %b %Y %H:%M:%S GMT")
            .to_string();
        let delay = parse_retry_after(&date).unwrap();
        assert!(delay >= Duration::from_secs(93) && delay <= Duration::from_secs(95));
        assert_eq!(parse_retry_after("bogus"), None);
        assert_eq!(parse_retry_after("-10"), None);
        assert_eq!(parse_retry_after("0"), Some(Duration::ZERO));
    }

    #[tokio::test(start_paused = true)]
    async fn google_admission_does_not_double_charge_the_http_budget() {
        let limiter = RateLimiter::google(Some(60), None);
        limiter.acquire().await;
        limiter.acquire().await;
        assert_eq!(limiter.google_stats().requests, 0);
        let start = Instant::now();
        assert!(limiter.acquire_http().await);
        assert_eq!(start.elapsed(), Duration::ZERO);
        limiter.acquire().await;
        assert_eq!(start.elapsed(), Duration::from_secs(1));
        assert!(limiter.acquire_http().await);
        assert_eq!(limiter.google_stats().requests, 2);
    }

    #[tokio::test(start_paused = true)]
    async fn shared_google_queue_is_fifo_and_cancelled_head_reserves_nothing() {
        let limiter = Arc::new(RateLimiter::google(Some(60), None));
        limiter.record_google_response(429, true, Some("75"));
        let cancelled = {
            let limiter = limiter.clone();
            tokio::spawn(async move { limiter.acquire_http().await })
        };
        tokio::task::yield_now().await;
        let mut workers = Vec::new();
        for index in 0..16 {
            let limiter = limiter.clone();
            workers.push(tokio::spawn(async move {
                assert!(limiter.acquire_http().await);
                (index, Instant::now())
            }));
            tokio::task::yield_now().await;
        }
        cancelled.abort();
        assert!(cancelled.await.unwrap_err().is_cancelled());
        let start = Instant::now();
        for (expected, worker) in workers.into_iter().enumerate() {
            let (actual, time) = worker.await.unwrap();
            assert_eq!(actual, expected);
            assert_eq!(time - start, Duration::from_secs(75 + expected as u64));
        }
    }

    #[tokio::test(start_paused = true)]
    async fn queued_google_wait_observes_new_cooldown() {
        let limiter = Arc::new(RateLimiter::google(Some(60), None));
        limiter.acquire_http().await;
        let waiting = {
            let limiter = limiter.clone();
            tokio::spawn(async move { limiter.acquire_http().await })
        };
        tokio::task::yield_now().await;
        limiter.record_google_response(429, true, Some("90"));
        let start = Instant::now();
        assert!(waiting.await.unwrap());
        assert_eq!(start.elapsed(), Duration::from_secs(90));
    }

    #[tokio::test(start_paused = true)]
    async fn probe_circuit_is_latched_after_three_consecutive_blocks() {
        let limiter = RateLimiter::google(None, Some(3));
        for _ in 0..2 {
            limiter.record_google_response(429, true, None);
        }
        limiter.record_google_response(200, false, None);
        for _ in 0..2 {
            limiter.record_google_response(429, true, None);
        }
        assert!(!limiter.google_stats().circuit_open);
        limiter.record_google_response(503, true, None);
        limiter.record_google_response(200, false, None);
        assert!(limiter.google_stats().circuit_open);
        assert!(!limiter.acquire_http().await);
    }
}
