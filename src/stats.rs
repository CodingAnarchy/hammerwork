use crate::priority::{JobPriority, PriorityStats};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, VecDeque},
    sync::Arc,
    time::Duration,
};

/// Statistics for job processing over a time window
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobStatistics {
    /// Number of job runs that finished in the time window: every recorded event
    /// except `Started` (a run is counted by its outcome)
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
    /// Error rate: failed, dead, timed out and retried (failed but retried) runs over
    /// `total_processed`
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

/// In-memory statistics collector with time-windowed data.
///
/// Events are kept in a ring buffer ordered by timestamp, bounded by
/// [`StatsConfig::max_events`] and [`StatsConfig::max_event_age_secs`]. Recording an
/// event is amortised O(1) (events normally arrive in time order, so they are appended
/// and the oldest ones fall off the front). Queries find the start of their window by
/// binary search and aggregate the events in it in one pass, without copying them.
pub struct InMemoryStatsCollector {
    events: Arc<std::sync::RwLock<VecDeque<JobEvent>>>,
    config: StatsConfig,
}

/// Configuration for statistics collection
#[derive(Debug, Clone)]
pub struct StatsConfig {
    /// Maximum number of events to keep in memory; the oldest are dropped first.
    pub max_events: usize,
    /// Unused: old events are now pruned whenever an event is recorded.
    #[deprecated(
        since = "1.15.6",
        note = "old events are pruned whenever an event is recorded; this value is ignored"
    )]
    pub cleanup_interval_secs: u64,
    /// Maximum age of events to keep (in seconds). Older events are pruned whenever an
    /// event is recorded, and events older than this are not recorded at all.
    pub max_event_age_secs: u64,
    /// Whether to keep the processing time of each event. When `false`, processing
    /// times are discarded, so the timing fields of [`JobStatistics`] and of the priority
    /// breakdown stay at zero.
    pub collect_timing: bool,
}

impl Default for StatsConfig {
    #[allow(deprecated)]
    fn default() -> Self {
        Self {
            max_events: 100_000,
            cleanup_interval_secs: 300, // 5 minutes
            max_event_age_secs: 3600,   // 1 hour
            collect_timing: true,
        }
    }
}

/// `now - age`, saturating at the earliest representable time.
///
/// `chrono::Duration::from_std(..).unwrap()` and `DateTime - Duration` both panic
/// for huge ages (e.g. a caller passing `Duration::MAX` as "everything"); here an
/// age that does not fit simply means "since the beginning of time".
fn cutoff_before(now: DateTime<Utc>, age: Duration) -> DateTime<Utc> {
    chrono::Duration::from_std(age)
        .ok()
        .and_then(|age| now.checked_sub_signed(age))
        .unwrap_or(DateTime::<Utc>::MIN_UTC)
}

/// Per-priority running totals for [`StatsAccumulator`].
#[derive(Default)]
struct PriorityTotals {
    events: u64,
    timed_events: u64,
    processing_time_ms: u128,
}

/// Single-pass aggregation of the events of one window into [`JobStatistics`].
#[derive(Default)]
struct StatsAccumulator {
    events: u64,
    started: u64,
    completed: u64,
    failed: u64,
    retried: u64,
    dead: u64,
    timed_out: u64,
    timed_events: u64,
    processing_time_ms: u128,
    min_processing_time_ms: Option<u64>,
    max_processing_time_ms: u64,
    priorities: HashMap<JobPriority, PriorityTotals>,
}

impl StatsAccumulator {
    fn add(&mut self, event: &JobEvent) {
        self.events += 1;
        match event.event_type {
            JobEventType::Started => self.started += 1,
            JobEventType::Completed => self.completed += 1,
            JobEventType::Failed => self.failed += 1,
            JobEventType::Retried => self.retried += 1,
            JobEventType::Dead => self.dead += 1,
            JobEventType::TimedOut => self.timed_out += 1,
        }

        let priority = self.priorities.entry(event.priority).or_default();
        priority.events += 1;
        if let Some(time) = event.processing_time_ms {
            priority.timed_events += 1;
            priority.processing_time_ms += u128::from(time);
            self.timed_events += 1;
            self.processing_time_ms += u128::from(time);
            self.min_processing_time_ms =
                Some(self.min_processing_time_ms.map_or(time, |m| m.min(time)));
            self.max_processing_time_ms = self.max_processing_time_ms.max(time);
        }
    }

