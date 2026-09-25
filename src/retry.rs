//! Retry configuration. Semantics match the Python SDK's `RetryPolicy` (Tenacity-based):
//! exponential backoff with subtractive jitter, `Retry-After`/`retry-after-ms` support, a max
//! retry count, and a total time budget that stops *before* a sleep that would exceed it.

use std::collections::BTreeSet;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use http::StatusCode;

use crate::error::{Error, Result};

/// Custom retry predicate, consulted in addition to the built-in rules.
pub type RetryPredicate = Arc<dyn Fn(&Error) -> bool + Send + Sync>;

/// How failed requests are retried.
///
/// Start from [`RetryPolicy::default`] or [`RetryPolicy::none`] and adjust with the builder methods
/// (or set the public fields directly).
#[derive(Clone)]
#[non_exhaustive]
pub struct RetryPolicy {
    /// Retries after the first attempt; `0` disables retries. Default `2`.
    pub max_retries: u32,
    /// First backoff delay, doubled per attempt up to `backoff_max`; zero disables backoff. Default 0.5s.
    pub backoff_initial: Duration,
    /// Maximum backoff delay; zero disables backoff. Default 5s.
    pub backoff_max: Duration,
    /// Fraction of each delay randomly subtracted, in `[0, 1]`. Default `0.25`.
    pub backoff_jitter: f64,
    /// Statuses that are retried. Default 408, 429 and 500–599 (which includes TypeSafe's 529).
    pub http_statuses: BTreeSet<StatusCode>,
    /// Honor `retry-after-ms` / `Retry-After`. Default `true`.
    pub respect_retry_after: bool,
    /// Retry [`Error::Connection`]. Default `true`.
    pub retry_connection_errors: bool,
    /// Retry [`Error::Timeout`]. Default `true`.
    pub retry_timeouts: bool,
    /// Extra predicate; returning `true` also triggers a retry.
    pub predicate: Option<RetryPredicate>,
    /// Total budget per SDK call including attempts and delays; `None` = unlimited. Default 30s.
    pub budget: Option<Duration>,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        let mut statuses: BTreeSet<StatusCode> = (500..600)
            .filter_map(|s| StatusCode::from_u16(s).ok())
            .collect();
        statuses.extend([StatusCode::REQUEST_TIMEOUT, StatusCode::TOO_MANY_REQUESTS]);
        Self {
            max_retries: 2,
            backoff_initial: Duration::from_millis(500),
            backoff_max: Duration::from_secs(5),
            backoff_jitter: 0.25,
            http_statuses: statuses,
            respect_retry_after: true,
            retry_connection_errors: true,
            retry_timeouts: true,
            predicate: None,
            budget: Some(Duration::from_secs(30)),
        }
    }
}

impl fmt::Debug for RetryPolicy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RetryPolicy")
            .field("max_retries", &self.max_retries)
            .field("backoff_initial", &self.backoff_initial)
            .field("backoff_max", &self.backoff_max)
            .field("backoff_jitter", &self.backoff_jitter)
            .field("http_statuses", &self.http_statuses)
            .field("respect_retry_after", &self.respect_retry_after)
            .field("retry_connection_errors", &self.retry_connection_errors)
            .field("retry_timeouts", &self.retry_timeouts)
            .field("predicate", &self.predicate.as_ref().map(|_| "<fn>"))
            .field("budget", &self.budget)
            .finish()
    }
}

impl RetryPolicy {
    /// A policy that never retries.
    pub fn none() -> Self {
        Self {
            max_retries: 0,
            ..Self::default()
        }
    }

    /// Set `max_retries`.
    #[must_use]
    pub fn max_retries(mut self, n: u32) -> Self {
        self.max_retries = n;
        self
    }

    /// Set the backoff bounds.
    #[must_use]
    pub fn backoff(mut self, initial: Duration, max: Duration) -> Self {
        self.backoff_initial = initial;
        self.backoff_max = max;
        self
    }

    /// Set the jitter fraction.
    #[must_use]
    pub fn jitter(mut self, fraction: f64) -> Self {
        self.backoff_jitter = fraction;
        self
    }

    /// Replace the retryable status set.
    ///
    /// ```
    /// use typesafe::{RetryPolicy, StatusCode};
    ///
    /// let policy = RetryPolicy::default().statuses([
    ///     StatusCode::TOO_MANY_REQUESTS,
    ///     StatusCode::SERVICE_UNAVAILABLE,
    /// ]);
    /// assert!(policy.http_statuses.contains(&StatusCode::SERVICE_UNAVAILABLE));
    /// ```
    #[must_use]
    pub fn statuses(mut self, statuses: impl IntoIterator<Item = StatusCode>) -> Self {
        self.http_statuses = statuses.into_iter().collect();
        self
    }

    /// Set the total time budget.
    #[must_use]
    pub fn budget(mut self, budget: Option<Duration>) -> Self {
        self.budget = budget;
        self
    }

