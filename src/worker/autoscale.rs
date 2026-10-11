//! Worker pool autoscaling: its configuration ([`AutoscaleConfig`]), metrics and
//! scaling decision.

use super::*;

/// Configuration for worker autoscaling behavior.
///
/// A [`WorkerPool`] does not autoscale until it is given a configuration with
/// [`WorkerPool::with_autoscaling`]. [`AutoscaleConfig::default`] (and the presets) are
/// enabled, so `with_autoscaling(AutoscaleConfig::default())` turns autoscaling on; see
/// [`WorkerPool::with_autoscaling`] for which workers it starts and retires.
#[derive(Debug, Clone)]
pub struct AutoscaleConfig {
    /// Whether autoscaling is enabled
    pub enabled: bool,
    /// Minimum number of workers to maintain
    pub min_workers: usize,
    /// Maximum number of workers to allow
    pub max_workers: usize,
    /// Queue depth per worker threshold to trigger scale-up
    pub scale_up_threshold: usize,
    /// Queue depth per worker threshold to trigger scale-down
    pub scale_down_threshold: usize,
    /// Minimum time between scaling decisions
    pub cooldown_period: Duration,
    /// Number of workers to add/remove during scaling events
    pub scale_step: usize,
    /// Time window for queue depth averaging
    pub evaluation_window: Duration,
}

impl Default for AutoscaleConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            min_workers: 1,
            max_workers: 10,
            scale_up_threshold: 5,
            scale_down_threshold: 2,
            cooldown_period: Duration::from_secs(60),
            scale_step: 1,
            evaluation_window: Duration::from_secs(30),
        }
    }
}

impl AutoscaleConfig {
    /// Create a new autoscale configuration with default values
    pub fn new() -> Self {
        Self::default()
    }

    /// Enable or disable autoscaling
    pub fn with_enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Set the minimum number of workers (at least 1). The maximum is raised to the
    /// minimum when it is lower.
    pub fn with_min_workers(mut self, min_workers: usize) -> Self {
        self.min_workers = min_workers.max(1); // Ensure at least 1 worker
        self.max_workers = self.max_workers.max(self.min_workers);
        self
    }

    /// Set the maximum number of workers
    pub fn with_max_workers(mut self, max_workers: usize) -> Self {
        self.max_workers = max_workers.max(self.min_workers);
        self
    }

    /// Set the queue depth threshold for scaling up
    pub fn with_scale_up_threshold(mut self, threshold: usize) -> Self {
        self.scale_up_threshold = threshold.max(1);
        self
    }

    /// Set the queue depth threshold for scaling down
    pub fn with_scale_down_threshold(mut self, threshold: usize) -> Self {
        self.scale_down_threshold = threshold;
        self
    }

    /// Set the cooldown period between scaling decisions
    pub fn with_cooldown_period(mut self, period: Duration) -> Self {
        self.cooldown_period = period;
        self
    }

    /// Set the number of workers to add/remove per scaling event
    pub fn with_scale_step(mut self, step: usize) -> Self {
        self.scale_step = step.max(1);
        self
    }

    /// Set the evaluation window for queue depth averaging
    pub fn with_evaluation_window(mut self, window: Duration) -> Self {
        self.evaluation_window = window;
        self
    }

    /// Create a conservative autoscaling configuration
    pub fn conservative() -> Self {
        Self {
            enabled: true,
            min_workers: 2,
            max_workers: 5,
            scale_up_threshold: 10,
            scale_down_threshold: 1,
            cooldown_period: Duration::from_secs(300), // 5 mins
            scale_step: 1,
            evaluation_window: Duration::from_secs(60),
        }
    }

    /// Create an aggressive autoscaling configuration
    pub fn aggressive() -> Self {
        Self {
            enabled: true,
            min_workers: 1,
            max_workers: 20,
            scale_up_threshold: 3,
            scale_down_threshold: 1,
            cooldown_period: Duration::from_secs(30),
            scale_step: 2,
            evaluation_window: Duration::from_secs(15),
        }
    }

