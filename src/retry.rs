//! Advanced retry strategies for job scheduling and backoff patterns.
//!
//! This module provides configurable retry strategies that determine how long to wait
//! before retrying a failed job. Different strategies are suitable for different types
//! of failures and system characteristics.
//!
//! # Retry Strategies
//!
//! - [`Fixed`](RetryStrategy::Fixed) - Constant delay between retries
//! - [`Linear`](RetryStrategy::Linear) - Linearly increasing delays
//! - [`Exponential`](RetryStrategy::Exponential) - Exponentially increasing delays with optional jitter
//! - [`Fibonacci`](RetryStrategy::Fibonacci) - Delays following the Fibonacci sequence
//! - [`Custom`](RetryStrategy::Custom) - User-defined retry logic
//!
//! # Examples
//!
//! ## Basic Usage
//!
//! ```rust
//! use hammerwork::{Job, retry::RetryStrategy};
//! use serde_json::json;
//! use std::time::Duration;
//!
//! // Exponential backoff with jitter
//! let job = Job::new("api_call".to_string(), json!({"url": "https://api.example.com"}))
//!     .with_exponential_backoff(
//!         Duration::from_secs(1),    // base delay
//!         2.0,                       // multiplier
//!         Duration::from_secs(10 * 60) // max delay
//!     );
//! ```
//!
//! ## Advanced Configuration
//!
//! ```rust
//! use hammerwork::{Job, retry::{RetryStrategy, JitterType}};
//! use serde_json::json;
//! use std::time::Duration;
//!
//! // Custom exponential backoff with multiplicative jitter
//! let strategy = RetryStrategy::Exponential {
//!     base: Duration::from_secs(2),
//!     multiplier: 1.5,
//!     max_delay: Some(Duration::from_secs(5 * 60)),
//!     jitter: Some(JitterType::Multiplicative(0.1)), // ±10% jitter
//! };
//!
//! let job = Job::new("data_processing".to_string(), json!({"task": "heavy"}))
//!     .with_retry_strategy(strategy);
//! ```

use rand::Rng;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;

/// Types of jitter that can be applied to retry delays.
///
/// Jitter helps prevent the "thundering herd" problem where many failing jobs
/// all retry at exactly the same time, potentially overwhelming downstream systems.
///
/// # Examples
///
/// ```rust
/// use hammerwork::retry::JitterType;
/// use std::time::Duration;
///
/// // Add ±2 seconds of randomness
/// let additive = JitterType::Additive(Duration::from_secs(2));
///
/// // Add ±20% randomness to the delay
/// let multiplicative = JitterType::Multiplicative(0.2);
/// ```
#[derive(Debug, Clone, PartialEq)]
pub enum JitterType {
    /// Add a random duration between 0 and the specified value.
    ///
    /// The jitter is uniform random: `delay ± rand(0, jitter_amount)`
    Additive(Duration),

    /// Multiply the delay by a random factor.
    ///
    /// The factor is uniform random: `delay * (1 ± rand(0, factor))`
    /// For example, with factor 0.1, the delay will be between 90% and 110% of the original.
    Multiplicative(f64),
}

impl JitterType {
    /// Apply jitter to a given delay duration.
    ///
    /// # Arguments
    ///
    /// * `delay` - The base delay to apply jitter to
    ///
    /// # Returns
    ///
    /// The delay with jitter applied. The result is clamped to ensure it's never negative.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::retry::JitterType;
    /// use std::time::Duration;
    ///
    /// let jitter = JitterType::Additive(Duration::from_secs(5));
    /// let base_delay = Duration::from_secs(30);
    /// let jittered = jitter.apply(base_delay);
    ///
    /// // Result will be between 25 and 35 seconds
    /// assert!(jittered >= Duration::from_secs(25));
    /// assert!(jittered <= Duration::from_secs(35));
    /// ```
    pub fn apply(&self, delay: Duration) -> Duration {
        let mut rng = rand::thread_rng();

        match self {
            JitterType::Additive(jitter_amount) => {
                let jitter_millis = rng.gen_range(0..=jitter_amount.as_millis() as u64);
                let jitter = Duration::from_millis(jitter_millis);

                // Randomly add or subtract jitter
                if rng.gen_bool(0.5) {
                    delay.saturating_add(jitter)
                } else {
                    delay.saturating_sub(jitter)
                }
            }
            JitterType::Multiplicative(factor) => {
                // A NaN, infinite, zero or negative factor means "no jitter"; factors
                // above 1.0 are clamped so the lower bound never goes negative.
                if !factor.is_finite() || *factor <= 0.0 {
                    return delay;
                }
                let factor = factor.min(1.0);
                let jitter_factor = rng.gen_range((1.0 - factor)..=(1.0 + factor));
                saturating_mul_f64(delay, jitter_factor)
            }
        }
    }
}

impl Serialize for JitterType {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;

        match self {
            JitterType::Additive(duration) => {
                let mut state = serializer.serialize_struct("JitterType", 2)?;
                state.serialize_field("type", "Additive")?;
                state.serialize_field("duration_ms", &(duration.as_millis() as u64))?;
                state.end()
            }
            JitterType::Multiplicative(factor) => {
                let mut state = serializer.serialize_struct("JitterType", 2)?;
                state.serialize_field("type", "Multiplicative")?;
                state.serialize_field("factor", factor)?;
                state.end()
            }
        }
    }
}

impl<'de> Deserialize<'de> for JitterType {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::{self, MapAccess, Visitor};
        use std::fmt;

        struct JitterTypeVisitor;

        impl<'de> Visitor<'de> for JitterTypeVisitor {
            type Value = JitterType;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a jitter type")
            }

            fn visit_map<M>(self, mut map: M) -> Result<Self::Value, M::Error>
            where
                M: MapAccess<'de>,
            {
                let mut jitter_type: Option<String> = None;
                let mut duration_ms: Option<u64> = None;
                let mut factor: Option<f64> = None;

                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "type" => {
                            if jitter_type.is_some() {
                                return Err(de::Error::duplicate_field("type"));
                            }
                            jitter_type = Some(map.next_value()?);
                        }
                        "duration_ms" => {
                            if duration_ms.is_some() {
                                return Err(de::Error::duplicate_field("duration_ms"));
                            }
                            duration_ms = Some(map.next_value()?);
                        }
                        "factor" => {
                            if factor.is_some() {
                                return Err(de::Error::duplicate_field("factor"));
                            }
                            factor = Some(map.next_value()?);
                        }
                        _ => {
                            let _: serde::de::IgnoredAny = map.next_value()?;
                        }
                    }
                }

                let jitter_type = jitter_type.ok_or_else(|| de::Error::missing_field("type"))?;

                match jitter_type.as_str() {
                    "Additive" => {
                        let duration_ms =
                            duration_ms.ok_or_else(|| de::Error::missing_field("duration_ms"))?;
                        Ok(JitterType::Additive(Duration::from_millis(duration_ms)))
                    }
                    "Multiplicative" => {
                        let factor = factor.ok_or_else(|| de::Error::missing_field("factor"))?;
                        Ok(JitterType::Multiplicative(factor))
                    }
                    _ => Err(de::Error::unknown_variant(
                        &jitter_type,
                        &["Additive", "Multiplicative"],
                    )),
                }
            }
        }

        deserializer.deserialize_struct(
            "JitterType",
            &["type", "duration_ms", "factor"],
            JitterTypeVisitor,
        )
    }
}

