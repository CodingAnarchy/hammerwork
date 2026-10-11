//! Helpers shared by the SQL backends (`postgres` and `mysql`): transactions and
//! conflict retries, column encoding and decoding, cron and dequeue helpers.
#![cfg_attr(not(any(feature = "postgres", feature = "mysql")), allow(dead_code))]

use super::*;

/// Job statuses after which an encrypted job may be deleted by
/// [`DatabaseQueue::purge_expired_encrypted_jobs`]. Safe to format into a query.
pub(crate) const FINISHED_STATUS_SQL: &str = "'Completed', 'Failed', 'Dead', 'TimedOut'";

/// Whether a database error aborted the transaction as a deadlock or serialization
/// victim (SQLSTATE `40001` on both backends, `40P01` on PostgreSQL). The transaction
/// was rolled back, so it is safe to run again.
pub(crate) fn is_transaction_conflict(error: &crate::HammerworkError) -> bool {
    matches!(
        error,
        crate::HammerworkError::Database(sqlx::Error::Database(db_error))
            if matches!(db_error.code().as_deref(), Some("40001") | Some("40P01"))
    )
}

/// Run a transaction, retrying a few times with a short backoff when the database
/// aborts it as a deadlock or serialization victim.
pub(crate) async fn retry_on_conflict<F, Fut, T>(mut transaction: F) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T>>,
{
    const ATTEMPTS: u32 = 5;
    let mut attempt = 1;
    loop {
        match transaction().await {
            Err(error) if attempt < ATTEMPTS && is_transaction_conflict(&error) => {
                tokio::time::sleep(std::time::Duration::from_millis(5 * u64::from(attempt))).await;
                attempt += 1;
            }
            result => return result,
        }
    }
}

/// End `tx` according to `result`: commit on success, roll back on error.
///
/// Rolling back explicitly (instead of relying on the rollback a dropped transaction
/// queues on its connection) guarantees the connection never goes back to the pool
/// with an open transaction still holding row locks.
pub(crate) async fn end_transaction<DB: sqlx::Database, T>(
    tx: sqlx::Transaction<'_, DB>,
    result: Result<T>,
) -> Result<T> {
    match result {
        Ok(value) => {
            tx.commit().await?;
            Ok(value)
        }
        Err(error) => {
            if let Err(rollback_error) = tx.rollback().await {
                tracing::warn!("Failed to roll back transaction after error: {rollback_error}");
            }
            Err(error)
        }
    }
}

/// The JSON stored in the `retry_strategy` column for `job`.
///
/// Validate a job passed to [`DatabaseQueue::enqueue_cron_job`] and mark it recurring.
///
/// The job must have a valid cron schedule (in its timezone, if set). When it has no
/// `next_run_at` yet, the next execution is computed and the job is scheduled for it.
pub(crate) fn prepare_cron_job(mut job: Job) -> Result<Job> {
    let schedule = match job.get_cron_schedule() {
        Some(Ok(schedule)) => schedule,
        Some(Err(e)) => {
            return Err(crate::HammerworkError::Queue {
                message: format!("Invalid cron schedule {:?}: {}", job.cron_schedule, e),
            });
        }
        None => {
            return Err(crate::HammerworkError::Queue {
                message: "Job must have a cron schedule".to_string(),
            });
        }
    };
    job.recurring = true;
    if job.next_run_at.is_none() {
        job.next_run_at = schedule.next_execution_from_now();
        if let Some(next_run_at) = job.next_run_at {
            job.scheduled_at = next_run_at;
        }
    }
    Ok(job)
}

/// When to schedule recurring job `job`, being enabled at `now`: `None` when it is
/// `Running` (its run's outcome reschedules it), otherwise the next occurrence of its
/// schedule after `now`. Fails for a job without a valid cron schedule.
pub(crate) fn enabled_recurring_next_run(
    job: &Job,
    now: DateTime<Utc>,
) -> Result<Option<DateTime<Utc>>> {
    let schedule = match job.get_cron_schedule() {
        Some(Ok(schedule)) => schedule,
        Some(Err(e)) => {
            return Err(crate::HammerworkError::Queue {
                message: format!(
                    "job {} has an invalid cron schedule {:?}: {e}",
                    job.id, job.cron_schedule
                ),
            });
        }
        None => return Err(not_a_cron_job(job.id)),
    };
    if job.status == crate::job::JobStatus::Running {
        return Ok(None);
    }
    schedule
        .next_execution(now)
        .map(Some)
        .ok_or_else(|| crate::HammerworkError::Queue {
            message: format!("the cron schedule of job {} has no next occurrence", job.id),
        })
}

