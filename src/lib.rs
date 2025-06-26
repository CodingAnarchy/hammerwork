pub mod cron;
pub mod error;
pub mod job;
pub mod priority;
pub mod queue;
pub mod rate_limit;
pub mod stats;
pub mod worker;

pub use cron::{CronError, CronSchedule};
pub use error::HammerworkError;
pub use job::{Job, JobId, JobStatus};
pub use priority::{
    JobPriority, PriorityError, PrioritySelectionStrategy, PriorityStats, PriorityWeights,
};
pub use queue::JobQueue;
pub use rate_limit::{RateLimit, RateLimiter, ThrottleConfig};
pub use stats::{
    DeadJobSummary, InMemoryStatsCollector, JobStatistics, QueueStats, StatisticsCollector,
};
pub use worker::{Worker, WorkerPool};

pub type Result<T> = std::result::Result<T, HammerworkError>;
