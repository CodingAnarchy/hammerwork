//! Job prioritization system for controlling execution order in the job queue.
//!
//! This module provides a comprehensive priority system with five priority levels and
//! flexible scheduling algorithms to ensure critical jobs are processed first while
//! preventing starvation of lower priority jobs.
//!
//! ## Priority Levels
//!
//! - **Critical (4)**: System alerts, emergency responses, critical failures
//! - **High (3)**: User-facing notifications, important API calls, urgent processing
//! - **Normal (2)**: Standard application jobs, regular processing (default)
//! - **Low (1)**: Analytics, non-urgent background tasks, optimization jobs
//! - **Background (0)**: Cleanup tasks, maintenance, lowest priority work
//!
//! ## Scheduling Algorithms
//!
//! ### Weighted Priority Scheduling (Default)
//!
//! Uses configurable weights to make higher priority jobs more likely to be selected
//! while still allowing lower priority jobs to be processed, preventing starvation.
//!
//! ### Strict Priority Scheduling
//!
//! Always processes the highest priority jobs first. Lower priority jobs are only
//! processed when no higher priority jobs are available.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use thiserror::Error;

/// Job priority levels that determine execution order.
///
/// Priority affects how jobs are selected for processing by workers. Higher priority
/// jobs are generally processed before lower priority jobs, but the exact behavior
/// depends on the scheduling algorithm configured on the worker.
///
/// # Examples
///
/// ```rust
/// use hammerwork::JobPriority;
/// use std::str::FromStr;
///
/// // Create priority from string
/// let priority = JobPriority::from_str("high").unwrap();
/// assert_eq!(priority, JobPriority::High);
///
/// // Get numeric value for database storage
/// assert_eq!(priority.as_i32(), 3);
///
/// // Check priority ordering
/// assert!(JobPriority::Critical > JobPriority::High);
/// assert!(JobPriority::High > JobPriority::Normal);
/// assert!(JobPriority::Normal > JobPriority::Low);
/// assert!(JobPriority::Low > JobPriority::Background);
/// ```
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default,
)]
pub enum JobPriority {
    /// Background jobs - lowest priority, execute when no other jobs available.
    ///
    /// Suitable for cleanup tasks, maintenance work, or any jobs that can wait
    /// indefinitely without impacting user experience.
    Background = 0,

    /// Low priority jobs - execute after normal and higher priority jobs.
    ///
    /// Suitable for analytics, reporting, optimization tasks, or background
    /// processing that isn't time-sensitive.
    Low = 1,

    /// Normal priority jobs - default priority level.
    ///
    /// The default priority for most application jobs. Suitable for standard
    /// business logic, regular data processing, and typical application workflows.
    #[default]
    Normal = 2,

    /// High priority jobs - execute before normal and lower priority jobs.
    ///
    /// Suitable for user-facing operations, important notifications, API responses,
    /// or time-sensitive business operations.
    High = 3,

    /// Critical priority jobs - highest priority, execute immediately.
    ///
    /// Should be used sparingly for truly urgent work like system alerts,
    /// emergency responses, security incidents, or critical system recovery.
    Critical = 4,
}

impl JobPriority {
    /// Gets the numeric value of the priority for database storage.
    ///
    /// This is used internally for storing priority values in the database
    /// and for comparison operations.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::JobPriority;
    ///
    /// assert_eq!(JobPriority::Background.as_i32(), 0);
    /// assert_eq!(JobPriority::Low.as_i32(), 1);
    /// assert_eq!(JobPriority::Normal.as_i32(), 2);
    /// assert_eq!(JobPriority::High.as_i32(), 3);
    /// assert_eq!(JobPriority::Critical.as_i32(), 4);
    /// ```
    pub fn as_i32(self) -> i32 {
        self as i32
    }

    /// Creates a JobPriority from an i32 value.
    ///
    /// This is used when loading priority values from the database or
    /// parsing priority values from external sources.
    ///
    /// # Arguments
    ///
    /// * `value` - The numeric priority value (0-4)
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::JobPriority;
    ///
    /// assert_eq!(JobPriority::from_i32(0).unwrap(), JobPriority::Background);
    /// assert_eq!(JobPriority::from_i32(2).unwrap(), JobPriority::Normal);
    /// assert_eq!(JobPriority::from_i32(4).unwrap(), JobPriority::Critical);
    ///
    /// // Invalid values return an error
    /// assert!(JobPriority::from_i32(10).is_err());
    /// ```
    pub fn from_i32(value: i32) -> Result<Self, PriorityError> {
        match value {
            0 => Ok(JobPriority::Background),
            1 => Ok(JobPriority::Low),
            2 => Ok(JobPriority::Normal),
            3 => Ok(JobPriority::High),
            4 => Ok(JobPriority::Critical),
            _ => Err(PriorityError::InvalidPriorityValue(value)),
        }
    }

