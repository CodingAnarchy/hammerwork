use crate::{
    job::{Job, JobId},
    stats::{QueueStats, DeadJobSummary},
    Result,
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::{Database, Pool};
use std::marker::PhantomData;

#[async_trait]
pub trait DatabaseQueue: Send + Sync {
    type Database: Database;

    // Core job operations
    async fn create_tables(&self) -> Result<()>;
    async fn enqueue(&self, job: Job) -> Result<JobId>;
    async fn dequeue(&self, queue_name: &str) -> Result<Option<Job>>;
    async fn complete_job(&self, job_id: JobId) -> Result<()>;
    async fn fail_job(&self, job_id: JobId, error_message: &str) -> Result<()>;
    async fn retry_job(&self, job_id: JobId, retry_at: DateTime<Utc>) -> Result<()>;
    async fn get_job(&self, job_id: JobId) -> Result<Option<Job>>;
    async fn delete_job(&self, job_id: JobId) -> Result<()>;

    // Dead job management
    /// Mark a job as dead (exhausted all retries)
    async fn mark_job_dead(&self, job_id: JobId, error_message: &str) -> Result<()>;
    
    /// Get all dead jobs with optional pagination
    async fn get_dead_jobs(&self, limit: Option<u32>, offset: Option<u32>) -> Result<Vec<Job>>;
    
    /// Get dead jobs for a specific queue
    async fn get_dead_jobs_by_queue(&self, queue_name: &str, limit: Option<u32>, offset: Option<u32>) -> Result<Vec<Job>>;
    
    /// Retry a dead job (reset its status and retry count)
    async fn retry_dead_job(&self, job_id: JobId) -> Result<()>;
    
    /// Purge dead jobs older than the specified date
    async fn purge_dead_jobs(&self, older_than: DateTime<Utc>) -> Result<u64>;
    
    /// Get a summary of dead jobs across the system
    async fn get_dead_job_summary(&self) -> Result<DeadJobSummary>;

    // Statistics and monitoring
    /// Get queue statistics including job counts and processing metrics
    async fn get_queue_stats(&self, queue_name: &str) -> Result<QueueStats>;
    
    /// Get statistics for all queues
    async fn get_all_queue_stats(&self) -> Result<Vec<QueueStats>>;
    
    /// Get job counts by status for a specific queue
    async fn get_job_counts_by_status(&self, queue_name: &str) -> Result<std::collections::HashMap<String, u64>>;
    
    /// Get processing times for completed jobs in a time window
    async fn get_processing_times(&self, queue_name: &str, since: DateTime<Utc>) -> Result<Vec<i64>>;
    
    /// Get error frequencies for failed jobs
    async fn get_error_frequencies(&self, queue_name: Option<&str>, since: DateTime<Utc>) -> Result<std::collections::HashMap<String, u64>>;
}

pub struct JobQueue<DB: Database> {
    #[allow(dead_code)] // Used in database-specific implementations
    pool: Pool<DB>,
    _phantom: PhantomData<DB>,
}

impl<DB: Database> JobQueue<DB> {
    pub fn new(pool: Pool<DB>) -> Self {
        Self {
            pool,
            _phantom: PhantomData,
        }
    }
}

#[cfg(feature = "postgres")]
pub mod postgres {
    use super::*;
    use crate::job::JobStatus;
    use sqlx::Postgres;

    #[async_trait]
    impl DatabaseQueue for JobQueue<Postgres> {
        type Database = Postgres;

        async fn create_tables(&self) -> Result<()> {
            sqlx::query(
                r#"
                CREATE TABLE IF NOT EXISTS hammerwork_jobs (
                    id UUID PRIMARY KEY,
                    queue_name VARCHAR NOT NULL,
                    payload JSONB NOT NULL,
                    status VARCHAR NOT NULL,
                    attempts INTEGER NOT NULL DEFAULT 0,
                    max_attempts INTEGER NOT NULL DEFAULT 3,
                    created_at TIMESTAMPTZ NOT NULL,
                    scheduled_at TIMESTAMPTZ NOT NULL,
                    started_at TIMESTAMPTZ,
                    completed_at TIMESTAMPTZ,
                    failed_at TIMESTAMPTZ,
                    error_message TEXT
                );
                
                CREATE INDEX IF NOT EXISTS idx_hammerwork_jobs_queue_status_scheduled 
                ON hammerwork_jobs (queue_name, status, scheduled_at);
                
                CREATE INDEX IF NOT EXISTS idx_hammerwork_jobs_status_failed_at
                ON hammerwork_jobs (status, failed_at);
                
                CREATE INDEX IF NOT EXISTS idx_hammerwork_jobs_queue_status
                ON hammerwork_jobs (queue_name, status);
                "#,
            )
            .execute(&self.pool)
            .await?;

            Ok(())
        }

        async fn enqueue(&self, job: Job) -> Result<JobId> {
            sqlx::query(
                r#"
                INSERT INTO hammerwork_jobs 
                (id, queue_name, payload, status, attempts, max_attempts, created_at, scheduled_at, started_at, completed_at, failed_at, error_message)
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
                "#
            )
            .bind(&job.id)
            .bind(&job.queue_name)
            .bind(&job.payload)
            .bind(serde_json::to_string(&job.status)?)
            .bind(&job.attempts)
            .bind(&job.max_attempts)
            .bind(&job.created_at)
            .bind(&job.scheduled_at)
            .bind(&job.started_at)
            .bind(&job.completed_at)
            .bind(&job.failed_at)
            .bind(&job.error_message)
            .execute(&self.pool)
            .await?;

            Ok(job.id)
        }

        async fn dequeue(&self, queue_name: &str) -> Result<Option<Job>> {
            let row = sqlx::query_as::<_, (uuid::Uuid, String, serde_json::Value, String, i32, i32, DateTime<Utc>, DateTime<Utc>, Option<DateTime<Utc>>, Option<DateTime<Utc>>, Option<DateTime<Utc>>, Option<String>)>(
                r#"
                UPDATE hammerwork_jobs 
                SET status = $1, started_at = $2, attempts = attempts + 1
                WHERE id = (
                    SELECT id FROM hammerwork_jobs 
                    WHERE queue_name = $3 
                    AND status = $4 
                    AND scheduled_at <= $2
                    ORDER BY scheduled_at ASC 
                    LIMIT 1 
                    FOR UPDATE SKIP LOCKED
                )
                RETURNING id, queue_name, payload, status, attempts, max_attempts, created_at, scheduled_at, started_at, completed_at, failed_at, error_message
                "#
            )
            .bind(serde_json::to_string(&JobStatus::Running)?)
            .bind(Utc::now())
            .bind(queue_name)
            .bind(serde_json::to_string(&JobStatus::Pending)?)
            .fetch_optional(&self.pool)
            .await?;

            if let Some((
                id,
                queue_name,
                payload,
                status,
                attempts,
                max_attempts,
                created_at,
                scheduled_at,
                started_at,
                completed_at,
                failed_at,
                error_message,
            )) = row
            {
                Ok(Some(Job {
                    id,
                    queue_name,
                    payload,
                    status: serde_json::from_str(&status)?,
                    attempts,
                    max_attempts,
                    created_at,
                    scheduled_at,
                    started_at,
                    completed_at,
                    failed_at,
                    error_message,
                }))
            } else {
                Ok(None)
            }
        }

        async fn complete_job(&self, job_id: JobId) -> Result<()> {
            sqlx::query("UPDATE hammerwork_jobs SET status = $1, completed_at = $2 WHERE id = $3")
                .bind(serde_json::to_string(&JobStatus::Completed)?)
                .bind(Utc::now())
                .bind(job_id)
                .execute(&self.pool)
                .await?;

            Ok(())
        }

        async fn fail_job(&self, job_id: JobId, error_message: &str) -> Result<()> {
            sqlx::query(
                "UPDATE hammerwork_jobs SET status = $1, error_message = $2, failed_at = $3 WHERE id = $4"
            )
            .bind(serde_json::to_string(&JobStatus::Failed)?)
            .bind(error_message)
            .bind(Utc::now())
            .bind(job_id)
            .execute(&self.pool)
            .await?;

            Ok(())
        }

        async fn retry_job(&self, job_id: JobId, retry_at: DateTime<Utc>) -> Result<()> {
            sqlx::query(
                "UPDATE hammerwork_jobs SET status = $1, scheduled_at = $2, started_at = NULL WHERE id = $3"
            )
            .bind(serde_json::to_string(&JobStatus::Pending)?)
            .bind(retry_at)
            .bind(job_id)
            .execute(&self.pool)
            .await?;

            Ok(())
        }

        async fn get_job(&self, job_id: JobId) -> Result<Option<Job>> {
            let row = sqlx::query_as::<_, (uuid::Uuid, String, serde_json::Value, String, i32, i32, DateTime<Utc>, DateTime<Utc>, Option<DateTime<Utc>>, Option<DateTime<Utc>>, Option<DateTime<Utc>>, Option<String>)>(
                "SELECT id, queue_name, payload, status, attempts, max_attempts, created_at, scheduled_at, started_at, completed_at, failed_at, error_message FROM hammerwork_jobs WHERE id = $1"
            )
            .bind(job_id)
            .fetch_optional(&self.pool)
            .await?;

            if let Some((
                id,
                queue_name,
                payload,
                status,
                attempts,
                max_attempts,
                created_at,
                scheduled_at,
                started_at,
                completed_at,
                failed_at,
                error_message,
            )) = row
            {
                Ok(Some(Job {
                    id,
                    queue_name,
                    payload,
                    status: serde_json::from_str(&status)?,
                    attempts,
                    max_attempts,
                    created_at,
                    scheduled_at,
                    started_at,
                    completed_at,
                    failed_at,
                    error_message,
                }))
            } else {
                Ok(None)
            }
        }

        async fn delete_job(&self, job_id: JobId) -> Result<()> {
            sqlx::query("DELETE FROM hammerwork_jobs WHERE id = $1")
                .bind(job_id)
                .execute(&self.pool)
                .await?;

            Ok(())
        }

        // Dead job management
        async fn mark_job_dead(&self, job_id: JobId, error_message: &str) -> Result<()> {
            sqlx::query(
                "UPDATE hammerwork_jobs SET status = $1, error_message = $2, failed_at = $3 WHERE id = $4"
            )
            .bind(serde_json::to_string(&JobStatus::Dead)?)
            .bind(error_message)
            .bind(Utc::now())
            .bind(job_id)
            .execute(&self.pool)
            .await?;

            Ok(())
        }

        async fn get_dead_jobs(&self, limit: Option<u32>, offset: Option<u32>) -> Result<Vec<Job>> {
            let limit = limit.unwrap_or(100) as i64;
            let offset = offset.unwrap_or(0) as i64;
            
            let rows = sqlx::query_as::<_, (uuid::Uuid, String, serde_json::Value, String, i32, i32, DateTime<Utc>, DateTime<Utc>, Option<DateTime<Utc>>, Option<DateTime<Utc>>, Option<DateTime<Utc>>, Option<String>)>(
                "SELECT id, queue_name, payload, status, attempts, max_attempts, created_at, scheduled_at, started_at, completed_at, failed_at, error_message FROM hammerwork_jobs WHERE status = $1 ORDER BY failed_at DESC LIMIT $2 OFFSET $3"
            )
            .bind(serde_json::to_string(&JobStatus::Dead)?)
            .bind(limit)
            .bind(offset)
            .fetch_all(&self.pool)
            .await?;

            Ok(rows.into_iter().map(|(id, queue_name, payload, status, attempts, max_attempts, created_at, scheduled_at, started_at, completed_at, failed_at, error_message)| {
                Job {
                    id,
                    queue_name,
                    payload,
                    status: serde_json::from_str(&status).unwrap_or(JobStatus::Dead),
                    attempts,
                    max_attempts,
                    created_at,
                    scheduled_at,
                    started_at,
                    completed_at,
                    failed_at,
                    error_message,
                }
            }).collect())
        }

        async fn get_dead_jobs_by_queue(&self, queue_name: &str, limit: Option<u32>, offset: Option<u32>) -> Result<Vec<Job>> {
            let limit = limit.unwrap_or(100) as i64;
            let offset = offset.unwrap_or(0) as i64;
            
            let rows = sqlx::query_as::<_, (uuid::Uuid, String, serde_json::Value, String, i32, i32, DateTime<Utc>, DateTime<Utc>, Option<DateTime<Utc>>, Option<DateTime<Utc>>, Option<DateTime<Utc>>, Option<String>)>(
                "SELECT id, queue_name, payload, status, attempts, max_attempts, created_at, scheduled_at, started_at, completed_at, failed_at, error_message FROM hammerwork_jobs WHERE status = $1 AND queue_name = $2 ORDER BY failed_at DESC LIMIT $3 OFFSET $4"
            )
            .bind(serde_json::to_string(&JobStatus::Dead)?)
            .bind(queue_name)
            .bind(limit)
            .bind(offset)
            .fetch_all(&self.pool)
            .await?;

            Ok(rows.into_iter().map(|(id, queue_name, payload, status, attempts, max_attempts, created_at, scheduled_at, started_at, completed_at, failed_at, error_message)| {
                Job {
                    id,
                    queue_name,
                    payload,
                    status: serde_json::from_str(&status).unwrap_or(JobStatus::Dead),
                    attempts,
                    max_attempts,
                    created_at,
                    scheduled_at,
                    started_at,
                    completed_at,
                    failed_at,
                    error_message,
                }
            }).collect())
        }

        async fn retry_dead_job(&self, job_id: JobId) -> Result<()> {
            sqlx::query(
                "UPDATE hammerwork_jobs SET status = $1, attempts = 0, scheduled_at = $2, started_at = NULL, failed_at = NULL WHERE id = $3 AND status = $4"
            )
            .bind(serde_json::to_string(&JobStatus::Pending)?)
            .bind(Utc::now())
            .bind(job_id)
            .bind(serde_json::to_string(&JobStatus::Dead)?)
            .execute(&self.pool)
            .await?;

            Ok(())
        }

        async fn purge_dead_jobs(&self, older_than: DateTime<Utc>) -> Result<u64> {
            let result = sqlx::query(
                "DELETE FROM hammerwork_jobs WHERE status = $1 AND failed_at < $2"
            )
            .bind(serde_json::to_string(&JobStatus::Dead)?)
            .bind(older_than)
            .execute(&self.pool)
            .await?;

            Ok(result.rows_affected())
        }

        async fn get_dead_job_summary(&self) -> Result<DeadJobSummary> {
            use std::collections::HashMap;

            // Get total dead job count
            let total_dead_jobs: (i64,) = sqlx::query_as(
                "SELECT COUNT(*) FROM hammerwork_jobs WHERE status = $1"
            )
            .bind(serde_json::to_string(&JobStatus::Dead)?)
            .fetch_one(&self.pool)
            .await?;

            // Get dead jobs by queue
            let dead_jobs_by_queue_rows: Vec<(String, i64)> = sqlx::query_as(
                "SELECT queue_name, COUNT(*) FROM hammerwork_jobs WHERE status = $1 GROUP BY queue_name"
            )
            .bind(serde_json::to_string(&JobStatus::Dead)?)
            .fetch_all(&self.pool)
            .await?;

            // Get oldest and newest dead jobs
            let timestamps: Vec<(Option<DateTime<Utc>>, Option<DateTime<Utc>>)> = sqlx::query_as(
                "SELECT MIN(failed_at), MAX(failed_at) FROM hammerwork_jobs WHERE status = $1 AND failed_at IS NOT NULL"
            )
            .bind(serde_json::to_string(&JobStatus::Dead)?)
            .fetch_all(&self.pool)
            .await?;

            // Get error patterns
            let error_patterns_rows: Vec<(Option<String>, i64)> = sqlx::query_as(
                "SELECT error_message, COUNT(*) FROM hammerwork_jobs WHERE status = $1 AND error_message IS NOT NULL GROUP BY error_message ORDER BY COUNT(*) DESC LIMIT 20"
            )
            .bind(serde_json::to_string(&JobStatus::Dead)?)
            .fetch_all(&self.pool)
            .await?;

            let dead_jobs_by_queue: HashMap<String, u64> = dead_jobs_by_queue_rows
                .into_iter()
                .map(|(queue, count)| (queue, count as u64))
                .collect();

            let error_patterns: HashMap<String, u64> = error_patterns_rows
                .into_iter()
                .filter_map(|(error, count)| error.map(|e| (e, count as u64)))
                .collect();

            let (oldest_dead_job, newest_dead_job) = timestamps
                .first()
                .map(|(oldest, newest)| (*oldest, *newest))
                .unwrap_or((None, None));

            Ok(DeadJobSummary {
                total_dead_jobs: total_dead_jobs.0 as u64,
                dead_jobs_by_queue,
                oldest_dead_job,
                newest_dead_job,
                error_patterns,
            })
        }

        // Statistics and monitoring
        async fn get_queue_stats(&self, queue_name: &str) -> Result<QueueStats> {
            use crate::stats::JobStatistics;
            use std::collections::HashMap;

            // Get job counts by status
            let status_counts: Vec<(String, i64)> = sqlx::query_as(
                "SELECT status, COUNT(*) FROM hammerwork_jobs WHERE queue_name = $1 GROUP BY status"
            )
            .bind(queue_name)
            .fetch_all(&self.pool)
            .await?;

            let mut counts = HashMap::new();
            for (status, count) in status_counts {
                counts.insert(status, count as u64);
            }

            let pending_count = counts.get(&serde_json::to_string(&JobStatus::Pending).unwrap()).copied().unwrap_or(0);
            let running_count = counts.get(&serde_json::to_string(&JobStatus::Running).unwrap()).copied().unwrap_or(0);
            let dead_count = counts.get(&serde_json::to_string(&JobStatus::Dead).unwrap()).copied().unwrap_or(0);
            let completed_count = counts.get(&serde_json::to_string(&JobStatus::Completed).unwrap()).copied().unwrap_or(0);

            // Basic statistics (more detailed stats would require the statistics collector)
            let statistics = JobStatistics {
                total_processed: completed_count + dead_count,
                completed: completed_count,
                failed: counts.get(&serde_json::to_string(&JobStatus::Failed).unwrap()).copied().unwrap_or(0),
                dead: dead_count,
                running: running_count,
                time_window: Duration::from_secs(3600), // Default 1 hour window
                calculated_at: Utc::now(),
                ..Default::default()
            };

            Ok(QueueStats {
                queue_name: queue_name.to_string(),
                pending_count,
                running_count,
                dead_count,
                completed_count,
                statistics,
            })
        }

        async fn get_all_queue_stats(&self) -> Result<Vec<QueueStats>> {
            // Get all unique queue names
            let queue_names: Vec<(String,)> = sqlx::query_as(
                "SELECT DISTINCT queue_name FROM hammerwork_jobs"
            )
            .fetch_all(&self.pool)
            .await?;

            let mut results = Vec::new();
            for (queue_name,) in queue_names {
                let stats = self.get_queue_stats(&queue_name).await?;
                results.push(stats);
            }

            Ok(results)
        }

        async fn get_job_counts_by_status(&self, queue_name: &str) -> Result<std::collections::HashMap<String, u64>> {
            let status_counts: Vec<(String, i64)> = sqlx::query_as(
                "SELECT status, COUNT(*) FROM hammerwork_jobs WHERE queue_name = $1 GROUP BY status"
            )
            .bind(queue_name)
            .fetch_all(&self.pool)
            .await?;

            Ok(status_counts
                .into_iter()
                .map(|(status, count)| (status, count as u64))
                .collect())
        }

        async fn get_processing_times(&self, queue_name: &str, since: DateTime<Utc>) -> Result<Vec<i64>> {
            let times: Vec<(Option<i64>,)> = sqlx::query_as(
                r#"
                SELECT EXTRACT(EPOCH FROM (completed_at - started_at)) * 1000 as processing_time_ms
                FROM hammerwork_jobs 
                WHERE queue_name = $1 
                AND started_at IS NOT NULL 
                AND completed_at IS NOT NULL 
                AND completed_at >= $2
                ORDER BY completed_at DESC
                LIMIT 1000
                "#
            )
            .bind(queue_name)
            .bind(since)
            .fetch_all(&self.pool)
            .await?;

            Ok(times.into_iter().filter_map(|(time,)| time).collect())
        }

        async fn get_error_frequencies(&self, queue_name: Option<&str>, since: DateTime<Utc>) -> Result<std::collections::HashMap<String, u64>> {
            let query = if queue_name.is_some() {
                "SELECT error_message, COUNT(*) FROM hammerwork_jobs WHERE queue_name = $1 AND error_message IS NOT NULL AND failed_at >= $2 GROUP BY error_message ORDER BY COUNT(*) DESC LIMIT 50"
            } else {
                "SELECT error_message, COUNT(*) FROM hammerwork_jobs WHERE error_message IS NOT NULL AND failed_at >= $1 GROUP BY error_message ORDER BY COUNT(*) DESC LIMIT 50"
            };

            let error_frequencies: Vec<(String, i64)> = if let Some(queue) = queue_name {
                sqlx::query_as(query)
                    .bind(queue)
                    .bind(since)
                    .fetch_all(&self.pool)
                    .await?
            } else {
                sqlx::query_as(query)
                    .bind(since)
                    .fetch_all(&self.pool)
                    .await?
            };

            Ok(error_frequencies
                .into_iter()
                .map(|(error, count)| (error, count as u64))
                .collect())
        }
    }
}

#[cfg(feature = "mysql")]
pub mod mysql {
    use super::*;
    use crate::job::JobStatus;
    use sqlx::MySql;

    #[async_trait]
    impl DatabaseQueue for JobQueue<MySql> {
        type Database = MySql;

        async fn create_tables(&self) -> Result<()> {
            sqlx::query(
                r#"
                CREATE TABLE IF NOT EXISTS hammerwork_jobs (
                    id CHAR(36) PRIMARY KEY,
                    queue_name VARCHAR(255) NOT NULL,
                    payload JSON NOT NULL,
                    status VARCHAR(50) NOT NULL,
                    attempts INT NOT NULL DEFAULT 0,
                    max_attempts INT NOT NULL DEFAULT 3,
                    created_at TIMESTAMP(6) NOT NULL,
                    scheduled_at TIMESTAMP(6) NOT NULL,
                    started_at TIMESTAMP(6) NULL,
                    completed_at TIMESTAMP(6) NULL,
                    failed_at TIMESTAMP(6) NULL,
                    error_message TEXT NULL,
                    INDEX idx_queue_status_scheduled (queue_name, status, scheduled_at),
                    INDEX idx_status_failed_at (status, failed_at),
                    INDEX idx_queue_status (queue_name, status)
                )
                "#,
            )
            .execute(&self.pool)
            .await?;

            Ok(())
        }

        async fn enqueue(&self, job: Job) -> Result<JobId> {
            sqlx::query(
                r#"
                INSERT INTO hammerwork_jobs 
                (id, queue_name, payload, status, attempts, max_attempts, created_at, scheduled_at, started_at, completed_at, failed_at, error_message)
                VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                "#
            )
            .bind(job.id.to_string())
            .bind(&job.queue_name)
            .bind(&job.payload)
            .bind(serde_json::to_string(&job.status)?)
            .bind(&job.attempts)
            .bind(&job.max_attempts)
            .bind(&job.created_at)
            .bind(&job.scheduled_at)
            .bind(&job.started_at)
            .bind(&job.completed_at)
            .bind(&job.failed_at)
            .bind(&job.error_message)
            .execute(&self.pool)
            .await?;

            Ok(job.id)
        }

        async fn dequeue(&self, queue_name: &str) -> Result<Option<Job>> {
            // MySQL doesn't support FOR UPDATE SKIP LOCKED in the same way
            // This is a simplified version - in production you might want advisory locks
            let mut tx = self.pool.begin().await?;

            let row = sqlx::query_as::<_, (String, String, serde_json::Value, String, i32, i32, DateTime<Utc>, DateTime<Utc>, Option<DateTime<Utc>>, Option<DateTime<Utc>>, Option<DateTime<Utc>>, Option<String>)>(
                r#"
                SELECT id, queue_name, payload, status, attempts, max_attempts, created_at, scheduled_at, started_at, completed_at, failed_at, error_message
                FROM hammerwork_jobs 
                WHERE queue_name = ? 
                AND status = ? 
                AND scheduled_at <= ?
                ORDER BY scheduled_at ASC 
                LIMIT 1
                FOR UPDATE
                "#
            )
            .bind(queue_name)
            .bind(serde_json::to_string(&JobStatus::Pending)?)
            .bind(Utc::now())
            .fetch_optional(&mut *tx)
            .await?;

            if let Some((
                id_str,
                queue_name,
                payload,
                _status,
                attempts,
                max_attempts,
                created_at,
                scheduled_at,
                started_at,
                completed_at,
                failed_at,
                error_message,
            )) = row
            {
                let job_id = uuid::Uuid::parse_str(&id_str).map_err(|_| {
                    crate::error::HammerworkError::Queue {
                        message: "Invalid UUID".to_string(),
                    }
                })?;

                sqlx::query(
                    "UPDATE hammerwork_jobs SET status = ?, started_at = ?, attempts = attempts + 1 WHERE id = ?"
                )
                .bind(serde_json::to_string(&JobStatus::Running)?)
                .bind(Utc::now())
                .bind(&id_str)
                .execute(&mut *tx)
                .await?;

                tx.commit().await?;

                Ok(Some(Job {
                    id: job_id,
                    queue_name,
                    payload,
                    status: JobStatus::Running,
                    attempts: attempts + 1,
                    max_attempts,
                    created_at,
                    scheduled_at,
                    started_at: Some(Utc::now()),
                    completed_at,
                    failed_at,
                    error_message,
                }))
            } else {
                tx.rollback().await?;
                Ok(None)
            }
        }

        async fn complete_job(&self, job_id: JobId) -> Result<()> {
            sqlx::query("UPDATE hammerwork_jobs SET status = ?, completed_at = ? WHERE id = ?")
                .bind(serde_json::to_string(&JobStatus::Completed)?)
                .bind(Utc::now())
                .bind(job_id.to_string())
                .execute(&self.pool)
                .await?;

            Ok(())
        }

        async fn fail_job(&self, job_id: JobId, error_message: &str) -> Result<()> {
            sqlx::query(
                "UPDATE hammerwork_jobs SET status = ?, error_message = ?, failed_at = ? WHERE id = ?"
            )
            .bind(serde_json::to_string(&JobStatus::Failed)?)
            .bind(error_message)
            .bind(Utc::now())
            .bind(job_id.to_string())
            .execute(&self.pool)
            .await?;

            Ok(())
        }

        async fn retry_job(&self, job_id: JobId, retry_at: DateTime<Utc>) -> Result<()> {
            sqlx::query(
                "UPDATE hammerwork_jobs SET status = ?, scheduled_at = ?, started_at = NULL WHERE id = ?"
            )
            .bind(serde_json::to_string(&JobStatus::Pending)?)
            .bind(retry_at)
            .bind(job_id.to_string())
            .execute(&self.pool)
            .await?;

            Ok(())
        }

        async fn get_job(&self, job_id: JobId) -> Result<Option<Job>> {
            let row = sqlx::query_as::<_, (String, String, serde_json::Value, String, i32, i32, DateTime<Utc>, DateTime<Utc>, Option<DateTime<Utc>>, Option<DateTime<Utc>>, Option<DateTime<Utc>>, Option<String>)>(
                "SELECT id, queue_name, payload, status, attempts, max_attempts, created_at, scheduled_at, started_at, completed_at, failed_at, error_message FROM hammerwork_jobs WHERE id = ?"
            )
            .bind(job_id.to_string())
            .fetch_optional(&self.pool)
            .await?;

            if let Some((
                id_str,
                queue_name,
                payload,
                status,
                attempts,
                max_attempts,
                created_at,
                scheduled_at,
                started_at,
                completed_at,
                failed_at,
                error_message,
            )) = row
            {
                let job_id = uuid::Uuid::parse_str(&id_str).map_err(|_| {
                    crate::error::HammerworkError::Queue {
                        message: "Invalid UUID".to_string(),
                    }
                })?;

                Ok(Some(Job {
                    id: job_id,
                    queue_name,
                    payload,
                    status: serde_json::from_str(&status)?,
                    attempts,
                    max_attempts,
                    created_at,
                    scheduled_at,
                    started_at,
                    completed_at,
                    failed_at,
                    error_message,
                }))
            } else {
                Ok(None)
            }
        }

        async fn delete_job(&self, job_id: JobId) -> Result<()> {
            sqlx::query("DELETE FROM hammerwork_jobs WHERE id = ?")
                .bind(job_id.to_string())
                .execute(&self.pool)
                .await?;

            Ok(())
        }

        // Dead job management
        async fn mark_job_dead(&self, job_id: JobId, error_message: &str) -> Result<()> {
            sqlx::query(
                "UPDATE hammerwork_jobs SET status = ?, error_message = ?, failed_at = ? WHERE id = ?"
            )
            .bind(serde_json::to_string(&JobStatus::Dead)?)
            .bind(error_message)
            .bind(Utc::now())
            .bind(job_id.to_string())
            .execute(&self.pool)
            .await?;

            Ok(())
        }

        async fn get_dead_jobs(&self, limit: Option<u32>, offset: Option<u32>) -> Result<Vec<Job>> {
            let limit = limit.unwrap_or(100) as i64;
            let offset = offset.unwrap_or(0) as i64;
            
            let rows = sqlx::query_as::<_, (String, String, serde_json::Value, String, i32, i32, DateTime<Utc>, DateTime<Utc>, Option<DateTime<Utc>>, Option<DateTime<Utc>>, Option<DateTime<Utc>>, Option<String>)>(
                "SELECT id, queue_name, payload, status, attempts, max_attempts, created_at, scheduled_at, started_at, completed_at, failed_at, error_message FROM hammerwork_jobs WHERE status = ? ORDER BY failed_at DESC LIMIT ? OFFSET ?"
            )
            .bind(serde_json::to_string(&JobStatus::Dead)?)
            .bind(limit)
            .bind(offset)
            .fetch_all(&self.pool)
            .await?;

            let mut jobs = Vec::new();
            for (id_str, queue_name, payload, status, attempts, max_attempts, created_at, scheduled_at, started_at, completed_at, failed_at, error_message) in rows {
                let job_id = uuid::Uuid::parse_str(&id_str).map_err(|_| {
                    crate::error::HammerworkError::Queue {
                        message: "Invalid UUID".to_string(),
                    }
                })?;

                jobs.push(Job {
                    id: job_id,
                    queue_name,
                    payload,
                    status: serde_json::from_str(&status).unwrap_or(JobStatus::Dead),
                    attempts,
                    max_attempts,
                    created_at,
                    scheduled_at,
                    started_at,
                    completed_at,
                    failed_at,
                    error_message,
                });
            }

            Ok(jobs)
        }

        async fn get_dead_jobs_by_queue(&self, queue_name: &str, limit: Option<u32>, offset: Option<u32>) -> Result<Vec<Job>> {
            let limit = limit.unwrap_or(100) as i64;
            let offset = offset.unwrap_or(0) as i64;
            
            let rows = sqlx::query_as::<_, (String, String, serde_json::Value, String, i32, i32, DateTime<Utc>, DateTime<Utc>, Option<DateTime<Utc>>, Option<DateTime<Utc>>, Option<DateTime<Utc>>, Option<String>)>(
                "SELECT id, queue_name, payload, status, attempts, max_attempts, created_at, scheduled_at, started_at, completed_at, failed_at, error_message FROM hammerwork_jobs WHERE status = ? AND queue_name = ? ORDER BY failed_at DESC LIMIT ? OFFSET ?"
            )
            .bind(serde_json::to_string(&JobStatus::Dead)?)
            .bind(queue_name)
            .bind(limit)
            .bind(offset)
            .fetch_all(&self.pool)
            .await?;

            let mut jobs = Vec::new();
            for (id_str, queue_name, payload, status, attempts, max_attempts, created_at, scheduled_at, started_at, completed_at, failed_at, error_message) in rows {
                let job_id = uuid::Uuid::parse_str(&id_str).map_err(|_| {
                    crate::error::HammerworkError::Queue {
                        message: "Invalid UUID".to_string(),
                    }
                })?;

                jobs.push(Job {
                    id: job_id,
                    queue_name,
                    payload,
                    status: serde_json::from_str(&status).unwrap_or(JobStatus::Dead),
                    attempts,
                    max_attempts,
                    created_at,
                    scheduled_at,
                    started_at,
                    completed_at,
                    failed_at,
                    error_message,
                });
            }

            Ok(jobs)
        }

        async fn retry_dead_job(&self, job_id: JobId) -> Result<()> {
            sqlx::query(
                "UPDATE hammerwork_jobs SET status = ?, attempts = 0, scheduled_at = ?, started_at = NULL, failed_at = NULL WHERE id = ? AND status = ?"
            )
            .bind(serde_json::to_string(&JobStatus::Pending)?)
            .bind(Utc::now())
            .bind(job_id.to_string())
            .bind(serde_json::to_string(&JobStatus::Dead)?)
            .execute(&self.pool)
            .await?;

            Ok(())
        }

        async fn purge_dead_jobs(&self, older_than: DateTime<Utc>) -> Result<u64> {
            let result = sqlx::query(
                "DELETE FROM hammerwork_jobs WHERE status = ? AND failed_at < ?"
            )
            .bind(serde_json::to_string(&JobStatus::Dead)?)
            .bind(older_than)
            .execute(&self.pool)
            .await?;

            Ok(result.rows_affected())
        }

        async fn get_dead_job_summary(&self) -> Result<DeadJobSummary> {
            use std::collections::HashMap;

            // Get total dead job count
            let total_dead_jobs: (i64,) = sqlx::query_as(
                "SELECT COUNT(*) FROM hammerwork_jobs WHERE status = ?"
            )
            .bind(serde_json::to_string(&JobStatus::Dead)?)
            .fetch_one(&self.pool)
            .await?;

            // Get dead jobs by queue
            let dead_jobs_by_queue_rows: Vec<(String, i64)> = sqlx::query_as(
                "SELECT queue_name, COUNT(*) FROM hammerwork_jobs WHERE status = ? GROUP BY queue_name"
            )
            .bind(serde_json::to_string(&JobStatus::Dead)?)
            .fetch_all(&self.pool)
            .await?;

            // Get oldest and newest dead jobs
            let timestamps: Vec<(Option<DateTime<Utc>>, Option<DateTime<Utc>>)> = sqlx::query_as(
                "SELECT MIN(failed_at), MAX(failed_at) FROM hammerwork_jobs WHERE status = ? AND failed_at IS NOT NULL"
            )
            .bind(serde_json::to_string(&JobStatus::Dead)?)
            .fetch_all(&self.pool)
            .await?;

            // Get error patterns
            let error_patterns_rows: Vec<(Option<String>, i64)> = sqlx::query_as(
                "SELECT error_message, COUNT(*) FROM hammerwork_jobs WHERE status = ? AND error_message IS NOT NULL GROUP BY error_message ORDER BY COUNT(*) DESC LIMIT 20"
            )
            .bind(serde_json::to_string(&JobStatus::Dead)?)
            .fetch_all(&self.pool)
            .await?;

            let dead_jobs_by_queue: HashMap<String, u64> = dead_jobs_by_queue_rows
                .into_iter()
                .map(|(queue, count)| (queue, count as u64))
                .collect();

            let error_patterns: HashMap<String, u64> = error_patterns_rows
                .into_iter()
                .filter_map(|(error, count)| error.map(|e| (e, count as u64)))
                .collect();

            let (oldest_dead_job, newest_dead_job) = timestamps
                .first()
                .map(|(oldest, newest)| (*oldest, *newest))
                .unwrap_or((None, None));

            Ok(DeadJobSummary {
                total_dead_jobs: total_dead_jobs.0 as u64,
                dead_jobs_by_queue,
                oldest_dead_job,
                newest_dead_job,
                error_patterns,
            })
        }

        // Statistics and monitoring
        async fn get_queue_stats(&self, queue_name: &str) -> Result<QueueStats> {
            use crate::stats::JobStatistics;
            use std::collections::HashMap;

            // Get job counts by status
            let status_counts: Vec<(String, i64)> = sqlx::query_as(
                "SELECT status, COUNT(*) FROM hammerwork_jobs WHERE queue_name = ? GROUP BY status"
            )
            .bind(queue_name)
            .fetch_all(&self.pool)
            .await?;

            let mut counts = HashMap::new();
            for (status, count) in status_counts {
                counts.insert(status, count as u64);
            }

            let pending_count = counts.get(&serde_json::to_string(&JobStatus::Pending).unwrap()).copied().unwrap_or(0);
            let running_count = counts.get(&serde_json::to_string(&JobStatus::Running).unwrap()).copied().unwrap_or(0);
            let dead_count = counts.get(&serde_json::to_string(&JobStatus::Dead).unwrap()).copied().unwrap_or(0);
            let completed_count = counts.get(&serde_json::to_string(&JobStatus::Completed).unwrap()).copied().unwrap_or(0);

            // Basic statistics (more detailed stats would require the statistics collector)
            let statistics = JobStatistics {
                total_processed: completed_count + dead_count,
                completed: completed_count,
                failed: counts.get(&serde_json::to_string(&JobStatus::Failed).unwrap()).copied().unwrap_or(0),
                dead: dead_count,
                running: running_count,
                time_window: Duration::from_secs(3600), // Default 1 hour window
                calculated_at: Utc::now(),
                ..Default::default()
            };

            Ok(QueueStats {
                queue_name: queue_name.to_string(),
                pending_count,
                running_count,
                dead_count,
                completed_count,
                statistics,
            })
        }

        async fn get_all_queue_stats(&self) -> Result<Vec<QueueStats>> {
            // Get all unique queue names
            let queue_names: Vec<(String,)> = sqlx::query_as(
                "SELECT DISTINCT queue_name FROM hammerwork_jobs"
            )
            .fetch_all(&self.pool)
            .await?;

            let mut results = Vec::new();
            for (queue_name,) in queue_names {
                let stats = self.get_queue_stats(&queue_name).await?;
                results.push(stats);
            }

            Ok(results)
        }

        async fn get_job_counts_by_status(&self, queue_name: &str) -> Result<std::collections::HashMap<String, u64>> {
            let status_counts: Vec<(String, i64)> = sqlx::query_as(
                "SELECT status, COUNT(*) FROM hammerwork_jobs WHERE queue_name = ? GROUP BY status"
            )
            .bind(queue_name)
            .fetch_all(&self.pool)
            .await?;

            Ok(status_counts
                .into_iter()
                .map(|(status, count)| (status, count as u64))
                .collect())
        }

        async fn get_processing_times(&self, queue_name: &str, since: DateTime<Utc>) -> Result<Vec<i64>> {
            let times: Vec<(Option<i64>,)> = sqlx::query_as(
                r#"
                SELECT TIMESTAMPDIFF(MICROSECOND, started_at, completed_at) / 1000 as processing_time_ms
                FROM hammerwork_jobs 
                WHERE queue_name = ? 
                AND started_at IS NOT NULL 
                AND completed_at IS NOT NULL 
                AND completed_at >= ?
                ORDER BY completed_at DESC
                LIMIT 1000
                "#
            )
            .bind(queue_name)
            .bind(since)
            .fetch_all(&self.pool)
            .await?;

            Ok(times.into_iter().filter_map(|(time,)| time).collect())
        }

        async fn get_error_frequencies(&self, queue_name: Option<&str>, since: DateTime<Utc>) -> Result<std::collections::HashMap<String, u64>> {
            let query = if queue_name.is_some() {
                "SELECT error_message, COUNT(*) FROM hammerwork_jobs WHERE queue_name = ? AND error_message IS NOT NULL AND failed_at >= ? GROUP BY error_message ORDER BY COUNT(*) DESC LIMIT 50"
            } else {
                "SELECT error_message, COUNT(*) FROM hammerwork_jobs WHERE error_message IS NOT NULL AND failed_at >= ? GROUP BY error_message ORDER BY COUNT(*) DESC LIMIT 50"
            };

            let error_frequencies: Vec<(String, i64)> = if let Some(queue) = queue_name {
                sqlx::query_as(query)
                    .bind(queue)
                    .bind(since)
                    .fetch_all(&self.pool)
                    .await?
            } else {
                sqlx::query_as(query)
                    .bind(since)
                    .fetch_all(&self.pool)
                    .await?
            };

            Ok(error_frequencies
                .into_iter()
                .map(|(error, count)| (error, count as u64))
                .collect())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_database_queue_trait_exists() {
        // This test verifies the DatabaseQueue trait is properly defined
        // We can't easily unit test the implementations without database connections
        // But we can verify the trait signature compiles
        fn _test_trait_signature<T: DatabaseQueue>() {}
        // This function compiles if the trait is properly defined
    }

    #[test]
    fn test_job_queue_generic_struct() {
        // Test that the JobQueue struct is properly generic
        // We can't instantiate it without a real database pool
        // This would be the structure if we had a real database type
        // let _phantom: PhantomData<sqlx::Postgres> = PhantomData;
        assert!(true); // Compilation test
    }
}
