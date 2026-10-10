//! Historical job queries backing the dashboard's trend, error-pattern and clear
//! endpoints.
//!
//! [`DatabaseQueue`] has no way to ask "how many jobs completed or failed in each
//! hour", "which errors occurred where" or "delete this queue's completed jobs". The
//! dashboard talks to a concrete [`JobQueue`], so [`JobHistory`] answers those with
//! parameterized SQL against `hammerwork_jobs` for both backends.
//!
//! "Failed" here means jobs *currently* in `Failed`, `Dead` or `TimedOut`, bucketed by
//! when they failed (`failed_at`, falling back to `timed_out_at`). A job that failed and
//! was then retried to success is counted only as completed, because retrying clears its
//! failure timestamps.

use super::archive::ArchiveFilterParams;
use chrono::{DateTime, Duration, NaiveDateTime, TimeZone, Utc};
use hammerwork::archive::{ArchivalReason, ArchivedJob};
use hammerwork::queue::DatabaseQueue;
use hammerwork::{JobQueue, JobStatus, Result};
use serde::Serialize;
use sqlx::Row;
use std::collections::BTreeMap;
use std::future::Future;

use crate::websocket::JobUpdate;

/// Completed/failed activity in one UTC hour.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct HourBucket {
    /// Start of the hour (UTC).
    pub hour: DateTime<Utc>,
    pub completed: u64,
    pub failed: u64,
    /// Mean run time of the jobs completed in the hour, if any recorded one.
    pub avg_processing_time_ms: Option<f64>,
}

/// One distinct error message with how often and where it happened.
#[derive(Debug, Clone, PartialEq)]
pub struct ErrorGroup {
    pub queue_name: String,
    pub message: String,
    pub count: u64,
    pub first_seen: DateTime<Utc>,
    pub last_seen: DateTime<Utc>,
}

/// Which finished jobs [`JobHistory::count_jobs`] / [`JobHistory::delete_jobs`] target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FinishedKind {
    Completed,
    Dead,
}

impl FinishedKind {
    fn status(self) -> &'static str {
        match self {
            FinishedKind::Completed => "Completed",
            FinishedKind::Dead => "Dead",
        }
    }

    /// Column the `older_than` cutoff applies to.
    fn age_column(self) -> &'static str {
        match self {
            FinishedKind::Completed => "completed_at",
            FinishedKind::Dead => "COALESCE(failed_at, timed_out_at, created_at)",
        }
    }
}

/// Maximum number of hourly buckets a single request may span.
pub const MAX_BUCKETS: i64 = 24 * 31;

/// Truncates to the start of the UTC hour.
pub fn hour_floor(t: DateTime<Utc>) -> DateTime<Utc> {
    let secs = t.timestamp() - t.timestamp().rem_euclid(3600);
    Utc.timestamp_opt(secs, 0).single().unwrap_or(t)
}

/// Dense list of hour starts covering `[start, end)`, oldest first.
pub fn hour_range(start: DateTime<Utc>, end: DateTime<Utc>) -> Vec<DateTime<Utc>> {
    let mut hours = Vec::new();
    let mut cursor = hour_floor(start);
    while cursor < end && (hours.len() as i64) < MAX_BUCKETS {
        hours.push(cursor);
        cursor += Duration::hours(1);
    }
    hours
}

/// Merges sparse per-hour counts into a zero-filled series over `hours`.
pub fn fill_buckets(
    hours: &[DateTime<Utc>],
    completed: &BTreeMap<DateTime<Utc>, (u64, Option<f64>)>,
    failed: &BTreeMap<DateTime<Utc>, u64>,
) -> Vec<HourBucket> {
    hours
        .iter()
        .map(|hour| {
            let (completed, avg) = completed.get(hour).copied().unwrap_or((0, None));
            HourBucket {
                hour: *hour,
                completed,
                failed: failed.get(hour).copied().unwrap_or(0),
                avg_processing_time_ms: avg,
            }
        })
        .collect()
}

fn to_u64(n: i64) -> u64 {
    u64::try_from(n).unwrap_or(0)
}

