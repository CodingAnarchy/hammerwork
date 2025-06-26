use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use crate::cron::CronSchedule;

pub type JobId = Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum JobStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Dead,
    TimedOut,
    Retrying,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    pub id: JobId,
    pub queue_name: String,
    pub payload: serde_json::Value,
    pub status: JobStatus,
    pub attempts: i32,
    pub max_attempts: i32,
    pub created_at: DateTime<Utc>,
    pub scheduled_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub failed_at: Option<DateTime<Utc>>,
    pub timed_out_at: Option<DateTime<Utc>>,
    pub timeout: Option<std::time::Duration>,
    pub error_message: Option<String>,
    // Cron-related fields
    pub cron_schedule: Option<String>,
    pub next_run_at: Option<DateTime<Utc>>,
    pub recurring: bool,
    pub timezone: Option<String>,
}

impl Job {
    pub fn new(queue_name: String, payload: serde_json::Value) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4(),
            queue_name,
            payload,
            status: JobStatus::Pending,
            attempts: 0,
            max_attempts: 3,
            created_at: now,
            scheduled_at: now,
            started_at: None,
            completed_at: None,
            failed_at: None,
            timed_out_at: None,
            timeout: None,
            error_message: None,
            cron_schedule: None,
            next_run_at: None,
            recurring: false,
            timezone: None,
        }
    }

    pub fn with_delay(
        queue_name: String,
        payload: serde_json::Value,
        delay: chrono::Duration,
    ) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4(),
            queue_name,
            payload,
            status: JobStatus::Pending,
            attempts: 0,
            max_attempts: 3,
            created_at: now,
            scheduled_at: now + delay,
            started_at: None,
            completed_at: None,
            failed_at: None,
            timed_out_at: None,
            timeout: None,
            error_message: None,
            cron_schedule: None,
            next_run_at: None,
            recurring: false,
            timezone: None,
        }
    }

    pub fn with_max_attempts(mut self, max_attempts: i32) -> Self {
        self.max_attempts = max_attempts;
        self
    }

    pub fn with_timeout(mut self, timeout: std::time::Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// Create a recurring job with a cron schedule
    pub fn with_cron_schedule(queue_name: String, payload: serde_json::Value, cron_schedule: CronSchedule) -> Result<Self, crate::cron::CronError> {
        let now = Utc::now();
        let next_run = cron_schedule.next_execution_from_now();
        
        Ok(Self {
            id: Uuid::new_v4(),
            queue_name,
            payload,
            status: JobStatus::Pending,
            attempts: 0,
            max_attempts: 3,
            created_at: now,
            scheduled_at: next_run.unwrap_or(now),
            started_at: None,
            completed_at: None,
            failed_at: None,
            timed_out_at: None,
            timeout: None,
            error_message: None,
            cron_schedule: Some(cron_schedule.expression.clone()),
            next_run_at: next_run,
            recurring: true,
            timezone: Some(cron_schedule.timezone.clone()),
        })
    }

    /// Add a cron schedule to an existing job
    pub fn with_cron(mut self, cron_schedule: CronSchedule) -> Result<Self, crate::cron::CronError> {
        let next_run = cron_schedule.next_execution_from_now();
        self.cron_schedule = Some(cron_schedule.expression.clone());
        self.next_run_at = next_run;
        self.recurring = true;
        self.timezone = Some(cron_schedule.timezone.clone());
        self.scheduled_at = next_run.unwrap_or(self.scheduled_at);
        Ok(self)
    }

    /// Set the job as recurring without a cron schedule (for manual rescheduling)
    pub fn as_recurring(mut self) -> Self {
        self.recurring = true;
        self
    }

    /// Set the timezone for the job
    pub fn with_timezone(mut self, timezone: String) -> Self {
        self.timezone = Some(timezone);
        self
    }

    /// Check if the job is dead (failed all retry attempts)
    pub fn is_dead(&self) -> bool {
        self.status == JobStatus::Dead
    }

    /// Check if the job has timed out
    pub fn is_timed_out(&self) -> bool {
        self.status == JobStatus::TimedOut
    }

    /// Check if the job has exhausted all retry attempts
    pub fn has_exhausted_retries(&self) -> bool {
        self.attempts >= self.max_attempts
    }

    /// Check if the job should timeout based on its start time and timeout setting
    pub fn should_timeout(&self) -> bool {
        if let (Some(started_at), Some(timeout)) = (self.started_at, self.timeout) {
            let elapsed = Utc::now() - started_at;
            let timeout_duration = chrono::Duration::from_std(timeout).unwrap_or_default();
            elapsed >= timeout_duration
        } else {
            false
        }
    }

    /// Get the duration since the job was created
    pub fn age(&self) -> chrono::Duration {
        Utc::now() - self.created_at
    }

    /// Get the processing duration if the job has started
    pub fn processing_duration(&self) -> Option<chrono::Duration> {
        self.started_at.map(|started| {
            self.completed_at
                .or(self.failed_at)
                .or(self.timed_out_at)
                .unwrap_or_else(Utc::now) - started
        })
    }

    /// Check if this is a recurring job
    pub fn is_recurring(&self) -> bool {
        self.recurring
    }

    /// Check if this job has a cron schedule
    pub fn has_cron_schedule(&self) -> bool {
        self.cron_schedule.is_some()
    }

    /// Get the cron schedule if it exists
    pub fn get_cron_schedule(&self) -> Option<Result<CronSchedule, crate::cron::CronError>> {
        self.cron_schedule.as_ref().map(|expr| {
            match &self.timezone {
                Some(tz) => CronSchedule::with_timezone(expr, tz),
                None => CronSchedule::new(expr),
            }
        })
    }

    /// Calculate the next run time for a recurring job
    pub fn calculate_next_run(&self) -> Option<DateTime<Utc>> {
        if !self.recurring {
            return None;
        }

        if let Some(cron_schedule) = self.get_cron_schedule() {
            match cron_schedule {
                Ok(schedule) => schedule.next_execution_from_now(),
                Err(_) => None,
            }
        } else {
            None
        }
    }

    /// Update the job for the next run (for recurring jobs)
    pub fn prepare_for_next_run(&mut self) -> Option<DateTime<Utc>> {
        if !self.recurring {
            return None;
        }

        let next_run = self.calculate_next_run();
        if let Some(next_time) = next_run {
            self.status = JobStatus::Pending;
            self.attempts = 0;
            self.scheduled_at = next_time;
            self.next_run_at = Some(next_time);
            self.started_at = None;
            self.completed_at = None;
            self.failed_at = None;
            self.timed_out_at = None;
            self.error_message = None;
        }
        next_run
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_job_new() {
        let queue_name = "test_queue".to_string();
        let payload = json!({"key": "value"});

        let job = Job::new(queue_name.clone(), payload.clone());

        assert_eq!(job.queue_name, queue_name);
        assert_eq!(job.payload, payload);
        assert_eq!(job.status, JobStatus::Pending);
        assert_eq!(job.attempts, 0);
        assert_eq!(job.max_attempts, 3);
        assert!(job.started_at.is_none());
        assert!(job.completed_at.is_none());
        assert!(job.failed_at.is_none());
        assert!(job.error_message.is_none());
        assert_eq!(job.created_at, job.scheduled_at);
    }

    #[test]
    fn test_job_with_delay() {
        let queue_name = "test_queue".to_string();
        let payload = json!({"key": "value"});
        let delay = chrono::Duration::minutes(5);

        let job = Job::with_delay(queue_name.clone(), payload.clone(), delay);

        assert_eq!(job.queue_name, queue_name);
        assert_eq!(job.payload, payload);
        assert_eq!(job.status, JobStatus::Pending);
        assert_eq!(job.attempts, 0);
        assert_eq!(job.max_attempts, 3);
        assert!(job.scheduled_at > job.created_at);
        assert_eq!(job.scheduled_at - job.created_at, delay);
    }

    #[test]
    fn test_job_with_max_attempts() {
        let queue_name = "test_queue".to_string();
        let payload = json!({"key": "value"});

        let job = Job::new(queue_name, payload).with_max_attempts(5);

        assert_eq!(job.max_attempts, 5);
    }

    #[test]
    fn test_job_with_delay_and_max_attempts() {
        let queue_name = "test_queue".to_string();
        let payload = json!({"key": "value"});
        let delay = chrono::Duration::hours(1);

        let job = Job::with_delay(queue_name, payload, delay).with_max_attempts(10);

        assert_eq!(job.max_attempts, 10);
        assert!(job.scheduled_at > job.created_at);
    }

    #[test]
    fn test_job_status_equality() {
        assert_eq!(JobStatus::Pending, JobStatus::Pending);
        assert_eq!(JobStatus::Running, JobStatus::Running);
        assert_eq!(JobStatus::Completed, JobStatus::Completed);
        assert_eq!(JobStatus::Failed, JobStatus::Failed);
        assert_eq!(JobStatus::Dead, JobStatus::Dead);
        assert_eq!(JobStatus::TimedOut, JobStatus::TimedOut);
        assert_eq!(JobStatus::Retrying, JobStatus::Retrying);

        assert_ne!(JobStatus::Pending, JobStatus::Running);
        assert_ne!(JobStatus::Completed, JobStatus::Failed);
        assert_ne!(JobStatus::Failed, JobStatus::Dead);
        assert_ne!(JobStatus::Dead, JobStatus::TimedOut);
        assert_ne!(JobStatus::TimedOut, JobStatus::Failed);
    }

    #[test]
    fn test_job_serialization() {
        let job = Job::new("test".to_string(), json!({"data": "test"}));

        let serialized = serde_json::to_string(&job).unwrap();
        let deserialized: Job = serde_json::from_str(&serialized).unwrap();

        assert_eq!(job.id, deserialized.id);
        assert_eq!(job.queue_name, deserialized.queue_name);
        assert_eq!(job.payload, deserialized.payload);
        assert_eq!(job.status, deserialized.status);
        assert_eq!(job.attempts, deserialized.attempts);
        assert_eq!(job.max_attempts, deserialized.max_attempts);
    }

    #[test]
    fn test_job_status_serialization() {
        let statuses = vec![
            JobStatus::Pending,
            JobStatus::Running,
            JobStatus::Completed,
            JobStatus::Failed,
            JobStatus::Dead,
            JobStatus::TimedOut,
            JobStatus::Retrying,
        ];

        for status in statuses {
            let serialized = serde_json::to_string(&status).unwrap();
            let deserialized: JobStatus = serde_json::from_str(&serialized).unwrap();
            assert_eq!(status, deserialized);
        }
    }

    #[test]
    fn test_job_dead_status_methods() {
        let mut job = Job::new("test".to_string(), json!({"data": "test"}));
        
        // Initially not dead
        assert!(!job.is_dead());
        assert!(!job.has_exhausted_retries());
        
        // Simulate exhausting retries
        job.attempts = 3;
        job.max_attempts = 3;
        assert!(job.has_exhausted_retries());
        assert!(!job.is_dead()); // Still not dead until status is set
        
        // Mark as dead
        job.status = JobStatus::Dead;
        job.failed_at = Some(Utc::now());
        assert!(job.is_dead());
        assert!(job.has_exhausted_retries());
    }

    #[test]
    fn test_job_processing_duration() {
        let mut job = Job::new("test".to_string(), json!({"data": "test"}));
        
        // No processing duration when not started
        assert!(job.processing_duration().is_none());
        
        // Set start time
        let start_time = Utc::now();
        job.started_at = Some(start_time);
        
        // Should have some duration now (very small)
        let duration = job.processing_duration().unwrap();
        assert!(duration.num_milliseconds() >= 0);
        
        // Set completion time
        let completion_time = start_time + chrono::Duration::seconds(5);
        job.completed_at = Some(completion_time);
        
        let final_duration = job.processing_duration().unwrap();
        assert_eq!(final_duration.num_seconds(), 5);
    }

    #[test]
    fn test_job_age() {
        let job = Job::new("test".to_string(), json!({"data": "test"}));
        let age = job.age();
        
        // Age should be very small (just created)
        assert!(age.num_milliseconds() >= 0);
        assert!(age.num_seconds() < 1);
    }

    #[test]
    fn test_job_with_timeout() {
        let timeout = std::time::Duration::from_secs(30);
        let job = Job::new("test".to_string(), json!({"data": "test"}))
            .with_timeout(timeout);
        
        assert_eq!(job.timeout, Some(timeout));
        assert!(!job.is_timed_out()); // Not timed out until status is set
    }

    #[test]
    fn test_job_timeout_status_methods() {
        let mut job = Job::new("test".to_string(), json!({"data": "test"}));
        
        // Initially not timed out
        assert!(!job.is_timed_out());
        
        // Set timed out status
        job.status = JobStatus::TimedOut;
        job.timed_out_at = Some(Utc::now());
        assert!(job.is_timed_out());
    }

    #[test]
    fn test_job_should_timeout() {
        let mut job = Job::new("test".to_string(), json!({"data": "test"}))
            .with_timeout(std::time::Duration::from_millis(100));
        
        // Should not timeout before it starts
        assert!(!job.should_timeout());
        
        // Set start time to simulate job starting
        job.started_at = Some(Utc::now() - chrono::Duration::milliseconds(200));
        
        // Should timeout since 200ms > 100ms timeout
        assert!(job.should_timeout());
        
        // Job without timeout should never timeout
        let mut job_no_timeout = Job::new("test".to_string(), json!({"data": "test"}));
        job_no_timeout.started_at = Some(Utc::now() - chrono::Duration::hours(1));
        assert!(!job_no_timeout.should_timeout());
    }

    #[test]
    fn test_job_with_delay_and_timeout() {
        let delay = chrono::Duration::minutes(5);
        let timeout = std::time::Duration::from_secs(120);
        
        let job = Job::with_delay("test".to_string(), json!({"data": "test"}), delay)
            .with_timeout(timeout)
            .with_max_attempts(5);
        
        assert_eq!(job.timeout, Some(timeout));
        assert_eq!(job.max_attempts, 5);
        assert!(job.scheduled_at > job.created_at);
        assert_eq!(job.scheduled_at - job.created_at, delay);
    }

    #[test]
    fn test_processing_duration_with_timeout() {
        let mut job = Job::new("test".to_string(), json!({"data": "test"}));
        
        // Set start time and timed out time
        let start_time = Utc::now() - chrono::Duration::seconds(5);
        let timeout_time = start_time + chrono::Duration::seconds(3);
        
        job.started_at = Some(start_time);
        job.timed_out_at = Some(timeout_time);
        
        let duration = job.processing_duration().unwrap();
        assert_eq!(duration.num_seconds(), 3);
    }

    #[test]
    fn test_timeout_builder_methods() {
        let job = Job::new("test".to_string(), json!({"key": "value"}))
            .with_timeout(std::time::Duration::from_secs(120))
            .with_max_attempts(5);
        
        assert_eq!(job.timeout, Some(std::time::Duration::from_secs(120)));
        assert_eq!(job.max_attempts, 5);
        assert_eq!(job.queue_name, "test");
    }

    #[test]
    fn test_job_timeout_edge_cases() {
        let mut job = Job::new("test".to_string(), json!({"data": "test"}));
        
        // Job without timeout should never timeout
        assert!(!job.should_timeout());
        
        // Job with timeout but not started should not timeout
        job.timeout = Some(std::time::Duration::from_millis(100));
        assert!(!job.should_timeout());
        
        // Job with timeout and started but within timeout window should not timeout
        job.started_at = Some(Utc::now() - chrono::Duration::milliseconds(50));
        assert!(!job.should_timeout());
        
        // Job with timeout and started beyond timeout window should timeout
        job.started_at = Some(Utc::now() - chrono::Duration::milliseconds(150));
        assert!(job.should_timeout());
    }

    #[test]
    fn test_job_status_transitions_with_timeout() {
        let mut job = Job::new("test".to_string(), json!({"data": "test"}));
        
        // Initial state
        assert_eq!(job.status, JobStatus::Pending);
        assert!(!job.is_timed_out());
        
        // Simulate timeout
        job.status = JobStatus::TimedOut;
        job.timed_out_at = Some(Utc::now());
        
        assert!(job.is_timed_out());
        assert!(!job.is_dead()); // TimedOut is different from Dead
    }

    #[test]
    fn test_timeout_serialization_compatibility() {
        let original_job = Job::new("test_queue".to_string(), json!({"data": "test"}))
            .with_timeout(std::time::Duration::from_secs(300))
            .with_max_attempts(5);
        
        // Serialize and deserialize
        let serialized = serde_json::to_string(&original_job).unwrap();
        let deserialized: Job = serde_json::from_str(&serialized).unwrap();
        
        // Verify timeout field is preserved
        assert_eq!(original_job.timeout, deserialized.timeout);
        assert_eq!(original_job.timed_out_at, deserialized.timed_out_at);
        assert_eq!(original_job.status, deserialized.status);
    }

    #[test]
    fn test_job_with_all_timeout_fields() {
        let timeout_duration = std::time::Duration::from_secs(60);
        let mut job = Job::new("comprehensive_test".to_string(), json!({"test": true}))
            .with_timeout(timeout_duration)
            .with_max_attempts(3);
        
        // Simulate job lifecycle with timeout
        job.started_at = Some(Utc::now() - chrono::Duration::seconds(30));
        job.status = JobStatus::Running;
        
        // Should not timeout yet (30s < 60s)
        assert!(!job.should_timeout());
        
        // Simulate timeout occurring
        job.status = JobStatus::TimedOut;
        job.timed_out_at = Some(Utc::now());
        job.error_message = Some("Job timed out after 60s".to_string());
        
        assert!(job.is_timed_out());
        assert_eq!(job.timeout, Some(timeout_duration));
        assert!(job.timed_out_at.is_some());
        assert!(job.error_message.is_some());
    }
}