    /// Gets a human-readable description of the priority.
    ///
    /// Provides detailed information about when each priority level
    /// should be used and how it affects job processing.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::JobPriority;
    ///
    /// let desc = JobPriority::Critical.description();
    /// assert!(desc.contains("highest priority"));
    ///
    /// let normal_desc = JobPriority::Normal.description();
    /// assert!(normal_desc.contains("default"));
    /// ```
    pub fn description(self) -> &'static str {
        match self {
            JobPriority::Background => "Background - execute when no other jobs available",
            JobPriority::Low => "Low - execute after normal and higher priority jobs",
            JobPriority::Normal => "Normal - default priority level",
            JobPriority::High => "High - execute before normal and lower priority jobs",
            JobPriority::Critical => "Critical - highest priority, execute immediately",
        }
    }

    /// Gets all priority levels in order from lowest to highest.
    ///
    /// This is useful for iterating over all possible priority levels
    /// or for building user interfaces that need to display all options.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::JobPriority;
    ///
    /// let priorities = JobPriority::all_priorities();
    /// assert_eq!(priorities.len(), 5);
    /// assert_eq!(priorities[0], JobPriority::Background);
    /// assert_eq!(priorities[4], JobPriority::Critical);
    ///
    /// // Verify they're in ascending order
    /// for i in 1..priorities.len() {
    ///     assert!(priorities[i] > priorities[i-1]);
    /// }
    /// ```
    pub fn all_priorities() -> Vec<JobPriority> {
        vec![
            JobPriority::Background,
            JobPriority::Low,
            JobPriority::Normal,
            JobPriority::High,
            JobPriority::Critical,
        ]
    }
}

impl std::fmt::Display for JobPriority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JobPriority::Background => write!(f, "background"),
            JobPriority::Low => write!(f, "low"),
            JobPriority::Normal => write!(f, "normal"),
            JobPriority::High => write!(f, "high"),
            JobPriority::Critical => write!(f, "critical"),
        }
    }
}

impl std::str::FromStr for JobPriority {
    type Err = PriorityError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "background" | "bg" => Ok(JobPriority::Background),
            "low" | "l" => Ok(JobPriority::Low),
            "normal" | "n" | "default" => Ok(JobPriority::Normal),
            "high" | "h" => Ok(JobPriority::High),
            "critical" | "crit" | "c" => Ok(JobPriority::Critical),
            _ => Err(PriorityError::InvalidPriorityString(s.to_string())),
        }
    }
}

/// Configuration for priority-based job processing weights.
///
/// This struct controls how jobs of different priorities are selected for processing.
/// It supports both weighted priority scheduling (where higher priority jobs are more
/// likely to be selected) and strict priority scheduling (where higher priority jobs
/// are always selected first).
///
/// # Weighted Priority Scheduling
///
/// In weighted mode, jobs are selected based on their priority weights. Higher weights
/// make jobs more likely to be selected, but don't guarantee they'll always be processed
/// first. This prevents starvation of lower priority jobs.
///
/// # Strict Priority Scheduling
///
/// In strict mode, jobs are always processed in strict priority order. Higher priority
/// jobs are processed first, and lower priority jobs are only processed when no higher
/// priority jobs are available.
///
/// # Examples
///
/// ```rust
/// use hammerwork::{PriorityWeights, JobPriority};
///
/// // Default weighted configuration
/// let weights = PriorityWeights::new();
/// assert!(!weights.is_strict());
/// assert_eq!(weights.fairness_factor(), 0.1);
///
/// // Custom weighted configuration
/// let custom_weights = PriorityWeights::new()
///     .with_weight(JobPriority::Critical, 50)
///     .with_weight(JobPriority::High, 20)
///     .with_weight(JobPriority::Normal, 10)
///     .with_fairness_factor(0.15);
///
/// // Strict priority configuration
/// let strict_weights = PriorityWeights::strict();
/// assert!(strict_weights.is_strict());
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PriorityWeights {
    /// Weight for each priority level - higher weight means more likely to be selected
    weights: HashMap<JobPriority, u32>,
    /// Whether to use strict priority (always process highest priority first)
    strict_priority: bool,
    /// Fairness factor to prevent starvation (0.0 = no fairness, 1.0 = round-robin)
    fairness_factor: f32,
}