/// SQL-backed history queries. Implemented for PostgreSQL and MySQL job queues.
pub trait JobHistory: DatabaseQueue + Send + Sync {
    /// Zero-filled hourly activity over `[start, end)`, optionally for one queue.
    fn hourly_activity(
        &self,
        queue: Option<&str>,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> impl Future<Output = Result<Vec<HourBucket>>> + Send;

    /// Distinct error messages of failed jobs since `since`, most frequent first.
    fn error_groups(
        &self,
        since: DateTime<Utc>,
        limit: u32,
    ) -> impl Future<Output = Result<Vec<ErrorGroup>>> + Send;

    /// Number of finished jobs of `kind`, optionally per queue and older than a cutoff.
    fn count_jobs(
        &self,
        queue: Option<&str>,
        kind: FinishedKind,
        older_than: Option<DateTime<Utc>>,
    ) -> impl Future<Output = Result<u64>> + Send;

    /// Deletes the jobs [`count_jobs`](Self::count_jobs) counts; returns how many.
    fn delete_jobs(
        &self,
        queue: Option<&str>,
        kind: FinishedKind,
        older_than: Option<DateTime<Utc>>,
    ) -> impl Future<Output = Result<u64>> + Send;

    /// One page of the archived jobs matching `filter`, newest first, and the number of
    /// matching archived jobs. Filtering, counting and paging all happen in the database.
    fn archived_jobs(
        &self,
        filter: &ArchiveFilterParams,
        limit: u32,
        offset: u32,
    ) -> impl Future<Output = Result<(Vec<ArchivedJob>, u64)>> + Send;

    /// The database's current time, the clock the job state timestamps are written with.
    fn database_now(&self) -> impl Future<Output = Result<DateTime<Utc>>> + Send;

    /// Jobs whose state changed after `since`, newest first, at most `limit`: jobs that
    /// were created, started, completed, failed or timed out since then. Each is reported
    /// with the time of its latest such change. Rows that cannot be decoded are skipped.
    fn recent_job_changes(
        &self,
        since: DateTime<Utc>,
        limit: u32,
    ) -> impl Future<Output = Result<Vec<JobUpdate>>> + Send;
}

/// The job timestamps that mark a change of state, newest first in
/// [`JobHistory::recent_job_changes`].
const CHANGE_COLUMNS: [&str; 5] = [
    "created_at",
    "started_at",
    "completed_at",
    "failed_at",
    "timed_out_at",
];

/// `WHERE` clause of [`JobHistory::recent_job_changes`]: any change column after the
/// cutoff. Each comparison can use an index on its column.
fn changed_since_filter(postgres: bool) -> String {
    let placeholder = if postgres { "$1" } else { "?" };
    CHANGE_COLUMNS
        .iter()
        .map(|column| format!("{column} > {placeholder}"))
        .collect::<Vec<_>>()
        .join(" OR ")
}

/// A changed job as sent to dashboard clients.
fn job_change(
    id: String,
    queue_name: String,
    status: &str,
    priority: i32,
    attempts: i32,
    changed_at: DateTime<Utc>,
) -> JobUpdate {
    JobUpdate {
        id,
        queue_name,
        status: status.trim_matches('"').to_string(),
        priority: hammerwork::JobPriority::from_i32(priority)
            .unwrap_or_default()
            .to_string(),
        attempts,
        updated_at: changed_at,
    }
}

/// A value bound to an archive filter placeholder.
#[derive(Debug, Clone, PartialEq)]
enum ArchiveBind {
    Text(String),
    Time(DateTime<Utc>),
    Bool(bool),
}

/// The `WHERE` clause (empty without filters) for `filter`, with `$n`/`?` placeholders, and
/// the values to bind to them in order. Statuses and reasons compare case-insensitively and
/// ignore the JSON quotes older rows may carry.
fn archive_conditions(filter: &ArchiveFilterParams, postgres: bool) -> (String, Vec<ArchiveBind>) {
    let mut clauses = Vec::new();
    let mut binds = Vec::new();
    let mut add = |clause: &str, bind: ArchiveBind| {
        binds.push(bind);
        let placeholder = if postgres {
            format!("${}", binds.len())
        } else {
            "?".to_string()
        };
        clauses.push(clause.replace('?', &placeholder));
    };
    if let Some(queue) = &filter.queue {
        add("queue_name = ?", ArchiveBind::Text(queue.clone()));
    }
    if let Some(reason) = &filter.reason {
        add(
            "LOWER(TRIM(BOTH '\"' FROM archival_reason)) = ?",
            ArchiveBind::Text(reason.to_lowercase()),
        );
    }
    if let Some(after) = filter.archived_after {
        add("archived_at >= ?", ArchiveBind::Time(after));
    }
    if let Some(before) = filter.archived_before {
        add("archived_at <= ?", ArchiveBind::Time(before));
    }
    if let Some(by) = &filter.archived_by {
        add("archived_by = ?", ArchiveBind::Text(by.clone()));
    }
    if let Some(compressed) = filter.compressed {
        add("payload_compressed = ?", ArchiveBind::Bool(compressed));
    }
    if let Some(status) = &filter.original_status {
        add(
            "LOWER(TRIM(BOTH '\"' FROM status)) = ?",
            ArchiveBind::Text(status.to_lowercase()),
        );
    }
    if clauses.is_empty() {
        (String::new(), binds)
    } else {
        (format!("WHERE {}", clauses.join(" AND ")), binds)
    }
}

/// Binds every [`ArchiveBind`] to a query, in order.
macro_rules! bind_archive_filter {
    ($query:expr, $binds:expr) => {{
        let mut query = $query;
        for bind in $binds {
            query = match bind {
                ArchiveBind::Text(value) => query.bind(value.clone()),
                ArchiveBind::Time(value) => query.bind(*value),
                ArchiveBind::Bool(value) => query.bind(*value),
            };
        }
        query
    }};
}

/// Columns of an archived job listing.
const ARCHIVED_JOB_COLUMNS: &str = "id, queue_name, status, created_at, archived_at, \
     archival_reason, original_payload_size, payload_compressed, archived_by";

/// The status stored in the archive (possibly JSON-quoted); unknown values read as `Dead`,
/// as the core library does.
fn archived_status(value: &str) -> JobStatus {
    let value = value.trim_matches('"');
    [
        JobStatus::Pending,
        JobStatus::Running,
        JobStatus::Completed,
        JobStatus::Failed,
        JobStatus::Dead,
        JobStatus::TimedOut,
        JobStatus::Retrying,
        JobStatus::Archived,
    ]
    .into_iter()
    .find(|status| status.as_str() == value)
    .unwrap_or(JobStatus::Dead)
}

/// The rows of a dashboard listing that could be decoded. A row that cannot be decoded
/// is skipped with a warning naming its id, so one corrupt row does not turn the whole
/// page into an error.
pub(crate) fn decodable<T>(rows: impl IntoIterator<Item = (String, Result<T>)>) -> Vec<T> {
    rows.into_iter()
        .filter_map(|(id, decoded)| match decoded {
            Ok(value) => Some(value),
            Err(error) => {
                tracing::warn!(job_id = %id, error = %error, "Skipping an undecodable row");
                None
            }
        })
        .collect()
}

/// An archived job from a row of [`ARCHIVED_JOB_COLUMNS`], with the id already decoded.
fn archived_job<R>(row: &R, id: uuid::Uuid) -> Result<ArchivedJob>
where
    R: Row,
    for<'a> &'a str: sqlx::ColumnIndex<R>,
    for<'a> String: sqlx::Decode<'a, R::Database> + sqlx::Type<R::Database>,
    for<'a> Option<String>: sqlx::Decode<'a, R::Database> + sqlx::Type<R::Database>,
    for<'a> Option<i32>: sqlx::Decode<'a, R::Database> + sqlx::Type<R::Database>,
    for<'a> bool: sqlx::Decode<'a, R::Database> + sqlx::Type<R::Database>,
    for<'a> DateTime<Utc>: sqlx::Decode<'a, R::Database> + sqlx::Type<R::Database>,
{
    Ok(ArchivedJob {
        id,
        queue_name: row.try_get("queue_name")?,
        status: archived_status(&row.try_get::<String, _>("status")?),
        created_at: row.try_get("created_at")?,
        archived_at: row.try_get("archived_at")?,
        archival_reason: ArchivalReason::parse_from_db(
            &row.try_get::<String, _>("archival_reason")?,
        )
        .unwrap_or_default(),
        original_payload_size: row
            .try_get::<Option<i32>, _>("original_payload_size")?
            .and_then(|size| usize::try_from(size).ok()),
        payload_compressed: row.try_get("payload_compressed")?,
        archived_by: row.try_get("archived_by")?,
    })
}

/// Filters shared by count and delete: `$n`/`?` placeholders, queue then cutoff.
fn finished_filter(kind: FinishedKind, queue: bool, cutoff: bool, postgres: bool) -> String {
    let mut n = 0;
    let mut ph = || {
        n += 1;
        if postgres {
            format!("${n}")
        } else {
            "?".to_string()
        }
    };
    let mut sql = format!("WHERE status = '{}'", kind.status());
    if queue {
        sql.push_str(&format!(" AND queue_name = {}", ph()));
    }
    if cutoff {
        sql.push_str(&format!(" AND {} < {}", kind.age_column(), ph()));
    }
    sql
}

impl JobHistory for JobQueue<sqlx::Postgres> {
    async fn hourly_activity(
        &self,
        queue: Option<&str>,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<Vec<HourBucket>> {
        let hours = hour_range(start, end);
        let (Some(first), Some(last)) = (hours.first().copied(), hours.last().copied()) else {
            return Ok(Vec::new());
        };
        let upper = last + Duration::hours(1);
        let queue_clause = if queue.is_some() {
            " AND queue_name = $3"
        } else {
            ""
        };

        let completed_sql = format!(
            "SELECT date_trunc('hour', completed_at AT TIME ZONE 'UTC') AS bucket, \
             COUNT(*)::bigint AS n, \
             AVG(EXTRACT(EPOCH FROM (completed_at - started_at)) * 1000)::float8 AS avg_ms \
             FROM hammerwork_jobs WHERE status = 'Completed' \
             AND completed_at >= $1 AND completed_at < $2{queue_clause} GROUP BY 1"
        );
        let mut q = sqlx::query(&completed_sql).bind(first).bind(upper);
        if let Some(name) = queue {
            q = q.bind(name.to_string());
        }
        let mut completed = BTreeMap::new();
        for row in q.fetch_all(&self.pool).await? {
            let bucket: NaiveDateTime = row.try_get("bucket")?;
            let n: i64 = row.try_get("n")?;
            let avg: Option<f64> = row.try_get("avg_ms")?;
            completed.insert(bucket.and_utc(), (to_u64(n), avg));
        }

        let failed_sql = format!(
            "SELECT date_trunc('hour', COALESCE(failed_at, timed_out_at) AT TIME ZONE 'UTC') AS bucket, \
             COUNT(*)::bigint AS n FROM hammerwork_jobs \
             WHERE status IN ('Failed', 'Dead', 'TimedOut') \
             AND COALESCE(failed_at, timed_out_at) >= $1 AND COALESCE(failed_at, timed_out_at) < $2{queue_clause} \
             GROUP BY 1"
        );
        let mut q = sqlx::query(&failed_sql).bind(first).bind(upper);
        if let Some(name) = queue {
            q = q.bind(name.to_string());
        }
        let mut failed = BTreeMap::new();
        for row in q.fetch_all(&self.pool).await? {
            let bucket: NaiveDateTime = row.try_get("bucket")?;
            let n: i64 = row.try_get("n")?;
            failed.insert(bucket.and_utc(), to_u64(n));
        }

        Ok(fill_buckets(&hours, &completed, &failed))
    }

    async fn error_groups(&self, since: DateTime<Utc>, limit: u32) -> Result<Vec<ErrorGroup>> {
        let rows = sqlx::query(
            "SELECT queue_name, error_message, COUNT(*)::bigint AS n, \
             MIN(COALESCE(failed_at, timed_out_at)) AS first_seen, \
             MAX(COALESCE(failed_at, timed_out_at)) AS last_seen \
             FROM hammerwork_jobs \
             WHERE status IN ('Failed', 'Dead', 'TimedOut') AND error_message IS NOT NULL \
             AND COALESCE(failed_at, timed_out_at) >= $1 \
             GROUP BY queue_name, error_message ORDER BY n DESC LIMIT $2",
        )
        .bind(since)
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await?;
        let mut groups = Vec::with_capacity(rows.len());
        for row in rows {
            groups.push(ErrorGroup {
                queue_name: row.try_get("queue_name")?,
                message: row.try_get("error_message")?,
                count: to_u64(row.try_get("n")?),
                first_seen: row.try_get("first_seen")?,
                last_seen: row.try_get("last_seen")?,
            });
        }
        Ok(groups)
    }

    async fn count_jobs(
        &self,
        queue: Option<&str>,
        kind: FinishedKind,
        older_than: Option<DateTime<Utc>>,
    ) -> Result<u64> {
        let sql = format!(
            "SELECT COUNT(*)::bigint FROM hammerwork_jobs {}",
            finished_filter(kind, queue.is_some(), older_than.is_some(), true)
        );
        let mut q = sqlx::query_scalar::<_, i64>(&sql);
        if let Some(name) = queue {
            q = q.bind(name.to_string());
        }
        if let Some(cutoff) = older_than {
            q = q.bind(cutoff);
        }
        Ok(to_u64(q.fetch_one(&self.pool).await?))
    }

    async fn delete_jobs(
        &self,
        queue: Option<&str>,
        kind: FinishedKind,
        older_than: Option<DateTime<Utc>>,
    ) -> Result<u64> {
        let sql = format!(
            "DELETE FROM hammerwork_jobs {}",
            finished_filter(kind, queue.is_some(), older_than.is_some(), true)
        );
        let mut q = sqlx::query(&sql);
        if let Some(name) = queue {
            q = q.bind(name.to_string());
        }
        if let Some(cutoff) = older_than {
            q = q.bind(cutoff);
        }
        Ok(q.execute(&self.pool).await?.rows_affected())
    }

    async fn archived_jobs(
        &self,
        filter: &ArchiveFilterParams,
        limit: u32,
        offset: u32,
    ) -> Result<(Vec<ArchivedJob>, u64)> {
        let (conditions, binds) = archive_conditions(filter, true);
        let count_sql = format!("SELECT COUNT(*) FROM hammerwork_jobs_archive {conditions}");
        let total = bind_archive_filter!(sqlx::query_scalar::<_, i64>(&count_sql), &binds)
            .fetch_one(&self.pool)
            .await?;
        let (limit_ph, offset_ph) = (
            format!("${}", binds.len() + 1),
            format!("${}", binds.len() + 2),
        );
        let page_sql = format!(
            "SELECT {ARCHIVED_JOB_COLUMNS} FROM hammerwork_jobs_archive {conditions} \
             ORDER BY archived_at DESC, id DESC LIMIT {limit_ph} OFFSET {offset_ph}"
        );
        let rows = bind_archive_filter!(sqlx::query(&page_sql), &binds)
            .bind(i64::from(limit))
            .bind(i64::from(offset))
            .fetch_all(&self.pool)
            .await?;
        let jobs = decodable(rows.iter().map(|row| {
            let id = row.try_get::<uuid::Uuid, _>("id");
            let label = id.as_ref().map(|id| id.to_string()).unwrap_or_default();
            (
                label,
                id.map_err(Into::into).and_then(|id| archived_job(row, id)),
            )
        }));
        Ok((jobs, to_u64(total)))
    }

    async fn database_now(&self) -> Result<DateTime<Utc>> {
        Ok(sqlx::query_scalar("SELECT NOW()")
            .fetch_one(&self.pool)
            .await?)
    }

    async fn recent_job_changes(&self, since: DateTime<Utc>, limit: u32) -> Result<Vec<JobUpdate>> {
        // GREATEST ignores NULLs on PostgreSQL.
        let sql = format!(
            "SELECT id, queue_name, status, priority, attempts, GREATEST({}) AS changed_at \
             FROM hammerwork_jobs WHERE {} ORDER BY changed_at DESC LIMIT $2",
            CHANGE_COLUMNS.join(", "),
            changed_since_filter(true)
        );
        let rows = sqlx::query(&sql)
            .bind(since)
            .bind(i64::from(limit))
            .fetch_all(&self.pool)
            .await?;
        Ok(decodable(rows.iter().map(|row| {
            let id = row.try_get::<uuid::Uuid, _>("id");
            let label = id.as_ref().map(|id| id.to_string()).unwrap_or_default();
            let change = id.map_err(Into::into).and_then(|id| {
                Ok(job_change(
                    id.to_string(),
                    row.try_get("queue_name")?,
                    &row.try_get::<String, _>("status")?,
                    row.try_get("priority")?,
                    row.try_get("attempts")?,
                    row.try_get("changed_at")?,
                ))
            });
            (label, change)
        })))
    }
}

impl JobHistory for JobQueue<sqlx::MySql> {
    async fn hourly_activity(
        &self,
        queue: Option<&str>,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<Vec<HourBucket>> {
        let hours = hour_range(start, end);
        let (Some(first), Some(last)) = (hours.first().copied(), hours.last().copied()) else {
            return Ok(Vec::new());
        };
        let upper = last + Duration::hours(1);
        let queue_clause = if queue.is_some() {
            " AND queue_name = ?"
        } else {
            ""
        };

        let completed_sql = format!(
            "SELECT DATE_FORMAT(completed_at, '%Y-%m-%d %H:00:00') AS bucket, \
             CAST(COUNT(*) AS SIGNED) AS n, \
             CAST(AVG(TIMESTAMPDIFF(MICROSECOND, started_at, completed_at)) / 1000 AS DOUBLE) AS avg_ms \
             FROM hammerwork_jobs WHERE status = 'Completed' \
             AND completed_at >= ? AND completed_at < ?{queue_clause} GROUP BY bucket"
        );
        let mut q = sqlx::query(&completed_sql).bind(first).bind(upper);
        if let Some(name) = queue {
            q = q.bind(name.to_string());
        }
        let mut completed = BTreeMap::new();
        for row in q.fetch_all(&self.pool).await? {
            let bucket: String = row.try_get("bucket")?;
            let n: i64 = row.try_get("n")?;
            let avg: Option<f64> = row.try_get("avg_ms")?;
            if let Some(hour) = parse_bucket(&bucket) {
                completed.insert(hour, (to_u64(n), avg));
            }
        }

        let failed_sql = format!(
            "SELECT DATE_FORMAT(COALESCE(failed_at, timed_out_at), '%Y-%m-%d %H:00:00') AS bucket, \
             CAST(COUNT(*) AS SIGNED) AS n FROM hammerwork_jobs \
             WHERE status IN ('Failed', 'Dead', 'TimedOut') \
             AND COALESCE(failed_at, timed_out_at) >= ? AND COALESCE(failed_at, timed_out_at) < ?{queue_clause} \
             GROUP BY bucket"
        );
        let mut q = sqlx::query(&failed_sql).bind(first).bind(upper);
        if let Some(name) = queue {
            q = q.bind(name.to_string());
        }
        let mut failed = BTreeMap::new();
        for row in q.fetch_all(&self.pool).await? {
            let bucket: String = row.try_get("bucket")?;
            let n: i64 = row.try_get("n")?;
            if let Some(hour) = parse_bucket(&bucket) {
                failed.insert(hour, to_u64(n));
            }
        }

        Ok(fill_buckets(&hours, &completed, &failed))
    }

    async fn error_groups(&self, since: DateTime<Utc>, limit: u32) -> Result<Vec<ErrorGroup>> {
        let rows = sqlx::query(
            "SELECT queue_name, error_message, CAST(COUNT(*) AS SIGNED) AS n, \
             MIN(COALESCE(failed_at, timed_out_at)) AS first_seen, \
             MAX(COALESCE(failed_at, timed_out_at)) AS last_seen \
             FROM hammerwork_jobs \
             WHERE status IN ('Failed', 'Dead', 'TimedOut') AND error_message IS NOT NULL \
             AND COALESCE(failed_at, timed_out_at) >= ? \
             GROUP BY queue_name, error_message ORDER BY n DESC LIMIT ?",
        )
        .bind(since)
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await?;
        let mut groups = Vec::with_capacity(rows.len());
        for row in rows {
            groups.push(ErrorGroup {
                queue_name: row.try_get("queue_name")?,
                message: row.try_get("error_message")?,
                count: to_u64(row.try_get("n")?),
                first_seen: row.try_get("first_seen")?,
                last_seen: row.try_get("last_seen")?,
            });
        }
        Ok(groups)
    }

    async fn count_jobs(
        &self,
        queue: Option<&str>,
        kind: FinishedKind,
        older_than: Option<DateTime<Utc>>,
    ) -> Result<u64> {
        let sql = format!(
            "SELECT CAST(COUNT(*) AS SIGNED) FROM hammerwork_jobs {}",
            finished_filter(kind, queue.is_some(), older_than.is_some(), false)
        );
        let mut q = sqlx::query_scalar::<_, i64>(&sql);
        if let Some(name) = queue {
            q = q.bind(name.to_string());
        }
        if let Some(cutoff) = older_than {
            q = q.bind(cutoff);
        }
        Ok(to_u64(q.fetch_one(&self.pool).await?))
    }

    async fn delete_jobs(
        &self,
        queue: Option<&str>,
        kind: FinishedKind,
        older_than: Option<DateTime<Utc>>,
    ) -> Result<u64> {
        let sql = format!(
            "DELETE FROM hammerwork_jobs {}",
            finished_filter(kind, queue.is_some(), older_than.is_some(), false)
        );
        let mut q = sqlx::query(&sql);
        if let Some(name) = queue {
            q = q.bind(name.to_string());
        }
        if let Some(cutoff) = older_than {
            q = q.bind(cutoff);
        }
        Ok(q.execute(&self.pool).await?.rows_affected())
    }

    async fn archived_jobs(
        &self,
        filter: &ArchiveFilterParams,
        limit: u32,
        offset: u32,
    ) -> Result<(Vec<ArchivedJob>, u64)> {
        let (conditions, binds) = archive_conditions(filter, false);
        let count_sql = format!("SELECT COUNT(*) FROM hammerwork_jobs_archive {conditions}");
        let total = bind_archive_filter!(sqlx::query_scalar::<_, i64>(&count_sql), &binds)
            .fetch_one(&self.pool)
            .await?;
        let page_sql = format!(
            "SELECT {ARCHIVED_JOB_COLUMNS} FROM hammerwork_jobs_archive {conditions} \
             ORDER BY archived_at DESC, id DESC LIMIT ? OFFSET ?"
        );
        let rows = bind_archive_filter!(sqlx::query(&page_sql), &binds)
            .bind(i64::from(limit))
            .bind(i64::from(offset))
            .fetch_all(&self.pool)
            .await?;
        let jobs = decodable(rows.iter().map(|row| {
            let label = row.try_get::<String, _>("id").unwrap_or_default();
            let job = uuid::Uuid::parse_str(&label)
                .map_err(Into::into)
                .and_then(|id| archived_job(row, id));
            (label, job)
        }));
        Ok((jobs, to_u64(total)))
    }

    async fn database_now(&self) -> Result<DateTime<Utc>> {
        Ok(sqlx::query_scalar("SELECT UTC_TIMESTAMP(6)")
            .fetch_one(&self.pool)
            .await?)
    }

    async fn recent_job_changes(&self, since: DateTime<Utc>, limit: u32) -> Result<Vec<JobUpdate>> {
        // GREATEST is NULL if any argument is on MySQL, so missing timestamps fall back to
        // `created_at`.
        let latest = CHANGE_COLUMNS
            .iter()
            .map(|column| format!("COALESCE({column}, created_at)"))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT id, queue_name, status, priority, attempts, \
             CAST(GREATEST({latest}) AS DATETIME(6)) AS changed_at \
             FROM hammerwork_jobs WHERE {} ORDER BY changed_at DESC LIMIT ?",
            changed_since_filter(false)
        );
        let mut query = sqlx::query(&sql);
        for _ in CHANGE_COLUMNS {
            query = query.bind(since);
        }
        let rows = query.bind(i64::from(limit)).fetch_all(&self.pool).await?;
        Ok(decodable(rows.iter().map(|row| {
            let label = row.try_get::<String, _>("id").unwrap_or_default();
            let change = (|| {
                Ok(job_change(
                    row.try_get("id")?,
                    row.try_get("queue_name")?,
                    &row.try_get::<String, _>("status")?,
                    row.try_get("priority")?,
                    row.try_get("attempts")?,
                    row.try_get("changed_at")?,
                ))
            })();
            (label, change)
        })))
    }
}

fn parse_bucket(bucket: &str) -> Option<DateTime<Utc>> {
    NaiveDateTime::parse_from_str(bucket, "%Y-%m-%d %H:%M:%S")
        .ok()
        .map(|t| t.and_utc())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(h: u32, m: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 1, 2, h, m, 0).unwrap()
    }

