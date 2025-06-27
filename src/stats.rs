use crate::priority::{JobPriority, PriorityStats};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, sync::Arc, time::Duration};

/// Statistics for job processing over a time window
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobStatistics {
    /// Total number of jobs processed in the time window
    pub total_processed: u64,
    /// Number of successfully completed jobs
    pub completed: u64,
    /// Number of failed jobs
    pub failed: u64,
    /// Number of dead jobs (exhausted all retries)
    pub dead: u64,
    /// Number of timed out jobs
    pub timed_out: u64,
    /// Number of currently running jobs
    pub running: u64,
    /// Average processing time in milliseconds
    pub avg_processing_time_ms: f64,
    /// Minimum processing time in milliseconds
    pub min_processing_time_ms: u64,
    /// Maximum processing time in milliseconds
    pub max_processing_time_ms: u64,
    /// Job throughput per minute
    pub throughput_per_minute: f64,
    /// Error rate (failed + dead + timed out jobs / total processed)
    pub error_rate: f64,
    /// Priority-based statistics breakdown
    pub priority_stats: Option<PriorityStats>,
    /// Time window these statistics cover
    pub time_window: Duration,
    /// When these statistics were calculated
    pub calculated_at: DateTime<Utc>,
}

impl Default for JobStatistics {
    fn default() -> Self {
        Self {
            total_processed: 0,
            completed: 0,
            failed: 0,
            dead: 0,
            timed_out: 0,
            running: 0,
            avg_processing_time_ms: 0.0,
            min_processing_time_ms: 0,
            max_processing_time_ms: 0,
            throughput_per_minute: 0.0,
            error_rate: 0.0,
            priority_stats: None,
            time_window: Duration::from_secs(60), // Default 1 minute
            calculated_at: Utc::now(),
        }
    }
}

/// Queue-specific statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueueStats {
    /// Name of the queue
    pub queue_name: String,
    /// Number of pending jobs in the queue
    pub pending_count: u64,
    /// Number of currently running jobs
    pub running_count: u64,
    /// Number of dead jobs in the queue
    pub dead_count: u64,
    /// Number of timed out jobs in the queue
    pub timed_out_count: u64,
    /// Number of completed jobs (may be pruned)
    pub completed_count: u64,
    /// Job processing statistics
    pub statistics: JobStatistics,
}

/// Summary of dead jobs across the system
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeadJobSummary {
    /// Total number of dead jobs across all queues
    pub total_dead_jobs: u64,
    /// Dead jobs by queue name
    pub dead_jobs_by_queue: HashMap<String, u64>,
    /// Oldest dead job timestamp
    pub oldest_dead_job: Option<DateTime<Utc>>,
    /// Most recent dead job timestamp
    pub newest_dead_job: Option<DateTime<Utc>>,
    /// Common error patterns (error message -> count)
    pub error_patterns: HashMap<String, u64>,
}

/// Job processing event for statistics collection
#[derive(Debug, Clone)]
pub struct JobEvent {
    pub job_id: uuid::Uuid,
    pub queue_name: String,
    pub event_type: JobEventType,
    pub priority: JobPriority,
    pub processing_time_ms: Option<u64>,
    pub error_message: Option<String>,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobEventType {
    Started,
    Completed,
    Failed,
    Retried,
    Dead,
    TimedOut,
}

/// Trait for collecting and storing job statistics
#[async_trait::async_trait]
pub trait StatisticsCollector: Send + Sync {
    /// Record a job processing event
    async fn record_event(&self, event: JobEvent) -> crate::Result<()>;

    /// Get statistics for a specific queue over a time window
    async fn get_queue_statistics(
        &self,
        queue_name: &str,
        window: Duration,
    ) -> crate::Result<JobStatistics>;

    /// Get statistics for all queues
    async fn get_all_statistics(&self, window: Duration) -> crate::Result<Vec<QueueStats>>;

