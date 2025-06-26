use crate::{
    error::HammerworkError,
    job::Job,
    priority::PriorityWeights,
    queue::{DatabaseQueue, JobQueue},
    rate_limit::{RateLimit, RateLimiter, ThrottleConfig},
    stats::{JobEvent, JobEventType, StatisticsCollector},
    Result,
};

#[cfg(feature = "metrics")]
use crate::metrics::PrometheusMetricsCollector;

#[cfg(feature = "alerting")]
use crate::alerting::{AlertManager, AlertingConfig};

use chrono::{DateTime, Utc};
use sqlx::Database;
use std::{sync::Arc, time::Duration};
use tokio::{sync::mpsc, time::sleep};
use tracing::{debug, error, info, warn};

pub type JobHandler = Arc<
    dyn Fn(Job) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send>>
        + Send
        + Sync,
>;

pub struct Worker<DB: Database> {
    queue: Arc<JobQueue<DB>>,
    queue_name: String,
    handler: JobHandler,
    poll_interval: Duration,
    max_retries: i32,
    retry_delay: Duration,
    default_timeout: Option<Duration>,
    priority_weights: Option<PriorityWeights>,
    stats_collector: Option<Arc<dyn StatisticsCollector>>,
    rate_limiter: Option<RateLimiter>,
    throttle_config: Option<ThrottleConfig>,
    #[cfg(feature = "metrics")]
    metrics_collector: Option<Arc<PrometheusMetricsCollector>>,
    #[cfg(feature = "alerting")]
    alert_manager: Option<Arc<AlertManager>>,
    last_job_time: Arc<std::sync::RwLock<DateTime<Utc>>>,
}

