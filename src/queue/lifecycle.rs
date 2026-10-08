//! The job lifecycle state machine.
//!
//! Two kinds of status change exist:
//!
//! - **Manual transitions** ([`DatabaseQueue::complete_job`], `fail_job`, `retry_job`,
//!   `mark_job_dead`, `mark_job_timed_out`, `reschedule_cron_job` and `retry_dead_job`)
//!   are for operators, tools and tests. Each one only applies from the source statuses
//!   listed by [`JobTransition::allowed_from`]; any other status is rejected with
//!   [`HammerworkError::InvalidJobTransition`](crate::HammerworkError::InvalidJobTransition)
//!   and a missing job with `JobNotFound`.
//! - **Run outcomes** ([`DatabaseQueue::finish_job_run`]) are what a worker records when
//!   a run it dequeued finishes. They only apply while the job is still `Running`
//!   *the same run* (same `attempts` and `started_at`), so a worker whose job was
//!   reclaimed by the stale-job reaper, requeued by an operator or picked up again by
//!   another worker cannot overwrite the job's newer state.
//!
//! Both kinds apply their side effects in the same transaction as the status change:
//! on completion, dependents whose dependencies have all completed become runnable;
//! on a terminal failure, the workflow's [`FailurePolicy`](crate::workflow::FailurePolicy)
//! and the batch's [`PartialFailureMode`](crate::batch::PartialFailureMode) are applied;
//! and workflow and batch counters are updated. A worker run of a recurring job that
//! ends (successfully or not) is rescheduled for its next cron occurrence instead.
//!
//! [`DatabaseQueue::complete_job`]: super::DatabaseQueue::complete_job
//! [`DatabaseQueue::finish_job_run`]: super::DatabaseQueue::finish_job_run

use crate::job::{Job, JobId, JobStatus};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A manual status transition, as performed by the corresponding
/// [`DatabaseQueue`](super::DatabaseQueue) method.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum JobTransition {
    /// [`complete_job`](super::DatabaseQueue::complete_job): to `Completed`.
    Complete,
    /// [`fail_job`](super::DatabaseQueue::fail_job): to `Failed`.
    Fail,
    /// [`retry_job`](super::DatabaseQueue::retry_job): back to `Pending` at a given time.
    Retry,
    /// [`mark_job_dead`](super::DatabaseQueue::mark_job_dead): to `Dead`.
    MarkDead,
    /// [`mark_job_timed_out`](super::DatabaseQueue::mark_job_timed_out): to `TimedOut`.
    MarkTimedOut,
    /// [`reschedule_cron_job`](super::DatabaseQueue::reschedule_cron_job): a recurring job
    /// back to `Pending` for its next run.
    RescheduleCron,
    /// [`retry_dead_job`](super::DatabaseQueue::retry_dead_job): a terminally failed job
    /// back to `Pending` with its attempts reset.
    RetryDead,
}