/// Advanced retry strategies for determining delay between job retry attempts.
///
/// Each strategy calculates the delay based on the number of previous attempts,
/// allowing for sophisticated backoff patterns that can help reduce system load
/// during failures while ensuring jobs are retried appropriately.
///
/// # Strategy Selection Guidelines
///
/// - **Fixed**: Simple scenarios with predictable failure patterns
/// - **Linear**: When you want gradually increasing delays without explosive growth
/// - **Exponential**: Most common choice for network/API failures; prevents overwhelming downstream
/// - **Fibonacci**: Similar to exponential but with gentler growth
/// - **Custom**: When you need domain-specific retry logic
///
/// # Examples
///
/// ```rust
/// use hammerwork::retry::RetryStrategy;
/// use std::time::Duration;
///
/// // Simple fixed delay
/// let fixed = RetryStrategy::Fixed(Duration::from_secs(30));
///
/// // Linear backoff: 10s, 20s, 30s, 40s...
/// let linear = RetryStrategy::Linear {
///     base: Duration::from_secs(10),
///     increment: Duration::from_secs(10),
///     max_delay: Some(Duration::from_secs(5 * 60)),
/// };
///
/// // Exponential backoff: 1s, 2s, 4s, 8s, 16s...
/// let exponential = RetryStrategy::Exponential {
///     base: Duration::from_secs(1),
///     multiplier: 2.0,
///     max_delay: Some(Duration::from_secs(10 * 60)),
///     jitter: None,
/// };
/// ```
pub enum RetryStrategy {
    /// Fixed delay between all retry attempts.
    ///
    /// This is the simplest strategy and matches the original Hammerwork behavior.
    /// All retries wait the same amount of time regardless of attempt number.
    ///
    /// # Use Cases
    /// - Simple systems with predictable failure patterns
    /// - When you want consistent retry timing
    /// - Testing and development environments
    ///
    /// # Example
    /// ```rust
    /// use hammerwork::retry::RetryStrategy;
    /// use std::time::Duration;
    ///
    /// let strategy = RetryStrategy::Fixed(Duration::from_secs(30));
    ///
    /// // All attempts wait 30 seconds: 30s, 30s, 30s...
    /// assert_eq!(strategy.calculate_delay(1), Duration::from_secs(30));
    /// assert_eq!(strategy.calculate_delay(5), Duration::from_secs(30));
    /// ```
    Fixed(Duration),

    /// Linear backoff with configurable increment and optional maximum delay.
    ///
    /// Each retry waits longer than the previous by a fixed increment.
    /// Formula: `base + (attempt * increment)`
    ///
    /// # Use Cases
    /// - When you want gradual backoff without explosive growth
    /// - Resource contention scenarios
    /// - Database lock conflicts
    ///
    /// # Example
    /// ```rust
    /// use hammerwork::retry::RetryStrategy;
    /// use std::time::Duration;
    ///
    /// let strategy = RetryStrategy::Linear {
    ///     base: Duration::from_secs(5),
    ///     increment: Duration::from_secs(10),
    ///     max_delay: Some(Duration::from_secs(2 * 60)),
    /// };
    ///
    /// // Delays: 5s, 15s, 25s, 35s, 45s, 55s, 65s, 75s, 85s, 95s, 120s (capped)...
    /// assert_eq!(strategy.calculate_delay(1), Duration::from_secs(15));
    /// assert_eq!(strategy.calculate_delay(10), Duration::from_secs(105));
    /// ```
    Linear {
        /// Base delay for the first retry attempt
        base: Duration,
        /// Amount to add for each subsequent attempt
        increment: Duration,
        /// Optional maximum delay to cap exponential growth
        max_delay: Option<Duration>,
    },

    /// Exponential backoff with configurable base, multiplier, and optional jitter.
    ///
    /// Each retry waits exponentially longer than the previous attempt.
    /// Formula: `base * (multiplier ^ (attempt - 1))`
    ///
    /// # Use Cases
    /// - Network and API failures (most common)
    /// - External service integration
    /// - Rate limiting scenarios
    /// - Any scenario where overwhelming downstream systems is a concern
    ///
    /// # Example
    /// ```rust
    /// use hammerwork::retry::{RetryStrategy, JitterType};
    /// use std::time::Duration;
    ///
    /// let strategy = RetryStrategy::Exponential {
    ///     base: Duration::from_secs(1),
    ///     multiplier: 2.0,
    ///     max_delay: Some(Duration::from_secs(10 * 60)),
    ///     jitter: Some(JitterType::Multiplicative(0.1)), // ±10% jitter
    /// };
    ///
    /// // Base delays: 1s, 2s, 4s, 8s, 16s, 32s, 64s, 128s, 256s, 512s (capped at 600s)
    /// // With jitter: each delay is randomly adjusted by ±10%
    /// ```
    Exponential {
        /// Base delay for the first retry attempt
        base: Duration,
        /// Multiplier for exponential growth (typically 2.0)
        multiplier: f64,
        /// Optional maximum delay to cap exponential growth
        max_delay: Option<Duration>,
        /// Optional jitter to prevent thundering herd problems
        jitter: Option<JitterType>,
    },

    /// Fibonacci sequence backoff with configurable base delay.
    ///
    /// Each retry waits according to the Fibonacci sequence multiplied by the base delay.
    /// Sequence: 1, 1, 2, 3, 5, 8, 13, 21, 34, 55, 89...
    /// Formula: `base * fibonacci(attempt)`
    ///
    /// # Use Cases
    /// - When you want growth slower than exponential but faster than linear
    /// - Mathematical elegance in retry patterns
    /// - Systems with moderate failure recovery times
    ///
    /// # Example
    /// ```rust
    /// use hammerwork::retry::RetryStrategy;
    /// use std::time::Duration;
    ///
    /// let strategy = RetryStrategy::Fibonacci {
    ///     base: Duration::from_secs(2),
    ///     max_delay: Some(Duration::from_secs(5 * 60)),
    /// };
    ///
    /// // Delays: 2s, 2s, 4s, 6s, 10s, 16s, 26s, 42s, 68s, 110s, 178s, 288s (capped at 300s)...
    /// assert_eq!(strategy.calculate_delay(1), Duration::from_secs(2));
    /// assert_eq!(strategy.calculate_delay(5), Duration::from_secs(10));
    /// ```
    Fibonacci {
        /// Base delay multiplied by the Fibonacci number
        base: Duration,
        /// Optional maximum delay to cap growth
        max_delay: Option<Duration>,
    },

