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