/// Why `disable_recurring_job` changed no row: fine for a job that is already
/// disabled, an error for a missing (or archived) job or one without a cron schedule.
pub(crate) fn disable_recurring_job_not_applied(job: Option<Job>, job_id: JobId) -> Result<()> {
    match job {
        Some(job) if job.status == crate::job::JobStatus::Archived => {
            Err(crate::HammerworkError::JobNotFound {
                id: job_id.to_string(),
            })
        }
        Some(job) if job.cron_schedule.is_some() => Ok(()),
        Some(_) => Err(not_a_cron_job(job_id)),
        None => Err(crate::HammerworkError::JobNotFound {
            id: job_id.to_string(),
        }),
    }
}

/// The error for a recurring job operation on a job without a cron schedule.
pub(crate) fn not_a_cron_job(job_id: JobId) -> crate::HammerworkError {
    crate::HammerworkError::Queue {
        message: format!("job {job_id} has no cron schedule, so it is not a recurring job"),
    }
}

/// A [`RetryStrategy::Custom`](crate::retry::RetryStrategy::Custom) holds a closure and
/// cannot be persisted, so enqueueing a job that carries one is rejected; set it as the
/// worker's default with
/// [`Worker::with_default_retry_strategy`](crate::worker::Worker::with_default_retry_strategy)
/// instead.
pub(crate) fn retry_strategy_json(job: &Job) -> Result<Option<serde_json::Value>> {
    match &job.retry_strategy {
        None => Ok(None),
        Some(crate::retry::RetryStrategy::Custom(_)) => {
            Err(crate::HammerworkError::InvalidJobPayload {
                message: format!(
                    "job {} has a custom retry strategy, which cannot be stored; use \
                     Worker::with_default_retry_strategy for custom strategies",
                    job.id
                ),
            })
        }
        Some(strategy) => Ok(Some(serde_json::to_value(strategy)?)),
    }
}

/// The encryption columns of a job row (migration 011), as written by the inserts.
#[derive(Debug, Default)]
pub(crate) struct EncryptionColumns {
    pub is_encrypted: bool,
    pub key_id: Option<String>,
    pub algorithm: Option<&'static str>,
    pub ciphertext: Option<Vec<u8>>,
    pub nonce: Option<Vec<u8>>,
    pub tag: Option<Vec<u8>>,
    pub metadata: Option<serde_json::Value>,
    pub payload_hash: Option<String>,
    pub retention_policy: Option<&'static str>,
    pub retention_delete_at: Option<DateTime<Utc>>,
    pub encrypted_at: Option<DateTime<Utc>>,
}