    /// Disable autoscaling
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            ..Self::default()
        }
    }
}

/// Metrics for autoscaling decisions
#[derive(Debug, Clone, Default)]
pub struct AutoscaleMetrics {
    /// Current number of active workers: those of the autoscaled queue when
    /// autoscaling is enabled, otherwise all of the pool's workers
    pub active_workers: usize,
    /// Average queue depth over evaluation window
    pub avg_queue_depth: f64,
    /// Current queue depth
    pub current_queue_depth: u64,
    /// Jobs processed per second (recent average)
    pub jobs_per_second: f64,
    /// Average worker utilization (0.0 to 1.0)
    pub worker_utilization: f64,
    /// Time since last scaling action
    pub time_since_last_scale: Duration,
    /// Timestamp of last scaling decision
    pub last_scale_time: Option<chrono::DateTime<Utc>>,
}

/// Scaling decision enum
#[derive(Debug, Clone, Copy)]
enum ScalingDecision {
    ScaleUp,
    ScaleDown,
}

/// Type alias for queue depth history storage
pub(super) type QueueDepthHistory = Arc<std::sync::RwLock<Vec<(chrono::DateTime<Utc>, u64)>>>;

/// Record a queue depth sample and decide whether to scale.
///
/// Returns the new worker count (within `min_workers..=max_workers`) when the
/// average depth per worker crossed a threshold outside the cooldown period.
pub(super) fn apply_queue_depth(
    current_depth: u64,
    now: DateTime<Utc>,
    config: &AutoscaleConfig,
    metrics: &Arc<std::sync::RwLock<AutoscaleMetrics>>,
    history: &QueueDepthHistory,
) -> Option<usize> {
    // Update queue depth history, dropping entries outside the evaluation window
    let avg_depth = match history.write() {
        Ok(mut hist) => {
            hist.push((now, current_depth));
            let cutoff = now
                - chrono::Duration::from_std(config.evaluation_window)
                    .unwrap_or(chrono::Duration::seconds(30));
            hist.retain(|(timestamp, _)| *timestamp > cutoff);
            if hist.is_empty() {
                current_depth as f64
            } else {
                hist.iter().map(|(_, depth)| *depth as f64).sum::<f64>() / hist.len() as f64
            }
        }
        Err(_) => current_depth as f64,
    };

    let mut m = metrics.write().ok()?;
    m.current_queue_depth = current_depth;
    m.avg_queue_depth = avg_depth;

    // Check cooldown period
    let time_since_last = m
        .last_scale_time
        .map(|t| now - t)
        .and_then(|d| d.to_std().ok())
        .unwrap_or(config.cooldown_period);
    m.time_since_last_scale = time_since_last;
    if time_since_last < config.cooldown_period {
        return None;
    }

    // Calculate queue depth per worker
    let depth_per_worker = if m.active_workers > 0 {
        avg_depth / m.active_workers as f64
    } else {
        avg_depth
    };

    let decision = if depth_per_worker > config.scale_up_threshold as f64
        && m.active_workers < config.max_workers
    {
        ScalingDecision::ScaleUp
    } else if depth_per_worker < config.scale_down_threshold as f64
        && m.active_workers > config.min_workers
    {
        ScalingDecision::ScaleDown
    } else {
        return None;
    };

    let new_count = match decision {
        ScalingDecision::ScaleUp => (m.active_workers + config.scale_step).min(config.max_workers),
        ScalingDecision::ScaleDown => {
            (m.active_workers.saturating_sub(config.scale_step)).max(config.min_workers)
        }
    };
    info!(
        "Autoscaling: {:?} from {} to {} workers (avg queue depth: {:.1})",
        decision, m.active_workers, new_count, m.avg_queue_depth
    );
    m.active_workers = new_count;
    m.last_scale_time = Some(now);
    Some(new_count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_autoscale_config_defaults() {
        let config = AutoscaleConfig::default();

        assert!(config.enabled);
        assert_eq!(config.min_workers, 1);
        assert_eq!(config.max_workers, 10);
        assert_eq!(config.scale_up_threshold, 5);
        assert_eq!(config.scale_down_threshold, 2);
        assert_eq!(config.cooldown_period, Duration::from_secs(60));
        assert_eq!(config.scale_step, 1);
        assert_eq!(config.evaluation_window, Duration::from_secs(30));
    }

    #[test]
    fn test_autoscale_config_builder() {
        let config = AutoscaleConfig::new()
            .with_min_workers(2)
            .with_max_workers(20)
            .with_scale_up_threshold(8)
            .with_scale_down_threshold(1)
            .with_cooldown_period(Duration::from_secs(120))
            .with_scale_step(2)
            .with_evaluation_window(Duration::from_secs(45));

        assert_eq!(config.min_workers, 2);
        assert_eq!(config.max_workers, 20);
        assert_eq!(config.scale_up_threshold, 8);
        assert_eq!(config.scale_down_threshold, 1);
        assert_eq!(config.cooldown_period, Duration::from_secs(120));
        assert_eq!(config.scale_step, 2);
        assert_eq!(config.evaluation_window, Duration::from_secs(45));
    }

    #[test]
    fn test_autoscale_config_presets() {
        let conservative = AutoscaleConfig::conservative();
        assert_eq!(conservative.min_workers, 2);
        assert_eq!(conservative.max_workers, 5);
        assert_eq!(conservative.scale_up_threshold, 10);
        assert_eq!(conservative.cooldown_period, Duration::from_secs(300));

        let aggressive = AutoscaleConfig::aggressive();
        assert_eq!(aggressive.min_workers, 1);
        assert_eq!(aggressive.max_workers, 20);
        assert_eq!(aggressive.scale_up_threshold, 3);
        assert_eq!(aggressive.cooldown_period, Duration::from_secs(30));

        let disabled = AutoscaleConfig::disabled();
        assert!(!disabled.enabled);
    }

    #[test]
    fn test_autoscale_config_validation() {
        // Test that min_workers is at least 1
        let config = AutoscaleConfig::new().with_min_workers(0);
        assert_eq!(config.min_workers, 1);

        // Test that max_workers is at least min_workers
        let config = AutoscaleConfig::new()
            .with_min_workers(5)
            .with_max_workers(3);
        assert_eq!(config.max_workers, 5);

        // Test that scale_up_threshold is at least 1
        let config = AutoscaleConfig::new().with_scale_up_threshold(0);
        assert_eq!(config.scale_up_threshold, 1);

        // Test that scale_step is at least 1
        let config = AutoscaleConfig::new().with_scale_step(0);
        assert_eq!(config.scale_step, 1);
    }

    #[test]
    fn test_autoscale_metrics_default() {
        let metrics = AutoscaleMetrics::default();

        assert_eq!(metrics.active_workers, 0);
        assert_eq!(metrics.avg_queue_depth, 0.0);
        assert_eq!(metrics.current_queue_depth, 0);
        assert_eq!(metrics.jobs_per_second, 0.0);
        assert_eq!(metrics.worker_utilization, 0.0);
        assert_eq!(metrics.time_since_last_scale, Duration::from_secs(0));
        assert!(metrics.last_scale_time.is_none());
    }

    fn autoscale_state(
        active_workers: usize,
    ) -> (Arc<std::sync::RwLock<AutoscaleMetrics>>, QueueDepthHistory) {
        (
            Arc::new(std::sync::RwLock::new(AutoscaleMetrics {
                active_workers,
                ..Default::default()
            })),
            Arc::new(std::sync::RwLock::new(Vec::new())),
        )
    }

    fn fast_autoscale() -> AutoscaleConfig {
        AutoscaleConfig::new()
            .with_min_workers(1)
            .with_max_workers(3)
            .with_scale_up_threshold(4)
            .with_scale_down_threshold(1)
            .with_cooldown_period(Duration::ZERO)
            .with_evaluation_window(Duration::from_secs(30))
    }

    #[test]
    fn autoscaling_scales_up_within_max_workers() {
        let config = fast_autoscale().with_scale_step(5);
        let (metrics, history) = autoscale_state(1);
        // 10 jobs for 1 worker is above the threshold of 4 per worker; the step of 5
        // is capped at max_workers.
        let decision = apply_queue_depth(10, Utc::now(), &config, &metrics, &history);
        assert_eq!(decision, Some(3));
        let m = metrics.read().unwrap().clone();
        assert_eq!(m.active_workers, 3);
        assert_eq!(m.current_queue_depth, 10);
        assert_eq!(m.avg_queue_depth, 10.0);
        assert!(m.last_scale_time.is_some());

        // At max_workers it never goes higher, however deep the queue.
        let decision = apply_queue_depth(1000, Utc::now(), &config, &metrics, &history);
        assert_eq!(decision, None);
        assert_eq!(metrics.read().unwrap().active_workers, 3);
    }

    #[test]
    fn autoscaling_scales_down_to_min_workers() {
        let config = fast_autoscale().with_scale_step(2);
        let (metrics, history) = autoscale_state(3);
        let decision = apply_queue_depth(0, Utc::now(), &config, &metrics, &history);
        assert_eq!(decision, Some(1));
        let decision = apply_queue_depth(0, Utc::now(), &config, &metrics, &history);
        assert_eq!(decision, None, "never below min_workers");
        assert_eq!(metrics.read().unwrap().active_workers, 1);
    }

    #[test]
    fn autoscaling_holds_between_thresholds_and_during_cooldown() {
        let (metrics, history) = autoscale_state(2);
        // 6 jobs / 2 workers = 3 per worker: between the thresholds (1 and 4).
        let decision = apply_queue_depth(6, Utc::now(), &fast_autoscale(), &metrics, &history);
        assert_eq!(decision, None);

        let config = fast_autoscale().with_cooldown_period(Duration::from_secs(3600));
        let (metrics, history) = autoscale_state(1);
        let now = Utc::now();
        assert_eq!(
            apply_queue_depth(100, now, &config, &metrics, &history),
            Some(2),
            "the first decision is not in a cooldown"
        );
        assert_eq!(
            apply_queue_depth(
                100,
                now + chrono::Duration::seconds(1),
                &config,
                &metrics,
                &history
            ),
            None,
            "a second decision within the cooldown period is suppressed"
        );
        let m = metrics.read().unwrap().clone();
        assert_eq!(m.active_workers, 2);
        assert_eq!(m.time_since_last_scale, Duration::from_secs(1));
    }

    #[test]
    fn autoscaling_averages_depth_over_the_evaluation_window() {
        let config = fast_autoscale()
            .with_scale_up_threshold(100)
            .with_scale_down_threshold(0)
            .with_evaluation_window(Duration::from_secs(10));
        let (metrics, history) = autoscale_state(1);
        let start = Utc::now();
        for (offset, depth) in [(0, 30), (4, 10), (8, 20)] {
            apply_queue_depth(
                depth,
                start + chrono::Duration::seconds(offset),
                &config,
                &metrics,
                &history,
            );
        }
        assert_eq!(metrics.read().unwrap().avg_queue_depth, 20.0);

        // 12 seconds later the first sample (at 0s) is outside the 10s window.
        apply_queue_depth(
            40,
            start + chrono::Duration::seconds(12),
            &config,
            &metrics,
            &history,
        );
        assert_eq!(history.read().unwrap().len(), 3);
        let m = metrics.read().unwrap().clone();
        assert_eq!(m.avg_queue_depth, (10.0 + 20.0 + 40.0) / 3.0);
        assert_eq!(m.current_queue_depth, 40);
    }

    /// Raising `min_workers` above the current `max_workers` raises `max_workers` too,
    /// so the configuration never ends up with `min_workers > max_workers`.
    #[test]
    fn with_min_workers_keeps_max_at_least_min() {
        let config = AutoscaleConfig::new().with_min_workers(15);
        assert_eq!(config.min_workers, 15);
        assert!(config.max_workers >= config.min_workers);
    }
}
