//! Job-state mutations for the CLI, routed through the library's `DatabaseQueue`.
//!
//! The CLI used to rewrite `hammerwork_jobs` rows with raw SQL (including lowercase
//! status literals the library never reads). Mutating commands now find the affected
//! job ids with a read-only query and apply each change through the library, so the
//! CLI gets the same transition guards and side effects (dependencies, workflows,
//! batches) as workers and application code.

use anyhow::{Result, anyhow};
use chrono::{DateTime, Utc};
use hammerwork::queue::DatabaseQueue;
use hammerwork::{JobId, JobStatus};
use sqlx::Row;
use tracing::warn;

use super::database::{DatabasePool, JobQueueWrapper};
use super::sql::{Backend, Bind, SqlParams, bind_mysql, bind_pg};

/// Which jobs a bulk command applies to. Statuses are the capitalized names the
/// library stores (`Pending`, `Failed`, ...). Every filter value, including the status
/// names and timestamps, is bound as a parameter.
#[derive(Debug, Default, Clone)]
pub struct JobSelector {
    pub statuses: Vec<JobStatus>,
    pub queue: Option<String>,
    /// Only jobs whose `failed_at` is later than this.
    pub failed_after: Option<DateTime<Utc>>,
    /// Only jobs whose `created_at` is earlier than this.
    pub created_before: Option<DateTime<Utc>>,
    /// Only jobs whose `started_at` is earlier than this.
    pub started_before: Option<DateTime<Utc>>,
    /// Only jobs with no `started_at`.
    pub never_started: bool,
    /// Only jobs that used all their attempts.
    pub attempts_exhausted: bool,
}

impl JobSelector {
    /// Build the `SELECT id ...` statement and its binds, in placeholder order.
    pub fn sql(&self, backend: Backend) -> (String, Vec<Bind>) {
        let mut params = SqlParams::new(backend);
        let mut conditions = Vec::new();
        if !self.statuses.is_empty() {
            let list = self
                .statuses
                .iter()
                .map(|s| params.text(status_name(s)))
                .collect::<Vec<_>>()
                .join(", ");
            conditions.push(format!("status IN ({list})"));
        }
        if let Some(queue) = &self.queue {
            conditions.push(format!("queue_name = {}", params.text(queue)));
        }
        if let Some(t) = self.failed_after {
            conditions.push(format!("failed_at > {}", params.time(t)));
        }
        if let Some(t) = self.created_before {
            conditions.push(format!("created_at < {}", params.time(t)));
        }
        if let Some(t) = self.started_before {
            conditions.push(format!("started_at < {}", params.time(t)));
        }
        if self.never_started {
            conditions.push("started_at IS NULL".to_string());
        }
        if self.attempts_exhausted {
            conditions.push("attempts >= max_attempts".to_string());
        }
        let where_clause = if conditions.is_empty() {
            String::new()
        } else {
            format!(" WHERE {}", conditions.join(" AND "))
        };
        (
            format!("SELECT id FROM hammerwork_jobs{where_clause} ORDER BY created_at"),
            params.into_binds(),
        )
    }
}

/// The capitalized status name as stored in the database.
pub fn status_name(status: &JobStatus) -> &'static str {
    match status {
        JobStatus::Pending => "Pending",
        JobStatus::Running => "Running",
        JobStatus::Completed => "Completed",
        JobStatus::Failed => "Failed",
        JobStatus::Dead => "Dead",
        JobStatus::TimedOut => "TimedOut",
        JobStatus::Retrying => "Retrying",
        JobStatus::Archived => "Archived",
    }
}