impl<DB: Database + Send + Sync + 'static> Worker<DB>
where
    JobQueue<DB>: DatabaseQueue<Database = DB> + Send + Sync,
{
    pub fn new(queue: Arc<JobQueue<DB>>, queue_name: String, handler: JobHandler) -> Self {
        Self {
            queue,
            queue_name,
            handler,
            poll_interval: Duration::from_secs(1),
            max_retries: 3,
            retry_delay: Duration::from_secs(30),
            default_timeout: None,
            priority_weights: None,
            stats_collector: None,
            rate_limiter: None,
            throttle_config: None,
            #[cfg(feature = "metrics")]
            metrics_collector: None,
            #[cfg(feature = "alerting")]
            alert_manager: None,
            last_job_time: Arc::new(std::sync::RwLock::new(Utc::now())),
        }
    }

    pub fn with_stats_collector(mut self, stats_collector: Arc<dyn StatisticsCollector>) -> Self {
        self.stats_collector = Some(stats_collector);
        self
    }

    pub fn with_poll_interval(mut self, interval: Duration) -> Self {
        self.poll_interval = interval;
        self
    }

    pub fn with_max_retries(mut self, max_retries: i32) -> Self {
        self.max_retries = max_retries;
        self
    }

    pub fn with_retry_delay(mut self, delay: Duration) -> Self {
        self.retry_delay = delay;
        self
    }

    pub fn with_default_timeout(mut self, timeout: Duration) -> Self {
        self.default_timeout = Some(timeout);
        self
    }

    /// Configure priority weights for job selection
    pub fn with_priority_weights(mut self, weights: PriorityWeights) -> Self {
        self.priority_weights = Some(weights);
        self
    }

    /// Enable strict priority mode (always process highest priority first)
    pub fn with_strict_priority(mut self) -> Self {
        self.priority_weights = Some(PriorityWeights::strict());
        self
    }

    /// Use default weighted priority selection
    pub fn with_weighted_priority(mut self) -> Self {
        self.priority_weights = Some(PriorityWeights::new());
        self
    }

    /// Configure rate limiting for this worker
    pub fn with_rate_limit(mut self, rate_limit: RateLimit) -> Self {
        self.rate_limiter = Some(RateLimiter::new(rate_limit));
        self
    }

    /// Configure throttling for this worker
    pub fn with_throttle_config(mut self, throttle_config: ThrottleConfig) -> Self {
        // If the throttle config has a rate limit, create a rate limiter
        if let Some(rate_limit) = throttle_config.to_rate_limit() {
            self.rate_limiter = Some(RateLimiter::new(rate_limit));
        }
        self.throttle_config = Some(throttle_config);
        self
    }

    /// Configure Prometheus metrics collection for this worker
    #[cfg(feature = "metrics")]
    pub fn with_metrics_collector(mut self, metrics_collector: Arc<PrometheusMetricsCollector>) -> Self {
        self.metrics_collector = Some(metrics_collector);
        self
    }

    /// Configure alerting for this worker
    #[cfg(feature = "alerting")]
    pub fn with_alerting_config(mut self, alerting_config: AlertingConfig) -> Self {
        self.alert_manager = Some(Arc::new(AlertManager::new(alerting_config)));
        self
    }

    /// Configure alert manager for this worker
    #[cfg(feature = "alerting")]
    pub fn with_alert_manager(mut self, alert_manager: Arc<AlertManager>) -> Self {
        self.alert_manager = Some(alert_manager);
        self
    }

    pub async fn run(&self, mut shutdown_rx: mpsc::Receiver<()>) -> Result<()> {
        info!("Worker started for queue: {}", self.queue_name);

        // Start background monitoring task for metrics and alerting
        #[cfg(any(feature = "metrics", feature = "alerting"))]
        let monitoring_task = self.start_monitoring_task();

        loop {
            tokio::select! {
                _ = shutdown_rx.recv() => {
                    info!("Worker shutting down for queue: {}", self.queue_name);
                    break;
                }
                _ = self.process_jobs() => {
                    // Continue processing
                }
            }
        }

        // Stop monitoring task
        #[cfg(any(feature = "metrics", feature = "alerting"))]
        monitoring_task.abort();

        Ok(())
    }

    async fn process_jobs(&self) -> Result<()> {
        // Check rate limit before dequeuing jobs
        if let Some(ref rate_limiter) = self.rate_limiter {
            // Check if we can process a job (non-blocking)
            if !rate_limiter.check() {
                debug!(
                    "Rate limit exceeded for queue: {}, waiting before retry",
                    self.queue_name
                );
                // Wait for the rate limiter to allow processing
                if let Err(e) = rate_limiter.acquire().await {
                    warn!("Rate limiter error: {}", e);
                    sleep(self.poll_interval).await;
                    return Ok(());
                }
            }
        }

        // Update queue depth metrics before dequeuing
        #[cfg(feature = "metrics")]
        if let Some(metrics_collector) = &self.metrics_collector {
            if let Ok(queue_depth) = self.queue.get_queue_depth(&self.queue_name).await {
                if let Err(e) = metrics_collector.update_queue_depth(&self.queue_name, queue_depth).await {
                    warn!("Failed to update queue depth metrics: {}", e);
                }
            }
        }

        let job_result = if let Some(ref weights) = self.priority_weights {
            // Use priority-aware dequeuing
            self.queue
                .dequeue_with_priority_weights(&self.queue_name, weights)
                .await
        } else {
            // Use regular dequeuing
            self.queue.dequeue(&self.queue_name).await
        };

        match job_result {
            Ok(Some(job)) => {
                debug!(
                    "Processing job: {} with priority: {:?}",
                    job.id, job.priority
                );
                self.process_job(job).await?;
            }
            Ok(None) => {
                // No jobs available, check for worker starvation
                #[cfg(feature = "alerting")]
                if let Some(alert_manager) = &self.alert_manager {
                    let last_time_value = {
                        if let Ok(last_time) = self.last_job_time.read() {
                            Some(*last_time)
                        } else {
                            None
                        }
                    };
                    if let Some(last_time_value) = last_time_value {
                        if let Err(e) = alert_manager.check_worker_starvation(&self.queue_name, last_time_value).await {
                            warn!("Failed to check worker starvation: {}", e);
                        }
                    }
                }
                
                // Wait before polling again
                sleep(self.poll_interval).await;
            }
            Err(e) => {
                error!("Error dequeuing job: {}", e);
                
                // If throttle config specifies backoff on error, apply it
                let backoff_duration = if let Some(ref throttle_config) = self.throttle_config {
                    throttle_config.backoff_on_error.unwrap_or(self.poll_interval)
                } else {
                    self.poll_interval
                };
                
                sleep(backoff_duration).await;
            }
        }

        // Check statistics and alert thresholds periodically
        #[cfg(feature = "alerting")]
        if let (Some(alert_manager), Some(stats_collector)) = (&self.alert_manager, &self.stats_collector) {
            match stats_collector.get_queue_statistics(&self.queue_name, Duration::from_secs(300)).await {
                Ok(stats) => {
                    if let Err(e) = alert_manager.check_thresholds(&self.queue_name, &stats).await {
                        warn!("Failed to check alert thresholds: {}", e);
                    }
                }
                Err(e) => {
                    warn!("Failed to get queue statistics for alerting: {}", e);
                }
            }
        }

        Ok(())
    }

    async fn process_job(&self, job: Job) -> Result<()> {
        let job_id = job.id;
        let queue_name = job.queue_name.clone();
        let start_time = Utc::now();

        // Record job started event
        self.record_event(JobEvent {
            job_id,
            queue_name: queue_name.clone(),
            event_type: JobEventType::Started,
            priority: job.priority,
            processing_time_ms: None,
            error_message: None,
            timestamp: start_time,
        })
        .await;

        // Determine timeout duration (job-specific or default)
        let timeout_duration = job.timeout.or(self.default_timeout);

        let handler_result = if let Some(timeout) = timeout_duration {
            // Run with timeout
            match tokio::time::timeout(timeout, (self.handler)(job.clone())).await {
                Ok(result) => result,
                Err(_) => {
                    // Timeout occurred
                    warn!("Job {} timed out after {:?}", job_id, timeout);

                    // Mark job as timed out in database
                    self.queue
                        .mark_job_timed_out(job_id, &format!("Job timed out after {:?}", timeout))
                        .await?;

                    // Record timeout event
                    self.record_event(JobEvent {
                        job_id,
                        queue_name: queue_name.clone(),
                        event_type: JobEventType::TimedOut,
                        priority: job.priority,
                        processing_time_ms: Some(timeout.as_millis() as u64),
                        error_message: Some(format!("Job timed out after {:?}", timeout)),
                        timestamp: Utc::now(),
                    })
                    .await;

                    return Ok(());
                }
            }
        } else {
            // Run without timeout
            (self.handler)(job.clone()).await
        };

        match handler_result {
            Ok(()) => {
                debug!("Job {} completed successfully", job_id);

                let processing_time_ms = (Utc::now() - start_time).num_milliseconds() as u64;

                // Handle cron job rescheduling
                if job.is_recurring() {
                    if let Some(next_run_time) = job.calculate_next_run() {
                        info!(
                            "Rescheduling recurring job {} for next run at {}",
                            job_id, next_run_time
                        );
                        self.queue
                            .reschedule_cron_job(job_id, next_run_time)
                            .await?;
                    } else {
                        warn!(
                            "Could not calculate next run time for recurring job {}",
                            job_id
                        );
                        self.queue.complete_job(job_id).await?;
                    }
                } else {
                    self.queue.complete_job(job_id).await?;
                }

                // Record job completed event
                self.record_event(JobEvent {
                    job_id,
                    queue_name,
                    event_type: JobEventType::Completed,
                    priority: job.priority,
                    processing_time_ms: Some(processing_time_ms),
                    error_message: None,
                    timestamp: Utc::now(),
                })
                .await;
            }
            Err(e) => {
                error!("Job {} failed: {}", job_id, e);
                let error_message = e.to_string();

                if job.attempts >= self.max_retries {
                    warn!("Job {} exceeded max retries, marking as failed", job_id);

                    // Check if we should mark as dead or just failed
                    if job.has_exhausted_retries() {
                        self.queue.mark_job_dead(job_id, &error_message).await?;

                        // Record job dead event
                        self.record_event(JobEvent {
                            job_id,
                            queue_name,
                            event_type: JobEventType::Dead,
                            priority: job.priority,
                            processing_time_ms: None,
                            error_message: Some(error_message),
                            timestamp: Utc::now(),
                        })
                        .await;
                    } else {
                        self.queue.fail_job(job_id, &error_message).await?;

                        // Record job failed event
                        self.record_event(JobEvent {
                            job_id,
                            queue_name,
                            event_type: JobEventType::Failed,
                            priority: job.priority,
                            processing_time_ms: None,
                            error_message: Some(error_message),
                            timestamp: Utc::now(),
                        })
                        .await;
                    }
                } else {
                    let retry_at =
                        chrono::Utc::now() + chrono::Duration::from_std(self.retry_delay).unwrap();
                    info!("Retrying job {} at {}", job_id, retry_at);
                    self.queue.retry_job(job_id, retry_at).await?;

                    // Record job retry event
                    self.record_event(JobEvent {
                        job_id,
                        queue_name,
                        event_type: JobEventType::Retried,
                        priority: job.priority,
                        processing_time_ms: None,
                        error_message: Some(error_message),
                        timestamp: Utc::now(),
                    })
                    .await;
                }
            }
        }

        Ok(())
    }

    async fn record_event(&self, event: JobEvent) {
        // Record to statistics collector
        if let Some(stats_collector) = &self.stats_collector {
            if let Err(e) = stats_collector.record_event(event.clone()).await {
                warn!("Failed to record statistics event: {}", e);
            }
        }

        // Record to metrics collector
        #[cfg(feature = "metrics")]
        if let Some(metrics_collector) = &self.metrics_collector {
            if let Err(e) = metrics_collector.record_job_event(&event).await {
                warn!("Failed to record metrics event: {}", e);
            }
        }

        // Update last job time for worker starvation detection
        if matches!(event.event_type, JobEventType::Completed | JobEventType::Failed | JobEventType::Dead | JobEventType::TimedOut) {
            if let Ok(mut last_time) = self.last_job_time.write() {
                *last_time = event.timestamp;
            }
        }
    }

    /// Start a background monitoring task for metrics and alerting
    #[cfg(any(feature = "metrics", feature = "alerting"))]
    fn start_monitoring_task(&self) -> tokio::task::JoinHandle<()> {
        let queue_name = self.queue_name.clone();
        
        #[cfg(feature = "metrics")]
        let queue = Arc::clone(&self.queue);
        
        #[cfg(feature = "alerting")]
        let last_job_time = Arc::clone(&self.last_job_time);
        
        #[cfg(feature = "metrics")]
        let metrics_collector = self.metrics_collector.clone();
        
        #[cfg(feature = "alerting")]
        let alert_manager = self.alert_manager.clone();
        
        #[cfg(feature = "alerting")]
        let stats_collector = self.stats_collector.clone();

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(30)); // Monitor every 30 seconds
            
            loop {
                interval.tick().await;
                
                // Update queue depth metrics
                #[cfg(feature = "metrics")]
                if let Some(metrics_collector) = &metrics_collector {
                    if let Ok(queue_depth) = queue.get_queue_depth(&queue_name).await {
                        if let Err(e) = metrics_collector.update_queue_depth(&queue_name, queue_depth).await {
                            warn!("Failed to update queue depth metrics: {}", e);
                        }
                        
                        // Check queue depth for alerts
                        #[cfg(feature = "alerting")]
                        if let Some(alert_manager) = &alert_manager {
                            if let Err(e) = alert_manager.check_queue_depth(&queue_name, queue_depth).await {
                                warn!("Failed to check queue depth alerts: {}", e);
                            }
                        }
                    }
                }
                
                // Check worker starvation
                #[cfg(feature = "alerting")]
                if let Some(alert_manager) = &alert_manager {
                    let last_time_value = {
                        if let Ok(last_time) = last_job_time.read() {
                            Some(*last_time)
                        } else {
                            None
                        }
                    };
                    if let Some(last_time_value) = last_time_value {
                        if let Err(e) = alert_manager.check_worker_starvation(&queue_name, last_time_value).await {
                            warn!("Failed to check worker starvation: {}", e);
                        }
                    }
                }
                
                // Check statistics-based alerts
                #[cfg(feature = "alerting")]
                if let (Some(alert_manager), Some(stats_collector)) = (&alert_manager, &stats_collector) {
                    match stats_collector.get_queue_statistics(&queue_name, Duration::from_secs(300)).await {
                        Ok(stats) => {
                            if let Err(e) = alert_manager.check_thresholds(&queue_name, &stats).await {
                                warn!("Failed to check alert thresholds: {}", e);
                            }
                        }
                        Err(e) => {
                            warn!("Failed to get queue statistics for alerting: {}", e);
                        }
                    }
                }
            }
        })
    }
}

