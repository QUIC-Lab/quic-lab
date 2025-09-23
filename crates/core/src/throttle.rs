use governor::{DefaultDirectRateLimiter, Quota};
use std::num::NonZeroU32;
use std::sync::Arc;

/// Simple wrapper around governor's direct limiter.
/// `None` means throttling is disabled.
#[derive(Clone)]
pub struct RateLimit {
    inner: Option<Arc<DefaultDirectRateLimiter>>,
}

impl RateLimit {
    /// Disabled limiter (no throttling).
    pub fn disabled() -> Self {
        Self { inner: None }
    }

    /// Global, process-wide RPS limiter with a short burst.
    /// If `rps == 0`, throttling is disabled.
    pub fn per_second(rps: u32, burst: u32) -> Self {
        if rps == 0 {
            return Self::disabled();
        }
        // Minimum burst of 1 to avoid zero-burst edge cases.
        let burst = burst.max(1);

        let quota = Quota::per_second(NonZeroU32::new(rps).unwrap())
            .allow_burst(NonZeroU32::new(burst).unwrap());
        let lim = DefaultDirectRateLimiter::direct(quota);

        Self {
            inner: Some(Arc::new(lim)),
        }
    }

    /// Block until a token is available (before each network attempt).
    pub fn until_ready(&self) {
        if let Some(lim) = &self.inner {
            let _ = lim.until_ready();
        }
    }
}
