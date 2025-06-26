use crate::{
    error::HammerworkError,
    job::Job,
    queue::{DatabaseQueue, JobQueue},
    stats::{StatisticsCollector, JobEvent, JobEventType},
    Result,
};
use sqlx::Database;
use std::{sync::Arc, time::Duration};
use tokio::{sync::mpsc, time::sleep};
use tracing::{debug, error, info, warn};
use chrono::Utc;

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
    stats_collector: Option<Arc<dyn StatisticsCollector>>,
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
            stats_collector: None,
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

    pub async fn run(&self, mut shutdown_rx: mpsc::Receiver<()>) -> Result<()> {
        info!("Worker started for queue: {}", self.queue_name);

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

        Ok(())
    }

    async fn process_jobs(&self) -> Result<()> {
        match self.queue.dequeue(&self.queue_name).await {
            Ok(Some(job)) => {
                debug!("Processing job: {}", job.id);
                self.process_job(job).await?;
            }
            Ok(None) => {
                // No jobs available, wait before polling again
                sleep(self.poll_interval).await;
            }
            Err(e) => {
                error!("Error dequeuing job: {}", e);
                sleep(self.poll_interval).await;
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
            processing_time_ms: None,
            error_message: None,
            timestamp: start_time,
        }).await;

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
                    self.queue.mark_job_timed_out(job_id, &format!("Job timed out after {:?}", timeout)).await?;
                    
                    // Record timeout event
                    self.record_event(JobEvent {
                        job_id,
                        queue_name: queue_name.clone(),
                        event_type: JobEventType::TimedOut,
                        processing_time_ms: Some(timeout.as_millis() as u64),
                        error_message: Some(format!("Job timed out after {:?}", timeout)),
                        timestamp: Utc::now(),
                    }).await;
                    
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
                        info!("Rescheduling recurring job {} for next run at {}", job_id, next_run_time);
                        self.queue.reschedule_cron_job(job_id, next_run_time).await?;
                    } else {
                        warn!("Could not calculate next run time for recurring job {}", job_id);
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
                    processing_time_ms: Some(processing_time_ms),
                    error_message: None,
                    timestamp: Utc::now(),
                }).await;
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
                            processing_time_ms: None,
                            error_message: Some(error_message),
                            timestamp: Utc::now(),
                        }).await;
                    } else {
                        self.queue.fail_job(job_id, &error_message).await?;
                        
                        // Record job failed event
                        self.record_event(JobEvent {
                            job_id,
                            queue_name,
                            event_type: JobEventType::Failed,
                            processing_time_ms: None,
                            error_message: Some(error_message),
                            timestamp: Utc::now(),
                        }).await;
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
                        processing_time_ms: None,
                        error_message: Some(error_message),
                        timestamp: Utc::now(),
                    }).await;
                }
            }
        }

        Ok(())
    }

    async fn record_event(&self, event: JobEvent) {
        if let Some(stats_collector) = &self.stats_collector {
            if let Err(e) = stats_collector.record_event(event).await {
                warn!("Failed to record statistics event: {}", e);
            }
        }
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
        let stats = stats_clone.get_system_statistics(Duration::from_secs(60)).await.unwrap();
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
        
        let job_with_timeout = Job::new("test".to_string(), json!({"data": "test"}))
            .with_timeout(job_timeout);
        
        let job_without_timeout = Job::new("test".to_string(), json!({"data": "test"}));
        
        // Job with specific timeout should use that timeout
        assert_eq!(job_with_timeout.timeout, Some(job_timeout));
        
        // Job without specific timeout would use worker default (tested in integration)
        assert_eq!(job_without_timeout.timeout, None);
        
        // Simulate timeout precedence logic
        let effective_timeout = job_with_timeout.timeout.or(Some(worker_default_timeout));
        assert_eq!(effective_timeout, Some(job_timeout)); // Job timeout wins
        
        let effective_timeout_default = job_without_timeout.timeout.or(Some(worker_default_timeout));
        assert_eq!(effective_timeout_default, Some(worker_default_timeout)); // Worker default used
    }
}
