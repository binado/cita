//! The 429 fallback behind proactive pacing.

use std::time::{Duration, SystemTime};

/// Retries after the initial request.
pub const MAX_RETRIES: usize = 3;

/// INSPIRE counts rejected requests against the quota, so waiting less than the
/// window guarantees a second rejection that also costs quota. Honouring a
/// `Retry-After` below this floor is worse than ignoring it.
pub const MIN_RETRY_AFTER: Duration = Duration::from_secs(5);

/// A `Retry-After` far in the future is more likely a misconfiguration than an
/// instruction worth obeying; beyond a minute, bibi gives up instead.
pub const MAX_RETRY_AFTER: Duration = Duration::from_secs(60);

/// What happened before a retry, for the observer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RetryEvent {
    /// What was being requested.
    pub resource: String,
    /// How long bibi will wait.
    pub delay: Duration,
    /// Which retry this is, from one.
    pub attempt: usize,
    /// How many retries are allowed in total.
    pub max_retries: usize,
}

/// Called immediately before each retry. The binary writes to stderr; tests
/// record.
pub type RetryObserver = std::sync::Arc<dyn Fn(&RetryEvent) + Send + Sync>;

/// How long to wait after a 429, given the header if there was one.
///
/// The clamp is a correction rather than a refinement: the value is honoured
/// only within `[MIN_RETRY_AFTER, MAX_RETRY_AFTER]`, and an absent or
/// unparseable header falls back to the floor.
pub fn delay_after(header: Option<&str>) -> Duration {
    header
        .and_then(parse)
        .unwrap_or(MIN_RETRY_AFTER)
        .clamp(MIN_RETRY_AFTER, MAX_RETRY_AFTER)
}

/// `Retry-After` is either delta-seconds or an HTTP date.
fn parse(value: &str) -> Option<Duration> {
    if let Ok(seconds) = value.trim().parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }
    httpdate::parse_http_date(value)
        .ok()
        .map(|at| at.duration_since(SystemTime::now()).unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_delay_below_the_floor_is_raised_to_it() {
        // Obeying `Retry-After: 1` costs a rejection that also costs quota.
        assert_eq!(delay_after(Some("1")), MIN_RETRY_AFTER);
        assert_eq!(delay_after(Some("0")), MIN_RETRY_AFTER);
        assert_eq!(delay_after(None), MIN_RETRY_AFTER);
        assert_eq!(delay_after(Some("not a delay")), MIN_RETRY_AFTER);
    }

    #[test]
    fn a_delay_inside_the_range_is_honoured_and_a_long_one_is_capped() {
        assert_eq!(delay_after(Some("30")), Duration::from_secs(30));
        assert_eq!(delay_after(Some(" 7 ")), Duration::from_secs(7));
        assert_eq!(delay_after(Some("3600")), MAX_RETRY_AFTER);
    }

    #[test]
    fn an_http_date_is_read_as_a_delay_from_now() {
        let soon = httpdate::fmt_http_date(SystemTime::now() + Duration::from_secs(30));
        let delay = delay_after(Some(&soon));
        assert!(
            delay > Duration::from_secs(20) && delay <= Duration::from_secs(31),
            "{delay:?}"
        );
        // A date in the past means "now", which the floor then raises.
        let past = httpdate::fmt_http_date(SystemTime::now() - Duration::from_secs(300));
        assert_eq!(delay_after(Some(&past)), MIN_RETRY_AFTER);
    }
}
