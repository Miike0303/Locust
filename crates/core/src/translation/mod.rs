mod batching;
mod manager;
mod protection;
mod provider;
mod rate_limit;
mod retry;

pub use manager::{
    load_pending_entries, run_fallback_chain, unique_provider_chain, TranslationManager,
};
pub use protection::collapse_runaway_letters;
pub use provider::{
    LangPair, ProviderInfo, ProviderRegistry, TranslationOptions, TranslationOptionsRequest,
    TranslationProvider,
};
pub use rate_limit::{
    default_requests_per_minute, rate_limiter_for, requests_per_minute_for, GoogleRateStats,
    RateLimiter, GOOGLE_INITIAL_RPM, GOOGLE_MAX_RPM, GOOGLE_MIN_RPM,
};
pub use retry::{is_retryable, with_retry, RetryConfig};

#[cfg(test)]
mod tests;
