//! Retry with exponential backoff and jitter for transient API failures
//! (429, 5xx, overloaded, network). A server `retry-after` takes precedence
//! over the computed delay; one longer than
//! [`RetryPolicy::max_retry_after`] ends retrying, so the caller gets the
//! error with its `retry_after` and can tell the user when to try again.

use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::AiError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryPolicy {
    /// Retries after the first attempt; 0 disables retrying.
    pub max_retries: u32,
    /// Delay before the first retry, before jitter.
    pub base_delay: Duration,
    /// Upper bound for the computed backoff delay.
    pub max_delay: Duration,
    /// Longest server-requested `retry-after` the client waits for.
    pub max_retry_after: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: 4,
            base_delay: Duration::from_millis(500),
            max_delay: Duration::from_secs(30),
            max_retry_after: Duration::from_secs(120),
        }
    }
}

impl RetryPolicy {
    /// Delay before retry number `retry` (0-based), or `None` to stop
    /// retrying.
    ///
    /// Without `retry_after`, this is `base * 2^retry` capped at
    /// `max_delay`, scaled into `[50%, 100%]` by `jitter` (a value in
    /// `[0, 1)`), so concurrent clients spread out. With `retry_after`, the
    /// server's value is used as is, or `None` when it exceeds
    /// `max_retry_after`.
    pub fn delay(
        &self,
        retry: u32,
        retry_after: Option<Duration>,
        jitter: f64,
    ) -> Option<Duration> {
        if let Some(wait) = retry_after {
            return (wait <= self.max_retry_after).then_some(wait);
        }
        let exp = self
            .base_delay
            .saturating_mul(2u32.saturating_pow(retry.min(31)))
            .min(self.max_delay);
        Some(exp.mul_f64(0.5 + 0.5 * jitter.clamp(0.0, 1.0)))
    }
}

/// Parse a `retry-after` header value in seconds (integer or decimal).
/// HTTP-date values are not used by the Anthropic API and yield `None`.
pub fn parse_retry_after(value: &str) -> Option<Duration> {
    let secs: f64 = value.trim().parse().ok()?;
    (secs.is_finite() && secs >= 0.0).then(|| Duration::from_secs_f64(secs))
}

/// Parse a `retry-after-ms` header value in milliseconds.
pub fn parse_retry_after_ms(value: &str) -> Option<Duration> {
    parse_retry_after(value).map(|d| d / 1000)
}

/// The server-requested wait, preferring `retry-after-ms` over
/// `retry-after` when both are present and valid.
pub fn retry_after_from_headers(
    retry_after_ms: Option<&str>,
    retry_after: Option<&str>,
) -> Option<Duration> {
    retry_after_ms
        .and_then(parse_retry_after_ms)
        .or_else(|| retry_after.and_then(parse_retry_after))
}

/// A jitter value in `[0, 1)`. Not cryptographic; only spreads retries.
pub fn jitter() -> f64 {
    static STATE: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let mut x = nanos ^ STATE.fetch_add(0x9e37_79b9_7f4a_7c15, Ordering::Relaxed);
    // splitmix64 finalizer
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^= x >> 31;
    (x >> 11) as f64 / (1u64 << 53) as f64
}