    /// #71: a dashboard listing skips the rows it cannot decode.
    #[test]
    fn undecodable_rows_are_skipped() {
        let bad: Result<u8> = Err(hammerwork::HammerworkError::Processing("bad".into()));
        let rows = vec![("a".to_string(), Ok(1u8)), ("b".to_string(), bad)];
        assert_eq!(decodable(rows), [1]);
    }

    #[test]
    fn job_changes_normalise_status_and_priority() {
        let at = Utc::now();
        let change = job_change("id".into(), "q".into(), "\"Running\"", 3, 2, at);
        assert_eq!(change.status, "Running");
        assert_eq!(change.priority, "high");
        assert_eq!(change.attempts, 2);
        assert_eq!(change.updated_at, at);
        assert_eq!(
            job_change("id".into(), "q".into(), "Pending", 99, 0, at).priority,
            "normal"
        );
        assert_eq!(
            changed_since_filter(true),
            "created_at > $1 OR started_at > $1 OR completed_at > $1 OR failed_at > $1 \
             OR timed_out_at > $1"
        );
        assert!(changed_since_filter(false).starts_with("created_at > ? OR started_at > ?"));
    }

    #[test]
    fn test_hour_floor_and_range() {
        assert_eq!(
            hour_floor(at(5, 59)),
            Utc.with_ymd_and_hms(2026, 1, 2, 5, 0, 0).unwrap()
        );
        let hours = hour_range(at(5, 10), at(8, 0));
        assert_eq!(hours.len(), 3);
        assert_eq!(hours[2], Utc.with_ymd_and_hms(2026, 1, 2, 7, 0, 0).unwrap());
        assert!(hour_range(at(8, 0), at(5, 0)).is_empty());
    }

