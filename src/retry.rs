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
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
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
                    delay + jitter
                } else {
                    delay.saturating_sub(jitter)
                }
            }
            JitterType::Multiplicative(factor) => {
                let jitter_factor = rng.gen_range((1.0 - factor)..=(1.0 + factor));
                let jittered_millis = (delay.as_millis() as f64 * jitter_factor) as u64;
                Duration::from_millis(jittered_millis)
            }
        }
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
    /// // Custom strategy: short delays for first few attempts, then longer
    /// let strategy = RetryStrategy::Custom(Box::new(|attempt| {
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
    Custom(Box<dyn Fn(u32) -> Duration + Send + Sync>),
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
        let base_delay = match self {
            RetryStrategy::Fixed(delay) => *delay,

            RetryStrategy::Linear {
                base,
                increment,
                max_delay,
            } => {
                let delay = *base + increment.mul_f64(attempt as f64);
                if let Some(max) = max_delay {
                    delay.min(*max)
                } else {
                    delay
                }
            }

            RetryStrategy::Exponential {
                base,
                multiplier,
                max_delay,
                jitter,
            } => {
                let delay_multiplier = multiplier.powi((attempt.saturating_sub(1)) as i32);
                let delay = base.mul_f64(delay_multiplier);

                let capped_delay = if let Some(max) = max_delay {
                    delay.min(*max)
                } else {
                    delay
                };

                if let Some(jitter_type) = jitter {
                    return jitter_type.apply(capped_delay);
                }

                capped_delay
            }

            RetryStrategy::Fibonacci { base, max_delay } => {
                let fib_number = fibonacci(attempt);
                let delay = base.mul_f64(fib_number as f64);

                if let Some(max) = max_delay {
                    delay.min(*max)
                } else {
                    delay
                }
            }

            RetryStrategy::Custom(func) => func(attempt),
        };

        // Ensure delay is never zero (minimum 1ms)
        base_delay.max(Duration::from_millis(1))
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
        RetryStrategy::Custom(Box::new(func))
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
            RetryStrategy::Custom(_) => {
                panic!("Cannot clone custom retry strategy functions")
            }
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
            (RetryStrategy::Custom(_), RetryStrategy::Custom(_)) => false, // Custom functions can't be compared
            _ => false,
        }
    }
}

impl Serialize for RetryStrategy {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStructVariant;

        match self {
            RetryStrategy::Fixed(duration) => {
                let mut state =
                    serializer.serialize_struct_variant("RetryStrategy", 0, "Fixed", 1)?;
                state.serialize_field("duration_ms", &duration.as_millis())?;
                state.end()
            }
            RetryStrategy::Linear {
                base,
                increment,
                max_delay,
            } => {
                let mut state =
                    serializer.serialize_struct_variant("RetryStrategy", 1, "Linear", 3)?;
                state.serialize_field("base_ms", &base.as_millis())?;
                state.serialize_field("increment_ms", &increment.as_millis())?;
                state.serialize_field("max_delay_ms", &max_delay.map(|d| d.as_millis()))?;
                state.end()
            }
            RetryStrategy::Exponential {
                base,
                multiplier,
                max_delay,
                jitter,
            } => {
                let mut state =
                    serializer.serialize_struct_variant("RetryStrategy", 2, "Exponential", 4)?;
                state.serialize_field("base_ms", &base.as_millis())?;
                state.serialize_field("multiplier", multiplier)?;
                state.serialize_field("max_delay_ms", &max_delay.map(|d| d.as_millis()))?;
                state.serialize_field("jitter", jitter)?;
                state.end()
            }
            RetryStrategy::Fibonacci { base, max_delay } => {
                let mut state =
                    serializer.serialize_struct_variant("RetryStrategy", 3, "Fibonacci", 2)?;
                state.serialize_field("base_ms", &base.as_millis())?;
                state.serialize_field("max_delay_ms", &max_delay.map(|d| d.as_millis()))?;
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
        use serde::de::{self, EnumAccess, MapAccess, VariantAccess, Visitor};
        use std::fmt;

        struct RetryStrategyVisitor;

        impl<'de> Visitor<'de> for RetryStrategyVisitor {
            type Value = RetryStrategy;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a retry strategy")
            }

            fn visit_enum<A>(self, data: A) -> Result<Self::Value, A::Error>
            where
                A: EnumAccess<'de>,
            {
                let (variant, variant_data) = data.variant()?;

                match variant {
                    "Fixed" => variant_data.struct_variant(&["duration_ms"], FixedVisitor),
                    "Linear" => variant_data.struct_variant(
                        &["base_ms", "increment_ms", "max_delay_ms"],
                        LinearVisitor,
                    ),
                    "Exponential" => variant_data.struct_variant(
                        &["base_ms", "multiplier", "max_delay_ms", "jitter"],
                        ExponentialVisitor,
                    ),
                    "Fibonacci" => {
                        variant_data.struct_variant(&["base_ms", "max_delay_ms"], FibonacciVisitor)
                    }
                    _ => Err(de::Error::unknown_variant(
                        variant,
                        &["Fixed", "Linear", "Exponential", "Fibonacci"],
                    )),
                }
            }
        }

        struct FixedVisitor;
        impl<'de> Visitor<'de> for FixedVisitor {
            type Value = RetryStrategy;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("fixed retry strategy data")
            }

            fn visit_map<M>(self, mut map: M) -> Result<Self::Value, M::Error>
            where
                M: MapAccess<'de>,
            {
                let mut duration_ms = None;

                while let Some(key) = map.next_key::<&str>()? {
                    match key {
                        "duration_ms" => {
                            if duration_ms.is_some() {
                                return Err(de::Error::duplicate_field("duration_ms"));
                            }
                            duration_ms = Some(map.next_value::<u64>()?);
                        }
                        _ => {
                            let _: serde::de::IgnoredAny = map.next_value()?;
                        }
                    }
                }

                let duration_ms =
                    duration_ms.ok_or_else(|| de::Error::missing_field("duration_ms"))?;
                Ok(RetryStrategy::Fixed(Duration::from_millis(duration_ms)))
            }
        }

