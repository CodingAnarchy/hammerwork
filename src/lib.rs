pub mod cron;
pub mod error;
pub mod job;
pub mod queue;
pub mod stats;
pub mod worker;

pub use cron::{CronSchedule, CronError};
pub use error::HammerworkError;
pub use job::{Job, JobId, JobStatus};
pub use queue::JobQueue;
pub use stats::{JobStatistics, QueueStats, DeadJobSummary, StatisticsCollector, InMemoryStatsCollector};
pub use worker::{Worker, WorkerPool};

pub type Result<T> = std::result::Result<T, HammerworkError>;