    /// Get overall system statistics
    async fn get_system_statistics(&self, window: Duration) -> crate::Result<JobStatistics>;

    /// Clear statistics older than the specified duration
    async fn cleanup_old_statistics(&self, older_than: Duration) -> crate::Result<u64>;
}

/// In-memory statistics collector with time-windowed data
pub struct InMemoryStatsCollector {
    events: Arc<std::sync::RwLock<Vec<JobEvent>>>,
    config: StatsConfig,
}

/// Configuration for statistics collection
#[derive(Debug, Clone)]
pub struct StatsConfig {
    /// Maximum number of events to keep in memory
    pub max_events: usize,
    /// How often to clean up old events (in seconds)
    pub cleanup_interval_secs: u64,
    /// Maximum age of events to keep (in seconds)
    pub max_event_age_secs: u64,
    /// Whether to collect detailed timing information
    pub collect_timing: bool,
}

impl Default for StatsConfig {
    fn default() -> Self {
        Self {
            max_events: 100_000,
            cleanup_interval_secs: 300, // 5 minutes
            max_event_age_secs: 3600,   // 1 hour
            collect_timing: true,
        }
    }
}

impl InMemoryStatsCollector {
    pub fn new(config: StatsConfig) -> Self {
        Self {
            events: Arc::new(std::sync::RwLock::new(Vec::new())),
            config,
        }
    }

    pub fn new_default() -> Self {
        Self::new(StatsConfig::default())
    }

    fn filter_events_by_window(&self, window: Duration) -> Vec<JobEvent> {
        let cutoff = Utc::now() - chrono::Duration::from_std(window).unwrap();
        let events = self.events.read().unwrap();
        events
            .iter()
            .filter(|event| event.timestamp >= cutoff)
            .cloned()
            .collect()
    }

    fn calculate_statistics(&self, events: &[JobEvent], window: Duration) -> JobStatistics {
        if events.is_empty() {
            return JobStatistics {
                time_window: window,
                calculated_at: Utc::now(),
                ..Default::default()
            };
        }

        let total_processed = events.len() as u64;
        let completed = events
            .iter()
            .filter(|e| e.event_type == JobEventType::Completed)
            .count() as u64;
        let failed = events
            .iter()
            .filter(|e| e.event_type == JobEventType::Failed)
            .count() as u64;
        let dead = events
            .iter()
            .filter(|e| e.event_type == JobEventType::Dead)
            .count() as u64;
        let timed_out = events
            .iter()
            .filter(|e| e.event_type == JobEventType::TimedOut)
            .count() as u64;
        let running = events
            .iter()
            .filter(|e| e.event_type == JobEventType::Started)
            .count() as u64;

        let processing_times: Vec<u64> =
            events.iter().filter_map(|e| e.processing_time_ms).collect();

        let (avg_processing_time_ms, min_processing_time_ms, max_processing_time_ms) =
            if processing_times.is_empty() {
                (0.0, 0, 0)
            } else {
                let sum: u64 = processing_times.iter().sum();
                let avg = sum as f64 / processing_times.len() as f64;
                let min = *processing_times.iter().min().unwrap();
                let max = *processing_times.iter().max().unwrap();
                (avg, min, max)
            };

        let error_rate = if total_processed > 0 {
            (failed + dead + timed_out) as f64 / total_processed as f64
        } else {
            0.0
        };

        let throughput_per_minute = if window.as_secs() > 0 {
            total_processed as f64 * 60.0 / window.as_secs() as f64
        } else {
            0.0
        };

        // Calculate priority statistics
        let priority_stats = self.calculate_priority_statistics(events);

        JobStatistics {
            total_processed,
            completed,
            failed,
            dead,
            timed_out,
            running,
            avg_processing_time_ms,
            min_processing_time_ms,
            max_processing_time_ms,
            throughput_per_minute,
            error_rate,
            priority_stats: Some(priority_stats),
            time_window: window,
            calculated_at: Utc::now(),
        }
    }