impl EncryptionColumns {
    /// The columns for `job`, which must already be sealed ([`JobQueue::seal_jobs`]).
    ///
    /// Refuses a job that has an encryption config but no ciphertext, so a code path that
    /// forgot to seal its jobs fails instead of storing the plaintext.
    pub(crate) fn for_job(job: &Job) -> Result<Self> {
        if !job.is_encrypted {
            if job.has_encryption() {
                return Err(crate::HammerworkError::Encryption {
                    message: format!(
                        "Job {} has an encryption config but its payload was not encrypted",
                        job.id
                    ),
                });
            }
            return Ok(Self::default());
        }

        #[cfg(feature = "encryption")]
        {
            use crate::encryption::{EncryptionAlgorithm, RetentionPolicy};

            let encrypted = job.encrypted_payload.as_ref().ok_or_else(|| {
                crate::HammerworkError::Encryption {
                    message: format!(
                        "Job {} is marked encrypted but has no encrypted payload",
                        job.id
                    ),
                }
            })?;
            let metadata = &encrypted.metadata;
            Ok(Self {
                is_encrypted: true,
                key_id: Some(metadata.key_id.clone()),
                algorithm: Some(match metadata.algorithm {
                    EncryptionAlgorithm::AES256GCM => "AES256GCM",
                    EncryptionAlgorithm::ChaCha20Poly1305 => "ChaCha20Poly1305",
                }),
                ciphertext: Some(encrypted.decode_ciphertext()?),
                nonce: Some(encrypted.decode_nonce()?),
                tag: Some(encrypted.decode_tag()?),
                metadata: Some(serde_json::to_value(metadata)?),
                payload_hash: Some(metadata.payload_hash.clone()),
                retention_policy: Some(match metadata.retention_policy {
                    // A retention too long to represent never expires
                    RetentionPolicy::DeleteAfter(_) if metadata.delete_at.is_none() => {
                        "KeepIndefinitely"
                    }
                    RetentionPolicy::DeleteAfter(_) => "DeleteAfter",
                    RetentionPolicy::DeleteAt(_) => "DeleteAt",
                    RetentionPolicy::KeepIndefinitely => "KeepIndefinitely",
                    RetentionPolicy::DeleteImmediately => "DeleteImmediately",
                    RetentionPolicy::UseDefault => "UseDefault",
                }),
                retention_delete_at: metadata.delete_at,
                encrypted_at: Some(metadata.encrypted_at),
            })
        }
        #[cfg(not(feature = "encryption"))]
        Err(crate::HammerworkError::Encryption {
            message: format!(
                "Job {} is marked encrypted; storing encrypted jobs needs the `encryption` feature",
                job.id
            ),
        })
    }
}

/// `pii_fields` of a job as stored (`NULL` when there are none).
pub(crate) fn pii_fields_column(job: &Job) -> Option<Vec<String>> {
    (!job.pii_fields.is_empty()).then(|| job.pii_fields.clone())
}

/// Decode the `retry_strategy` column; unknown or invalid values decode as `None`.
pub(crate) fn retry_strategy_from_json(
    value: Option<serde_json::Value>,
) -> Result<Option<crate::retry::RetryStrategy>> {
    // A stored strategy that no longer decodes is corrupt data: report it instead of
    // silently running the job with the worker's default strategy.
    Ok(value.map(serde_json::from_value).transpose()?)
}

/// The stored `timeout_seconds` for a job timeout. The column holds whole seconds, so a
/// sub-second timeout is rounded up (never stored as an immediate zero) and an enormous one
/// saturates at `i32::MAX` instead of wrapping negative. A zero timeout is stored as none.
pub(crate) fn timeout_db_seconds(timeout: Option<std::time::Duration>) -> Option<i32> {
    let timeout = timeout.filter(|timeout| !timeout.is_zero())?;
    let seconds = timeout
        .as_secs()
        .saturating_add(u64::from(timeout.subsec_nanos() > 0));
    Some(i32::try_from(seconds).unwrap_or(i32::MAX))
}

