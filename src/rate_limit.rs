use crate::error::HammerworkError;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::time::sleep;

// Serde helper functions for Duration
fn serialize_duration<S>(duration: &Option<Duration>, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    match duration {
        Some(d) => serializer.serialize_some(&d.as_secs()),
        None => serializer.serialize_none(),
    }
}

fn deserialize_duration<'de, D>(deserializer: D) -> Result<Option<Duration>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize;
    let secs: Option<u64> = Option::deserialize(deserializer)?;
    Ok(secs.map(Duration::from_secs))
}

/// Rate limiting configuration for job processing: at most `rate` operations per `per`,
/// with bursts of up to `burst_limit`.
///
/// Built with [`per_second`](Self::per_second), [`per_minute`](Self::per_minute) or
/// [`per_hour`](Self::per_hour), which reject a rate of zero: a limiter that never
/// refills would block every operation forever.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RateLimit {
    /// Maximum number of operations per time window (never zero)
    rate: u64,
    /// Time window for the rate limit
    per: Duration,
    /// Maximum burst capacity (tokens that can be consumed at once)
    burst_limit: u64,
}

impl RateLimit {
    fn try_new(rate: u64, per: Duration, unit: &str) -> crate::Result<Self> {
        if rate == 0 {
            return Err(HammerworkError::Config(format!(
                "rate limit must allow at least one operation per {unit} (got 0)"
            )));
        }
        Ok(Self {
            rate,
            per,
            burst_limit: rate,
        })
    }

    /// A rate limit of `rate` operations per second, with bursts of up to `rate`.
    ///
    /// Fails with [`HammerworkError::Config`] when `rate` is zero.
    ///
    /// ```
    /// use hammerwork::RateLimit;
    ///
    /// let limit = RateLimit::per_second(10)?.with_burst_limit(20)?;
    /// assert_eq!(limit.rate(), 10);
    /// assert!(RateLimit::per_second(0).is_err());
    /// # Ok::<(), hammerwork::HammerworkError>(())
    /// ```
    pub fn per_second(rate: u64) -> crate::Result<Self> {
        Self::try_new(rate, Duration::from_secs(1), "second")
    }

    /// A rate limit of `rate` operations per minute, with bursts of up to `rate`.
    ///
    /// Fails with [`HammerworkError::Config`] when `rate` is zero.
    pub fn per_minute(rate: u64) -> crate::Result<Self> {
        Self::try_new(rate, Duration::from_secs(60), "minute")
    }

    /// A rate limit of `rate` operations per hour, with bursts of up to `rate`.
    ///
    /// Fails with [`HammerworkError::Config`] when `rate` is zero.
    pub fn per_hour(rate: u64) -> crate::Result<Self> {
        Self::try_new(rate, Duration::from_secs(3600), "hour")
    }

    /// Set the burst limit (maximum tokens that can be consumed at once).
    ///
    /// Fails with [`HammerworkError::Config`] when `burst_limit` is zero: a bucket that
    /// can never hold a token would never let an operation through.
    pub fn with_burst_limit(mut self, burst_limit: u64) -> crate::Result<Self> {
        if burst_limit == 0 {
            return Err(HammerworkError::Config(
                "rate limit burst limit must be at least 1 (got 0)".to_string(),
            ));
        }
        self.burst_limit = burst_limit;
        Ok(self)
    }

    /// Maximum number of operations per time window; never zero.
    pub fn rate(&self) -> u64 {
        self.rate
    }

    /// The time window of the rate.
    pub fn per(&self) -> Duration {
        self.per
    }

    /// Maximum burst capacity.
    pub fn burst_limit(&self) -> u64 {
        self.burst_limit
    }

    /// Calculate the refill rate in tokens per millisecond
    fn refill_rate_per_ms(&self) -> f64 {
        self.rate as f64 / self.per.as_millis() as f64
    }
}