    fn calculate_priority_statistics(&self, events: &[JobEvent]) -> PriorityStats {
        let mut priority_stats = PriorityStats::new();

        // Count jobs by priority
        for event in events {
            *priority_stats.job_counts.entry(event.priority).or_insert(0) += 1;
        }

        // Calculate average processing times by priority
        let mut priority_processing_times: HashMap<JobPriority, Vec<u64>> = HashMap::new();
        for event in events {
            if let Some(processing_time) = event.processing_time_ms {
                priority_processing_times
                    .entry(event.priority)
                    .or_default()
                    .push(processing_time);
            }
        }

        for (priority, times) in priority_processing_times {
            if !times.is_empty() {
                let avg = times.iter().sum::<u64>() as f64 / times.len() as f64;
                priority_stats.avg_processing_times.insert(priority, avg);
            }
        }

        // Calculate recent throughput (count events in the time window)
        for event in events {
            *priority_stats
                .recent_throughput
                .entry(event.priority)
                .or_insert(0) += 1;
        }

        // Calculate priority distribution percentages
        priority_stats.calculate_distribution();

        priority_stats
    }

    /// Clean up events older than max_event_age_secs
    pub fn cleanup_old_events(&self) -> usize {
        let cutoff = Utc::now() - chrono::Duration::seconds(self.config.max_event_age_secs as i64);
        let mut events = self.events.write().unwrap();
        let original_len = events.len();
        events.retain(|event| event.timestamp >= cutoff);

        // Also limit by max_events if we still have too many
        if events.len() > self.config.max_events {
            let excess = events.len() - self.config.max_events;
            events.drain(0..excess);
            original_len - events.len()
        } else {
            original_len - events.len()
        }
    }
}

#[async_trait::async_trait]
impl StatisticsCollector for InMemoryStatsCollector {
    async fn record_event(&self, event: JobEvent) -> crate::Result<()> {
        let mut events = self.events.write().unwrap();
        events.push(event);

        // Periodic cleanup to prevent memory growth
        if events.len() > self.config.max_events {
            let excess = events.len() - self.config.max_events;
            events.drain(0..excess);
        }

        Ok(())
    }

    async fn get_queue_statistics(
        &self,
        queue_name: &str,
        window: Duration,
    ) -> crate::Result<JobStatistics> {
        let events = self.filter_events_by_window(window);
        let queue_events: Vec<JobEvent> = events
            .into_iter()
            .filter(|e| e.queue_name == queue_name)
            .collect();

        Ok(self.calculate_statistics(&queue_events, window))
    }

    async fn get_all_statistics(&self, window: Duration) -> crate::Result<Vec<QueueStats>> {
        let events = self.filter_events_by_window(window);
        let mut queue_events: HashMap<String, Vec<JobEvent>> = HashMap::new();

        for event in events {
            queue_events
                .entry(event.queue_name.clone())
                .or_default()
                .push(event);
        }

        let mut results = Vec::new();
        for (queue_name, events) in queue_events {
            let statistics = self.calculate_statistics(&events, window);

            // Note: pending/running/dead counts would come from database queries
            // This is just for the statistics calculation
            results.push(QueueStats {
                queue_name,
                pending_count: 0, // Would be filled by database implementation
                running_count: statistics.running,
                dead_count: statistics.dead,
                timed_out_count: statistics.timed_out,
                completed_count: statistics.completed,
                statistics,
            });
        }

        Ok(results)
    }

    async fn get_system_statistics(&self, window: Duration) -> crate::Result<JobStatistics> {
        let events = self.filter_events_by_window(window);
        Ok(self.calculate_statistics(&events, window))
    }