/// An unsigned setting (a result TTL in seconds, a size in bytes) as a `BIGINT` column
/// value, saturating at `i64::MAX` instead of wrapping negative, which would make the row
/// unreadable.
pub(crate) fn saturating_i64(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

/// The decoded rows of a listing (dead jobs, ready jobs, archived jobs, ...), skipping and
/// logging the rows that cannot be decoded.
///
/// One corrupt row (say a negative `timeout_seconds` written by an older version) must
/// not make every listing that includes it fail, which would hide all the healthy jobs
/// around it from the dashboard and the CLI. Each item is the row's id (for the log) and
/// its decoding result. Reads of a single job keep returning the error.
pub(crate) fn decodable_rows<T>(
    listing: &str,
    rows: impl IntoIterator<Item = (String, Result<T>)>,
) -> Vec<T> {
    rows.into_iter()
        .filter_map(|(id, decoded)| match decoded {
            Ok(value) => Some(value),
            Err(error) => {
                tracing::warn!(
                    job_id = %id,
                    error = %error,
                    "Skipping a row that cannot be decoded while listing {listing}"
                );
                None
            }
        })
        .collect()
}

/// Log a stale job the reaper reclaimed although its row cannot be decoded, so the side
/// effects of its death (dependents, workflow, batch, recurring schedule) were skipped.
pub(crate) fn warn_undecodable_stale_job(job_id: JobId, error: &crate::HammerworkError) {
    tracing::warn!(
        job_id = %job_id,
        error = %error,
        "Reclaimed a stale job whose row cannot be decoded; its dependents, workflow, \
         batch and recurring schedule were not updated"
    );
}

/// Convert an integer read from the database into the type the API exposes (e.g. a
/// `COUNT(*)` `i64` into `u64`), failing instead of wrapping on out-of-range values.
pub(crate) fn db_int<T, S>(value: S, column: &str) -> Result<T>
where
    T: TryFrom<S>,
    S: Copy + std::fmt::Display,
{
    T::try_from(value).map_err(|_| crate::HammerworkError::Queue {
        message: format!("{column} read from the database is out of range: {value}"),
    })
}

/// A duration in whole seconds read from the database (e.g. `timeout_seconds`).
pub(crate) fn db_seconds<S>(value: S, column: &str) -> Result<std::time::Duration>
where
    u64: TryFrom<S>,
    S: Copy + std::fmt::Display,
{
    Ok(std::time::Duration::from_secs(db_int(value, column)?))
}

/// Turn `(key, COUNT(*))` rows into a map, rejecting negative counts.
pub(crate) fn count_map(rows: Vec<(String, i64)>, column: &str) -> Result<HashMap<String, u64>> {
    rows.into_iter()
        .map(|(key, count)| Ok((key, db_int(count, column)?)))
        .collect()
}

/// The priority stored as `value`; unknown values count as `Normal` in statistics.
pub(crate) fn priority_or_normal(value: i32) -> crate::priority::JobPriority {
    crate::priority::JobPriority::from_i32(value).unwrap_or(crate::priority::JobPriority::Normal)
}

/// The priorities a weighted dequeue tries.
pub(crate) const ALL_PRIORITIES: [crate::priority::JobPriority; 5] = [
    crate::priority::JobPriority::Critical,
    crate::priority::JobPriority::High,
    crate::priority::JobPriority::Normal,
    crate::priority::JobPriority::Low,
    crate::priority::JobPriority::Background,
];

/// SQL selecting the priority levels in `priorities` that have a job matching
/// `runnable` (a condition on the alias `j`, ending with the priority check).
pub(crate) fn runnable_priorities_sql(runnable: &str) -> String {
    let levels = ALL_PRIORITIES
        .iter()
        .map(|priority| format!("SELECT {} AS priority", priority.as_i32()))
        .collect::<Vec<_>>()
        .join(" UNION ALL ");
    format!(
        "SELECT levels.priority FROM ({levels}) levels \
         WHERE EXISTS (SELECT 1 FROM hammerwork_jobs j WHERE {runnable} \
         AND j.priority = levels.priority)"
    )
}

/// Pick one of `candidates` (the priorities that have runnable jobs) by weight.
///
/// The probability of a priority is its weight over the candidates' total weight, so
/// every priority with a runnable job and a non-zero weight is chosen regularly, no
/// matter how many jobs of higher priority are queued. Priorities with weight zero
/// are only chosen when no candidate has a weight (then the highest one wins).
/// `seed` drives the choice; callers pass something that varies between calls.
pub(crate) fn pick_weighted_priority(
    candidates: &[crate::priority::JobPriority],
    weights: &crate::priority::PriorityWeights,
    seed: u64,
) -> Option<crate::priority::JobPriority> {
    let total: u64 = candidates
        .iter()
        .map(|priority| u64::from(weights.get_weight(*priority)))
        .sum();
    if total == 0 {
        return candidates.iter().copied().max();
    }
    let mut point = seed % total;
    for priority in candidates {
        let weight = u64::from(weights.get_weight(*priority));
        if point < weight {
            return Some(*priority);
        }
        point -= weight;
    }
    None
}

/// Fill a weighted batch claim of up to `max_jobs` jobs of `queue_name`
/// ([`DatabaseQueue::dequeue_batch_leased`] with weights): pick one of `candidates` (the
/// levels with runnable jobs) by weight, claim up to the room left with
/// `claim(level, room)`, and repeat with the remaining levels until the batch is full or
/// no level is left.
///
/// Each `claim` commits its jobs before the next one runs, so if a later claim fails the
/// jobs already claimed are returned (and the error logged) rather than the error: they
/// are `Running` under this caller's lease, and dropping them would leave them stuck
/// until their leases expire, with an attempt spent (a job on its last attempt would go
/// to `Dead` without having run). The error is returned when nothing was claimed.
pub(crate) async fn fill_weighted_batch<F, Fut>(
    queue_name: &str,
    mut candidates: Vec<crate::priority::JobPriority>,
    weights: &crate::priority::PriorityWeights,
    max_jobs: usize,
    mut claim: F,
) -> Result<Vec<Job>>
where
    F: FnMut(crate::priority::JobPriority, usize) -> Fut,
    Fut: std::future::Future<Output = Result<Vec<Job>>>,
{
    let mut jobs = Vec::with_capacity(max_jobs);
    let mut attempt = 0;
    while jobs.len() < max_jobs
        && let Some(priority) =
            pick_weighted_priority(&candidates, weights, weighted_seed(queue_name, attempt))
    {
        match claim(priority, max_jobs - jobs.len()).await {
            Ok(claimed) => jobs.extend(claimed),
            Err(error) if jobs.is_empty() => return Err(error),
            Err(error) => {
                tracing::warn!(
                    "claiming jobs of queue {} failed after {} were claimed; returning those: {}",
                    queue_name,
                    jobs.len(),
                    error
                );
                break;
            }
        }
        candidates.retain(|candidate| *candidate != priority);
        attempt += 1;
    }
    Ok(jobs)
}

/// A seed for [`pick_weighted_priority`]: a hash of the queue name and the current
/// time (hash-based so the future stays `Send`).
pub(crate) fn weighted_seed(queue_name: &str, attempt: usize) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    queue_name.hash(&mut hasher);
    attempt.hash(&mut hasher);
    Utc::now()
        .timestamp_nanos_opt()
        .unwrap_or(0)
        .hash(&mut hasher);
    std::thread::current().id().hash(&mut hasher);
    hasher.finish()
}

