//! Job-state mutations for the CLI, routed through the library's `DatabaseQueue`.
//!
//! The CLI used to rewrite `hammerwork_jobs` rows with raw SQL (including lowercase
//! status literals the library never reads). Mutating commands now find the affected
//! job ids with a read-only query and apply each change through the library, so the
//! CLI gets the same transition guards and side effects (dependencies, workflows,
//! batches) as workers and application code.

use anyhow::{Result, anyhow};
use chrono::Utc;
use hammerwork::queue::DatabaseQueue;
use hammerwork::{JobId, JobStatus};
use sqlx::Row;
use tracing::warn;

use super::database::{DatabasePool, JobQueueWrapper};

/// Which jobs a bulk command applies to. Statuses are the capitalized names the
/// library stores (`Pending`, `Failed`, ...); `extra` holds trusted SQL conditions.
#[derive(Debug, Default, Clone)]
pub struct JobSelector {
    pub statuses: Vec<JobStatus>,
    pub queue: Option<String>,
    /// Additional trusted SQL fragments (never user-supplied strings), AND-ed in.
    pub extra: Vec<String>,
}

impl JobSelector {
    /// Build the `SELECT id ...` statement. The queue name is the only bound value.
    fn sql(&self, placeholder: &str) -> String {
        let mut conditions = Vec::new();
        if !self.statuses.is_empty() {
            let list = self
                .statuses
                .iter()
                .map(|s| format!("'{}'", status_name(s)))
                .collect::<Vec<_>>()
                .join(", ");
            conditions.push(format!("status IN ({list})"));
        }
        if self.queue.is_some() {
            conditions.push(format!("queue_name = {placeholder}"));
        }
        conditions.extend(self.extra.iter().cloned());
        let where_clause = if conditions.is_empty() {
            String::new()
        } else {
            format!(" WHERE {}", conditions.join(" AND "))
        };
        format!("SELECT id FROM hammerwork_jobs{where_clause} ORDER BY created_at")
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
    match pool {
        DatabasePool::Postgres(pg) => {
            let sql = selector.sql("$1");
            let mut query = sqlx::query(&sql);
            if let Some(queue) = &selector.queue {
                query = query.bind(queue);
            }
            let rows = query.fetch_all(pg).await?;
            rows.iter()
                .map(|row| row.try_get::<JobId, _>("id").map_err(Into::into))
                .collect()
        }
        DatabasePool::MySQL(my) => {
            let sql = selector.sql("?");
            let mut query = sqlx::query(&sql);
            if let Some(queue) = &selector.queue {
                query = query.bind(queue);
            }
            let rows = query.fetch_all(my).await?;
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
    fn selector_uses_capitalized_statuses_and_binds_queue() {
        let selector = JobSelector {
            statuses: vec![JobStatus::Failed, JobStatus::Dead, JobStatus::TimedOut],
            queue: Some("q'; DROP TABLE hammerwork_jobs; --".to_string()),
            extra: vec!["attempts >= max_attempts".to_string()],
        };
        let sql = selector.sql("$1");
        assert!(sql.contains("status IN ('Failed', 'Dead', 'TimedOut')"));
        assert!(sql.contains("queue_name = $1"));
        assert!(sql.contains("attempts >= max_attempts"));
        assert!(!sql.contains("DROP"));
        assert!(!sql.contains("'failed'") && !sql.contains("'pending'"));
    }

    #[test]
    fn empty_selector_has_no_where_clause() {
        assert!(!JobSelector::default().sql("?").contains("WHERE"));
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
}
