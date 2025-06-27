//! Example demonstrating the retry strategy functionality in Hammerwork
//!
//! This example shows how to use different retry strategies:
//! 1. Job-specific retry strategy (takes precedence)
//! 2. Worker default retry strategy (fallback)
//! 3. Fixed retry delay (legacy fallback)

use hammerwork::{Job, Worker, retry::RetryStrategy, JitterType};
use serde_json::json;
use std::time::Duration;

#[tokio::main]
async fn main() {
    println!("=== Hammerwork Retry Strategy Example ===\n");
    
    // Example 1: Job with exponential backoff strategy
    println!("1. Job with exponential backoff:");
    let job_with_exponential = Job::new("test".to_string(), json!({"task": "api_call"}))
        .with_retry_strategy(RetryStrategy::exponential(
            Duration::from_secs(1),    // base delay
            2.0,                       // multiplier
            Some(Duration::from_secs(300)) // max delay (5 minutes)
        ));
    
    println!("   Retry delays for exponential backoff:");
    if let Some(strategy) = &job_with_exponential.retry_strategy {
        for attempt in 1..=6 {
            let delay = strategy.calculate_delay(attempt);
            println!("   Attempt {}: {:?}", attempt, delay);
        }
    }
    println!();
    
    // Example 2: Job with linear backoff strategy
    println!("2. Job with linear backoff:");
    let job_with_linear = Job::new("test".to_string(), json!({"task": "database_operation"}))
        .with_retry_strategy(RetryStrategy::linear(
            Duration::from_secs(5),     // base delay
            Duration::from_secs(10),    // increment
            Some(Duration::from_secs(120)) // max delay (2 minutes)
        ));
    
    println!("   Retry delays for linear backoff:");
    if let Some(strategy) = &job_with_linear.retry_strategy {
        for attempt in 1..=8 {
            let delay = strategy.calculate_delay(attempt);
            println!("   Attempt {}: {:?}", attempt, delay);
        }
    }
    println!();
    
    // Example 3: Job with Fibonacci backoff strategy
    println!("3. Job with Fibonacci backoff:");
    let job_with_fibonacci = Job::new("test".to_string(), json!({"task": "file_processing"}))
        .with_retry_strategy(RetryStrategy::fibonacci(
            Duration::from_secs(2),     // base delay
            Some(Duration::from_secs(180)) // max delay (3 minutes)
        ));
    
    println!("   Retry delays for Fibonacci backoff:");
    if let Some(strategy) = &job_with_fibonacci.retry_strategy {
        for attempt in 1..=10 {
            let delay = strategy.calculate_delay(attempt);
            println!("   Attempt {}: {:?}", attempt, delay);
        }
    }
    println!();
    
    // Example 4: Job with exponential backoff + jitter
    println!("4. Job with exponential backoff + jitter:");
    let job_with_jitter = Job::new("test".to_string(), json!({"task": "external_api"}))
        .with_retry_strategy(RetryStrategy::exponential_with_jitter(
            Duration::from_secs(1),     // base delay
            2.0,                        // multiplier
            Some(Duration::from_secs(300)), // max delay
            JitterType::Multiplicative(0.2) // ±20% jitter
        ));
    
    println!("   Retry delays for exponential backoff with jitter:");
    println!("   (Note: jitter makes each run different)");
    if let Some(strategy) = &job_with_jitter.retry_strategy {
        for attempt in 1..=5 {
            let delay = strategy.calculate_delay(attempt);
            println!("   Attempt {}: {:?}", attempt, delay);
        }
    }
    println!();
    
    // Example 5: Custom retry strategy
    println!("5. Job with custom retry strategy:");
    let custom_strategy = RetryStrategy::custom(|attempt| {
        match attempt {
            1..=3 => Duration::from_secs(5),   // Quick retries for transient issues
            4..=6 => Duration::from_secs(60),  // Medium delays for persistent issues
            _ => Duration::from_secs(300),     // Long delays for severe issues
        }
    });
    
    let job_with_custom = Job::new("test".to_string(), json!({"task": "complex_operation"}))
        .with_retry_strategy(custom_strategy);
    
    println!("   Retry delays for custom strategy:");
    if let Some(strategy) = &job_with_custom.retry_strategy {
        for attempt in 1..=8 {
            let delay = strategy.calculate_delay(attempt);
            println!("   Attempt {}: {:?}", attempt, delay);
        }
    }
    println!();
    
    println!("=== Worker Configuration Examples ===\n");
    
    // Example: Worker with default retry strategy
    println!("6. Worker with default exponential backoff strategy:");
    println!("   This strategy applies to jobs that don't have their own retry strategy.");
    println!("   Default strategy: exponential backoff (1s base, 1.5x multiplier, 10min max)");
    
    // Note: We can't actually create a complete worker without a database connection,
    // but we can show how it would be configured:
    println!("   
    let worker = Worker::new(queue, \"api_queue\".to_string(), handler)
        .with_default_retry_strategy(RetryStrategy::exponential(
            Duration::from_secs(1),
            1.5,
            Some(Duration::from_secs(600))
        ));
    ");
    
    println!("\n=== Retry Strategy Priority ===");
    println!("When a job fails, the retry delay is calculated using this priority:");
    println!("1. Job-specific retry strategy (highest priority)");
    println!("2. Worker default retry strategy");
    println!("3. Fixed retry delay (legacy fallback)");
    println!("\nThis allows for flexible retry behavior where individual jobs can");
    println!("override the worker's default strategy when needed.");
}