/// Throttling configuration for queues
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub struct ThrottleConfig {
    /// Maximum number of concurrent jobs for this queue
    pub max_concurrent: Option<u64>,
    /// Rate limit for this queue, in jobs per minute. Zero is invalid (see
    /// [`validate`](Self::validate)).
    pub rate_per_minute: Option<u64>,
    /// Backoff duration when errors occur (in seconds)
    #[serde(
        serialize_with = "serialize_duration",
        deserialize_with = "deserialize_duration",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub backoff_on_error: Option<Duration>,
    /// Enable/disable throttling
    pub enabled: bool,
}

impl ThrottleConfig {
    /// Create a new throttle configuration
    pub fn new() -> Self {
        Self {
            max_concurrent: None,
            rate_per_minute: None,
            backoff_on_error: None,
            enabled: true,
        }
    }

    /// Set maximum concurrent jobs
    pub fn max_concurrent(mut self, max: u64) -> Self {
        self.max_concurrent = Some(max);
        self
    }

    /// Set rate per minute.
    ///
    /// A rate of zero would never let a job through; [`validate`](Self::validate),
    /// [`to_rate_limit`](Self::to_rate_limit) and everything that applies a throttle
    /// ([`Worker::with_throttle_config`](crate::Worker::with_throttle_config),
    /// [`JobQueue::set_throttle`](crate::JobQueue::set_throttle), loading a
    /// configuration file) reject it.
    pub fn rate_per_minute(mut self, rate: u64) -> Self {
        self.rate_per_minute = Some(rate);
        self
    }

    /// Set backoff duration on errors
    pub fn backoff_on_error(mut self, duration: Duration) -> Self {
        self.backoff_on_error = Some(duration);
        self
    }

    /// Enable or disable throttling
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Reject a throttle that could never let a job through: an enabled throttle with
    /// `rate_per_minute` set to zero. A disabled throttle is ignored, so it is not
    /// checked. Fails with [`HammerworkError::Config`].
    pub fn validate(&self) -> crate::Result<()> {
        if self.enabled {
            self.to_rate_limit()?;
        }
        Ok(())
    }

    /// The [`RateLimit`] for `rate_per_minute`, or `None` when it is not set.
    ///
    /// Fails with [`HammerworkError::Config`] when `rate_per_minute` is zero.
    pub fn to_rate_limit(&self) -> crate::Result<Option<RateLimit>> {
        self.rate_per_minute.map(RateLimit::per_minute).transpose()
    }
}

impl Default for ThrottleConfig {
    fn default() -> Self {
        Self::new()
    }
}

/// Token bucket rate limiter implementation
#[derive(Debug)]
pub struct TokenBucket {
    /// Current number of tokens
    tokens: f64,
    /// Maximum capacity
    capacity: f64,
    /// Refill rate in tokens per millisecond
    refill_rate: f64,
    /// Last refill timestamp
    last_refill: Instant,
}

impl TokenBucket {
    /// Create a new token bucket
    pub fn new(capacity: f64, refill_rate: f64) -> Self {
        Self {
            tokens: capacity,
            capacity,
            refill_rate,
            last_refill: Instant::now(),
        }
    }

    /// Try to consume the specified number of tokens
    /// Returns true if successful, false if insufficient tokens
    pub fn try_consume(&mut self, tokens: f64) -> bool {
        self.refill();

        if self.tokens >= tokens {
            self.tokens -= tokens;
            true
        } else {
            false
        }
    }

    /// Return tokens taken by [`try_consume`](Self::try_consume) that were not used,
    /// up to the capacity.
    pub fn refund(&mut self, tokens: f64) {
        self.refill();
        self.tokens = (self.tokens + tokens).min(self.capacity);
    }

    /// Refill tokens based on elapsed time.
    ///
    /// Uses the elapsed time at full precision: rounding it down to whole milliseconds
    /// would drop the remainder on every call, so frequent calls would refill slower
    /// than the configured rate (or not at all).
    fn refill(&mut self) {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_refill);
        let tokens_to_add = self.refill_rate * elapsed.as_secs_f64() * 1000.0;