/// Ids of the jobs matching `selector` (read-only).
pub async fn select_job_ids(pool: &DatabasePool, selector: &JobSelector) -> Result<Vec<JobId>> {
    let (sql, binds) = selector.sql(pool.backend());
    match pool {
        DatabasePool::Postgres(pg) => {
            let rows = bind_pg(sqlx::query(&sql), &binds).fetch_all(pg).await?;
            rows.iter()
                .map(|row| row.try_get::<JobId, _>("id").map_err(Into::into))
                .collect()
        }
        DatabasePool::MySQL(my) => {
            let rows = bind_mysql(sqlx::query(&sql), &binds).fetch_all(my).await?;
            rows.iter()
                .map(|row| {
                    let id: String = row.try_get("id")?;
                    JobId::parse_str(&id).map_err(Into::into)
                })
                .collect()
        }
    }
}

/// Outcome of applying an operation to a set of jobs.
#[derive(Debug, Default)]
pub struct BulkResult {
    pub succeeded: u64,
    /// Jobs the library refused (or that vanished), with the reason.
    pub skipped: Vec<(JobId, String)>,
}

/// Re-run one job: `Dead`/`TimedOut` go through `retry_dead_job` (attempts reset);
/// `Failed` goes through `retry_job`. Other statuses are rejected by the library's
/// transition guard (or here, for a missing job).
pub async fn retry_one<Q: DatabaseQueue + ?Sized>(queue: &Q, id: JobId) -> Result<()> {
    let job = queue
        .get_job(id)
        .await?
        .ok_or_else(|| anyhow!("job {id} not found"))?;
    match job.status {
        JobStatus::Dead | JobStatus::TimedOut => queue.retry_dead_job(id).await?,
        _ => queue.retry_job(id, Utc::now()).await?,
    }
    Ok(())
}

/// Delete one job, but only while it is still in one of `allowed` statuses.
pub async fn cancel_one<Q: DatabaseQueue + ?Sized>(
    queue: &Q,
    id: JobId,
    allowed: &[JobStatus],
) -> Result<()> {
    let job = queue
        .get_job(id)
        .await?
        .ok_or_else(|| anyhow!("job {id} not found"))?;
    if !allowed.contains(&job.status) {
        return Err(anyhow!(
            "job {id} is {} and cannot be cancelled",
            status_name(&job.status)
        ));
    }
    queue.delete_job(id).await?;
    Ok(())
}

/// Apply `retry_one` to every id, collecting per-job failures instead of aborting.
pub async fn retry_many(queue: &JobQueueWrapper, ids: &[JobId]) -> BulkResult {
    let mut result = BulkResult::default();
    for &id in ids {
        let outcome = match queue {
            JobQueueWrapper::Postgres(q) => retry_one(q, id).await,
            JobQueueWrapper::MySQL(q) => retry_one(q, id).await,
        };
        record(&mut result, id, outcome);
    }
    result
}

/// Apply `cancel_one` to every id, collecting per-job failures instead of aborting.
pub async fn cancel_many(
    queue: &JobQueueWrapper,
    ids: &[JobId],
    allowed: &[JobStatus],
) -> BulkResult {
    let mut result = BulkResult::default();
    for &id in ids {
        let outcome = match queue {
            JobQueueWrapper::Postgres(q) => cancel_one(q, id, allowed).await,
            JobQueueWrapper::MySQL(q) => cancel_one(q, id, allowed).await,
        };
        record(&mut result, id, outcome);
    }
    result
}