    #[test]
    fn test_hour_range_is_capped() {
        let end = at(0, 0);
        let hours = hour_range(end - Duration::days(400), end);
        assert_eq!(hours.len() as i64, MAX_BUCKETS);
    }

    #[test]
    fn test_fill_buckets_zero_fills() {
        let hours = hour_range(at(5, 0), at(8, 0));
        let mut completed = BTreeMap::new();
        completed.insert(hours[0], (4, Some(12.5)));
        let mut failed = BTreeMap::new();
        failed.insert(hours[2], 2);
        let buckets = fill_buckets(&hours, &completed, &failed);
        assert_eq!(buckets.len(), 3);
        assert_eq!((buckets[0].completed, buckets[0].failed), (4, 0));
        assert_eq!(buckets[0].avg_processing_time_ms, Some(12.5));
        assert_eq!((buckets[1].completed, buckets[1].failed), (0, 0));
        assert_eq!(buckets[1].avg_processing_time_ms, None);
        assert_eq!((buckets[2].completed, buckets[2].failed), (0, 2));
    }

    /// M11: archive filters become SQL, so the listing never reads the whole archive.
    #[test]
    fn archive_filters_become_sql_conditions() {
        let (sql, binds) = archive_conditions(&ArchiveFilterParams::default(), true);
        assert_eq!(sql, "");
        assert!(binds.is_empty());

        let at = Utc.with_ymd_and_hms(2024, 5, 1, 0, 0, 0).unwrap();
        let filter = ArchiveFilterParams {
            queue: Some("emails".into()),
            reason: Some("Manual".into()),
            archived_after: Some(at),
            archived_before: Some(at),
            archived_by: Some("ops".into()),
            compressed: Some(true),
            original_status: Some("COMPLETED".into()),
        };
        let (pg, binds) = archive_conditions(&filter, true);
        assert_eq!(
            pg,
            "WHERE queue_name = $1 AND LOWER(TRIM(BOTH '\"' FROM archival_reason)) = $2 \
             AND archived_at >= $3 AND archived_at <= $4 AND archived_by = $5 \
             AND payload_compressed = $6 AND LOWER(TRIM(BOTH '\"' FROM status)) = $7"
        );
        assert_eq!(
            binds,
            vec![
                ArchiveBind::Text("emails".into()),
                ArchiveBind::Text("manual".into()),
                ArchiveBind::Time(at),
                ArchiveBind::Time(at),
                ArchiveBind::Text("ops".into()),
                ArchiveBind::Bool(true),
                ArchiveBind::Text("completed".into()),
            ]
        );
        let (mysql, _) = archive_conditions(&filter, false);
        assert_eq!(mysql.matches('?').count(), 7);
        assert!(!mysql.contains('$'));
    }