    fn finish(self, window: Duration) -> JobStatistics {
        if self.events == 0 {
            return JobStatistics {
                time_window: window,
                calculated_at: Utc::now(),
                ..Default::default()
            };
        }

        // A run is counted once, when it finishes: `Started` marks the beginning of a
        // run that is counted by its outcome event.
        let total_processed = self.events - self.started;

        let avg_processing_time_ms = if self.timed_events > 0 {
            self.processing_time_ms as f64 / self.timed_events as f64
        } else {
            0.0
        };

        let error_rate = if total_processed > 0 {
            (self.failed + self.dead + self.timed_out + self.retried) as f64
                / total_processed as f64
        } else {
            0.0
        };

        let throughput_per_minute = if window.as_secs() > 0 {
            total_processed as f64 * 60.0 / window.as_secs() as f64
        } else {
            0.0
        };

        let mut priority_stats = PriorityStats::new();
        for (priority, totals) in self.priorities {
            priority_stats.job_counts.insert(priority, totals.events);
            // Every event in the window counts towards the recent throughput.
            priority_stats
                .recent_throughput
                .insert(priority, totals.events);
            if totals.timed_events > 0 {
                priority_stats.avg_processing_times.insert(
                    priority,
                    totals.processing_time_ms as f64 / totals.timed_events as f64,
                );
            }
        }
        priority_stats.calculate_distribution();

        JobStatistics {
            total_processed,
            completed: self.completed,
            failed: self.failed,
            dead: self.dead,
            timed_out: self.timed_out,
            running: self.started,
            avg_processing_time_ms,
            min_processing_time_ms: self.min_processing_time_ms.unwrap_or(0),
            max_processing_time_ms: self.max_processing_time_ms,
            throughput_per_minute,
            error_rate,
            priority_stats: Some(priority_stats),
            time_window: window,
            calculated_at: Utc::now(),
        }
    }
}

impl InMemoryStatsCollector {
    pub fn new(config: StatsConfig) -> Self {
        Self {
            events: Arc::new(std::sync::RwLock::new(VecDeque::new())),
            config,
        }
    }

    pub fn new_default() -> Self {
        Self::new(StatsConfig::default())
    }

    /// Lock the event buffer for reading, recovering from a poisoned lock.
    ///
    /// The buffer is only ever appended to, drained or filtered, so a panic in
    /// another thread cannot leave it in a state that is unsafe to keep using.
    fn read_events(&self) -> std::sync::RwLockReadGuard<'_, VecDeque<JobEvent>> {
        self.events
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Lock the event buffer for writing, recovering from a poisoned lock.
    fn write_events(&self) -> std::sync::RwLockWriteGuard<'_, VecDeque<JobEvent>> {
        self.events
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The oldest timestamp an event may have and still be kept.
    fn retention_cutoff(&self) -> DateTime<Utc> {
        cutoff_before(
            Utc::now(),
            Duration::from_secs(self.config.max_event_age_secs),
        )
    }

    /// Aggregate the events of the last `window` that `include` accepts.
    fn aggregate_window(
        &self,
        window: Duration,
        mut include: impl FnMut(&JobEvent) -> bool,
    ) -> StatsAccumulator {
        let cutoff = cutoff_before(Utc::now(), window);
        let events = self.read_events();
        // The buffer is ordered by timestamp, so the window is a suffix of it.
        let start = events.partition_point(|event| event.timestamp < cutoff);
        let mut totals = StatsAccumulator::default();
        for event in events.range(start..).filter(|event| include(event)) {
            totals.add(event);
        }
        totals
    }

    /// Clean up events older than max_event_age_secs
    pub fn cleanup_old_events(&self) -> usize {
        let cutoff = self.retention_cutoff();
        let mut events = self.write_events();
        let original_len = events.len();
        events.retain(|event| event.timestamp >= cutoff);

        // Also limit by max_events if we still have too many
        if events.len() > self.config.max_events {
            let excess = events.len() - self.config.max_events;
            events.drain(..excess);
        }
        original_len - events.len()
    }
}