        struct LinearVisitor;
        impl<'de> Visitor<'de> for LinearVisitor {
            type Value = RetryStrategy;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("linear retry strategy data")
            }

            fn visit_map<M>(self, mut map: M) -> Result<Self::Value, M::Error>
            where
                M: MapAccess<'de>,
            {
                let mut base_ms = None;
                let mut increment_ms = None;
                let mut max_delay_ms = None;

                while let Some(key) = map.next_key::<&str>()? {
                    match key {
                        "base_ms" => {
                            if base_ms.is_some() {
                                return Err(de::Error::duplicate_field("base_ms"));
                            }
                            base_ms = Some(map.next_value::<u64>()?);
                        }
                        "increment_ms" => {
                            if increment_ms.is_some() {
                                return Err(de::Error::duplicate_field("increment_ms"));
                            }
                            increment_ms = Some(map.next_value::<u64>()?);
                        }
                        "max_delay_ms" => {
                            if max_delay_ms.is_some() {
                                return Err(de::Error::duplicate_field("max_delay_ms"));
                            }
                            max_delay_ms = Some(map.next_value::<Option<u64>>()?);
                        }
                        _ => {
                            let _: serde::de::IgnoredAny = map.next_value()?;
                        }
                    }
                }

                let base_ms = base_ms.ok_or_else(|| de::Error::missing_field("base_ms"))?;
                let increment_ms =
                    increment_ms.ok_or_else(|| de::Error::missing_field("increment_ms"))?;

                Ok(RetryStrategy::Linear {
                    base: Duration::from_millis(base_ms),
                    increment: Duration::from_millis(increment_ms),
                    max_delay: max_delay_ms.flatten().map(Duration::from_millis),
                })
            }
        }

        struct ExponentialVisitor;
        impl<'de> Visitor<'de> for ExponentialVisitor {
            type Value = RetryStrategy;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("exponential retry strategy data")
            }

            fn visit_map<M>(self, mut map: M) -> Result<Self::Value, M::Error>
            where
                M: MapAccess<'de>,
            {
                let mut base_ms = None;
                let mut multiplier = None;
                let mut max_delay_ms = None;
                let mut jitter = None;

                while let Some(key) = map.next_key::<&str>()? {
                    match key {
                        "base_ms" => {
                            if base_ms.is_some() {
                                return Err(de::Error::duplicate_field("base_ms"));
                            }
                            base_ms = Some(map.next_value::<u64>()?);
                        }
                        "multiplier" => {
                            if multiplier.is_some() {
                                return Err(de::Error::duplicate_field("multiplier"));
                            }
                            multiplier = Some(map.next_value::<f64>()?);
                        }
                        "max_delay_ms" => {
                            if max_delay_ms.is_some() {
                                return Err(de::Error::duplicate_field("max_delay_ms"));
                            }
                            max_delay_ms = Some(map.next_value::<Option<u64>>()?);
                        }
                        "jitter" => {
                            if jitter.is_some() {
                                return Err(de::Error::duplicate_field("jitter"));
                            }
                            jitter = Some(map.next_value::<Option<JitterType>>()?);
                        }
                        _ => {
                            let _: serde::de::IgnoredAny = map.next_value()?;
                        }
                    }
                }

                let base_ms = base_ms.ok_or_else(|| de::Error::missing_field("base_ms"))?;
                let multiplier =
                    multiplier.ok_or_else(|| de::Error::missing_field("multiplier"))?;

                Ok(RetryStrategy::Exponential {
                    base: Duration::from_millis(base_ms),
                    multiplier,
                    max_delay: max_delay_ms.flatten().map(Duration::from_millis),
                    jitter: jitter.flatten(),
                })
            }
        }

        struct FibonacciVisitor;
        impl<'de> Visitor<'de> for FibonacciVisitor {
            type Value = RetryStrategy;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("fibonacci retry strategy data")
            }

            fn visit_map<M>(self, mut map: M) -> Result<Self::Value, M::Error>
            where
                M: MapAccess<'de>,
            {
                let mut base_ms = None;
                let mut max_delay_ms = None;

                while let Some(key) = map.next_key::<&str>()? {
                    match key {
                        "base_ms" => {
                            if base_ms.is_some() {
                                return Err(de::Error::duplicate_field("base_ms"));
                            }
                            base_ms = Some(map.next_value::<u64>()?);
                        }
                        "max_delay_ms" => {
                            if max_delay_ms.is_some() {
                                return Err(de::Error::duplicate_field("max_delay_ms"));
                            }
                            max_delay_ms = Some(map.next_value::<Option<u64>>()?);
                        }
                        _ => {
                            let _: serde::de::IgnoredAny = map.next_value()?;
                        }
                    }
                }

                let base_ms = base_ms.ok_or_else(|| de::Error::missing_field("base_ms"))?;

                Ok(RetryStrategy::Fibonacci {
                    base: Duration::from_millis(base_ms),
                    max_delay: max_delay_ms.flatten().map(Duration::from_millis),
                })
            }
        }

        deserializer.deserialize_enum(
            "RetryStrategy",
            &["Fixed", "Linear", "Exponential", "Fibonacci"],
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
        let strategy = RetryStrategy::Custom(Box::new(|attempt| match attempt {
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
        let strategy = RetryStrategy::Custom(Box::new(|_| Duration::from_millis(0)));
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
}