impl JobTransition {
    /// The statuses this transition may start from.
    ///
    /// | Transition | Allowed from |
    /// |---|---|
    /// | `Complete`, `Fail` | `Pending`, `Running`, `Retrying` |
    /// | `Retry` | `Running`, `Retrying`, `Failed`, `TimedOut` |
    /// | `MarkDead` | `Pending`, `Running`, `Retrying`, `Failed`, `TimedOut` |
    /// | `MarkTimedOut` | `Running` |
    /// | `RescheduleCron` | any status except `Completed` and `Archived` |
    /// | `RetryDead` | `Dead`, `TimedOut` |
    ///
    /// `Completed` and `Archived` are final for every manual transition; `Dead` can only
    /// be left through `retry_dead_job` (or `reschedule_cron_job` for recurring jobs).
    pub fn allowed_from(self) -> &'static [JobStatus] {
        use JobStatus as S;
        match self {
            Self::Complete | Self::Fail => &[S::Pending, S::Running, S::Retrying],
            Self::Retry => &[S::Running, S::Retrying, S::Failed, S::TimedOut],
            Self::MarkDead => &[S::Pending, S::Running, S::Retrying, S::Failed, S::TimedOut],
            Self::MarkTimedOut => &[S::Running],
            Self::RescheduleCron => &[
                S::Pending,
                S::Running,
                S::Retrying,
                S::Failed,
                S::Dead,
                S::TimedOut,
            ],
            Self::RetryDead => &[S::Dead, S::TimedOut],
        }
    }

    /// Whether this transition may start from `status`.
    pub fn is_allowed_from(self, status: JobStatus) -> bool {
        self.allowed_from().contains(&status)
    }

    /// A short verb phrase for error messages ("completed", "retried", ...).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Complete => "completed",
            Self::Fail => "failed",
            Self::Retry => "retried",
            Self::MarkDead => "marked dead",
            Self::MarkTimedOut => "marked timed out",
            Self::RescheduleCron => "rescheduled",
            Self::RetryDead => "retried from dead",
        }
    }

    /// `'Pending', 'Running', ...`: the allowed source statuses as a SQL list.
    ///
    /// Only contains the fixed status names, so it is safe to format into a query.
    #[cfg_attr(not(feature = "postgres"), allow(dead_code))]
    pub(crate) fn sql_status_list(self) -> String {
        self.allowed_from()
            .iter()
            .map(|status| format!("'{}'", status.as_str()))
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// The error returned when this transition is rejected for a job in `status`.
    pub(crate) fn rejected(self, job_id: JobId, status: JobStatus) -> crate::HammerworkError {
        crate::HammerworkError::InvalidJobTransition {
            job_id: job_id.to_string(),
            status: status.as_str().to_string(),
            transition: self.as_str().to_string(),
        }
    }
}

/// How one run of a job ended, as reported by the worker that ran it to
/// [`DatabaseQueue::finish_job_run`](super::DatabaseQueue::finish_job_run).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobOutcome {
    /// The handler succeeded.
    Completed,
    /// The handler failed or timed out and the job has attempts left: run it again at
    /// `retry_at`.
    Retry {
        /// When the job becomes eligible to run again.
        retry_at: DateTime<Utc>,
        /// The error of this run, recorded on the job.
        error: String,
        /// Whether this run timed out (records `timed_out_at`).
        timed_out: bool,
    },
    /// The handler failed and the job has no attempts left.
    Dead {
        /// The error of this run, recorded on the job.
        error: String,
    },
    /// The handler timed out and the job has no attempts left.
    TimedOut {
        /// The error of this run, recorded on the job.
        error: String,
    },
}

impl JobOutcome {
    /// The status a non-recurring job ends in, or `None` for a retry.
    pub fn terminal_status(&self) -> Option<JobStatus> {
        match self {
            Self::Completed => Some(JobStatus::Completed),
            Self::Retry { .. } => None,
            Self::Dead { .. } => Some(JobStatus::Dead),
            Self::TimedOut { .. } => Some(JobStatus::TimedOut),
        }
    }
}

/// What [`DatabaseQueue::finish_job_run`](super::DatabaseQueue::finish_job_run) (or a
/// manual transition) recorded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedOutcome {
    /// The status the job is in now: the terminal status, or `Pending` for a retry or a
    /// recurring job rescheduled for its next run.
    pub status: JobStatus,
    /// Set when a recurring job was rescheduled: its next run time.
    pub next_run_at: Option<DateTime<Utc>>,
    /// Jobs whose dependencies have now all completed and that became runnable.
    pub unblocked: Vec<JobId>,
    /// Jobs failed as a consequence of this one failing: dependents that can no longer
    /// run, or the remaining jobs of a fail-fast workflow or batch.
    pub cancelled: Vec<JobId>,
}

impl RecordedOutcome {
    pub(crate) fn new(status: JobStatus) -> Self {
        Self {
            status,
            next_run_at: None,
            unblocked: Vec::new(),
            cancelled: Vec::new(),
        }
    }
}

