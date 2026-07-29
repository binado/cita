//! Proactive pacing, on an injected clock.

use std::{
    collections::VecDeque,
    future::Future,
    pin::Pin,
    sync::Mutex,
    time::{Duration, Instant},
};

/// INSPIRE documents fifteen requests per IP in any five-second window.
const WINDOW: Duration = Duration::from_secs(5);

/// Twelve, not fifteen: rejected requests still count against the quota, so
/// sitting exactly on the documented limit converts a small clock skew into a
/// rejection that makes the next attempt likelier to fail too.
const PERMITS: usize = 12;

/// Time, as the transport sees it.
///
/// Pacing puts sleeps on the *success* path, so a suite that slept for real
/// would be unusable. Tests supply a clock that records the durations it was
/// asked to wait, advances a virtual instant, and returns immediately, and then
/// assert on the resulting schedule rather than on elapsed time.
pub trait Clock: Send + Sync + std::fmt::Debug {
    /// The current instant.
    fn now(&self) -> Instant;

    /// Wait for `duration`.
    fn sleep(&self, duration: Duration) -> Pin<Box<dyn Future<Output = ()> + Send + '_>>;
}

/// The real clock.
#[derive(Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }

    fn sleep(&self, duration: Duration) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(tokio::time::sleep(duration))
    }
}

/// A rolling-window token bucket shared by every request the client issues.
///
/// Batching does most of the work — a three-hundred-record project costs six
/// requests — so the bucket rarely binds. It is kept anyway: a rate limit that
/// is respected only by accident of request volume is not respected, and the
/// cases where it does bind (many batches, commands run back to back, a future
/// provider that batches less) are exactly the ones where being rejected would
/// cost quota and cascade.
#[derive(Debug)]
pub struct RateLimiter {
    permits: usize,
    window: Duration,
    issued: Mutex<VecDeque<Instant>>,
}

impl Default for RateLimiter {
    fn default() -> Self {
        Self::new(PERMITS, WINDOW)
    }
}

impl RateLimiter {
    /// Build a limiter with an explicit budget.
    pub fn new(permits: usize, window: Duration) -> Self {
        Self {
            permits,
            window,
            issued: Mutex::new(VecDeque::new()),
        }
    }

    /// Wait until issuing one more request stays inside the budget.
    pub async fn acquire(&self, clock: &dyn Clock) {
        loop {
            let wait = {
                let now = clock.now();
                let mut issued = self.issued.lock().expect("rate limiter");
                while issued
                    .front()
                    .is_some_and(|at| now.duration_since(*at) >= self.window)
                {
                    issued.pop_front();
                }
                if issued.len() < self.permits {
                    issued.push_back(now);
                    return;
                }
                // The oldest request in the window decides when a permit frees.
                let oldest = *issued.front().expect("the window is full");
                self.window.saturating_sub(now.duration_since(oldest))
            };
            // The lock is released before awaiting: a guard held across an
            // await would serialize every request behind the waiting one.
            clock.sleep(wait.max(Duration::from_millis(1))).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TestClock;

    #[tokio::test]
    async fn a_burst_within_the_budget_never_waits() {
        let clock = TestClock::new();
        let limiter = RateLimiter::default();
        for _ in 0..PERMITS {
            limiter.acquire(&clock).await;
        }
        assert!(clock.waits().is_empty());
    }

    #[tokio::test]
    async fn exceeding_the_budget_waits_for_the_window_to_roll() {
        let clock = TestClock::new();
        let limiter = RateLimiter::default();
        for _ in 0..=PERMITS {
            limiter.acquire(&clock).await;
        }
        let waits = clock.waits();
        assert_eq!(waits.len(), 1, "one wait, not one per request");
        assert!(waits[0] <= WINDOW, "waited {:?}", waits[0]);
        // Sustained throughput stays under the documented limit.
        assert!(clock.elapsed() >= WINDOW.saturating_sub(Duration::from_millis(1)));
    }

    #[tokio::test]
    async fn permits_free_as_the_window_rolls_forward() {
        let clock = TestClock::new();
        let limiter = RateLimiter::new(2, Duration::from_secs(5));
        limiter.acquire(&clock).await;
        limiter.acquire(&clock).await;
        clock.advance(Duration::from_secs(6));
        // Both earlier requests have aged out, so neither of these waits.
        limiter.acquire(&clock).await;
        limiter.acquire(&clock).await;
        assert!(clock.waits().is_empty());
    }
}