impl PriorityWeights {
    /// Creates a new PriorityWeights configuration with default weights.
    ///
    /// The default weights are designed to provide reasonable priority differentiation
    /// while preventing starvation of lower priority jobs:
    /// - Critical: 20 (20x more likely than background)
    /// - High: 10 (10x more likely than background)
    /// - Normal: 5 (5x more likely than background)
    /// - Low: 2 (2x more likely than background)
    /// - Background: 1 (baseline)
    ///
    /// A fairness factor of 0.1 (10%) is included to ensure lower priority jobs
    /// still get some processing time.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::{PriorityWeights, JobPriority};
    ///
    /// let weights = PriorityWeights::new();
    /// assert_eq!(weights.get_weight(JobPriority::Critical), 20);
    /// assert_eq!(weights.get_weight(JobPriority::Normal), 5);
    /// assert_eq!(weights.get_weight(JobPriority::Background), 1);
    /// assert_eq!(weights.fairness_factor(), 0.1);
    /// ```
    pub fn new() -> Self {
        let mut weights = HashMap::new();
        weights.insert(JobPriority::Background, 1);
        weights.insert(JobPriority::Low, 2);
        weights.insert(JobPriority::Normal, 5);
        weights.insert(JobPriority::High, 10);
        weights.insert(JobPriority::Critical, 20);

        Self {
            weights,
            strict_priority: false,
            fairness_factor: 0.1, // 10% fairness by default
        }
    }

    /// Creates strict priority weights (always process highest priority first).
    ///
    /// In strict priority mode, jobs are processed in strict priority order.
    /// Higher priority jobs are always processed before lower priority jobs,
    /// regardless of how long lower priority jobs have been waiting.
    ///
    /// This mode is suitable for systems where priority order must be strictly
    /// maintained, such as real-time systems or critical infrastructure.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::PriorityWeights;
    ///
    /// let strict_weights = PriorityWeights::strict();
    /// assert!(strict_weights.is_strict());
    /// assert_eq!(strict_weights.fairness_factor(), 0.0);
    /// ```
    pub fn strict() -> Self {
        Self {
            weights: HashMap::new(), // Weights don't matter in strict mode
            strict_priority: true,
            fairness_factor: 0.0,
        }
    }

    /// Set a custom weight for a specific priority level
    pub fn with_weight(mut self, priority: JobPriority, weight: u32) -> Self {
        self.weights.insert(priority, weight);
        self
    }

    /// Set multiple priority weights at once
    pub fn with_weights(mut self, weights: HashMap<JobPriority, u32>) -> Self {
        self.weights.extend(weights);
        self
    }

    /// Enable or disable strict priority mode
    pub fn with_strict_priority(mut self, strict: bool) -> Self {
        self.strict_priority = strict;
        self
    }

    /// Set the fairness factor (0.0 = no fairness, 1.0 = round-robin)
    pub fn with_fairness_factor(mut self, factor: f32) -> Self {
        self.fairness_factor = factor.clamp(0.0, 1.0);
        self
    }

    /// Get the weight for a specific priority level
    pub fn get_weight(&self, priority: JobPriority) -> u32 {
        if self.strict_priority {
            // In strict mode, weight is the priority level itself
            priority.as_i32() as u32
        } else {
            self.weights.get(&priority).copied().unwrap_or(1)
        }
    }

    /// Check if strict priority mode is enabled
    pub fn is_strict(&self) -> bool {
        self.strict_priority
    }

    /// Get the fairness factor
    pub fn fairness_factor(&self) -> f32 {
        self.fairness_factor
    }

    /// Calculate the total weight across all priorities
    pub fn total_weight(&self) -> u32 {
        if self.strict_priority {
            // In strict mode, just return a sum of priority values
            JobPriority::all_priorities()
                .into_iter()
                .map(|p| p.as_i32() as u32)
                .sum()
        } else {
            self.weights.values().sum()
        }
    }

    /// Get all configured weights
    pub fn weights(&self) -> &HashMap<JobPriority, u32> {
        &self.weights
    }
}

impl Default for PriorityWeights {
    fn default() -> Self {
        Self::new()
    }
}

/// Errors related to priority handling
#[derive(Error, Debug)]
pub enum PriorityError {
    #[error("Invalid priority value: {0}. Must be between 0 and 4")]
    InvalidPriorityValue(i32),

    #[error(
        "Invalid priority string: '{0}'. Valid values are: background, low, normal, high, critical"
    )]
    InvalidPriorityString(String),

    #[error("Priority weights cannot be empty")]
    EmptyWeights,

    #[error("Priority calculation error: {0}")]
    CalculationError(String),
}

/// Priority-aware job selection strategy
#[derive(Debug, Clone, Default)]
pub enum PrioritySelectionStrategy {
    /// Weighted random selection based on priority weights
    #[default]
    WeightedRandom,
    /// Strict priority - always select highest priority first
    Strict,
    /// Round-robin with priority boost
    RoundRobinWithBoost,
    /// Fair scheduling with priority weights
    FairWeighted,
}

/// Priority queue statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PriorityStats {
    /// Number of jobs per priority level
    pub job_counts: HashMap<JobPriority, u64>,
    /// Average processing time per priority level
    pub avg_processing_times: HashMap<JobPriority, f64>,
    /// Jobs processed in the last time window per priority
    pub recent_throughput: HashMap<JobPriority, u64>,
    /// Priority distribution percentages
    pub priority_distribution: HashMap<JobPriority, f32>,
}