/// Run `op` until it succeeds, fails with a non-retryable error, or the
/// policy's retries are used up. Returns the last error in that case.
pub async fn with_retry<T, Op, Fut>(policy: RetryPolicy, mut op: Op) -> Result<T, AiError>
where
    Op: FnMut() -> Fut,
    Fut: Future<Output = Result<T, AiError>>,
{
    let mut retry = 0;
    loop {
        match op().await {
            Ok(value) => return Ok(value),
            Err(err) if err.is_retryable() && retry < policy.max_retries => {
                let Some(delay) = policy.delay(retry, err.retry_after(), jitter()) else {
                    return Err(err);
                };
                tokio::time::sleep(delay).await;
                retry += 1;
            }
            Err(err) => return Err(err),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU32;

    const MS: Duration = Duration::from_millis(1);

    fn policy() -> RetryPolicy {
        RetryPolicy {
            max_retries: 3,
            base_delay: 100 * MS,
            max_delay: 1000 * MS,
            max_retry_after: 5000 * MS,
        }
    }

    #[test]
    fn exponential_with_cap() {
        let p = policy();
        // jitter 1.0 clamps to the full delay.
        assert_eq!(p.delay(0, None, 1.0), Some(100 * MS));
        assert_eq!(p.delay(1, None, 1.0), Some(200 * MS));
        assert_eq!(p.delay(2, None, 1.0), Some(400 * MS));
        assert_eq!(p.delay(3, None, 1.0), Some(800 * MS));
        assert_eq!(p.delay(4, None, 1.0), Some(1000 * MS));
        assert_eq!(p.delay(40, None, 1.0), Some(1000 * MS));
    }

    #[test]
    fn jitter_scales_into_upper_half() {
        let p = policy();
        assert_eq!(p.delay(2, None, 0.0), Some(200 * MS));
        assert_eq!(p.delay(2, None, 0.5), Some(300 * MS));
        assert_eq!(p.delay(2, None, -3.0), Some(200 * MS));
        for _ in 0..1000 {
            let j = jitter();
            assert!((0.0..1.0).contains(&j));
            let d = p.delay(1, None, j).unwrap();
            assert!(d >= 100 * MS && d <= 200 * MS);
        }
    }

    #[test]
    fn retry_after_is_honoured_up_to_its_cap() {
        let p = policy();
        assert_eq!(p.delay(0, Some(700 * MS), 0.0), Some(700 * MS));
        assert_eq!(p.delay(3, Some(50 * MS), 0.0), Some(50 * MS));
        // Above max_delay but within max_retry_after: waited in full.
        assert_eq!(p.delay(0, Some(4000 * MS), 0.0), Some(4000 * MS));
        assert_eq!(p.delay(0, Some(5000 * MS), 0.0), Some(5000 * MS));
        // Past max_retry_after: stop retrying.
        assert_eq!(p.delay(0, Some(5001 * MS), 0.0), None);
        let default = RetryPolicy::default();
        assert_eq!(
            default.delay(0, Some(Duration::from_secs(60)), 0.0),
            Some(Duration::from_secs(60))
        );
        assert_eq!(default.delay(0, Some(Duration::from_secs(121)), 0.0), None);
    }

    #[test]
    fn prefers_retry_after_ms() {
        assert_eq!(parse_retry_after_ms("1500"), Some(1500 * MS));
        assert_eq!(
            retry_after_from_headers(Some("250"), Some("3")),
            Some(250 * MS)
        );
        assert_eq!(
            retry_after_from_headers(None, Some("3")),
            Some(Duration::from_secs(3))
        );
        assert_eq!(
            retry_after_from_headers(Some("soon"), Some("3")),
            Some(Duration::from_secs(3))
        );
        assert_eq!(retry_after_from_headers(None, None), None);
    }

    #[test]
    fn parses_retry_after() {
        assert_eq!(parse_retry_after("3"), Some(Duration::from_secs(3)));
        assert_eq!(parse_retry_after(" 1.5 "), Some(1500 * MS));
        assert_eq!(parse_retry_after("0"), Some(Duration::ZERO));
        assert_eq!(parse_retry_after("-1"), None);
        assert_eq!(parse_retry_after("Wed, 21 Oct 2026 07:28:00 GMT"), None);
        assert_eq!(parse_retry_after("inf"), None);
    }

    fn overloaded() -> AiError {
        AiError::Api {
            status: 529,
            kind: "overloaded_error".into(),
            message: "Overloaded".into(),
            retry_after: Some(MS),
        }
    }

    fn fast() -> RetryPolicy {
        RetryPolicy {
            max_retries: 3,
            base_delay: MS,
            max_delay: 2 * MS,
            max_retry_after: 2 * MS,
        }
    }

    #[tokio::test]
    async fn retries_transient_errors_until_success() {
        let calls = AtomicU32::new(0);
        let result = with_retry(fast(), || async {
            match calls.fetch_add(1, Ordering::SeqCst) {
                0 => Err(overloaded()),
                1 => Err(AiError::Network("reset".into())),
                _ => Ok("done"),
            }
        })
        .await;
        assert_eq!(result, Ok("done"));
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn gives_up_after_max_retries() {
        let calls = AtomicU32::new(0);
        let result: Result<(), _> = with_retry(fast(), || async {
            calls.fetch_add(1, Ordering::SeqCst);
            Err(overloaded())
        })
        .await;
        assert_eq!(result, Err(overloaded()));
        assert_eq!(calls.load(Ordering::SeqCst), 4);
    }

    #[tokio::test]
    async fn returns_long_retry_after_without_waiting() {
        let calls = AtomicU32::new(0);
        let limited = AiError::Api {
            status: 429,
            kind: "rate_limit_error".into(),
            message: "slow down".into(),
            retry_after: Some(Duration::from_secs(600)),
        };
        let started = std::time::Instant::now();
        let result: Result<(), _> = with_retry(fast(), || async {
            calls.fetch_add(1, Ordering::SeqCst);
            Err(limited.clone())
        })
        .await;
        assert_eq!(result, Err(limited.clone()));
        assert_eq!(
            result.unwrap_err().retry_after(),
            Some(Duration::from_secs(600))
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[tokio::test]
    async fn does_not_retry_permanent_errors() {
        let calls = AtomicU32::new(0);
        let result: Result<(), _> = with_retry(fast(), || async {
            calls.fetch_add(1, Ordering::SeqCst);
            Err(AiError::Api {
                status: 400,
                kind: "invalid_request_error".into(),
                message: "bad".into(),
                retry_after: None,
            })
        })
        .await;
        assert!(result.is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}