    #[test]
    fn archived_statuses_parse_like_the_core_library() {
        assert_eq!(archived_status("Completed"), JobStatus::Completed);
        assert_eq!(archived_status("\"TimedOut\""), JobStatus::TimedOut);
        assert_eq!(archived_status("Archived"), JobStatus::Archived);
        assert_eq!(archived_status("whatever"), JobStatus::Dead);
    }

    #[test]
    fn test_finished_filter_placeholders() {
        let pg = finished_filter(FinishedKind::Completed, true, true, true);
        assert_eq!(
            pg,
            "WHERE status = 'Completed' AND queue_name = $1 AND completed_at < $2"
        );
        let my = finished_filter(FinishedKind::Dead, false, true, false);
        assert!(my.contains("status = 'Dead'"));
        assert!(my.ends_with("< ?"));
        assert!(!my.contains("queue_name"));
    }

    #[test]
    fn test_parse_bucket() {
        assert_eq!(
            parse_bucket("2026-01-02 05:00:00"),
            Some(Utc.with_ymd_and_hms(2026, 1, 2, 5, 0, 0).unwrap())
        );
        assert_eq!(parse_bucket("garbage"), None);
    }
}

#[cfg(test)]
mod db_tests {
    use super::*;
    use hammerwork::JobQueue;