fn record(result: &mut BulkResult, id: JobId, outcome: Result<()>) {
    match outcome {
        Ok(()) => result.succeeded += 1,
        Err(err) => {
            warn!("Skipped job {}: {}", id, err);
            result.skipped.push((id, err.to_string()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selector_binds_statuses_queue_and_timestamps() {
        let cutoff = Utc::now();
        let selector = JobSelector {
            statuses: vec![JobStatus::Failed, JobStatus::Dead, JobStatus::TimedOut],
            queue: Some("q'; DROP TABLE hammerwork_jobs; --".to_string()),
            failed_after: Some(cutoff),
            attempts_exhausted: true,
            ..Default::default()
        };
        let (sql, binds) = selector.sql(Backend::Postgres);
        assert_eq!(
            sql,
            "SELECT id FROM hammerwork_jobs WHERE status IN ($1, $2, $3) AND queue_name = $4 \
             AND failed_at > $5 AND attempts >= max_attempts ORDER BY created_at"
        );
        assert_eq!(
            binds,
            vec![
                Bind::Text("Failed".into()),
                Bind::Text("Dead".into()),
                Bind::Text("TimedOut".into()),
                Bind::Text("q'; DROP TABLE hammerwork_jobs; --".into()),
                Bind::Time(cutoff),
            ]
        );
        assert!(!sql.contains("DROP"));

        let (sql, binds) = selector.sql(Backend::MySql);
        assert!(sql.contains("status IN (?, ?, ?) AND queue_name = ? AND failed_at > ?"));
        assert_eq!(binds.len(), 5);
    }

    #[test]
    fn selector_supports_started_created_and_never_started_filters() {
        let t = Utc::now();
        let selector = JobSelector {
            created_before: Some(t),
            started_before: Some(t),
            never_started: true,
            ..Default::default()
        };
        let (sql, binds) = selector.sql(Backend::MySql);
        assert!(sql.contains("WHERE created_at < ? AND started_at < ? AND started_at IS NULL"));
        assert_eq!(binds, vec![Bind::Time(t), Bind::Time(t)]);
    }

    #[test]
    fn empty_selector_has_no_where_clause() {
        let (sql, binds) = JobSelector::default().sql(Backend::MySql);
        assert!(!sql.contains("WHERE"));
        assert!(binds.is_empty());
    }

    #[test]
    fn status_names_are_capitalized() {
        for status in [
            JobStatus::Pending,
            JobStatus::Running,
            JobStatus::Completed,
            JobStatus::Failed,
            JobStatus::Dead,
            JobStatus::TimedOut,
            JobStatus::Retrying,
            JobStatus::Archived,
        ] {
            let name = status_name(&status);
            assert!(name.chars().next().unwrap().is_uppercase());
        }
    }

    use crate::utils::test_support::*;

    async fn selector_roundtrip(pool: DatabasePool) {
        let hostile = hostile_queue();
        let other = format!("other_{}", uuid::Uuid::new_v4().simple());
        let mut failed = crate::utils::test_support::SeedJob::new(&hostile, "Failed");
        failed.failed_now = true;
        let failed_id = seed(&pool, &failed).await;
        seed(&pool, &SeedJob::new(&hostile, "Pending")).await;
        seed(&pool, &SeedJob::new(&other, "Failed")).await;

        let selector = JobSelector {
            statuses: vec![JobStatus::Failed, JobStatus::Dead],
            queue: Some(hostile.clone()),
            failed_after: Some(Utc::now() - chrono::Duration::hours(1)),
            created_before: Some(Utc::now() + chrono::Duration::hours(1)),
            ..Default::default()
        };
        let ids = select_job_ids(&pool, &selector).await.unwrap();
        assert_eq!(ids.len(), 1);
        assert_eq!(ids[0].to_string(), failed_id);

        let none = JobSelector {
            queue: Some(hostile.clone()),
            started_before: Some(Utc::now()),
            ..Default::default()
        };
        assert!(select_job_ids(&pool, &none).await.unwrap().is_empty());

        assert!(table_exists(&pool).await);
        cleanup(&pool, &[&hostile, &other]).await;
    }

    #[tokio::test]
    #[ignore = "requires DATABASE_URL (PostgreSQL)"]
    async fn test_job_selector_is_injection_safe_postgres() {
        selector_roundtrip(pg_pool().await).await;
    }

    #[tokio::test]
    #[ignore = "requires MYSQL_DATABASE_URL"]
    async fn test_job_selector_is_injection_safe_mysql() {
        selector_roundtrip(mysql_pool().await).await;
    }
}