    /// Custom retry strategy using a user-defined function.
    ///
    /// Allows for completely custom retry logic based on attempt number.
    /// The function receives the attempt number (1-based) and returns the delay.
    ///
    /// # Use Cases
    /// - Domain-specific retry patterns
    /// - Complex business logic for retry timing
    /// - Integration with external scheduling systems
    /// - When none of the built-in strategies fit your needs
    ///
    /// # Example
    /// ```rust
    /// use hammerwork::retry::RetryStrategy;
    /// use std::time::Duration;
    ///
    /// use std::sync::Arc;
    ///
    /// // Custom strategy: short delays for first few attempts, then longer
    /// let strategy = RetryStrategy::Custom(Arc::new(|attempt| {
    ///     if attempt <= 3 {
    ///         Duration::from_secs(5)  // Quick retries for transient issues
    ///     } else if attempt <= 6 {
    ///         Duration::from_secs(60) // Medium delays for persistent issues
    ///     } else {
    ///         Duration::from_secs(300) // Long delays for severe issues
    ///     }
    /// }));
    ///
    /// assert_eq!(strategy.calculate_delay(1), Duration::from_secs(5));
    /// assert_eq!(strategy.calculate_delay(4), Duration::from_secs(60));
    /// assert_eq!(strategy.calculate_delay(7), Duration::from_secs(300));
    /// ```
    ///
    /// The function is reference-counted so that cloning a strategy (for example when a
    /// [`Worker`](crate::Worker) is cloned for autoscaling) is cheap and never fails.
    /// Prefer the [`RetryStrategy::custom`] constructor, which wraps the closure for you.
    Custom(CustomRetryFn),
}

impl RetryStrategy {
    /// Calculate the delay before the next retry attempt.
    ///
    /// # Arguments
    ///
    /// * `attempt` - The attempt number (1-based). The first retry is attempt 1.
    ///
    /// # Returns
    ///
    /// The duration to wait before the next retry attempt.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::retry::RetryStrategy;
    /// use std::time::Duration;
    ///
    /// let strategy = RetryStrategy::Exponential {
    ///     base: Duration::from_secs(1),
    ///     multiplier: 2.0,
    ///     max_delay: None,
    ///     jitter: None,
    /// };
    ///
    /// assert_eq!(strategy.calculate_delay(1), Duration::from_secs(1));
    /// assert_eq!(strategy.calculate_delay(2), Duration::from_secs(2));
    /// assert_eq!(strategy.calculate_delay(3), Duration::from_secs(4));
    /// assert_eq!(strategy.calculate_delay(4), Duration::from_secs(8));
    /// ```
    pub fn calculate_delay(&self, attempt: u32) -> Duration {
        // All arithmetic below saturates instead of panicking: huge attempt numbers,
        // huge bases and invalid multipliers (NaN, negative) all produce a bounded
        // delay, clamped to `max_delay` when one is configured.
        let base_delay = match self {
            RetryStrategy::Fixed(delay) => *delay,

            RetryStrategy::Linear {
                base,
                increment,
                max_delay,
            } => {
                let step = increment.checked_mul(attempt).unwrap_or(Duration::MAX);
                cap_delay(base.saturating_add(step), *max_delay)
            }

            RetryStrategy::Exponential {
                base,
                multiplier,
                max_delay,
                jitter,
            } => {
                let exponent = attempt.saturating_sub(1).min(i32::MAX as u32) as i32;
                let delay_multiplier = sanitize_multiplier(*multiplier).powi(exponent);
                let capped_delay =
                    cap_delay(saturating_mul_f64(*base, delay_multiplier), *max_delay);

                if let Some(jitter_type) = jitter {
                    return jitter_type
                        .apply(capped_delay)
                        .max(Duration::from_millis(1));
                }

                capped_delay
            }

            RetryStrategy::Fibonacci { base, max_delay } => {
                let fib_number = fibonacci(attempt);
                cap_delay(saturating_mul_f64(*base, fib_number as f64), *max_delay)
            }

            RetryStrategy::Custom(func) => func(attempt),
        };

        // Ensure delay is never zero (minimum 1ms)
        base_delay.max(Duration::from_millis(1))
    }