    struct Seed {
        queue: String,
        status: &'static str,
        started: Option<DateTime<Utc>>,
        completed: Option<DateTime<Utc>>,
        failed: Option<DateTime<Utc>>,
        error: Option<&'static str>,
    }

    fn seeds(queue: &str, other: &str, now: DateTime<Utc>) -> Vec<Seed> {
        let base = hour_floor(now);
        let done = |q: &str, at: DateTime<Utc>| Seed {
            queue: q.to_string(),
            status: "Completed",
            started: Some(at - Duration::milliseconds(100)),
            completed: Some(at),
            failed: None,
            error: None,
        };
        vec![
            done(queue, base - Duration::hours(2) + Duration::minutes(5)),
            done(queue, base - Duration::hours(2) + Duration::minutes(30)),
            done(other, base - Duration::hours(2) + Duration::minutes(10)),
            Seed {
                queue: queue.to_string(),
                status: "Dead",
                started: None,
                completed: None,
                failed: Some(base - Duration::hours(1) + Duration::minutes(15)),
                error: Some("connection refused"),
            },
        ]
    }

    async fn assert_history<Q: JobHistory>(q: &Q, queue: &str, other: &str, now: DateTime<Utc>) {
        let base = hour_floor(now);
        let buckets = q
            .hourly_activity(Some(queue), base - Duration::hours(3), now)
            .await
            .unwrap();
        assert_eq!(buckets.len(), 4, "3h ago .. current hour, zero filled");
        assert_eq!((buckets[0].completed, buckets[0].failed), (0, 0));
        assert_eq!((buckets[1].completed, buckets[1].failed), (2, 0));
        let avg = buckets[1].avg_processing_time_ms.expect("avg");
        assert!((avg - 100.0).abs() < 5.0, "avg was {avg}");
        assert_eq!((buckets[2].completed, buckets[2].failed), (0, 1));
        assert_eq!(buckets[2].avg_processing_time_ms, None);
        assert_eq!(buckets[1].hour, base - Duration::hours(2));

        // Without a queue filter the other queue's completion is included too.
        let all = q
            .hourly_activity(None, base - Duration::hours(3), now)
            .await
            .unwrap();
        assert!(all[1].completed >= 3);

        let groups = q.error_groups(now - Duration::hours(5), 500).await.unwrap();
        let ours: Vec<_> = groups.iter().filter(|g| g.queue_name == queue).collect();
        assert_eq!(ours.len(), 1);
        assert_eq!(ours[0].message, "connection refused");
        assert_eq!(ours[0].count, 1);
        assert_eq!(
            ours[0].first_seen.timestamp(),
            (base - Duration::hours(1) + Duration::minutes(15)).timestamp()
        );
        assert_eq!(ours[0].last_seen, ours[0].first_seen);

        // Counting and deleting are scoped to the queue and kind.
        let completed = FinishedKind::Completed;
        let dead = FinishedKind::Dead;
        assert_eq!(q.count_jobs(Some(queue), completed, None).await.unwrap(), 2);
        assert_eq!(q.count_jobs(Some(queue), dead, None).await.unwrap(), 1);
        let week_ago = Some(now - Duration::days(7));
        assert_eq!(q.count_jobs(Some(queue), dead, week_ago).await.unwrap(), 0);
        assert_eq!(
            q.delete_jobs(Some(queue), completed, None).await.unwrap(),
            2
        );
        assert_eq!(q.count_jobs(Some(queue), completed, None).await.unwrap(), 0);
        assert_eq!(q.count_jobs(Some(queue), dead, None).await.unwrap(), 1);
        assert_eq!(q.count_jobs(Some(other), completed, None).await.unwrap(), 1);
    }