/// The new state a transition writes. Shared by manual transitions and run outcomes.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(not(any(feature = "postgres", feature = "mysql")), allow(dead_code))]
pub(crate) enum Target {
    Completed,
    Failed(String),
    Dead(String),
    TimedOut(String),
    Retry {
        retry_at: DateTime<Utc>,
        error: Option<String>,
        timed_out: bool,
    },
}

#[cfg_attr(not(any(feature = "postgres", feature = "mysql")), allow(dead_code))]
impl Target {
    /// The terminal status this target writes, or `None` for a retry.
    pub fn terminal_status(&self) -> Option<JobStatus> {
        match self {
            Self::Completed => Some(JobStatus::Completed),
            Self::Failed(_) => Some(JobStatus::Failed),
            Self::Dead(_) => Some(JobStatus::Dead),
            Self::TimedOut(_) => Some(JobStatus::TimedOut),
            Self::Retry { .. } => None,
        }
    }
}

impl From<JobOutcome> for Target {
    fn from(outcome: JobOutcome) -> Self {
        match outcome {
            JobOutcome::Completed => Self::Completed,
            JobOutcome::Retry {
                retry_at,
                error,
                timed_out,
            } => Self::Retry {
                retry_at,
                error: Some(error),
                timed_out,
            },
            JobOutcome::Dead { error } => Self::Dead(error),
            JobOutcome::TimedOut { error } => Self::TimedOut(error),
        }
    }
}

/// Which current states a transition accepts.
#[derive(Debug, Clone, Copy)]
#[cfg_attr(not(any(feature = "postgres", feature = "mysql")), allow(dead_code))]
pub(crate) enum Guard<'a> {
    /// A manual transition: any of its allowed source statuses.
    Manual(JobTransition),
    /// A run outcome: the job must still be `Running` the run `run` was dequeued as.
    Run(&'a Job),
}

#[cfg_attr(not(any(feature = "postgres", feature = "mysql")), allow(dead_code))]
impl Guard<'_> {
    /// Whether the job, in its current (locked) state, accepts the transition.
    pub fn admits(&self, current: &Job) -> bool {
        match self {
            Self::Manual(transition) => transition.is_allowed_from(current.status),
            Self::Run(run) => current.status == JobStatus::Running && is_same_run(current, run),
        }
    }
}

/// Whether `current` (the job as stored now) is still the run `run` was dequeued as.
///
/// Every dequeue increments `attempts` and sets `started_at`, so a run is identified by
/// both. `attempts` alone is not enough for recurring jobs, whose attempts restart at
/// zero on every reschedule. `started_at` is compared with a 1ms tolerance because
/// backends store microseconds while `Job` carries nanoseconds.
pub(crate) fn is_same_run(current: &Job, run: &Job) -> bool {
    if current.id != run.id || current.attempts != run.attempts {
        return false;
    }
    match (current.started_at, run.started_at) {
        (Some(stored), Some(claimed)) => (stored - claimed)
            .num_microseconds()
            .is_some_and(|delta| delta.abs() < 1_000),
        // A run without a start time (e.g. a hand-built job) is identified by attempts.
        (_, None) => true,
        (None, Some(_)) => false,
    }
}

/// What a backend's transition did.
#[derive(Debug)]
#[cfg_attr(not(any(feature = "postgres", feature = "mysql")), allow(dead_code))]
pub(crate) enum TransitionResult {
    Applied(RecordedOutcome),
    /// The guard rejected the job's current status (or run).
    Rejected(JobStatus),
    NotFound,
}

#[cfg_attr(not(any(feature = "postgres", feature = "mysql")), allow(dead_code))]
impl TransitionResult {
    /// The result of a manual transition: rejections are errors.
    pub fn into_manual(
        self,
        job_id: JobId,
        transition: JobTransition,
    ) -> crate::Result<RecordedOutcome> {
        match self {
            Self::Applied(recorded) => Ok(recorded),
            Self::Rejected(status) => Err(transition.rejected(job_id, status)),
            Self::NotFound => Err(crate::HammerworkError::JobNotFound {
                id: job_id.to_string(),
            }),
        }
    }