        self.tokens = (self.tokens + tokens_to_add).min(self.capacity);
        self.last_refill = now;
    }

    /// Get current token count
    pub fn available_tokens(&mut self) -> f64 {
        self.refill();
        self.tokens
    }

    /// Calculate time until the next token is available
    pub fn time_until_token(&mut self) -> Duration {
        self.refill();

        if self.tokens >= 1.0 {
            Duration::from_millis(0)
        } else {
            let tokens_needed = 1.0 - self.tokens;
            let ms_needed = (tokens_needed / self.refill_rate).ceil() as u64;
            Duration::from_millis(ms_needed)
        }
    }
}

/// Rate limiter that enforces rate limits using token bucket algorithm
#[derive(Debug, Clone)]
pub struct RateLimiter {
    bucket: Arc<Mutex<TokenBucket>>,
    rate_limit: RateLimit,
}

impl RateLimiter {
    /// Create a new rate limiter
    pub fn new(rate_limit: RateLimit) -> Self {
        // At least one: `with_burst_limit` rejects 0, and the default is the rate.
        let capacity = rate_limit.burst_limit as f64;
        // Never zero: the constructors of `RateLimit` reject a zero rate.
        let refill_rate = rate_limit.refill_rate_per_ms();

        Self {
            bucket: Arc::new(Mutex::new(TokenBucket::new(capacity, refill_rate))),
            rate_limit,
        }
    }

    /// Check if an operation is allowed (non-blocking)
    pub fn check(&self) -> bool {
        if let Ok(mut bucket) = self.bucket.lock() {
            bucket.try_consume(1.0)
        } else {
            // If lock is poisoned, allow the operation
            true
        }
    }

    /// Wait until an operation is allowed
    pub async fn acquire(&self) -> Result<(), HammerworkError> {
        loop {
            let wait_time = {
                if let Ok(mut bucket) = self.bucket.lock() {
                    if bucket.try_consume(1.0) {
                        return Ok(());
                    }
                    bucket.time_until_token()
                } else {
                    return Err(HammerworkError::RateLimit {
                        message: "Rate limiter lock poisoned".to_string(),
                    });
                }
            };

            if wait_time > Duration::from_millis(0) {
                sleep(wait_time).await;
            }
        }
    }

    /// Try to acquire a permit, returning immediately
    pub fn try_acquire(&self) -> bool {
        self.check()
    }

    /// Hand back a permit taken by [`check`](Self::check), [`acquire`](Self::acquire) or
    /// [`try_acquire`](Self::try_acquire) that was not used.
    ///
    /// Workers take a permit before they poll and refund it when the poll claims no job,
    /// so empty or paused polls do not spend the rate limit.
    pub fn refund(&self) {
        if let Ok(mut bucket) = self.bucket.lock() {
            bucket.refund(1.0);
        }
    }

    /// Get the current rate limit configuration
    pub fn rate_limit(&self) -> &RateLimit {
        &self.rate_limit
    }