    #[tokio::test]
    #[ignore = "requires DATABASE_URL (PostgreSQL)"]
    async fn test_job_history_queries_postgres() {
        let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
        let pool = sqlx::PgPool::connect(&url).await.unwrap();
        let tag = uuid::Uuid::new_v4().simple().to_string();
        let (queue, other) = (format!("hist_a_{tag}"), format!("hist_b_{tag}"));
        let now = Utc::now();
        for s in seeds(&queue, &other, now) {
            sqlx::query(
                "INSERT INTO hammerwork_jobs (id, queue_name, payload, status, priority, attempts, \
                 max_attempts, created_at, scheduled_at, started_at, completed_at, failed_at, error_message) \
                 VALUES ($1, $2, '{}'::jsonb, $3, 2, 1, 3, NOW(), NOW(), $4, $5, $6, $7)",
            )
            .bind(uuid::Uuid::new_v4())
            .bind(&s.queue)
            .bind(s.status)
            .bind(s.started)
            .bind(s.completed)
            .bind(s.failed)
            .bind(s.error)
            .execute(&pool)
            .await
            .unwrap();
        }
        let jq = JobQueue::<sqlx::Postgres>::new(pool.clone());
        assert_history(&jq, &queue, &other, now).await;
        for name in [&queue, &other] {
            sqlx::query("DELETE FROM hammerwork_jobs WHERE queue_name = $1")
                .bind(name)
                .execute(&pool)
                .await
                .unwrap();
        }
    }