    /// Check that the strategy's parameters are sensible.
    ///
    /// [`calculate_delay`](Self::calculate_delay) never panics, even for invalid
    /// parameters (a NaN or negative multiplier is treated as `1.0`), but such values
    /// are almost certainly configuration mistakes. Call this when loading a strategy
    /// from configuration to surface them early.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::retry::RetryStrategy;
    /// use std::time::Duration;
    ///
    /// assert!(RetryStrategy::exponential(Duration::from_secs(1), 2.0, None).validate().is_ok());
    /// assert!(RetryStrategy::exponential(Duration::from_secs(1), f64::NAN, None).validate().is_err());
    /// ```
    pub fn validate(&self) -> crate::Result<()> {
        let invalid = |message: String| crate::HammerworkError::Worker { message };
        match self {
            RetryStrategy::Exponential {
                multiplier, jitter, ..
            } => {
                if !multiplier.is_finite() || *multiplier <= 0.0 {
                    return Err(invalid(format!(
                        "exponential retry multiplier must be finite and > 0, got {multiplier}"
                    )));
                }
                if let Some(JitterType::Multiplicative(factor)) = jitter
                    && (!factor.is_finite() || !(0.0..=1.0).contains(factor))
                {
                    return Err(invalid(format!(
                        "multiplicative jitter factor must be within 0.0..=1.0, got {factor}"
                    )));
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    /// Create a fixed delay retry strategy.
    ///
    /// # Arguments
    ///
    /// * `delay` - The fixed delay between retry attempts
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::retry::RetryStrategy;
    /// use std::time::Duration;
    ///
    /// let strategy = RetryStrategy::fixed(Duration::from_secs(30));
    /// ```
    pub fn fixed(delay: Duration) -> Self {
        RetryStrategy::Fixed(delay)
    }

    /// Create a linear backoff retry strategy.
    ///
    /// # Arguments
    ///
    /// * `base` - Base delay for the first attempt
    /// * `increment` - Amount to add for each subsequent attempt
    /// * `max_delay` - Optional maximum delay cap
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::retry::RetryStrategy;
    /// use std::time::Duration;
    ///
    /// let strategy = RetryStrategy::linear(
    ///     Duration::from_secs(10),
    ///     Duration::from_secs(5),
    ///     Some(Duration::from_secs(2 * 60))
    /// );
    /// ```
    pub fn linear(base: Duration, increment: Duration, max_delay: Option<Duration>) -> Self {
        RetryStrategy::Linear {
            base,
            increment,
            max_delay,
        }
    }

    /// Create an exponential backoff retry strategy.
    ///
    /// # Arguments
    ///
    /// * `base` - Base delay for the first attempt
    /// * `multiplier` - Exponential growth multiplier
    /// * `max_delay` - Optional maximum delay cap
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::retry::RetryStrategy;
    /// use std::time::Duration;
    ///
    /// let strategy = RetryStrategy::exponential(
    ///     Duration::from_secs(1),
    ///     2.0,
    ///     Some(Duration::from_secs(10 * 60))
    /// );
    /// ```
    pub fn exponential(base: Duration, multiplier: f64, max_delay: Option<Duration>) -> Self {
        RetryStrategy::Exponential {
            base,
            multiplier,
            max_delay,
            jitter: None,
        }
    }

    /// Create an exponential backoff retry strategy with jitter.
    ///
    /// # Arguments
    ///
    /// * `base` - Base delay for the first attempt
    /// * `multiplier` - Exponential growth multiplier
    /// * `max_delay` - Optional maximum delay cap
    /// * `jitter` - Jitter type to apply
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::retry::{RetryStrategy, JitterType};
    /// use std::time::Duration;
    ///
    /// let strategy = RetryStrategy::exponential_with_jitter(
    ///     Duration::from_secs(1),
    ///     2.0,
    ///     Some(Duration::from_secs(10 * 60)),
    ///     JitterType::Multiplicative(0.1)
    /// );
    /// ```
    pub fn exponential_with_jitter(
        base: Duration,
        multiplier: f64,
        max_delay: Option<Duration>,
        jitter: JitterType,
    ) -> Self {
        RetryStrategy::Exponential {
            base,
            multiplier,
            max_delay,
            jitter: Some(jitter),
        }
    }

    /// Create a Fibonacci sequence retry strategy.
    ///
    /// # Arguments
    ///
    /// * `base` - Base delay multiplied by Fibonacci numbers
    /// * `max_delay` - Optional maximum delay cap
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::retry::RetryStrategy;
    /// use std::time::Duration;
    ///
    /// let strategy = RetryStrategy::fibonacci(
    ///     Duration::from_secs(2),
    ///     Some(Duration::from_secs(5 * 60))
    /// );
    /// ```
    pub fn fibonacci(base: Duration, max_delay: Option<Duration>) -> Self {
        RetryStrategy::Fibonacci { base, max_delay }
    }

    /// Create a custom retry strategy using a user-defined function.
    ///
    /// # Arguments
    ///
    /// * `func` - Function that takes attempt number and returns delay
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::retry::RetryStrategy;
    /// use std::time::Duration;
    ///
    /// let strategy = RetryStrategy::custom(|attempt| {
    ///     match attempt {
    ///         1..=3 => Duration::from_secs(5),
    ///         4..=6 => Duration::from_secs(30),
    ///         _ => Duration::from_secs(300),
    ///     }
    /// });
    /// ```
    pub fn custom<F>(func: F) -> Self
    where
        F: Fn(u32) -> Duration + Send + Sync + 'static,
    {
        RetryStrategy::Custom(Arc::new(func))
    }
}

/// The function type stored by [`RetryStrategy::Custom`].
pub type CustomRetryFn = Arc<dyn Fn(u32) -> Duration + Send + Sync>;

/// Replace a NaN or negative exponential multiplier with `1.0` (constant delay).
fn sanitize_multiplier(multiplier: f64) -> f64 {
    if multiplier.is_nan() || multiplier < 0.0 {
        1.0
    } else {
        multiplier
    }
}

/// Multiply a duration by a float without panicking.
///
/// Non-positive and NaN factors yield zero; results too large for a `Duration`
/// (including an infinite factor) saturate to `Duration::MAX`.
fn saturating_mul_f64(delay: Duration, factor: f64) -> Duration {
    if factor.is_nan() || factor <= 0.0 {
        return Duration::ZERO;
    }
    Duration::try_from_secs_f64(delay.as_secs_f64() * factor).unwrap_or(Duration::MAX)
}

/// Clamp a delay to an optional maximum.
fn cap_delay(delay: Duration, max_delay: Option<Duration>) -> Duration {
    match max_delay {
        Some(max) => delay.min(max),
        None => delay,
    }
}

/// Calculate the nth Fibonacci number efficiently.
///
/// Uses an iterative approach to avoid recursion overhead and stack overflow
/// for large attempt numbers.
///
/// # Arguments
///
/// * `n` - The position in the Fibonacci sequence (1-based)
///
/// # Returns
///
/// The nth Fibonacci number
///
/// # Examples
///
/// ```rust
/// use hammerwork::retry::fibonacci;
///
/// assert_eq!(fibonacci(1), 1);
/// assert_eq!(fibonacci(2), 1);
/// assert_eq!(fibonacci(3), 2);
/// assert_eq!(fibonacci(4), 3);
/// assert_eq!(fibonacci(5), 5);
/// assert_eq!(fibonacci(6), 8);
/// ```
pub fn fibonacci(n: u32) -> u64 {
    if n == 0 {
        return 0;
    }
    if n <= 2 {
        return 1;
    }

    let mut prev = 1u64;
    let mut curr = 1u64;

    for _ in 3..=n {
        let next = prev.saturating_add(curr);
        prev = curr;
        curr = next;
    }

    curr
}

// Manual implementations for RetryStrategy to handle the Custom variant

impl std::fmt::Debug for RetryStrategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RetryStrategy::Fixed(duration) => f.debug_tuple("Fixed").field(duration).finish(),
            RetryStrategy::Linear {
                base,
                increment,
                max_delay,
            } => f
                .debug_struct("Linear")
                .field("base", base)
                .field("increment", increment)
                .field("max_delay", max_delay)
                .finish(),
            RetryStrategy::Exponential {
                base,
                multiplier,
                max_delay,
                jitter,
            } => f
                .debug_struct("Exponential")
                .field("base", base)
                .field("multiplier", multiplier)
                .field("max_delay", max_delay)
                .field("jitter", jitter)
                .finish(),
            RetryStrategy::Fibonacci { base, max_delay } => f
                .debug_struct("Fibonacci")
                .field("base", base)
                .field("max_delay", max_delay)
                .finish(),
            RetryStrategy::Custom(_) => f.write_str("Custom(<function>)"),
        }
    }
}

impl Clone for RetryStrategy {
    fn clone(&self) -> Self {
        match self {
            RetryStrategy::Fixed(duration) => RetryStrategy::Fixed(*duration),
            RetryStrategy::Linear {
                base,
                increment,
                max_delay,
            } => RetryStrategy::Linear {
                base: *base,
                increment: *increment,
                max_delay: *max_delay,
            },
            RetryStrategy::Exponential {
                base,
                multiplier,
                max_delay,
                jitter,
            } => RetryStrategy::Exponential {
                base: *base,
                multiplier: *multiplier,
                max_delay: *max_delay,
                jitter: jitter.clone(),
            },
            RetryStrategy::Fibonacci { base, max_delay } => RetryStrategy::Fibonacci {
                base: *base,
                max_delay: *max_delay,
            },
            RetryStrategy::Custom(func) => RetryStrategy::Custom(Arc::clone(func)),
        }
    }
}

impl PartialEq for RetryStrategy {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (RetryStrategy::Fixed(a), RetryStrategy::Fixed(b)) => a == b,
            (
                RetryStrategy::Linear {
                    base: a_base,
                    increment: a_inc,
                    max_delay: a_max,
                },
                RetryStrategy::Linear {
                    base: b_base,
                    increment: b_inc,
                    max_delay: b_max,
                },
            ) => a_base == b_base && a_inc == b_inc && a_max == b_max,
            (
                RetryStrategy::Exponential {
                    base: a_base,
                    multiplier: a_mult,
                    max_delay: a_max,
                    jitter: a_jitter,
                },
                RetryStrategy::Exponential {
                    base: b_base,
                    multiplier: b_mult,
                    max_delay: b_max,
                    jitter: b_jitter,
                },
            ) => a_base == b_base && a_mult == b_mult && a_max == b_max && a_jitter == b_jitter,
            (
                RetryStrategy::Fibonacci {
                    base: a_base,
                    max_delay: a_max,
                },
                RetryStrategy::Fibonacci {
                    base: b_base,
                    max_delay: b_max,
                },
            ) => a_base == b_base && a_max == b_max,
            // Closures can't be compared; two strategies sharing the same function are equal.
            (RetryStrategy::Custom(a), RetryStrategy::Custom(b)) => Arc::ptr_eq(a, b),
            _ => false,
        }
    }
}