#[async_trait::async_trait]
impl StatisticsCollector for InMemoryStatsCollector {
    async fn record_event(&self, mut event: JobEvent) -> crate::Result<()> {
        if !self.config.collect_timing {
            event.processing_time_ms = None;
        }
        let cutoff = self.retention_cutoff();
        if event.timestamp < cutoff {
            return Ok(());
        }

        let mut events = self.write_events();
        // Keep the buffer ordered by timestamp. Events normally arrive in order, so
        // this is an append; a late one is inserted near the back.
        if events
            .back()
            .is_none_or(|last| last.timestamp <= event.timestamp)
        {
            events.push_back(event);
        } else {
            let at = events.partition_point(|e| e.timestamp <= event.timestamp);
            events.insert(at, event);
        }

        // Prune from the front: expired events, then anything over the size limit.
        while events.front().is_some_and(|e| e.timestamp < cutoff) {
            events.pop_front();
        }
        while events.len() > self.config.max_events {
            events.pop_front();
        }

        Ok(())
    }

    async fn get_queue_statistics(
        &self,
        queue_name: &str,
        window: Duration,
    ) -> crate::Result<JobStatistics> {
        Ok(self
            .aggregate_window(window, |event| event.queue_name == queue_name)
            .finish(window))
    }

    async fn get_all_statistics(&self, window: Duration) -> crate::Result<Vec<QueueStats>> {
        let cutoff = cutoff_before(Utc::now(), window);
        let mut queues: HashMap<String, StatsAccumulator> = HashMap::new();
        {
            let events = self.read_events();
            let start = events.partition_point(|event| event.timestamp < cutoff);
            for event in events.range(start..) {
                match queues.get_mut(&event.queue_name) {
                    Some(totals) => totals.add(event),
                    None => {
                        let mut totals = StatsAccumulator::default();
                        totals.add(event);
                        queues.insert(event.queue_name.clone(), totals);
                    }
                }
            }
        }

        Ok(queues
            .into_iter()
            .map(|(queue_name, totals)| {
                let statistics = totals.finish(window);
                // Note: pending/running/dead counts would come from database queries
                // This is just for the statistics calculation
                QueueStats {
                    queue_name,
                    pending_count: 0, // Would be filled by database implementation
                    running_count: statistics.running,
                    dead_count: statistics.dead,
                    timed_out_count: statistics.timed_out,
                    completed_count: statistics.completed,
                    statistics,
                }
            })
            .collect())
    }

    async fn get_system_statistics(&self, window: Duration) -> crate::Result<JobStatistics> {
        Ok(self.aggregate_window(window, |_| true).finish(window))
    }