    /// The result of a run outcome: a stale run is `None`.
    pub fn into_run(self) -> Option<RecordedOutcome> {
        match self {
            Self::Applied(recorded) => Some(recorded),
            Self::Rejected(_) | Self::NotFound => None,
        }
    }
}

/// Parse a `status` column value (old rows may hold it JSON-quoted).
#[cfg_attr(not(any(feature = "postgres", feature = "mysql")), allow(dead_code))]
pub(crate) fn job_status_from_db(value: &str) -> Option<JobStatus> {
    Some(match value.trim_matches('"') {
        "Pending" => JobStatus::Pending,
        "Running" => JobStatus::Running,
        "Completed" => JobStatus::Completed,
        "Failed" => JobStatus::Failed,
        "Dead" => JobStatus::Dead,
        "TimedOut" => JobStatus::TimedOut,
        "Retrying" => JobStatus::Retrying,
        "Archived" => JobStatus::Archived,
        _ => return None,
    })
}

/// The error for a manual transition whose guarded UPDATE matched no row, given the
/// job's current status (`None` when the job does not exist).
#[cfg_attr(not(any(feature = "postgres", feature = "mysql")), allow(dead_code))]
pub(crate) fn rejection_error(
    job_id: JobId,
    transition: JobTransition,
    current_status: Option<&str>,
) -> crate::HammerworkError {
    match current_status {
        None => crate::HammerworkError::JobNotFound {
            id: job_id.to_string(),
        },
        Some(raw) => match job_status_from_db(raw) {
            Some(status) if !transition.is_allowed_from(status) => {
                transition.rejected(job_id, status)
            }
            // The status was allowed, so the other condition failed.
            _ if transition == JobTransition::RescheduleCron => crate::HammerworkError::Queue {
                message: format!("Job {job_id} is not a recurring job"),
            },
            _ => crate::HammerworkError::InvalidJobTransition {
                job_id: job_id.to_string(),
                status: raw.to_string(),
                transition: transition.as_str().to_string(),
            },
        },
    }
}

/// Error message for a dependent that can no longer run because `failed` failed.
#[cfg_attr(not(any(feature = "postgres", feature = "mysql")), allow(dead_code))]
pub(crate) fn dependency_failed_message(failed: JobId) -> String {
    format!("Dependency failed: job {failed} failed")
}

/// Error message for a pending job failed by a fail-fast workflow.
#[cfg_attr(not(any(feature = "postgres", feature = "mysql")), allow(dead_code))]
pub(crate) fn workflow_failed_message(failed: JobId) -> String {
    format!("Workflow failed: job {failed} failed (fail-fast policy)")
}

/// Error message for a pending job failed by a fail-fast batch.
#[cfg_attr(not(any(feature = "postgres", feature = "mysql")), allow(dead_code))]
pub(crate) fn batch_failed_message(failed: JobId) -> String {
    format!("Batch failed: job {failed} failed (fail-fast mode)")
}