    /// Whether to honor `retry-after-ms` / `Retry-After`.
    #[must_use]
    pub fn respect_retry_after(mut self, yes: bool) -> Self {
        self.respect_retry_after = yes;
        self
    }

    /// Whether to retry [`Error::Connection`].
    #[must_use]
    pub fn retry_connection_errors(mut self, yes: bool) -> Self {
        self.retry_connection_errors = yes;
        self
    }

    /// Whether to retry [`Error::Timeout`].
    #[must_use]
    pub fn retry_timeouts(mut self, yes: bool) -> Self {
        self.retry_timeouts = yes;
        self
    }

    /// Add a custom predicate.
    #[must_use]
    pub fn retry_if(mut self, f: impl Fn(&Error) -> bool + Send + Sync + 'static) -> Self {
        self.predicate = Some(Arc::new(f));
        self
    }

    pub(crate) fn validate(&self) -> Result<()> {
        if !(0.0..=1.0).contains(&self.backoff_jitter) {
            return Err(Error::config("backoff_jitter must be between zero and one"));
        }
        if self.budget == Some(Duration::ZERO) {
            return Err(Error::config("retry budget must be a positive duration"));
        }
        Ok(())
    }

    pub(crate) fn is_retryable(&self, err: &Error) -> bool {
        let builtin = match err {
            Error::Timeout(_) => self.retry_timeouts,
            Error::Connection(_) => self.retry_connection_errors,
            Error::Api(e) => self.http_statuses.contains(&e.status),
            _ => false,
        };
        builtin || self.predicate.as_ref().is_some_and(|p| p(err))
    }

    /// Delay before the next attempt; `attempt` is the 1-based number of the attempt that just failed.
    pub(crate) fn delay(&self, attempt: u32, err: &Error) -> Duration {
        if self.respect_retry_after
            && let Some(d) = err.as_api().and_then(|e| e.retry_after())
        {
            return d;
        }
        backoff(
            attempt,
            self.backoff_initial,
            self.backoff_max,
            self.backoff_jitter,
            rand::random::<f64>(),
        )
    }

    /// Whether to stop instead of sleeping `upcoming` after `attempts` attempts and `elapsed` time.
    pub(crate) fn should_stop(&self, attempts: u32, elapsed: Duration, upcoming: Duration) -> bool {
        attempts > self.max_retries
            || self
                .budget
                .is_some_and(|b| elapsed.saturating_add(upcoming) >= b)
    }
}

fn backoff(attempt: u32, initial: Duration, max: Duration, jitter: f64, r: f64) -> Duration {
    let (initial, max) = (initial.as_secs_f64(), max.as_secs_f64());
    if initial == 0.0 || max == 0.0 {
        return Duration::ZERO;
    }
    let exponent = attempt.saturating_sub(1) as f64;
    let exponential = if exponent >= max.log2() - initial.log2() {
        max
    } else {
        initial * 2f64.powf(exponent)
    };
    let delay = exponential * (1.0 - r * jitter);
    let rounded = (delay * 1000.0).round() / 1000.0;
    Duration::from_secs_f64(exponential.min(rounded))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_doubles_and_caps() {
        let (i, m) = (Duration::from_millis(500), Duration::from_secs(5));
        let d = |a| backoff(a, i, m, 0.25, 0.0);
        assert_eq!(d(1), Duration::from_millis(500));
        assert_eq!(d(2), Duration::from_secs(1));
        assert_eq!(d(4), Duration::from_secs(4));
        assert_eq!(d(5), Duration::from_secs(5));
        assert_eq!(d(40), Duration::from_secs(5));
        // full jitter subtracts up to 25%
        assert_eq!(backoff(1, i, m, 0.25, 1.0), Duration::from_millis(375));
        assert_eq!(backoff(1, Duration::ZERO, m, 0.25, 0.5), Duration::ZERO);
    }

    #[test]
    fn stop_rules() {
        let p = RetryPolicy::default();
        assert!(!p.should_stop(1, Duration::ZERO, Duration::from_secs(1)));
        assert!(!p.should_stop(2, Duration::ZERO, Duration::from_secs(1)));
        assert!(p.should_stop(3, Duration::ZERO, Duration::from_secs(1)));
        assert!(p.should_stop(1, Duration::from_secs(29), Duration::from_secs(1)));
        assert!(!RetryPolicy::default().budget(None).should_stop(
            1,
            Duration::from_secs(99),
            Duration::from_secs(1)
        ));
    }

    #[test]
    fn default_statuses_cover_529() {
        let p = RetryPolicy::default();
        for s in [408, 429, 500, 503, 529, 599] {
            assert!(p.http_statuses.contains(&StatusCode::from_u16(s).unwrap()));
        }
        assert!(!p.http_statuses.contains(&StatusCode::UNPROCESSABLE_ENTITY));
        assert!(RetryPolicy::default().jitter(1.5).validate().is_err());
    }
}
