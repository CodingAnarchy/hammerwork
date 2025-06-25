pub mod error;
pub mod job;
pub mod queue;
pub mod worker;

pub use error::HammerworkError;
pub use job::{Job, JobId, JobStatus};
pub use queue::JobQueue;
pub use worker::Worker;

pub type Result<T> = std::result::Result<T, HammerworkError>;