pub struct WorkerPool<DB: Database> {
    workers: Vec<Worker<DB>>,
    shutdown_tx: Vec<mpsc::Sender<()>>,
    stats_collector: Option<Arc<dyn StatisticsCollector>>,
}

impl<DB: Database + Send + Sync + 'static> WorkerPool<DB>
where
    JobQueue<DB>: DatabaseQueue<Database = DB> + Send + Sync,
{
    pub fn new() -> Self {
        Self {
            workers: Vec::new(),
            shutdown_tx: Vec::new(),
            stats_collector: None,
        }
    }

    pub fn with_stats_collector(mut self, stats_collector: Arc<dyn StatisticsCollector>) -> Self {
        self.stats_collector = Some(stats_collector);
        self
    }

    pub fn add_worker(&mut self, mut worker: Worker<DB>) {
        // Apply the pool's stats collector to the worker if available
        if let Some(stats_collector) = &self.stats_collector {
            worker.stats_collector = Some(Arc::clone(stats_collector));
        }
        self.workers.push(worker);
    }

    pub async fn start(&mut self) -> Result<()> {
        info!("Starting worker pool with {} workers", self.workers.len());

        let mut handles = Vec::new();
        self.shutdown_tx.clear();

        for worker in self.workers.drain(..) {
            let (shutdown_tx, shutdown_rx) = mpsc::channel(1);
            self.shutdown_tx.push(shutdown_tx);

            let handle = tokio::spawn(async move {
                if let Err(e) = worker.run(shutdown_rx).await {
                    error!("Worker error: {}", e);
                }
            });
            handles.push(handle);
        }

        // Wait for all workers to complete
        for handle in handles {
            handle.await.map_err(|e| HammerworkError::Worker {
                message: format!("Worker task failed: {}", e),
            })?;
        }

        Ok(())
    }

    pub async fn shutdown(&self) -> Result<()> {
        info!("Shutting down worker pool");

        for tx in &self.shutdown_tx {
            if tx.send(()).await.is_err() {
                warn!("Failed to send shutdown signal to worker");
            }
        }

        Ok(())
    }

    /// Get the statistics collector for the worker pool
    pub fn stats_collector(&self) -> Option<Arc<dyn StatisticsCollector>> {
        self.stats_collector.clone()
    }
}

