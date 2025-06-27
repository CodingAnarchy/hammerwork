use crate::error::HammerworkError;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::time::sleep;

/// Rate limiting configuration for job processing
#[derive(Debug, Clone)]
pub struct RateLimit {
    /// Maximum number of operations per time window
    pub rate: u64,
    /// Time window for the rate limit
    pub per: Duration,
    /// Maximum burst capacity (tokens that can be consumed at once)
    pub burst_limit: u64,
}

impl RateLimit {
    /// Create a rate limit of X operations per second
    pub fn per_second(rate: u64) -> Self {
        Self {
            rate,
            per: Duration::from_secs(1),
            burst_limit: rate,
        }
    }

    /// Create a rate limit of X operations per minute
    pub fn per_minute(rate: u64) -> Self {
        Self {
            rate,
            per: Duration::from_secs(60),
            burst_limit: rate,
        }
    }

    /// Create a rate limit of X operations per hour
    pub fn per_hour(rate: u64) -> Self {
        Self {
            rate,
            per: Duration::from_secs(3600),
            burst_limit: rate,
        }
    }

    /// Set the burst limit (maximum tokens that can be consumed at once)
    pub fn with_burst_limit(mut self, burst_limit: u64) -> Self {
        self.burst_limit = burst_limit;
        self
    }

    /// Calculate the refill rate in tokens per millisecond
    fn refill_rate_per_ms(&self) -> f64 {
        self.rate as f64 / self.per.as_millis() as f64
    }
}

/// Throttling configuration for queues
#[derive(Debug, Clone)]
pub struct ThrottleConfig {
    /// Maximum number of concurrent jobs for this queue
    pub max_concurrent: Option<u64>,
    /// Rate limit for this queue
    pub rate_per_minute: Option<u64>,
    /// Backoff duration when errors occur
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

    /// Set rate per minute
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

    /// Convert to RateLimit if rate_per_minute is set
    pub fn to_rate_limit(&self) -> Option<RateLimit> {
        self.rate_per_minute.map(RateLimit::per_minute)
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

    /// Refill tokens based on elapsed time
    fn refill(&mut self) {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_refill);
        let tokens_to_add = self.refill_rate * elapsed.as_millis() as f64;

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
#[derive(Debug)]
pub struct RateLimiter {
    bucket: Arc<Mutex<TokenBucket>>,
    rate_limit: RateLimit,
}

impl RateLimiter {
    /// Create a new rate limiter
    pub fn new(rate_limit: RateLimit) -> Self {
        let capacity = rate_limit.burst_limit as f64;
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

impl Clone for RateLimiter {
    fn clone(&self) -> Self {
        Self {
            bucket: Arc::clone(&self.bucket),
            rate_limit: self.rate_limit.clone(),
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
        let rate_limit = RateLimit::per_second(10);
        assert_eq!(rate_limit.rate, 10);
        assert_eq!(rate_limit.per, Duration::from_secs(1));
        assert_eq!(rate_limit.burst_limit, 10);

        let rate_limit = RateLimit::per_minute(60).with_burst_limit(100);
        assert_eq!(rate_limit.rate, 60);
        assert_eq!(rate_limit.per, Duration::from_secs(60));
        assert_eq!(rate_limit.burst_limit, 100);
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

        let rate_limit = config.to_rate_limit().unwrap();
        assert_eq!(rate_limit.rate, 100);
        assert_eq!(rate_limit.per, Duration::from_secs(60));
    }

    #[test]
    fn test_token_bucket() {
        let mut bucket = TokenBucket::new(10.0, 1.0); // 10 tokens, 1 token per ms

        // Should start with full capacity
        assert_eq!(bucket.available_tokens(), 10.0);

        // Should be able to consume tokens
        assert!(bucket.try_consume(5.0));
        assert_eq!(bucket.available_tokens(), 5.0);

        // Should not be able to consume more than available
        assert!(!bucket.try_consume(10.0));
        assert_eq!(bucket.available_tokens(), 5.0);

        // Should be able to consume remaining tokens
        assert!(bucket.try_consume(5.0));
        assert_eq!(bucket.available_tokens(), 0.0);
    }

    #[tokio::test]
    async fn test_token_bucket_refill() {
        let mut bucket = TokenBucket::new(10.0, 10.0); // 10 tokens capacity, 10 tokens per ms

        // Consume all tokens
        assert!(bucket.try_consume(10.0));
        assert_eq!(bucket.available_tokens(), 0.0);

        // Wait for refill (1ms should add 10 tokens, bringing us back to capacity)
        sleep(Duration::from_millis(2)).await;

        // Should have refilled to capacity
        let tokens = bucket.available_tokens();
        assert_eq!(tokens, 10.0);
    }

    #[test]
    fn test_rate_limiter_creation() {
        let rate_limit = RateLimit::per_second(10);
        let limiter = RateLimiter::new(rate_limit);

        assert_eq!(limiter.rate_limit().rate, 10);
        assert!(limiter.available_tokens() > 0.0);
    }

    #[tokio::test]
    async fn test_rate_limiter_check() {
        let rate_limit = RateLimit::per_second(2); // 2 operations per second
        let limiter = RateLimiter::new(rate_limit);

        // Should initially allow operations
        assert!(limiter.check());
        assert!(limiter.check());

        // Should reject after consuming burst
        assert!(!limiter.check());
    }

    #[tokio::test]
    async fn test_rate_limiter_acquire() {
        let rate_limit = RateLimit::per_second(1000); // High rate for fast test
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
        let rate_limit = RateLimit::per_second(1);
        let limiter = RateLimiter::new(rate_limit);

        // Should initially succeed
        assert!(limiter.try_acquire());

        // Should fail after consuming the token
        assert!(!limiter.try_acquire());
    }

    #[test]
    fn test_rate_limit_refill_calculation() {
        let rate_limit = RateLimit::per_second(10);
        let refill_rate = rate_limit.refill_rate_per_ms();

        // 10 tokens per 1000ms = 0.01 tokens per ms
        assert!((refill_rate - 0.01).abs() < 0.001);

        let rate_limit = RateLimit::per_minute(60);
        let refill_rate = rate_limit.refill_rate_per_ms();

        // 60 tokens per 60000ms = 0.001 tokens per ms
        assert!((refill_rate - 0.001).abs() < 0.0001);
    }

    #[tokio::test]
    async fn test_rate_limiter_clone() {
        let rate_limit = RateLimit::per_second(2);
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
        // Zero rate should still work
        let rate_limit = RateLimit::per_second(0);
        assert_eq!(rate_limit.rate, 0);

        // Very high rate
        let rate_limit = RateLimit::per_second(1_000_000);
        assert_eq!(rate_limit.rate, 1_000_000);

        // Very long duration
        let rate_limit = RateLimit::per_hour(1);
        assert_eq!(rate_limit.per, Duration::from_secs(3600));
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
}