    /// Get available tokens (for monitoring)
    pub fn available_tokens(&self) -> f64 {
        if let Ok(mut bucket) = self.bucket.lock() {
            bucket.available_tokens()
        } else {
            0.0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::time::{Instant, sleep};

    #[test]
    fn test_rate_limit_creation() {
        let rate_limit = RateLimit::per_second(10).unwrap();
        assert_eq!(rate_limit.rate(), 10);
        assert_eq!(rate_limit.per(), Duration::from_secs(1));
        assert_eq!(rate_limit.burst_limit(), 10);

        let rate_limit = RateLimit::per_minute(60)
            .unwrap()
            .with_burst_limit(100)
            .unwrap();
        assert_eq!(rate_limit.rate(), 60);
        assert_eq!(rate_limit.per(), Duration::from_secs(60));
        assert_eq!(rate_limit.burst_limit(), 100);
    }

    #[test]
    fn test_throttle_config() {
        let config = ThrottleConfig::new()
            .max_concurrent(5)
            .rate_per_minute(100)
            .backoff_on_error(Duration::from_secs(30));

        assert_eq!(config.max_concurrent, Some(5));
        assert_eq!(config.rate_per_minute, Some(100));
        assert_eq!(config.backoff_on_error, Some(Duration::from_secs(30)));
        assert!(config.enabled);

        let rate_limit = config.to_rate_limit().unwrap().unwrap();
        assert_eq!(rate_limit.rate(), 100);
        assert_eq!(rate_limit.per(), Duration::from_secs(60));
    }

    #[test]
    fn test_token_bucket() {
        // 10 tokens, refilling so slowly that the test never sees a refill
        let mut bucket = TokenBucket::new(10.0, 1e-9);
        let close = |a: f64, b: f64| (a - b).abs() < 1e-3;

        // Should start with full capacity
        assert!(close(bucket.available_tokens(), 10.0));

        // Should be able to consume tokens
        assert!(bucket.try_consume(5.0));
        assert!(close(bucket.available_tokens(), 5.0));

        // Should not be able to consume more than available
        assert!(!bucket.try_consume(10.0));
        assert!(close(bucket.available_tokens(), 5.0));

        // Should be able to consume remaining tokens
        assert!(bucket.try_consume(5.0));
        assert!(close(bucket.available_tokens(), 0.0));
    }

    #[test]
    fn test_token_bucket_refills_between_sub_millisecond_calls() {
        // 1 token per ms. Calls closer together than 1ms used to refill nothing (the
        // elapsed time was truncated to whole milliseconds and then discarded).
        let mut bucket = TokenBucket::new(5.0, 1.0);
        assert!(bucket.try_consume(5.0));
        let start = std::time::Instant::now();
        while start.elapsed() < Duration::from_millis(3) {
            bucket.available_tokens();
        }
        assert!(
            bucket.available_tokens() >= 2.5,
            "{}",
            bucket.available_tokens()
        );
    }

    #[test]
    fn test_refund_returns_an_unused_permit() {
        let limiter = RateLimiter::new(RateLimit::per_hour(1).unwrap());
        assert!(limiter.try_acquire());
        assert!(!limiter.try_acquire());
        // The permit was not used (an empty poll): hand it back.
        limiter.refund();
        assert!(limiter.try_acquire());
        // Refunds never raise the bucket above its capacity.
        limiter.refund();
        limiter.refund();
        assert!(limiter.available_tokens() <= 1.0);
    }

    #[tokio::test]
    async fn test_token_bucket_refill() {
        let mut bucket = TokenBucket::new(10.0, 10.0); // 10 tokens capacity, 10 tokens per ms

        // Consume all tokens (refill is continuous, so a sliver may already be back)
        assert!(bucket.try_consume(10.0));
        assert!(bucket.available_tokens() < 1.0);

        // Wait for refill (1ms should add 10 tokens, bringing us back to capacity)
        sleep(Duration::from_millis(2)).await;

        // Should have refilled to capacity
        let tokens = bucket.available_tokens();
        assert_eq!(tokens, 10.0);
    }

    #[test]
    fn test_rate_limiter_creation() {
        let rate_limit = RateLimit::per_second(10).unwrap();
        let limiter = RateLimiter::new(rate_limit);

        assert_eq!(limiter.rate_limit().rate(), 10);
        assert!(limiter.available_tokens() > 0.0);
    }

    #[tokio::test]
    async fn test_rate_limiter_check() {
        let rate_limit = RateLimit::per_second(2).unwrap(); // 2 operations per second
        let limiter = RateLimiter::new(rate_limit);

        // Should initially allow operations
        assert!(limiter.check());
        assert!(limiter.check());

        // Should reject after consuming burst
        assert!(!limiter.check());
    }

    #[tokio::test]
    async fn test_rate_limiter_acquire() {
        let rate_limit = RateLimit::per_second(1000).unwrap(); // High rate for fast test
        let limiter = RateLimiter::new(rate_limit);

        // Should acquire immediately
        let start = Instant::now();
        limiter.acquire().await.unwrap();
        let elapsed = start.elapsed();

        // Should be very fast (less than 10ms)
        assert!(elapsed < Duration::from_millis(10));
    }

    #[tokio::test]
    async fn test_rate_limiter_try_acquire() {
        let rate_limit = RateLimit::per_second(1).unwrap();
        let limiter = RateLimiter::new(rate_limit);

        // Should initially succeed
        assert!(limiter.try_acquire());

        // Should fail after consuming the token
        assert!(!limiter.try_acquire());
    }

    #[test]
    fn test_rate_limit_refill_calculation() {
        let rate_limit = RateLimit::per_second(10).unwrap();
        let refill_rate = rate_limit.refill_rate_per_ms();

        // 10 tokens per 1000ms = 0.01 tokens per ms
        assert!((refill_rate - 0.01).abs() < 0.001);

        let rate_limit = RateLimit::per_minute(60).unwrap();
        let refill_rate = rate_limit.refill_rate_per_ms();

        // 60 tokens per 60000ms = 0.001 tokens per ms
        assert!((refill_rate - 0.001).abs() < 0.0001);
    }

    #[tokio::test]
    async fn test_rate_limiter_clone() {
        let rate_limit = RateLimit::per_second(2).unwrap();
        let limiter1 = RateLimiter::new(rate_limit);
        let limiter2 = limiter1.clone();

        // Both limiters should share the same bucket
        assert!(limiter1.try_acquire());
        assert!(limiter2.try_acquire());

        // Should be exhausted for both
        assert!(!limiter1.try_acquire());
        assert!(!limiter2.try_acquire());
    }

    #[test]
    fn test_throttle_config_defaults() {
        let config = ThrottleConfig::default();
        assert!(config.enabled);
        assert!(config.max_concurrent.is_none());
        assert!(config.rate_per_minute.is_none());
        assert!(config.backoff_on_error.is_none());
    }

    #[test]
    fn test_rate_limit_edge_cases() {
        // Very high rate
        let rate_limit = RateLimit::per_second(1_000_000).unwrap();
        assert_eq!(rate_limit.rate(), 1_000_000);

        // Very long duration
        let rate_limit = RateLimit::per_hour(1).unwrap();
        assert_eq!(rate_limit.per(), Duration::from_secs(3600));
    }

    #[tokio::test]
    async fn test_token_bucket_time_until_token() {
        let mut bucket = TokenBucket::new(1.0, 1.0); // 1 token capacity, 1 token per ms

        // Should have tokens available initially
        assert_eq!(bucket.time_until_token(), Duration::from_millis(0));

        // Consume the token
        assert!(bucket.try_consume(1.0));

        // Should need to wait for next token
        let wait_time = bucket.time_until_token();
        assert!(wait_time > Duration::from_millis(0));
        assert!(wait_time <= Duration::from_millis(1));
    }

    /// A burst limit of 0 would make a bucket that can never hold a token, so
    /// `acquire` would wait forever: `with_burst_limit` rejects it (#130).
    #[test]
    fn test_zero_burst_limit_is_rejected() {
        let err = RateLimit::per_second(100)
            .unwrap()
            .with_burst_limit(0)
            .unwrap_err();
        assert!(matches!(err, HammerworkError::Config(_)), "{err}");
        let limit = RateLimit::per_second(100)
            .unwrap()
            .with_burst_limit(1)
            .unwrap();
        assert_eq!(limit.burst_limit(), 1);
    }

    /// A zero rate would never refill, so `acquire` would wait forever: every
    /// constructor rejects it, and so does a throttle with `rate_per_minute(0)`.
    #[test]
    fn test_zero_rate_is_rejected() {
        for result in [
            RateLimit::per_second(0),
            RateLimit::per_minute(0),
            RateLimit::per_hour(0),
        ] {
            match result {
                Err(HammerworkError::Config(message)) => {
                    assert!(message.contains("at least one operation"), "{message}")
                }
                other => panic!("expected a configuration error, got {other:?}"),
            }
        }

        let throttle = ThrottleConfig::new().rate_per_minute(0);
        assert!(matches!(
            throttle.to_rate_limit(),
            Err(HammerworkError::Config(_))
        ));
        assert!(matches!(
            throttle.validate(),
            Err(HammerworkError::Config(_))
        ));
        // A disabled throttle is ignored, so it is not rejected.
        assert!(throttle.clone().enabled(false).validate().is_ok());
        // Without a rate there is nothing to reject.
        assert!(ThrottleConfig::new().validate().is_ok());
        assert_eq!(ThrottleConfig::new().to_rate_limit().unwrap(), None);
        assert!(ThrottleConfig::new().rate_per_minute(1).validate().is_ok());
    }
}