/// The statuses that count as "not finished yet" for workflow and batch progress.
#[cfg_attr(not(any(feature = "postgres", feature = "mysql")), allow(dead_code))]
pub(crate) const UNFINISHED_STATUS_SQL: &str = "'Pending', 'Running', 'Retrying'";

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn job_in(status: JobStatus) -> Job {
        let mut job = Job::new("lifecycle".to_string(), json!({}));
        job.status = status;
        job
    }

    #[test]
    fn terminal_statuses_cannot_be_resurrected_by_manual_transitions() {
        for transition in [
            JobTransition::Complete,
            JobTransition::Fail,
            JobTransition::Retry,
            JobTransition::MarkDead,
            JobTransition::MarkTimedOut,
        ] {
            assert!(!transition.is_allowed_from(JobStatus::Completed));
            assert!(!transition.is_allowed_from(JobStatus::Dead));
            assert!(!transition.is_allowed_from(JobStatus::Archived));
        }
        // The audit's repros: a TimedOut job cannot be completed, a Dead one not retried.
        assert!(!JobTransition::Complete.is_allowed_from(JobStatus::TimedOut));
        assert!(!JobTransition::Retry.is_allowed_from(JobStatus::Dead));
        // ...but the explicit manual paths are allowed.
        assert!(JobTransition::RetryDead.is_allowed_from(JobStatus::Dead));
        assert!(JobTransition::RetryDead.is_allowed_from(JobStatus::TimedOut));
        assert!(JobTransition::Retry.is_allowed_from(JobStatus::Failed));
        assert!(JobTransition::RescheduleCron.is_allowed_from(JobStatus::Dead));
        assert!(!JobTransition::RescheduleCron.is_allowed_from(JobStatus::Completed));
        assert!(JobTransition::MarkTimedOut.is_allowed_from(JobStatus::Running));
        assert!(!JobTransition::MarkTimedOut.is_allowed_from(JobStatus::Pending));
    }

    #[test]
    fn sql_status_list_matches_allowed_from() {
        assert_eq!(JobTransition::MarkTimedOut.sql_status_list(), "'Running'");
        assert_eq!(
            JobTransition::RetryDead.sql_status_list(),
            "'Dead', 'TimedOut'"
        );
    }

    #[test]
    fn rejected_transition_error_names_job_status_and_transition() {
        let id = uuid::Uuid::new_v4();
        let error = JobTransition::Complete.rejected(id, JobStatus::TimedOut);
        let message = error.to_string();
        assert!(message.contains(&id.to_string()));
        assert!(message.contains("TimedOut"));
        assert!(message.contains("completed"));
    }

    #[test]
    fn run_guard_requires_running_same_attempt_and_start() {
        let started = Utc::now();
        let mut run = job_in(JobStatus::Running);
        run.attempts = 2;
        run.started_at = Some(started);

        let mut current = run.clone();
        // Stored with microsecond precision
        current.started_at = Some(started - chrono::Duration::nanoseconds(500));
        assert!(Guard::Run(&run).admits(&current));

        // Reclaimed by the reaper and requeued
        current.status = JobStatus::Pending;
        assert!(!Guard::Run(&run).admits(&current));

        // Picked up again by another worker: a newer attempt
        current.status = JobStatus::Running;
        current.attempts = 3;
        current.started_at = Some(started + chrono::Duration::seconds(30));
        assert!(!Guard::Run(&run).admits(&current));

        // A recurring job's later run with the same attempt number
        current.attempts = 2;
        assert!(!Guard::Run(&run).admits(&current));

        // Terminal states are never overwritten
        for status in [JobStatus::Completed, JobStatus::Dead, JobStatus::TimedOut] {
            let mut finished = run.clone();
            finished.status = status;
            assert!(!Guard::Run(&run).admits(&finished));
        }
    }

    #[test]
    fn manual_guard_uses_allowed_from() {
        let guard = Guard::Manual(JobTransition::Complete);
        assert!(guard.admits(&job_in(JobStatus::Running)));
        assert!(guard.admits(&job_in(JobStatus::Pending)));
        assert!(!guard.admits(&job_in(JobStatus::TimedOut)));
    }

    #[test]
    fn outcome_maps_to_target() {
        let at = Utc::now();
        assert_eq!(Target::from(JobOutcome::Completed), Target::Completed);
        assert_eq!(
            Target::from(JobOutcome::Retry {
                retry_at: at,
                error: "e".into(),
                timed_out: true
            }),
            Target::Retry {
                retry_at: at,
                error: Some("e".into()),
                timed_out: true
            }
        );
        assert_eq!(
            JobOutcome::TimedOut { error: "t".into() }.terminal_status(),
            Some(JobStatus::TimedOut)
        );
        assert_eq!(
            Target::Dead("d".into()).terminal_status(),
            Some(JobStatus::Dead)
        );
    }
}