    async fn cleanup_old_statistics(&self, older_than: Duration) -> crate::Result<u64> {
        let cutoff = cutoff_before(Utc::now(), older_than);
        let mut events = self.write_events();
        let original_len = events.len();
        events.retain(|event| event.timestamp >= cutoff);
        Ok((original_len - events.len()) as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_cutoff_before_saturates_on_huge_age() {
        let now = Utc::now();
        assert_eq!(cutoff_before(now, Duration::MAX), DateTime::<Utc>::MIN_UTC);
        assert_eq!(
            cutoff_before(now, Duration::from_secs(60)),
            now - chrono::Duration::seconds(60)
        );
    }

    #[tokio::test]
    async fn test_huge_window_does_not_panic() {
        let collector = InMemoryStatsCollector::new_default();
        collector
            .record_event(create_test_job_event(
                "q",
                JobEventType::Completed,
                Some(JobPriority::Normal),
                Some(5),
                None,
            ))
            .await
            .unwrap();
        let stats = collector
            .get_system_statistics(Duration::MAX)
            .await
            .unwrap();
        assert_eq!(stats.total_processed, 1);
        assert_eq!(
            collector
                .cleanup_old_statistics(Duration::MAX)
                .await
                .unwrap(),
            0
        );
    }

    #[tokio::test]
    async fn test_poisoned_lock_is_recovered() {
        let collector = std::sync::Arc::new(InMemoryStatsCollector::new_default());
        let poisoner = collector.clone();
        let _ = std::thread::spawn(move || {
            let _guard = poisoner.events.write().unwrap();
            panic!("poison the lock");
        })
        .join();
        assert!(collector.events.is_poisoned());

        // None of these may panic on the poisoned lock.
        collector
            .record_event(create_test_job_event(
                "q",
                JobEventType::Completed,
                Some(JobPriority::Normal),
                None,
                None,
            ))
            .await
            .unwrap();
        let stats = collector
            .get_system_statistics(Duration::from_secs(60))
            .await
            .unwrap();
        assert_eq!(stats.total_processed, 1);
        assert_eq!(collector.cleanup_old_events(), 0);
    }

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
    #[allow(deprecated)]
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
        // One run: `Started` is not counted separately from its outcome.
        assert_eq!(stats.total_processed, 1);
        assert_eq!(stats.completed, 1);
        assert_eq!(stats.error_rate, 0.0);
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
            events.push_back(JobEvent {
                job_id: uuid::Uuid::new_v4(),
                queue_name: "test".to_string(),
                event_type: JobEventType::Completed,
                priority: JobPriority::Normal,
                processing_time_ms: None,
                error_message: None,
                timestamp: Utc::now() - chrono::Duration::seconds(2), // Old event
            });
            events.push_back(JobEvent {
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

        assert_eq!(stats.total_processed, 4, "Started is not a finished run");
        assert_eq!(stats.completed, 2);
        assert_eq!(stats.failed, 1);
        assert_eq!(stats.dead, 1);
        assert_eq!(stats.running, 1);
        assert_eq!(stats.avg_processing_time_ms, 1000.0); // (1500 + 500) / 2
        assert_eq!(stats.min_processing_time_ms, 500);
        assert_eq!(stats.max_processing_time_ms, 1500);
        assert_eq!(stats.error_rate, 0.5); // (1 failed + 1 dead) / 4 finished runs
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
        // Keep events for 3 hours so the 2-hour-old one is recorded.
        let collector = InMemoryStatsCollector::new(StatsConfig {
            max_event_age_secs: 3 * 3600,
            ..Default::default()
        });

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

        assert_eq!(stats.total_processed, 3, "Started is not a finished run");
        assert_eq!(stats.completed, 1);
        assert_eq!(stats.failed, 1);
        assert_eq!(stats.timed_out, 1);
        assert_eq!(stats.running, 1);
        assert_eq!(stats.error_rate, 2.0 / 3.0); // (1 failed + 1 timed out) / 3 runs
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

    /// A queue where every run fails must report a 100% error rate (`Started` events
    /// used to be counted as processed jobs, capping the error rate at 50%), and a run
    /// that failed but is retried counts as an error.
    #[tokio::test]
    async fn test_error_rate_counts_runs_not_events() {
        let collector = InMemoryStatsCollector::new_default();
        for event_type in [
            JobEventType::Started,
            JobEventType::Dead,
            JobEventType::Started,
            JobEventType::Retried,
        ] {
            collector
                .record_event(create_test_job_event(
                    "failing", event_type, None, None, None,
                ))
                .await
                .unwrap();
        }
        let stats = collector
            .get_queue_statistics("failing", Duration::from_secs(60))
            .await
            .unwrap();
        assert_eq!(stats.total_processed, 2);
        assert_eq!(stats.error_rate, 1.0);
        assert_eq!(stats.throughput_per_minute, 2.0);
    }

    fn event_at(queue: &str, seconds_ago: i64, processing_time_ms: Option<u64>) -> JobEvent {
        JobEvent {
            timestamp: Utc::now() - chrono::Duration::seconds(seconds_ago),
            ..create_test_job_event(
                queue,
                JobEventType::Completed,
                None,
                processing_time_ms,
                None,
            )
        }
    }

    /// M7: at the size limit, recording drops the oldest event instead of shifting the
    /// whole buffer, and the buffer never grows past `max_events`.
    #[tokio::test]
    async fn test_record_event_is_bounded_and_drops_the_oldest() {
        let collector = InMemoryStatsCollector::new(StatsConfig {
            max_events: 3,
            ..Default::default()
        });
        for seconds_ago in [50, 40, 30, 20, 10] {
            collector
                .record_event(event_at("q", seconds_ago, Some(seconds_ago as u64)))
                .await
                .unwrap();
        }
        let times: Vec<_> = collector
            .read_events()
            .iter()
            .map(|e| e.processing_time_ms.unwrap())
            .collect();
        assert_eq!(times, vec![30, 20, 10]);
    }

    /// Late events are inserted in timestamp order, so windows stay exact.
    #[tokio::test]
    async fn test_out_of_order_events_are_kept_in_time_order() {
        let collector = InMemoryStatsCollector::new_default();
        for seconds_ago in [10, 500, 5, 120] {
            collector
                .record_event(event_at("q", seconds_ago, Some(seconds_ago as u64)))
                .await
                .unwrap();
        }
        {
            let events = collector.read_events();
            assert!(
                events
                    .iter()
                    .zip(events.iter().skip(1))
                    .all(|(a, b)| a.timestamp <= b.timestamp)
            );
        }

        // The last 60 seconds hold the events from 10 s and 5 s ago only.
        let stats = collector
            .get_queue_statistics("q", Duration::from_secs(60))
            .await
            .unwrap();
        assert_eq!(stats.total_processed, 2);
        assert_eq!(
            (stats.min_processing_time_ms, stats.max_processing_time_ms),
            (5, 10)
        );
        let all = collector
            .get_system_statistics(Duration::from_secs(3600))
            .await
            .unwrap();
        assert_eq!(all.total_processed, 4);
    }

    /// `max_event_age_secs` is applied when recording, without a cleanup call.
    #[tokio::test]
    async fn test_expired_events_are_pruned_when_recording() {
        let collector = InMemoryStatsCollector::new(StatsConfig {
            max_event_age_secs: 60,
            ..Default::default()
        });
        collector
            .record_event(event_at("q", 120, None))
            .await
            .unwrap();
        assert_eq!(collector.read_events().len(), 0, "too old to record");

        // Recorded while fresh, then expired by the time the next event arrives.
        collector
            .record_event(event_at("q", 59, None))
            .await
            .unwrap();
        collector.write_events()[0].timestamp = Utc::now() - chrono::Duration::seconds(61);
        collector
            .record_event(event_at("q", 0, None))
            .await
            .unwrap();
        assert_eq!(collector.read_events().len(), 1);
    }

    /// `collect_timing = false` discards processing times.
    #[tokio::test]
    async fn test_collect_timing_false_discards_processing_times() {
        let collector = InMemoryStatsCollector::new(StatsConfig {
            collect_timing: false,
            ..Default::default()
        });
        collector
            .record_event(event_at("q", 0, Some(250)))
            .await
            .unwrap();
        let stats = collector
            .get_queue_statistics("q", Duration::from_secs(60))
            .await
            .unwrap();
        assert_eq!(stats.total_processed, 1);
        assert_eq!(stats.avg_processing_time_ms, 0.0);
        assert_eq!(stats.max_processing_time_ms, 0);
        assert!(
            stats
                .priority_stats
                .unwrap()
                .avg_processing_times
                .is_empty()
        );
    }

    /// Per-priority and per-queue aggregates match the events.
    #[tokio::test]
    async fn test_priority_breakdown_and_queue_split() {
        let collector = InMemoryStatsCollector::new_default();
        for (queue, priority, time) in [
            ("a", JobPriority::High, Some(10)),
            ("a", JobPriority::High, Some(30)),
            ("a", JobPriority::Low, None),
            ("b", JobPriority::Low, Some(7)),
        ] {
            collector
                .record_event(create_test_job_event(
                    queue,
                    JobEventType::Completed,
                    Some(priority),
                    time,
                    None,
                ))
                .await
                .unwrap();
        }
        let a = collector
            .get_queue_statistics("a", Duration::from_secs(60))
            .await
            .unwrap();
        let priorities = a.priority_stats.unwrap();
        assert_eq!(priorities.job_counts[&JobPriority::High], 2);
        assert_eq!(priorities.job_counts[&JobPriority::Low], 1);
        assert_eq!(priorities.avg_processing_times[&JobPriority::High], 20.0);
        assert!(
            !priorities
                .avg_processing_times
                .contains_key(&JobPriority::Low)
        );
        assert_eq!(a.avg_processing_time_ms, 20.0);

        let mut all = collector
            .get_all_statistics(Duration::from_secs(60))
            .await
            .unwrap();
        all.sort_by(|x, y| x.queue_name.cmp(&y.queue_name));
        let counts: Vec<_> = all
            .iter()
            .map(|q| (q.queue_name.as_str(), q.statistics.total_processed))
            .collect();
        assert_eq!(counts, vec![("a", 3), ("b", 1)]);
    }
}
