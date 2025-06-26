use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub type JobId = Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum JobStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Dead,
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
    pub error_message: Option<String>,
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
            error_message: None,
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
            error_message: None,
        }
    }

    pub fn with_max_attempts(mut self, max_attempts: i32) -> Self {
        self.max_attempts = max_attempts;
        self
    }

    /// Check if the job is dead (failed all retry attempts)
    pub fn is_dead(&self) -> bool {
        self.status == JobStatus::Dead
    }

    /// Check if the job has exhausted all retry attempts
    pub fn has_exhausted_retries(&self) -> bool {
        self.attempts >= self.max_attempts
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
                .unwrap_or_else(Utc::now) - started
        })
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
        assert_eq!(JobStatus::Retrying, JobStatus::Retrying);

        assert_ne!(JobStatus::Pending, JobStatus::Running);
        assert_ne!(JobStatus::Completed, JobStatus::Failed);
        assert_ne!(JobStatus::Failed, JobStatus::Dead);
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
}