impl Serialize for RetryStrategy {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;

        match self {
            RetryStrategy::Fixed(duration) => {
                let mut state = serializer.serialize_struct("RetryStrategy", 2)?;
                state.serialize_field("type", "Fixed")?;
                state.serialize_field("duration_ms", &(duration.as_millis() as u64))?;
                state.end()
            }
            RetryStrategy::Linear {
                base,
                increment,
                max_delay,
            } => {
                let mut state = serializer.serialize_struct("RetryStrategy", 4)?;
                state.serialize_field("type", "Linear")?;
                state.serialize_field("base_ms", &(base.as_millis() as u64))?;
                state.serialize_field("increment_ms", &(increment.as_millis() as u64))?;
                state.serialize_field("max_delay_ms", &max_delay.map(|d| d.as_millis() as u64))?;
                state.end()
            }
            RetryStrategy::Exponential {
                base,
                multiplier,
                max_delay,
                jitter,
            } => {
                let mut state = serializer.serialize_struct("RetryStrategy", 5)?;
                state.serialize_field("type", "Exponential")?;
                state.serialize_field("base_ms", &(base.as_millis() as u64))?;
                state.serialize_field("multiplier", multiplier)?;
                state.serialize_field("max_delay_ms", &max_delay.map(|d| d.as_millis() as u64))?;
                state.serialize_field("jitter", jitter)?;
                state.end()
            }
            RetryStrategy::Fibonacci { base, max_delay } => {
                let mut state = serializer.serialize_struct("RetryStrategy", 3)?;
                state.serialize_field("type", "Fibonacci")?;
                state.serialize_field("base_ms", &(base.as_millis() as u64))?;
                state.serialize_field("max_delay_ms", &max_delay.map(|d| d.as_millis() as u64))?;
                state.end()
            }
            RetryStrategy::Custom(_) => Err(serde::ser::Error::custom(
                "Cannot serialize custom retry strategy functions",
            )),
        }
    }
}

impl<'de> Deserialize<'de> for RetryStrategy {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::{self, MapAccess, Visitor};
        use std::fmt;

        struct RetryStrategyVisitor;

        impl<'de> Visitor<'de> for RetryStrategyVisitor {
            type Value = RetryStrategy;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a retry strategy")
            }