/// Put claimed jobs in claim order: highest priority first, then oldest (`priority
/// DESC, scheduled_at ASC`), as [`DatabaseQueue::dequeue_batch_leased`] returns them.
#[cfg_attr(not(feature = "postgres"), allow(dead_code))]
pub(crate) fn sort_in_claim_order(jobs: &mut [Job]) {
    jobs.sort_by(|a, b| {
        b.priority
            .as_i32()
            .cmp(&a.priority.as_i32())
            .then(a.scheduled_at.cmp(&b.scheduled_at))
    });
}

/// The lease interval written for `lease`: a zero lease never expires (it is clamped to
/// the longest interval, like any lease longer than that).
pub(crate) fn lease_interval(lease: std::time::Duration) -> chrono::Duration {
    if lease.is_zero() {
        clamp_interval(std::time::Duration::MAX)
    } else {
        clamp_interval(lease)
    }
}

/// The range a stored `started_at` must fall in for the row to be the run `run` was
/// dequeued as (see [`lifecycle::is_same_run`]): within 1ms either way, because the
/// backends store microseconds while `Job` carries nanoseconds. `None` when `run` has
/// no start time.
pub(crate) fn run_started_range(run: &Job) -> Option<(DateTime<Utc>, DateTime<Utc>)> {
    let started = run.started_at?;
    let tolerance = chrono::Duration::milliseconds(1);
    Some((
        started.checked_sub_signed(tolerance).unwrap_or(started),
        started.checked_add_signed(tolerance).unwrap_or(started),
    ))
}

/// The longest lease or staleness window passed to the database as an interval;
/// longer durations are clamped to it (100 years).
pub(crate) fn clamp_interval(duration: std::time::Duration) -> chrono::Duration {
    const MAX: std::time::Duration = std::time::Duration::from_secs(100 * 365 * 24 * 3600);
    saturating_chrono_duration(duration.min(MAX))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::priority::JobPriority;

    #[test]
    fn stored_priorities_decode_with_a_normal_fallback() {
        assert_eq!(priority_or_normal(0), JobPriority::Background);
        assert_eq!(priority_or_normal(1), JobPriority::Low);
        assert_eq!(priority_or_normal(2), JobPriority::Normal);
        assert_eq!(priority_or_normal(3), JobPriority::High);
        assert_eq!(priority_or_normal(4), JobPriority::Critical);
        assert_eq!(priority_or_normal(-1), JobPriority::Normal);
        assert_eq!(priority_or_normal(99), JobPriority::Normal);
    }
}