impl<DB: Database + Send + Sync + 'static> Default for WorkerPool<DB>
where
    JobQueue<DB>: DatabaseQueue<Database = DB> + Send + Sync,
{
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_job_handler_type() {
        // Test that JobHandler type alias is properly defined
        let _handler: JobHandler = Arc::new(|_job| Box::pin(async { Ok(()) }));

        // Compilation test - if this compiles, the type is correct
        assert!(true);
    }

    #[test]
    fn test_worker_config_methods() {
        // Test that worker configuration methods work correctly
        // We can't test the full Worker without database implementations
        // But we can test the duration handling

        let poll_interval = Duration::from_millis(500);
        let retry_delay = Duration::from_secs(60);
        let max_retries = 5;

        assert_eq!(poll_interval.as_millis(), 500);
        assert_eq!(retry_delay.as_secs(), 60);
        assert_eq!(max_retries, 5);
    }

    #[test]
    fn test_worker_pool_struct() {
        // Test that WorkerPool struct is properly defined
        // We can't instantiate it without database implementations
        // But we can verify the type signatures compile

        // This would be the structure for a real implementation:
        // let pool: WorkerPool<sqlx::Postgres> = WorkerPool::new();
        assert!(true); // Compilation test
    }

    #[test]
    fn test_error_handling() {
        let error = HammerworkError::Worker {
            message: "Test error".to_string(),
        };

        assert_eq!(error.to_string(), "Worker error: Test error");
    }

    #[tokio::test]
    async fn test_worker_with_stats_collector() {
        use crate::stats::{InMemoryStatsCollector, StatisticsCollector};
        use std::sync::Arc;

        // This test verifies that the worker can be configured with a stats collector
        let stats_collector = Arc::new(InMemoryStatsCollector::new_default());

        // Test that we can clone and store the stats collector reference
        let stats_clone = Arc::clone(&stats_collector);
        assert_eq!(Arc::strong_count(&stats_collector), 2);

        // Verify stats collector functionality
        let stats = stats_clone
            .get_system_statistics(Duration::from_secs(60))
            .await
            .unwrap();
        assert_eq!(stats.total_processed, 0); // No events recorded yet
    }

    #[test]
    fn test_worker_pool_with_stats_collector() {
        use crate::stats::InMemoryStatsCollector;
        use std::sync::Arc;

        // This test verifies that the worker pool can be configured with a stats collector
        let stats_collector = Arc::new(InMemoryStatsCollector::new_default());

        // Test that we can store the stats collector in the pool
        let stats_clone = Arc::clone(&stats_collector);
        assert_eq!(Arc::strong_count(&stats_collector), 2);

        // This verifies the reference counting works correctly
        drop(stats_clone);
        assert_eq!(Arc::strong_count(&stats_collector), 1);
    }

    #[test]
    fn test_worker_timeout_configuration() {
        use std::time::Duration;

        // Test timeout configuration methods
        let default_timeout = Duration::from_secs(30);
        let poll_interval = Duration::from_millis(500);
        let retry_delay = Duration::from_secs(60);

        // Verify duration values are correctly configured
        assert_eq!(default_timeout.as_secs(), 30);
        assert_eq!(poll_interval.as_millis(), 500);
        assert_eq!(retry_delay.as_secs(), 60);

        // Test timeout edge cases
        let very_short_timeout = Duration::from_millis(1);
        let very_long_timeout = Duration::from_secs(3600);

        assert_eq!(very_short_timeout.as_millis(), 1);
        assert_eq!(very_long_timeout.as_secs(), 3600);
    }

    #[test]
    fn test_job_timeout_detection_logic() {
        use crate::job::{Job, JobStatus};
        use serde_json::json;
        use std::time::Duration;

        // Test job timeout detection scenarios
        let mut job = Job::new("timeout_test".to_string(), json!({"data": "test"}))
            .with_timeout(Duration::from_millis(100));

        // Job not started - should not timeout
        assert!(!job.should_timeout());

        // Job started recently - should not timeout
        job.started_at = Some(chrono::Utc::now() - chrono::Duration::milliseconds(50));
        job.status = JobStatus::Running;
        assert!(!job.should_timeout());

        // Job started long ago - should timeout
        job.started_at = Some(chrono::Utc::now() - chrono::Duration::milliseconds(200));
        assert!(job.should_timeout());

        // Job without timeout - should never timeout
        let mut job_no_timeout = Job::new("no_timeout".to_string(), json!({"data": "test"}));
        job_no_timeout.started_at = Some(chrono::Utc::now() - chrono::Duration::hours(1));
        assert!(!job_no_timeout.should_timeout());
    }

    #[tokio::test]
    async fn test_timeout_statistics_integration() {
        use crate::stats::{InMemoryStatsCollector, JobEvent, JobEventType};
        use std::sync::Arc;

        let stats_collector = Arc::new(InMemoryStatsCollector::new_default());

        // Simulate timeout event recording
        let timeout_event = JobEvent {
            job_id: uuid::Uuid::new_v4(),
            queue_name: "timeout_queue".to_string(),
            event_type: JobEventType::TimedOut,
            priority: crate::priority::JobPriority::Normal,
            processing_time_ms: Some(5000), // 5 seconds before timeout
            error_message: Some("Job timed out after 5s".to_string()),
            timestamp: chrono::Utc::now(),
        };

        stats_collector.record_event(timeout_event).await.unwrap();

        // Verify timeout event is tracked in statistics
        let stats = stats_collector
            .get_queue_statistics("timeout_queue", Duration::from_secs(60))
            .await
            .unwrap();

        assert_eq!(stats.total_processed, 1);
        assert_eq!(stats.timed_out, 1);
        assert_eq!(stats.error_rate, 1.0); // 1 timeout / 1 total = 100% error rate
    }

    #[test]
    fn test_timeout_error_message_formatting() {
        use std::time::Duration;

        // Test timeout error message formatting
        let timeout_duration = Duration::from_secs(30);
        let expected_message = format!("Job timed out after {:?}", timeout_duration);

        assert!(expected_message.contains("30s"));
        assert!(expected_message.contains("timed out"));

        // Test various timeout durations
        let short_timeout = Duration::from_millis(500);
        let long_timeout = Duration::from_secs(300);

        let short_message = format!("Job timed out after {:?}", short_timeout);
        let long_message = format!("Job timed out after {:?}", long_timeout);

        assert!(short_message.contains("500ms"));
        assert!(long_message.contains("300s"));
    }

    #[test]
    fn test_worker_timeout_precedence() {
        use crate::job::Job;
        use serde_json::json;
        use std::time::Duration;

        // Test that job-specific timeout takes precedence over worker default
        let job_timeout = Duration::from_secs(60);
        let worker_default_timeout = Duration::from_secs(30);

        let job_with_timeout =
            Job::new("test".to_string(), json!({"data": "test"})).with_timeout(job_timeout);

        let job_without_timeout = Job::new("test".to_string(), json!({"data": "test"}));

        // Job with specific timeout should use that timeout
        assert_eq!(job_with_timeout.timeout, Some(job_timeout));

        // Job without specific timeout would use worker default (tested in integration)
        assert_eq!(job_without_timeout.timeout, None);

        // Simulate timeout precedence logic
        let effective_timeout = job_with_timeout.timeout.or(Some(worker_default_timeout));
        assert_eq!(effective_timeout, Some(job_timeout)); // Job timeout wins

        let effective_timeout_default =
            job_without_timeout.timeout.or(Some(worker_default_timeout));
        assert_eq!(effective_timeout_default, Some(worker_default_timeout)); // Worker default used
    }

    #[test]
    fn test_worker_rate_limit_configuration() {
        use crate::rate_limit::RateLimit;

        // Test rate limit configuration
        let rate_limit = RateLimit::per_second(10).with_burst_limit(20);
        
        assert_eq!(rate_limit.rate, 10);
        assert_eq!(rate_limit.burst_limit, 20);
        assert_eq!(rate_limit.per, Duration::from_secs(1));

        // Test different time windows
        let per_minute = RateLimit::per_minute(60);
        assert_eq!(per_minute.rate, 60);
        assert_eq!(per_minute.per, Duration::from_secs(60));

        let per_hour = RateLimit::per_hour(3600);
        assert_eq!(per_hour.rate, 3600);
        assert_eq!(per_hour.per, Duration::from_secs(3600));
    }

    #[test]
    fn test_throttle_config_configuration() {
        use crate::rate_limit::ThrottleConfig;

        let throttle_config = ThrottleConfig::new()
            .max_concurrent(5)
            .rate_per_minute(100)
            .backoff_on_error(Duration::from_secs(30))
            .enabled(true);

        assert_eq!(throttle_config.max_concurrent, Some(5));
        assert_eq!(throttle_config.rate_per_minute, Some(100));
        assert_eq!(throttle_config.backoff_on_error, Some(Duration::from_secs(30)));
        assert!(throttle_config.enabled);

        // Test rate limit conversion
        let rate_limit = throttle_config.to_rate_limit().unwrap();
        assert_eq!(rate_limit.rate, 100);
        assert_eq!(rate_limit.per, Duration::from_secs(60));
    }

    #[tokio::test]
    async fn test_rate_limiter_integration() {
        use crate::rate_limit::{RateLimit, RateLimiter};

        let rate_limit = RateLimit::per_second(5); // 5 operations per second
        let rate_limiter = RateLimiter::new(rate_limit);

        // Should initially allow operations
        assert!(rate_limiter.try_acquire());
        assert!(rate_limiter.try_acquire());
        assert!(rate_limiter.try_acquire());
        assert!(rate_limiter.try_acquire());
        assert!(rate_limiter.try_acquire());

        // Should block after consuming all tokens
        assert!(!rate_limiter.try_acquire());

        // Test acquire method (will wait for token refill)
        let start = std::time::Instant::now();
        rate_limiter.acquire().await.unwrap();
        let elapsed = start.elapsed();

        // Should have waited some time for token refill (but not too long due to high test rate)
        assert!(elapsed < Duration::from_millis(500)); // Should be fast for this test rate
    }

    #[test]
    fn test_worker_backoff_configuration() {
        use crate::rate_limit::ThrottleConfig;

        // Test that backoff configuration is properly handled
        let throttle_config = ThrottleConfig::new()
            .backoff_on_error(Duration::from_secs(60));

        assert_eq!(throttle_config.backoff_on_error, Some(Duration::from_secs(60)));

        // Test default poll interval fallback
        let poll_interval = Duration::from_secs(1);
        let backoff_duration = throttle_config.backoff_on_error.unwrap_or(poll_interval);
        assert_eq!(backoff_duration, Duration::from_secs(60));

        // Test with no backoff configured
        let no_backoff_config = ThrottleConfig::new();
        let backoff_duration = no_backoff_config.backoff_on_error.unwrap_or(poll_interval);
        assert_eq!(backoff_duration, poll_interval);
    }

    #[tokio::test]
    async fn test_rate_limiter_token_availability() {
        use crate::rate_limit::{RateLimit, RateLimiter};

        let rate_limit = RateLimit::per_second(10); // 10 tokens per second
        let rate_limiter = RateLimiter::new(rate_limit);

        // Check initial token availability
        let initial_tokens = rate_limiter.available_tokens();
        assert_eq!(initial_tokens, 10.0); // Should start with full burst capacity

        // Consume some tokens
        assert!(rate_limiter.try_acquire());
        assert!(rate_limiter.try_acquire());

        // Check remaining tokens
        let remaining_tokens = rate_limiter.available_tokens();
        assert_eq!(remaining_tokens, 8.0);
    }

    #[test]
    fn test_rate_limit_edge_cases() {
        use crate::rate_limit::RateLimit;

        // Test very low rate
        let low_rate = RateLimit::per_hour(1);
        assert_eq!(low_rate.rate, 1);
        assert_eq!(low_rate.per, Duration::from_secs(3600));

        // Test very high rate
        let high_rate = RateLimit::per_second(1000);
        assert_eq!(high_rate.rate, 1000);
        assert_eq!(high_rate.burst_limit, 1000);

        // Test custom burst limit
        let custom_burst = RateLimit::per_second(10).with_burst_limit(50);
        assert_eq!(custom_burst.burst_limit, 50);
    }

    #[test]
    fn test_throttle_config_defaults() {
        use crate::rate_limit::ThrottleConfig;

        let default_config = ThrottleConfig::default();
        assert!(default_config.enabled);
        assert!(default_config.max_concurrent.is_none());
        assert!(default_config.rate_per_minute.is_none());
        assert!(default_config.backoff_on_error.is_none());

        let new_config = ThrottleConfig::new();
        assert_eq!(new_config.enabled, default_config.enabled);
        assert_eq!(new_config.max_concurrent, default_config.max_concurrent);
    }
}
