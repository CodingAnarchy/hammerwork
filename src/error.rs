use thiserror::Error;

#[derive(Error, Debug)]
pub enum HammerworkError {
    #[error("Database error: {0}")]
    Database(#[from] sqlx::Error),

    #[error("Job not found: {id}")]
    JobNotFound { id: String },

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("UUID parsing error: {0}")]
    UuidParsing(#[from] uuid::Error),

    #[error("Worker error: {message}")]
    Worker { message: String },

    #[error("Queue error: {message}")]
    Queue { message: String },

    #[error("Rate limit error: {message}")]
    RateLimit { message: String },

    #[error("Metrics error: {message}")]
    Metrics { message: String },

    #[error("Alerting error: {message}")]
    Alerting { message: String },

    #[error("Batch operation error: {message}")]
    Batch { message: String },

    #[error("Processing error: {0}")]
    Processing(String),

    #[error("Workflow error: {message}")]
    Workflow { message: String },

    #[error("Tracing error: {message}")]
    Tracing { message: String },

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Archive error: {message}")]
    Archive { message: String },

    #[error("Spawn error: {0}")]
    SpawnError(#[from] crate::spawn::SpawnError),

    #[error("Invalid job payload: {message}")]
    InvalidJobPayload { message: String },

    #[error("Streaming error: {message}")]
    Streaming { message: String },

    #[error("Webhook error: {message}")]
    Webhook { message: String },

    #[error("Event system error: {message}")]
    Event { message: String },

    #[error("Configuration error: {0}")]
    Config(String),
}

// Add From implementations for toml errors
impl From<toml::de::Error> for HammerworkError {
    fn from(err: toml::de::Error) -> Self {
        HammerworkError::Config(format!("TOML deserialization error: {}", err))
    }
}

impl From<toml::ser::Error> for HammerworkError {
    fn from(err: toml::ser::Error) -> Self {
        HammerworkError::Config(format!("TOML serialization error: {}", err))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_display() {
        let worker_error = HammerworkError::Worker {
            message: "Test worker error".to_string(),
        };
        assert_eq!(worker_error.to_string(), "Worker error: Test worker error");

        let queue_error = HammerworkError::Queue {
            message: "Test queue error".to_string(),
        };
        assert_eq!(queue_error.to_string(), "Queue error: Test queue error");

        let job_not_found = HammerworkError::JobNotFound {
            id: "test-id".to_string(),
        };
        assert_eq!(job_not_found.to_string(), "Job not found: test-id");
    }

    #[test]
    fn test_error_from_serde_json() {
        let json_error = serde_json::from_str::<serde_json::Value>("invalid json");
        assert!(json_error.is_err());

        let hammerwork_error: HammerworkError = json_error.unwrap_err().into();
        assert!(matches!(
            hammerwork_error,
            HammerworkError::Serialization(_)
        ));
    }

    #[test]
    fn test_error_debug() {
        let error = HammerworkError::Worker {
            message: "Debug test".to_string(),
        };

        let debug_str = format!("{:?}", error);
        assert!(debug_str.contains("Worker"));
        assert!(debug_str.contains("Debug test"));
    }
}
