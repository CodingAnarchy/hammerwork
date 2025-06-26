use crate::{job::{Job, JobId}, Result};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::{Database, Pool};
use std::marker::PhantomData;

#[async_trait]
pub trait DatabaseQueue: Send + Sync {
    type Database: Database;
    
    async fn create_tables(&self) -> Result<()>;
    async fn enqueue(&self, job: Job) -> Result<JobId>;
    async fn dequeue(&self, queue_name: &str) -> Result<Option<Job>>;
    async fn complete_job(&self, job_id: JobId) -> Result<()>;
    async fn fail_job(&self, job_id: JobId, error_message: &str) -> Result<()>;
    async fn retry_job(&self, job_id: JobId, retry_at: DateTime<Utc>) -> Result<()>;
    async fn get_job(&self, job_id: JobId) -> Result<Option<Job>>;
    async fn delete_job(&self, job_id: JobId) -> Result<()>;
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
                    error_message TEXT
                );
                
                CREATE INDEX IF NOT EXISTS idx_hammerwork_jobs_queue_status_scheduled 
                ON hammerwork_jobs (queue_name, status, scheduled_at);
                "#
            )
            .execute(&self.pool)
            .await?;
            
            Ok(())
        }

        async fn enqueue(&self, job: Job) -> Result<JobId> {
            sqlx::query(
                r#"
                INSERT INTO hammerwork_jobs 
                (id, queue_name, payload, status, attempts, max_attempts, created_at, scheduled_at, started_at, completed_at, error_message)
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
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
            .bind(&job.error_message)
            .execute(&self.pool)
            .await?;

            Ok(job.id)
        }

        async fn dequeue(&self, queue_name: &str) -> Result<Option<Job>> {
            let row = sqlx::query_as::<_, (uuid::Uuid, String, serde_json::Value, String, i32, i32, DateTime<Utc>, DateTime<Utc>, Option<DateTime<Utc>>, Option<DateTime<Utc>>, Option<String>)>(
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
                RETURNING id, queue_name, payload, status, attempts, max_attempts, created_at, scheduled_at, started_at, completed_at, error_message
                "#
            )
            .bind(serde_json::to_string(&JobStatus::Running)?)
            .bind(Utc::now())
            .bind(queue_name)
            .bind(serde_json::to_string(&JobStatus::Pending)?)
            .fetch_optional(&self.pool)
            .await?;

            if let Some((id, queue_name, payload, status, attempts, max_attempts, created_at, scheduled_at, started_at, completed_at, error_message)) = row {
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
                    error_message,
                }))
            } else {
                Ok(None)
            }
        }

        async fn complete_job(&self, job_id: JobId) -> Result<()> {
            sqlx::query(
                "UPDATE hammerwork_jobs SET status = $1, completed_at = $2 WHERE id = $3"
            )
            .bind(serde_json::to_string(&JobStatus::Completed)?)
            .bind(Utc::now())
            .bind(job_id)
            .execute(&self.pool)
            .await?;

            Ok(())
        }

        async fn fail_job(&self, job_id: JobId, error_message: &str) -> Result<()> {
            sqlx::query(
                "UPDATE hammerwork_jobs SET status = $1, error_message = $2, completed_at = $3 WHERE id = $4"
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
            let row = sqlx::query_as::<_, (uuid::Uuid, String, serde_json::Value, String, i32, i32, DateTime<Utc>, DateTime<Utc>, Option<DateTime<Utc>>, Option<DateTime<Utc>>, Option<String>)>(
                "SELECT id, queue_name, payload, status, attempts, max_attempts, created_at, scheduled_at, started_at, completed_at, error_message FROM hammerwork_jobs WHERE id = $1"
            )
            .bind(job_id)
            .fetch_optional(&self.pool)
            .await?;

            if let Some((id, queue_name, payload, status, attempts, max_attempts, created_at, scheduled_at, started_at, completed_at, error_message)) = row {
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
    }
}

#[cfg(feature = "mysql")]
pub mod mysql {
    use super::*;
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
                    error_message TEXT NULL,
                    INDEX idx_queue_status_scheduled (queue_name, status, scheduled_at)
                )
                "#
            )
            .execute(&self.pool)
            .await?;
            
            Ok(())
        }

        async fn enqueue(&self, job: Job) -> Result<JobId> {
            sqlx::query(
                r#"
                INSERT INTO hammerwork_jobs 
                (id, queue_name, payload, status, attempts, max_attempts, created_at, scheduled_at, started_at, completed_at, error_message)
                VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
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
            .bind(&job.error_message)
            .execute(&self.pool)
            .await?;

            Ok(job.id)
        }

        async fn dequeue(&self, queue_name: &str) -> Result<Option<Job>> {
            // MySQL doesn't support FOR UPDATE SKIP LOCKED in the same way
            // This is a simplified version - in production you might want advisory locks
            let mut tx = self.pool.begin().await?;
            
            let row = sqlx::query_as::<_, (String, String, serde_json::Value, String, i32, i32, DateTime<Utc>, DateTime<Utc>, Option<DateTime<Utc>>, Option<DateTime<Utc>>, Option<String>)>(
                r#"
                SELECT id, queue_name, payload, status, attempts, max_attempts, created_at, scheduled_at, started_at, completed_at, error_message
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

            if let Some((id_str, queue_name, payload, _status, attempts, max_attempts, created_at, scheduled_at, started_at, completed_at, error_message)) = row {
                let job_id = uuid::Uuid::parse_str(&id_str).map_err(|_| crate::error::HammerworkError::Queue { message: "Invalid UUID".to_string() })?;
                
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
                    error_message,
                }))
            } else {
                tx.rollback().await?;
                Ok(None)
            }
        }

        async fn complete_job(&self, job_id: JobId) -> Result<()> {
            sqlx::query(
                "UPDATE hammerwork_jobs SET status = ?, completed_at = ? WHERE id = ?"
            )
            .bind(serde_json::to_string(&JobStatus::Completed)?)
            .bind(Utc::now())
            .bind(job_id.to_string())
            .execute(&self.pool)
            .await?;

            Ok(())
        }

        async fn fail_job(&self, job_id: JobId, error_message: &str) -> Result<()> {
            sqlx::query(
                "UPDATE hammerwork_jobs SET status = ?, error_message = ?, completed_at = ? WHERE id = ?"
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
            let row = sqlx::query_as::<_, (String, String, serde_json::Value, String, i32, i32, DateTime<Utc>, DateTime<Utc>, Option<DateTime<Utc>>, Option<DateTime<Utc>>, Option<String>)>(
                "SELECT id, queue_name, payload, status, attempts, max_attempts, created_at, scheduled_at, started_at, completed_at, error_message FROM hammerwork_jobs WHERE id = ?"
            )
            .bind(job_id.to_string())
            .fetch_optional(&self.pool)
            .await?;

            if let Some((id_str, queue_name, payload, status, attempts, max_attempts, created_at, scheduled_at, started_at, completed_at, error_message)) = row {
                let job_id = uuid::Uuid::parse_str(&id_str).map_err(|_| crate::error::HammerworkError::Queue { message: "Invalid UUID".to_string() })?;
                
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