impl PriorityStats {
    pub fn new() -> Self {
        Self {
            job_counts: HashMap::new(),
            avg_processing_times: HashMap::new(),
            recent_throughput: HashMap::new(),
            priority_distribution: HashMap::new(),
        }
    }

    /// Calculate priority distribution percentages
    pub fn calculate_distribution(&mut self) {
        let total: u64 = self.job_counts.values().sum();
        if total > 0 {
            for (priority, count) in &self.job_counts {
                let percentage = (*count as f32 / total as f32) * 100.0;
                self.priority_distribution.insert(*priority, percentage);
            }
        }
    }

    /// Get the most active priority level
    pub fn most_active_priority(&self) -> Option<JobPriority> {
        self.job_counts
            .iter()
            .max_by_key(|(_, count)| *count)
            .map(|(priority, _)| *priority)
    }

    /// Check if there's priority starvation (low/background jobs not being processed)
    pub fn check_starvation(&self, threshold_percentage: f32) -> Vec<JobPriority> {
        let mut starved_priorities = Vec::new();

        for priority in [JobPriority::Background, JobPriority::Low] {
            if let Some(percentage) = self.priority_distribution.get(&priority) {
                if *percentage < threshold_percentage {
                    starved_priorities.push(priority);
                }
            }
        }

        starved_priorities
    }
}

impl Default for PriorityStats {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_job_priority_ordering() {
        assert!(JobPriority::Critical > JobPriority::High);
        assert!(JobPriority::High > JobPriority::Normal);
        assert!(JobPriority::Normal > JobPriority::Low);
        assert!(JobPriority::Low > JobPriority::Background);
    }

    #[test]
    fn test_job_priority_conversion() {
        assert_eq!(JobPriority::Normal.as_i32(), 2);
        assert_eq!(JobPriority::from_i32(2).unwrap(), JobPriority::Normal);
        assert!(JobPriority::from_i32(10).is_err());
    }

    #[test]
    fn test_job_priority_string_parsing() {
        assert_eq!("high".parse::<JobPriority>().unwrap(), JobPriority::High);
        assert_eq!(
            "normal".parse::<JobPriority>().unwrap(),
            JobPriority::Normal
        );
        assert_eq!(
            "crit".parse::<JobPriority>().unwrap(),
            JobPriority::Critical
        );
        assert!("invalid".parse::<JobPriority>().is_err());
    }

    #[test]
    fn test_priority_weights_default() {
        let weights = PriorityWeights::new();
        assert_eq!(weights.get_weight(JobPriority::Normal), 5);
        assert_eq!(weights.get_weight(JobPriority::Critical), 20);
        assert!(!weights.is_strict());
    }

    #[test]
    fn test_priority_weights_strict() {
        let weights = PriorityWeights::strict();
        assert!(weights.is_strict());
        assert_eq!(weights.get_weight(JobPriority::Critical), 4);
        assert_eq!(weights.get_weight(JobPriority::Background), 0);
    }

    #[test]
    fn test_priority_weights_custom() {
        let weights = PriorityWeights::new()
            .with_weight(JobPriority::High, 15)
            .with_fairness_factor(0.2);

        assert_eq!(weights.get_weight(JobPriority::High), 15);
        assert_eq!(weights.fairness_factor(), 0.2);
    }

    #[test]
    fn test_priority_stats() {
        let mut stats = PriorityStats::new();
        stats.job_counts.insert(JobPriority::Normal, 80);
        stats.job_counts.insert(JobPriority::High, 15);
        stats.job_counts.insert(JobPriority::Low, 5);

        stats.calculate_distribution();

        assert_eq!(stats.priority_distribution[&JobPriority::Normal], 80.0);
        assert_eq!(stats.most_active_priority(), Some(JobPriority::Normal));

        let starved = stats.check_starvation(10.0);
        assert!(starved.contains(&JobPriority::Low));
    }

    #[test]
    fn test_priority_display() {
        assert_eq!(JobPriority::Critical.to_string(), "critical");
        assert_eq!(JobPriority::Normal.to_string(), "normal");
        assert_eq!(JobPriority::Background.to_string(), "background");
    }

    #[test]
    fn test_priority_description() {
        assert!(
            JobPriority::Critical
                .description()
                .contains("highest priority")
        );
        assert!(JobPriority::Normal.description().contains("default"));
    }

    #[test]
    fn test_all_priorities() {
        let priorities = JobPriority::all_priorities();
        assert_eq!(priorities.len(), 5);
        assert_eq!(priorities[0], JobPriority::Background);
        assert_eq!(priorities[4], JobPriority::Critical);
    }
}