    #[tokio::test]
    #[ignore = "requires MYSQL_DATABASE_URL"]
    async fn test_job_history_queries_mysql() {
        let url = std::env::var("MYSQL_DATABASE_URL").expect("MYSQL_DATABASE_URL");
        let pool = sqlx::MySqlPool::connect(&url).await.unwrap();
        let tag = uuid::Uuid::new_v4().simple().to_string();
        let (queue, other) = (format!("hist_a_{tag}"), format!("hist_b_{tag}"));
        let now = Utc::now();
        for s in seeds(&queue, &other, now) {
            sqlx::query(
                "INSERT INTO hammerwork_jobs (id, queue_name, payload, status, priority, attempts, \
                 max_attempts, created_at, scheduled_at, started_at, completed_at, failed_at, error_message) \
                 VALUES (?, ?, '{}', ?, 2, 1, 3, UTC_TIMESTAMP(6), UTC_TIMESTAMP(6), ?, ?, ?, ?)",
            )
            .bind(uuid::Uuid::new_v4().to_string())
            .bind(&s.queue)
            .bind(s.status)
            .bind(s.started)
            .bind(s.completed)
            .bind(s.failed)
            .bind(s.error)
            .execute(&pool)
            .await
            .unwrap();
        }
        let jq = JobQueue::<sqlx::MySql>::new(pool.clone());
        assert_history(&jq, &queue, &other, now).await;
        for name in [&queue, &other] {
            sqlx::query("DELETE FROM hammerwork_jobs WHERE queue_name = ?")
                .bind(name)
                .execute(&pool)
                .await
                .unwrap();
        }
    }
}
