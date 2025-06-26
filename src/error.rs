use thiserror::Error;

#[derive(Error, Debug)]
pub enum HammerworkError {
    #[error("Database error: {0}")]
    Database(#[from] sqlx::Error),
    
    #[error("Job not found: {id}")]
    JobNotFound { id: String },
    
    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
    
    #[error("Worker error: {message}")]
    Worker { message: String },
    
    #[error("Queue error: {message}")]
    Queue { message: String },
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
        assert!(matches!(hammerwork_error, HammerworkError::Serialization(_)));
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