            fn visit_map<M>(self, mut map: M) -> Result<Self::Value, M::Error>
            where
                M: MapAccess<'de>,
            {
                let mut strategy_type: Option<String> = None;
                let mut duration_ms: Option<u64> = None;
                let mut base_ms: Option<u64> = None;
                let mut increment_ms: Option<u64> = None;
                let mut max_delay_ms: Option<Option<u64>> = None;
                let mut multiplier: Option<f64> = None;
                let mut jitter: Option<Option<JitterType>> = None;

                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "type" => {
                            if strategy_type.is_some() {
                                return Err(de::Error::duplicate_field("type"));
                            }
                            strategy_type = Some(map.next_value()?);
                        }
                        "duration_ms" => {
                            if duration_ms.is_some() {
                                return Err(de::Error::duplicate_field("duration_ms"));
                            }
                            duration_ms = Some(map.next_value()?);
                        }
                        "base_ms" => {
                            if base_ms.is_some() {
                                return Err(de::Error::duplicate_field("base_ms"));
                            }
                            base_ms = Some(map.next_value()?);
                        }
                        "increment_ms" => {
                            if increment_ms.is_some() {
                                return Err(de::Error::duplicate_field("increment_ms"));
                            }
                            increment_ms = Some(map.next_value()?);
                        }
                        "max_delay_ms" => {
                            if max_delay_ms.is_some() {
                                return Err(de::Error::duplicate_field("max_delay_ms"));
                            }
                            max_delay_ms = Some(map.next_value()?);
                        }
                        "multiplier" => {
                            if multiplier.is_some() {
                                return Err(de::Error::duplicate_field("multiplier"));
                            }
                            multiplier = Some(map.next_value()?);
                        }
                        "jitter" => {
                            if jitter.is_some() {
                                return Err(de::Error::duplicate_field("jitter"));
                            }
                            jitter = Some(map.next_value()?);
                        }
                        _ => {
                            let _: serde::de::IgnoredAny = map.next_value()?;
                        }
                    }
                }

                let strategy_type =
                    strategy_type.ok_or_else(|| de::Error::missing_field("type"))?;

                match strategy_type.as_str() {
                    "Fixed" => {
                        let duration_ms =
                            duration_ms.ok_or_else(|| de::Error::missing_field("duration_ms"))?;
                        Ok(RetryStrategy::Fixed(Duration::from_millis(duration_ms)))
                    }
                    "Linear" => {
                        let base_ms = base_ms.ok_or_else(|| de::Error::missing_field("base_ms"))?;
                        let increment_ms =
                            increment_ms.ok_or_else(|| de::Error::missing_field("increment_ms"))?;
                        // Absent means no maximum (TOML cannot express null).
                        let max_delay_ms = max_delay_ms.flatten();
                        Ok(RetryStrategy::Linear {
                            base: Duration::from_millis(base_ms),
                            increment: Duration::from_millis(increment_ms),
                            max_delay: max_delay_ms.map(Duration::from_millis),
                        })
                    }
                    "Exponential" => {
                        let base_ms = base_ms.ok_or_else(|| de::Error::missing_field("base_ms"))?;
                        let multiplier =
                            multiplier.ok_or_else(|| de::Error::missing_field("multiplier"))?;
                        // Absent means no maximum (TOML cannot express null).
                        let max_delay_ms = max_delay_ms.flatten();
                        let jitter = jitter.unwrap_or(None);
                        Ok(RetryStrategy::Exponential {
                            base: Duration::from_millis(base_ms),
                            multiplier,
                            max_delay: max_delay_ms.map(Duration::from_millis),
                            jitter,
                        })
                    }
                    "Fibonacci" => {
                        let base_ms = base_ms.ok_or_else(|| de::Error::missing_field("base_ms"))?;
                        // Absent means no maximum (TOML cannot express null).
                        let max_delay_ms = max_delay_ms.flatten();
                        Ok(RetryStrategy::Fibonacci {
                            base: Duration::from_millis(base_ms),
                            max_delay: max_delay_ms.map(Duration::from_millis),
                        })
                    }
                    _ => Err(de::Error::unknown_variant(
                        &strategy_type,
                        &["Fixed", "Linear", "Exponential", "Fibonacci"],
                    )),
                }
            }
        }

        deserializer.deserialize_struct(
            "RetryStrategy",
            &[
                "type",
                "duration_ms",
                "base_ms",
                "increment_ms",
                "max_delay_ms",
                "multiplier",
                "jitter",
            ],
            RetryStrategyVisitor,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_fibonacci_sequence() {
        assert_eq!(fibonacci(0), 0);
        assert_eq!(fibonacci(1), 1);
        assert_eq!(fibonacci(2), 1);
        assert_eq!(fibonacci(3), 2);
        assert_eq!(fibonacci(4), 3);
        assert_eq!(fibonacci(5), 5);
        assert_eq!(fibonacci(6), 8);
        assert_eq!(fibonacci(7), 13);
        assert_eq!(fibonacci(8), 21);
        assert_eq!(fibonacci(9), 34);
        assert_eq!(fibonacci(10), 55);
    }

    #[test]
    fn test_fixed_retry_strategy() {
        let strategy = RetryStrategy::Fixed(Duration::from_secs(30));

        assert_eq!(strategy.calculate_delay(1), Duration::from_secs(30));
        assert_eq!(strategy.calculate_delay(5), Duration::from_secs(30));
        assert_eq!(strategy.calculate_delay(10), Duration::from_secs(30));
    }

    #[test]
    fn test_linear_retry_strategy() {
        let strategy = RetryStrategy::Linear {
            base: Duration::from_secs(10),
            increment: Duration::from_secs(5),
            max_delay: Some(Duration::from_secs(40)),
        };

        assert_eq!(strategy.calculate_delay(1), Duration::from_secs(15)); // 10 + (1 * 5)
        assert_eq!(strategy.calculate_delay(2), Duration::from_secs(20)); // 10 + (2 * 5)
        assert_eq!(strategy.calculate_delay(3), Duration::from_secs(25)); // 10 + (3 * 5)
        assert_eq!(strategy.calculate_delay(6), Duration::from_secs(40)); // Capped at max_delay
        assert_eq!(strategy.calculate_delay(10), Duration::from_secs(40)); // Still capped
    }

    #[test]
    fn test_exponential_retry_strategy() {
        let strategy = RetryStrategy::Exponential {
            base: Duration::from_secs(1),
            multiplier: 2.0,
            max_delay: Some(Duration::from_secs(60)),
            jitter: None,
        };

        assert_eq!(strategy.calculate_delay(1), Duration::from_secs(1)); // 1 * 2^0 = 1
        assert_eq!(strategy.calculate_delay(2), Duration::from_secs(2)); // 1 * 2^1 = 2
        assert_eq!(strategy.calculate_delay(3), Duration::from_secs(4)); // 1 * 2^2 = 4
        assert_eq!(strategy.calculate_delay(4), Duration::from_secs(8)); // 1 * 2^3 = 8
        assert_eq!(strategy.calculate_delay(5), Duration::from_secs(16)); // 1 * 2^4 = 16
        assert_eq!(strategy.calculate_delay(6), Duration::from_secs(32)); // 1 * 2^5 = 32
        assert_eq!(strategy.calculate_delay(7), Duration::from_secs(60)); // Capped at max_delay
        assert_eq!(strategy.calculate_delay(10), Duration::from_secs(60)); // Still capped
    }

    #[test]
    fn test_fibonacci_retry_strategy() {
        let strategy = RetryStrategy::Fibonacci {
            base: Duration::from_secs(2),
            max_delay: Some(Duration::from_secs(100)),
        };

        assert_eq!(strategy.calculate_delay(1), Duration::from_secs(2)); // 2 * 1 = 2
        assert_eq!(strategy.calculate_delay(2), Duration::from_secs(2)); // 2 * 1 = 2  
        assert_eq!(strategy.calculate_delay(3), Duration::from_secs(4)); // 2 * 2 = 4
        assert_eq!(strategy.calculate_delay(4), Duration::from_secs(6)); // 2 * 3 = 6
        assert_eq!(strategy.calculate_delay(5), Duration::from_secs(10)); // 2 * 5 = 10
        assert_eq!(strategy.calculate_delay(6), Duration::from_secs(16)); // 2 * 8 = 16
        assert_eq!(strategy.calculate_delay(7), Duration::from_secs(26)); // 2 * 13 = 26
    }

    #[test]
    fn test_custom_retry_strategy() {
        let strategy = RetryStrategy::Custom(Arc::new(|attempt| match attempt {
            1..=3 => Duration::from_secs(5),
            4..=6 => Duration::from_secs(30),
            _ => Duration::from_secs(300),
        }));

        assert_eq!(strategy.calculate_delay(1), Duration::from_secs(5));
        assert_eq!(strategy.calculate_delay(3), Duration::from_secs(5));
        assert_eq!(strategy.calculate_delay(4), Duration::from_secs(30));
        assert_eq!(strategy.calculate_delay(6), Duration::from_secs(30));
        assert_eq!(strategy.calculate_delay(7), Duration::from_secs(300));
        assert_eq!(strategy.calculate_delay(100), Duration::from_secs(300));
    }

    #[test]
    fn test_additive_jitter() {
        let jitter = JitterType::Additive(Duration::from_secs(10));
        let base_delay = Duration::from_secs(60);

        // Test multiple applications to ensure it's within range
        for _ in 0..100 {
            let jittered = jitter.apply(base_delay);
            assert!(jittered >= Duration::from_secs(50)); // 60 - 10
            assert!(jittered <= Duration::from_secs(70)); // 60 + 10
        }
    }

    #[test]
    fn test_multiplicative_jitter() {
        let jitter = JitterType::Multiplicative(0.2); // ±20%
        let base_delay = Duration::from_secs(100);

        // Test multiple applications to ensure it's within range
        for _ in 0..100 {
            let jittered = jitter.apply(base_delay);
            assert!(jittered >= Duration::from_secs(80)); // 100 * 0.8
            assert!(jittered <= Duration::from_secs(120)); // 100 * 1.2
        }
    }

    #[test]
    fn test_exponential_with_jitter() {
        let strategy = RetryStrategy::Exponential {
            base: Duration::from_secs(1),
            multiplier: 2.0,
            max_delay: None,
            jitter: Some(JitterType::Multiplicative(0.1)), // ±10%
        };

        // Test first attempt (should be around 1 second ±10%)
        let delay = strategy.calculate_delay(1);
        assert!(delay >= Duration::from_millis(900)); // 1000ms * 0.9
        assert!(delay <= Duration::from_millis(1100)); // 1000ms * 1.1

        // Test third attempt (should be around 4 seconds ±10%)
        let delay = strategy.calculate_delay(3);
        assert!(delay >= Duration::from_millis(3600)); // 4000ms * 0.9
        assert!(delay <= Duration::from_millis(4400)); // 4000ms * 1.1
    }

    #[test]
    fn test_strategy_builder_methods() {
        let fixed = RetryStrategy::fixed(Duration::from_secs(30));
        assert_eq!(fixed.calculate_delay(1), Duration::from_secs(30));

        let linear = RetryStrategy::linear(
            Duration::from_secs(5),
            Duration::from_secs(10),
            Some(Duration::from_secs(120)),
        );
        assert_eq!(linear.calculate_delay(1), Duration::from_secs(15));

        let exponential =
            RetryStrategy::exponential(Duration::from_secs(1), 2.0, Some(Duration::from_secs(600)));
        assert_eq!(exponential.calculate_delay(1), Duration::from_secs(1));

        let fibonacci =
            RetryStrategy::fibonacci(Duration::from_secs(2), Some(Duration::from_secs(300)));
        assert_eq!(fibonacci.calculate_delay(1), Duration::from_secs(2));
    }

    #[test]
    fn test_minimum_delay_enforcement() {
        // Test that zero delays are converted to 1ms minimum
        let strategy = RetryStrategy::Custom(Arc::new(|_| Duration::from_millis(0)));
        assert_eq!(strategy.calculate_delay(1), Duration::from_millis(1));
    }

    #[test]
    fn test_serialization() {
        let strategies = vec![
            RetryStrategy::Fixed(Duration::from_secs(30)),
            RetryStrategy::Linear {
                base: Duration::from_secs(10),
                increment: Duration::from_secs(5),
                max_delay: Some(Duration::from_secs(60)),
            },
            RetryStrategy::Exponential {
                base: Duration::from_secs(1),
                multiplier: 2.0,
                max_delay: Some(Duration::from_secs(600)),
                jitter: None, // No jitter for serialization test to avoid randomness
            },
            RetryStrategy::Fibonacci {
                base: Duration::from_secs(2),
                max_delay: Some(Duration::from_secs(300)),
            },
        ];

        for strategy in strategies {
            // Test that we can serialize and deserialize
            let serialized = serde_json::to_string(&strategy).unwrap();
            let deserialized: RetryStrategy = serde_json::from_str(&serialized).unwrap();

            // Test that behavior is preserved (except for Custom which can't be serialized)
            if !matches!(strategy, RetryStrategy::Custom(_)) {
                assert_eq!(strategy.calculate_delay(1), deserialized.calculate_delay(1));
                assert_eq!(strategy.calculate_delay(3), deserialized.calculate_delay(3));
            }
        }
    }

    #[test]
    fn test_exponential_huge_attempt_saturates_instead_of_panicking() {
        let uncapped = RetryStrategy::exponential(Duration::from_secs(1), 2.0, None);
        assert_eq!(uncapped.calculate_delay(65), Duration::MAX);
        assert_eq!(uncapped.calculate_delay(u32::MAX), Duration::MAX);

        let capped =
            RetryStrategy::exponential(Duration::from_secs(1), 2.0, Some(Duration::from_secs(600)));
        assert_eq!(capped.calculate_delay(1_000), Duration::from_secs(600));
        assert_eq!(capped.calculate_delay(u32::MAX), Duration::from_secs(600));
    }

    #[test]
    fn test_exponential_invalid_multipliers_do_not_panic() {
        for multiplier in [f64::NAN, -2.0, -0.5] {
            let strategy = RetryStrategy::exponential(Duration::from_secs(3), multiplier, None);
            // Invalid multipliers are treated as 1.0 (constant delay).
            assert_eq!(strategy.calculate_delay(1), Duration::from_secs(3));
            assert_eq!(strategy.calculate_delay(50), Duration::from_secs(3));
            assert!(strategy.validate().is_err());
        }

        let infinite = RetryStrategy::exponential(
            Duration::from_secs(1),
            f64::INFINITY,
            Some(Duration::from_secs(60)),
        );
        assert_eq!(infinite.calculate_delay(2), Duration::from_secs(60));
        assert!(infinite.validate().is_err());

        let zero = RetryStrategy::exponential(Duration::from_secs(1), 0.0, None);
        assert_eq!(zero.calculate_delay(5), Duration::from_millis(1));

        assert!(
            RetryStrategy::exponential(Duration::from_secs(1), 2.0, None)
                .validate()
                .is_ok()
        );
    }

    #[test]
    fn test_jitter_with_invalid_factors_does_not_panic() {
        let base = Duration::from_secs(10);
        for factor in [f64::NAN, -1.0, f64::INFINITY, 0.0] {
            assert_eq!(JitterType::Multiplicative(factor).apply(base), base);
        }
        // Factors above 1.0 are clamped to 1.0: the result stays within [0, 2 * base].
        for _ in 0..50 {
            let jittered = JitterType::Multiplicative(5.0).apply(base);
            assert!(jittered <= base * 2);
        }
        // Additive jitter on a huge delay saturates instead of overflowing.
        for _ in 0..50 {
            let jittered = JitterType::Additive(Duration::from_secs(5)).apply(Duration::MAX);
            assert!(jittered >= Duration::MAX - Duration::from_secs(5));
        }
        let strategy = RetryStrategy::exponential_with_jitter(
            Duration::from_secs(1),
            2.0,
            None,
            JitterType::Multiplicative(0.1),
        );
        assert!(strategy.calculate_delay(u32::MAX) > Duration::from_secs(1));
        assert!(
            RetryStrategy::exponential_with_jitter(
                Duration::from_secs(1),
                2.0,
                None,
                JitterType::Multiplicative(2.0),
            )
            .validate()
            .is_err()
        );
    }

    #[test]
    fn test_fibonacci_huge_attempt_saturates() {
        let uncapped = RetryStrategy::fibonacci(Duration::from_secs(2), None);
        assert_eq!(uncapped.calculate_delay(200), Duration::MAX);
        let capped =
            RetryStrategy::fibonacci(Duration::from_secs(2), Some(Duration::from_secs(300)));
        assert_eq!(
            capped.calculate_delay(u32::MAX - 1),
            Duration::from_secs(300)
        );
    }

    #[test]
    fn test_linear_overflow_saturates() {
        let strategy = RetryStrategy::linear(Duration::MAX, Duration::from_secs(1), None);
        assert_eq!(strategy.calculate_delay(10), Duration::MAX);
        let strategy = RetryStrategy::linear(
            Duration::from_secs(1),
            Duration::from_secs(u64::MAX / 2),
            Some(Duration::from_secs(120)),
        );
        assert_eq!(strategy.calculate_delay(u32::MAX), Duration::from_secs(120));
    }

    #[test]
    fn test_custom_strategy_clone_is_total_and_shares_function() {
        let strategy = RetryStrategy::custom(|attempt| Duration::from_secs(attempt as u64 * 7));
        let cloned = strategy.clone();
        assert_eq!(cloned.calculate_delay(3), Duration::from_secs(21));
        assert_eq!(strategy, cloned);
        assert_ne!(strategy, RetryStrategy::custom(|_| Duration::from_secs(1)));
        assert!(strategy.validate().is_ok());

        // Cloning through Option, as Worker::clone does, must not panic either.
        let as_option = Some(strategy);
        let cloned_option = as_option.clone();
        assert_eq!(
            cloned_option.unwrap().calculate_delay(1),
            Duration::from_secs(7)
        );
    }

    #[test]
    fn test_jitter_type_serde_roundtrip_and_errors() {
        for jitter in [
            JitterType::Additive(Duration::from_millis(250)),
            JitterType::Multiplicative(0.2),
        ] {
            let json = serde_json::to_value(&jitter).unwrap();
            let back: JitterType = serde_json::from_value(json).unwrap();
            assert_eq!(back, jitter);
        }
        assert_eq!(
            serde_json::to_value(JitterType::Additive(Duration::from_millis(250))).unwrap(),
            serde_json::json!({ "type": "Additive", "duration_ms": 250 })
        );
        // Unknown fields are ignored.
        let jitter: JitterType = serde_json::from_value(
            serde_json::json!({ "type": "Multiplicative", "factor": 0.5, "note": "x" }),
        )
        .unwrap();
        assert_eq!(jitter, JitterType::Multiplicative(0.5));

        for (bad, expected) in [
            (serde_json::json!({ "factor": 0.5 }), "missing field `type`"),
            (
                serde_json::json!({ "type": "Additive" }),
                "missing field `duration_ms`",
            ),
            (
                serde_json::json!({ "type": "Multiplicative" }),
                "missing field `factor`",
            ),
            (serde_json::json!({ "type": "Gaussian" }), "unknown variant"),
            (serde_json::json!("Additive"), "a jitter type"),
        ] {
            let err = serde_json::from_value::<JitterType>(bad).unwrap_err();
            assert!(err.to_string().contains(expected), "{err}");
        }
        for duplicate in [
            r#"{"type":"Additive","type":"Additive","duration_ms":1}"#,
            r#"{"type":"Additive","duration_ms":1,"duration_ms":2}"#,
            r#"{"type":"Multiplicative","factor":1.0,"factor":2.0}"#,
        ] {
            let err = serde_json::from_str::<JitterType>(duplicate).unwrap_err();
            assert!(err.to_string().contains("duplicate field"), "{err}");
        }
    }

    #[test]
    fn test_retry_strategy_serde_errors() {
        for (bad, expected) in [
            (
                serde_json::json!({ "duration_ms": 1 }),
                "missing field `type`",
            ),
            (
                serde_json::json!({ "type": "Fixed" }),
                "missing field `duration_ms`",
            ),
            (
                serde_json::json!({ "type": "Linear", "increment_ms": 1 }),
                "missing field `base_ms`",
            ),
            (
                serde_json::json!({ "type": "Linear", "base_ms": 1 }),
                "missing field `increment_ms`",
            ),
            (
                serde_json::json!({ "type": "Exponential", "base_ms": 1 }),
                "missing field `multiplier`",
            ),
            (serde_json::json!({ "type": "Custom" }), "unknown variant"),
            (serde_json::json!(42), "a retry strategy"),
        ] {
            let err = serde_json::from_value::<RetryStrategy>(bad).unwrap_err();
            assert!(err.to_string().contains(expected), "{err}");
        }
        for field in [
            "type",
            "duration_ms",
            "base_ms",
            "increment_ms",
            "max_delay_ms",
            "multiplier",
            "jitter",
        ] {
            let value = match field {
                "type" => "\"Fixed\"",
                "jitter" => "null",
                _ => "1",
            };
            let json = format!(
                r#"{{"type":"Fixed","duration_ms":1,"{field}":{value},"{field}":{value}}}"#
            );
            let err = serde_json::from_str::<RetryStrategy>(&json).unwrap_err();
            assert!(
                err.to_string().contains("duplicate field"),
                "{field}: {err}"
            );
        }
        let custom = RetryStrategy::custom(|_| Duration::from_secs(1));
        let err = serde_json::to_value(&custom).unwrap_err();
        assert!(err.to_string().contains("custom retry strategy"), "{err}");
    }

    /// Strategies without a maximum delay must survive a configuration file round trip.
    /// TOML has no null, so `max_delay_ms` is simply absent; it used to be a required
    /// field, so such a strategy could be saved but not loaded.
    #[test]
    fn test_unbounded_strategies_roundtrip_through_toml() {
        #[derive(serde::Serialize, serde::Deserialize)]
        struct Wrapper {
            strategy: RetryStrategy,
        }
        for strategy in [
            RetryStrategy::linear(Duration::from_secs(1), Duration::from_secs(2), None),
            RetryStrategy::exponential(Duration::from_secs(1), 2.0, None),
            RetryStrategy::exponential_with_jitter(
                Duration::from_secs(1),
                2.0,
                None,
                JitterType::Additive(Duration::from_millis(100)),
            ),
            RetryStrategy::fibonacci(Duration::from_secs(1), None),
            RetryStrategy::fibonacci(Duration::from_secs(1), Some(Duration::from_secs(60))),
        ] {
            let text = toml::to_string(&Wrapper {
                strategy: strategy.clone(),
            })
            .unwrap();
            let back: Wrapper = toml::from_str(&text).unwrap_or_else(|e| panic!("{text}: {e}"));
            assert_eq!(back.strategy, strategy, "{text}");
        }
    }

    #[test]
    fn test_retry_strategy_debug_clone_and_eq() {
        let strategies = [
            RetryStrategy::fixed(Duration::from_secs(1)),
            RetryStrategy::linear(
                Duration::from_secs(1),
                Duration::from_secs(2),
                Some(Duration::from_secs(9)),
            ),
            RetryStrategy::exponential(Duration::from_secs(1), 2.0, None),
            RetryStrategy::fibonacci(Duration::from_secs(1), Some(Duration::from_secs(9))),
        ];
        for (i, a) in strategies.iter().enumerate() {
            assert_eq!(&a.clone(), a);
            for (j, b) in strategies.iter().enumerate() {
                assert_eq!(a == b, i == j, "{a:?} vs {b:?}");
            }
        }
        assert_eq!(format!("{:?}", strategies[0]), "Fixed(1s)");
        assert!(format!("{:?}", strategies[1]).starts_with("Linear { base: 1s"));
        assert!(format!("{:?}", strategies[2]).contains("multiplier: 2.0"));
        assert!(format!("{:?}", strategies[3]).starts_with("Fibonacci"));
        let custom = RetryStrategy::custom(|attempt| Duration::from_secs(attempt as u64));
        assert_eq!(format!("{custom:?}"), "Custom(<function>)");
        // Functions cannot be compared: a clone (sharing the function) is equal, a
        // separately built strategy is not.
        assert_eq!(custom, custom.clone());
        assert_ne!(
            custom,
            RetryStrategy::custom(|attempt| Duration::from_secs(attempt as u64))
        );
        assert_eq!(custom.clone().calculate_delay(3), Duration::from_secs(3));
    }
}