    async fn cleanup_old_statistics(&self, older_than: Duration) -> crate::Result<u64> {
        let cutoff = Utc::now() - chrono::Duration::from_std(older_than).unwrap();
        let mut events = self.events.write().unwrap();
        let original_len = events.len();
        events.retain(|event| event.timestamp >= cutoff);
        Ok((original_len - events.len()) as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    // Helper function for creating test JobEvents
    fn create_test_job_event(
        queue_name: &str,
        event_type: JobEventType,
        priority: Option<JobPriority>,
        processing_time_ms: Option<u64>,
        error_message: Option<String>,
    ) -> JobEvent {
        JobEvent {
            job_id: uuid::Uuid::new_v4(),
            queue_name: queue_name.to_string(),
            event_type,
            priority: priority.unwrap_or(JobPriority::Normal),
            processing_time_ms,
            error_message,
            timestamp: Utc::now(),
        }
    }

    #[test]
    fn test_stats_config_default() {
        let config = StatsConfig::default();
        assert_eq!(config.max_events, 100_000);
        assert_eq!(config.cleanup_interval_secs, 300);
        assert_eq!(config.max_event_age_secs, 3600);
        assert!(config.collect_timing);
    }

    #[test]
    fn test_job_statistics_default() {
        let stats = JobStatistics::default();
        assert_eq!(stats.total_processed, 0);
        assert_eq!(stats.completed, 0);
        assert_eq!(stats.failed, 0);
        assert_eq!(stats.dead, 0);
        assert_eq!(stats.timed_out, 0);
        assert_eq!(stats.error_rate, 0.0);
    }

    #[tokio::test]
    async fn test_in_memory_stats_collector() {
        let collector = InMemoryStatsCollector::new_default();

        // Record some events
        let event1 = create_test_job_event("test_queue", JobEventType::Started, None, None, None);

        let event2 = create_test_job_event(
            "test_queue",
            JobEventType::Completed,
            None,
            Some(1000),
            None,
        );

        collector.record_event(event1).await.unwrap();
        collector.record_event(event2).await.unwrap();

        // Get statistics
        let stats = collector
            .get_queue_statistics("test_queue", Duration::from_secs(60))
            .await
            .unwrap();
        assert_eq!(stats.total_processed, 2);
        assert_eq!(stats.completed, 1);
        assert_eq!(stats.avg_processing_time_ms, 1000.0);
    }

    #[test]
    fn test_event_cleanup() {
        let config = StatsConfig {
            max_events: 2,
            max_event_age_secs: 1,
            ..Default::default()
        };
        let collector = InMemoryStatsCollector::new(config);

        // Add events
        {
            let mut events = collector.events.write().unwrap();
            events.push(JobEvent {
                job_id: uuid::Uuid::new_v4(),
                queue_name: "test".to_string(),
                event_type: JobEventType::Completed,
                priority: JobPriority::Normal,
                processing_time_ms: None,
                error_message: None,
                timestamp: Utc::now() - chrono::Duration::seconds(2), // Old event
            });
            events.push(JobEvent {
                job_id: uuid::Uuid::new_v4(),
                queue_name: "test".to_string(),
                event_type: JobEventType::Completed,
                priority: JobPriority::Normal,
                processing_time_ms: None,
                error_message: None,
                timestamp: Utc::now(), // Recent event
            });
        }

        let cleaned = collector.cleanup_old_events();
        assert_eq!(cleaned, 1); // Should remove 1 old event

        let events = collector.events.read().unwrap();
        assert_eq!(events.len(), 1);
    }

    #[tokio::test]
    async fn test_statistics_calculation_with_multiple_events() {
        let collector = InMemoryStatsCollector::new_default();

        // Record various events
        let events = vec![
            JobEvent {
                job_id: uuid::Uuid::new_v4(),
                queue_name: "test_queue".to_string(),
                event_type: JobEventType::Started,
                priority: JobPriority::Normal,
                processing_time_ms: None,
                error_message: None,
                timestamp: Utc::now(),
            },
            JobEvent {
                job_id: uuid::Uuid::new_v4(),
                queue_name: "test_queue".to_string(),
                event_type: JobEventType::Completed,
                priority: JobPriority::Normal,
                processing_time_ms: Some(1500),
                error_message: None,
                timestamp: Utc::now(),
            },
            JobEvent {
                job_id: uuid::Uuid::new_v4(),
                queue_name: "test_queue".to_string(),
                event_type: JobEventType::Completed,
                priority: JobPriority::Normal,
                processing_time_ms: Some(500),
                error_message: None,
                timestamp: Utc::now(),
            },
            JobEvent {
                job_id: uuid::Uuid::new_v4(),
                queue_name: "test_queue".to_string(),
                event_type: JobEventType::Failed,
                priority: JobPriority::Normal,
                processing_time_ms: None,
                error_message: Some("Test error".to_string()),
                timestamp: Utc::now(),
            },
            JobEvent {
                job_id: uuid::Uuid::new_v4(),
                queue_name: "test_queue".to_string(),
                event_type: JobEventType::Dead,
                priority: JobPriority::Normal,
                processing_time_ms: None,
                error_message: Some("Max retries exceeded".to_string()),
                timestamp: Utc::now(),
            },
        ];

        for event in events {
            collector.record_event(event).await.unwrap();
        }

        // Get statistics
        let stats = collector
            .get_queue_statistics("test_queue", Duration::from_secs(60))
            .await
            .unwrap();

        assert_eq!(stats.total_processed, 5);
        assert_eq!(stats.completed, 2);
        assert_eq!(stats.failed, 1);
        assert_eq!(stats.dead, 1);
        assert_eq!(stats.running, 1);
        assert_eq!(stats.avg_processing_time_ms, 1000.0); // (1500 + 500) / 2
        assert_eq!(stats.min_processing_time_ms, 500);
        assert_eq!(stats.max_processing_time_ms, 1500);
        assert_eq!(stats.error_rate, 0.4); // (1 failed + 1 dead + 0 timed out) / 5 total
    }

    #[tokio::test]
    async fn test_system_statistics() {
        let collector = InMemoryStatsCollector::new_default();

        // Record events for multiple queues
        let events = vec![
            JobEvent {
                job_id: uuid::Uuid::new_v4(),
                queue_name: "queue1".to_string(),
                event_type: JobEventType::Completed,
                priority: JobPriority::Normal,
                processing_time_ms: Some(1000),
                error_message: None,
                timestamp: Utc::now(),
            },
            JobEvent {
                job_id: uuid::Uuid::new_v4(),
                queue_name: "queue2".to_string(),
                event_type: JobEventType::Completed,
                priority: JobPriority::Normal,
                processing_time_ms: Some(2000),
                error_message: None,
                timestamp: Utc::now(),
            },
            JobEvent {
                job_id: uuid::Uuid::new_v4(),
                queue_name: "queue1".to_string(),
                event_type: JobEventType::Failed,
                priority: JobPriority::Normal,
                processing_time_ms: None,
                error_message: Some("Error".to_string()),
                timestamp: Utc::now(),
            },
        ];

        for event in events {
            collector.record_event(event).await.unwrap();
        }

        // Get system-wide statistics
        let stats = collector
            .get_system_statistics(Duration::from_secs(60))
            .await
            .unwrap();

        assert_eq!(stats.total_processed, 3);
        assert_eq!(stats.completed, 2);
        assert_eq!(stats.failed, 1);
        assert_eq!(stats.avg_processing_time_ms, 1500.0); // (1000 + 2000) / 2
    }

    #[tokio::test]
    async fn test_all_queue_statistics() {
        let collector = InMemoryStatsCollector::new_default();

        // Record events for multiple queues
        let events = vec![
            JobEvent {
                job_id: uuid::Uuid::new_v4(),
                queue_name: "email_queue".to_string(),
                event_type: JobEventType::Completed,
                priority: JobPriority::Normal,
                processing_time_ms: Some(500),
                error_message: None,
                timestamp: Utc::now(),
            },
            JobEvent {
                job_id: uuid::Uuid::new_v4(),
                queue_name: "notification_queue".to_string(),
                event_type: JobEventType::Completed,
                priority: JobPriority::Normal,
                processing_time_ms: Some(1000),
                error_message: None,
                timestamp: Utc::now(),
            },
            JobEvent {
                job_id: uuid::Uuid::new_v4(),
                queue_name: "email_queue".to_string(),
                event_type: JobEventType::Failed,
                priority: JobPriority::Normal,
                processing_time_ms: None,
                error_message: Some("SMTP error".to_string()),
                timestamp: Utc::now(),
            },
        ];

        for event in events {
            collector.record_event(event).await.unwrap();
        }

        // Get all queue statistics
        let all_stats = collector
            .get_all_statistics(Duration::from_secs(60))
            .await
            .unwrap();

        assert_eq!(all_stats.len(), 2);

        let email_stats = all_stats
            .iter()
            .find(|s| s.queue_name == "email_queue")
            .unwrap();
        assert_eq!(email_stats.statistics.total_processed, 2);
        assert_eq!(email_stats.statistics.completed, 1);
        assert_eq!(email_stats.statistics.failed, 1);

        let notification_stats = all_stats
            .iter()
            .find(|s| s.queue_name == "notification_queue")
            .unwrap();
        assert_eq!(notification_stats.statistics.total_processed, 1);
        assert_eq!(notification_stats.statistics.completed, 1);
        assert_eq!(notification_stats.statistics.failed, 0);
    }

    #[tokio::test]
    async fn test_cleanup_old_statistics() {
        let collector = InMemoryStatsCollector::new_default();

        // Add an old event
        let old_event = JobEvent {
            job_id: uuid::Uuid::new_v4(),
            queue_name: "test".to_string(),
            event_type: JobEventType::Completed,
            priority: JobPriority::Normal,
            processing_time_ms: None,
            error_message: None,
            timestamp: Utc::now() - chrono::Duration::hours(2),
        };

        // Add a recent event
        let recent_event = JobEvent {
            job_id: uuid::Uuid::new_v4(),
            queue_name: "test".to_string(),
            event_type: JobEventType::Completed,
            priority: JobPriority::Normal,
            processing_time_ms: None,
            error_message: None,
            timestamp: Utc::now(),
        };

        collector.record_event(old_event).await.unwrap();
        collector.record_event(recent_event).await.unwrap();

        // Clean up events older than 1 hour
        let cleaned = collector
            .cleanup_old_statistics(Duration::from_secs(3600))
            .await
            .unwrap();
        assert_eq!(cleaned, 1);

        // Verify only recent event remains
        let events = collector.events.read().unwrap();
        assert_eq!(events.len(), 1);
    }

    #[test]
    fn test_dead_job_summary_structure() {
        use std::collections::HashMap;

        let mut dead_jobs_by_queue = HashMap::new();
        dead_jobs_by_queue.insert("email_queue".to_string(), 5);
        dead_jobs_by_queue.insert("notification_queue".to_string(), 3);

        let mut error_patterns = HashMap::new();
        error_patterns.insert("Connection timeout".to_string(), 10);
        error_patterns.insert("Invalid payload".to_string(), 5);

        let summary = DeadJobSummary {
            total_dead_jobs: 8,
            dead_jobs_by_queue,
            oldest_dead_job: Some(Utc::now() - chrono::Duration::days(7)),
            newest_dead_job: Some(Utc::now()),
            error_patterns,
        };

        assert_eq!(summary.total_dead_jobs, 8);
        assert_eq!(summary.dead_jobs_by_queue.len(), 2);
        assert_eq!(summary.error_patterns.len(), 2);
        assert!(summary.oldest_dead_job.is_some());
        assert!(summary.newest_dead_job.is_some());
    }

    #[test]
    fn test_queue_stats_structure() {
        let statistics = JobStatistics {
            total_processed: 100,
            completed: 80,
            failed: 15,
            dead: 5,
            timed_out: 2,
            running: 2,
            avg_processing_time_ms: 1500.0,
            min_processing_time_ms: 100,
            max_processing_time_ms: 5000,
            throughput_per_minute: 10.0,
            error_rate: 0.2,
            priority_stats: None,
            time_window: Duration::from_secs(3600),
            calculated_at: Utc::now(),
        };

        let queue_stats = QueueStats {
            queue_name: "test_queue".to_string(),
            pending_count: 5,
            running_count: 2,
            dead_count: 5,
            timed_out_count: 3,
            completed_count: 80,
            statistics,
        };

        assert_eq!(queue_stats.queue_name, "test_queue");
        assert_eq!(queue_stats.pending_count, 5);
        assert_eq!(queue_stats.running_count, 2);
        assert_eq!(queue_stats.dead_count, 5);
        assert_eq!(queue_stats.timed_out_count, 3);
        assert_eq!(queue_stats.completed_count, 80);
        assert_eq!(queue_stats.statistics.total_processed, 100);
    }

    #[tokio::test]
    async fn test_timeout_statistics() {
        let collector = InMemoryStatsCollector::new_default();

        // Record events including timeout
        let events = vec![
            JobEvent {
                job_id: uuid::Uuid::new_v4(),
                queue_name: "test_queue".to_string(),
                event_type: JobEventType::Started,
                priority: JobPriority::Normal,
                processing_time_ms: None,
                error_message: None,
                timestamp: Utc::now(),
            },
            JobEvent {
                job_id: uuid::Uuid::new_v4(),
                queue_name: "test_queue".to_string(),
                event_type: JobEventType::Completed,
                priority: JobPriority::Normal,
                processing_time_ms: Some(1000),
                error_message: None,
                timestamp: Utc::now(),
            },
            JobEvent {
                job_id: uuid::Uuid::new_v4(),
                queue_name: "test_queue".to_string(),
                event_type: JobEventType::TimedOut,
                priority: JobPriority::Normal,
                processing_time_ms: Some(5000),
                error_message: Some("Job timed out after 5s".to_string()),
                timestamp: Utc::now(),
            },
            JobEvent {
                job_id: uuid::Uuid::new_v4(),
                queue_name: "test_queue".to_string(),
                event_type: JobEventType::Failed,
                priority: JobPriority::Normal,
                processing_time_ms: None,
                error_message: Some("Processing error".to_string()),
                timestamp: Utc::now(),
            },
        ];

        for event in events {
            collector.record_event(event).await.unwrap();
        }

        // Get statistics
        let stats = collector
            .get_queue_statistics("test_queue", Duration::from_secs(60))
            .await
            .unwrap();

        assert_eq!(stats.total_processed, 4);
        assert_eq!(stats.completed, 1);
        assert_eq!(stats.failed, 1);
        assert_eq!(stats.timed_out, 1);
        assert_eq!(stats.running, 1);
        assert_eq!(stats.error_rate, 0.5); // (1 failed + 1 timed out) / 4 total
        assert_eq!(stats.avg_processing_time_ms, 3000.0); // (1000 + 5000) / 2
    }

    #[test]
    fn test_job_event_types() {
        let event_types = [
            JobEventType::Started,
            JobEventType::Completed,
            JobEventType::Failed,
            JobEventType::Retried,
            JobEventType::Dead,
            JobEventType::TimedOut,
        ];

        // Test equality
        assert_eq!(JobEventType::Started, JobEventType::Started);
        assert_ne!(JobEventType::Started, JobEventType::Completed);

        // Test all variants exist
        assert_eq!(event_types.len(), 6);
    }
}
