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
            error_message: None,
        }
    }

    pub fn with_max_attempts(mut self, max_attempts: i32) -> Self {
        self.max_attempts = max_attempts;
        self
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
        assert_eq!(JobStatus::Retrying, JobStatus::Retrying);

        assert_ne!(JobStatus::Pending, JobStatus::Running);
        assert_ne!(JobStatus::Completed, JobStatus::Failed);
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
            JobStatus::Retrying,
        ];

        for status in statuses {
            let serialized = serde_json::to_string(&status).unwrap();
            let deserialized: JobStatus = serde_json::from_str(&serialized).unwrap();
            assert_eq!(status, deserialized);
        }
    }
}
