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

#![cfg_attr(not(any(feature = "postgres", feature = "mysql")), allow(dead_code))]

use crate::job::{Job, JobId, JobStatus};
use crate::{Result, workflow::FailurePolicy};
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
    /// Child jobs enqueued together with the completion, by
    /// [`DatabaseQueue::complete_job_run_with_children`](super::DatabaseQueue::complete_job_run_with_children).
    pub spawned: Vec<JobId>,
}

impl RecordedOutcome {
    pub(crate) fn new(status: JobStatus) -> Self {
        Self {
            status,
            next_run_at: None,
            unblocked: Vec::new(),
            cancelled: Vec::new(),
            spawned: Vec::new(),
        }
    }
}

/// The new state a transition writes. Shared by manual transitions and run outcomes.
#[derive(Debug, Clone, PartialEq, Eq)]
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

    /// The target as written for a run outcome (`guard` is a [`Guard::Run`]), with
    /// times on the database clock.
    ///
    /// A worker computes `retry_at` as "now + backoff" on its own clock; the dequeue
    /// compares it with the database clock. Keeping the backoff and moving it onto the
    /// database clock (`db_now`) means clock skew between the worker and the database
    /// neither shortens nor lengthens the delay. Manual transitions keep the absolute
    /// time the caller asked for.
    pub fn on_db_clock(&self, guard: &Guard<'_>, db_now: DateTime<Utc>) -> Self {
        match (self, guard) {
            (
                Self::Retry {
                    retry_at,
                    error,
                    timed_out,
                },
                Guard::Run(_),
            ) => {
                let delay = *retry_at - Utc::now();
                Self::Retry {
                    retry_at: db_now.checked_add_signed(delay).unwrap_or(*retry_at),
                    error: error.clone(),
                    timed_out: *timed_out,
                }
            }
            _ => self.clone(),
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
pub(crate) enum Guard<'a> {
    /// A manual transition: any of its allowed source statuses.
    Manual(JobTransition),
    /// A run outcome: the job must still be `Running` the run `run` was dequeued as.
    Run(&'a Job),
}

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
pub(crate) enum TransitionResult {
    Applied(RecordedOutcome),
    /// The guard rejected the job's current status (or run).
    Rejected(JobStatus),
    NotFound,
}

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
pub(crate) fn dependency_failed_message(failed: JobId) -> String {
    format!("Dependency failed: job {failed} failed")
}

/// Error message for a pending job failed by a fail-fast workflow.
pub(crate) fn workflow_failed_message(failed: JobId) -> String {
    format!("Workflow failed: job {failed} failed (fail-fast policy)")
}

/// Error message for a pending job failed by a fail-fast batch.
pub(crate) fn batch_failed_message(failed: JobId) -> String {
    format!("Batch failed: job {failed} failed (fail-fast mode)")
}

/// Error message for a job of a fail-fast batch that a batch-mode worker had claimed
/// but not started when the batch failed.
pub(crate) const BATCH_FAILED_BEFORE_START_MESSAGE: &str =
    "Batch failed (fail-fast mode) before this claimed job started";

/// The statuses that count as "not finished yet" for workflow and batch progress.
pub(crate) const UNFINISHED_STATUS_SQL: &str = "'Pending', 'Running', 'Retrying'";

/// Error message recorded on the unfinished jobs of a cancelled workflow.
pub(crate) const WORKFLOW_CANCELLED_MESSAGE: &str = "Workflow cancelled";

/// How many missed occurrences [`next_cron_run`] steps through before it gives up
/// finding the latest one and schedules the catch-up run at `now`.
const CRON_CATCH_UP_SCAN_LIMIT: usize = 10_000;

/// The next run of a recurring job whose run ended at `now` (by the database clock).
///
/// The next occurrence is computed from the run's scheduled slot (the earlier of
/// `next_run_at` and `scheduled_at`), not from the time the run ended, so a run that
/// ends late does not skip the following slot. Catch-up semantics:
///
/// - the next occurrence after the slot is still in the future: run then;
/// - it has already passed because the run overran: the occurrences that passed while
///   it ran are coalesced into **one** run, scheduled at the latest of them, so it is due
///   immediately. After that run, the job is back on its regular schedule. Missed
///   occurrences never pile up into a burst of runs.
///
/// Occurrences that had already passed when the run *started* (workers were down, or
/// the queue was paused) are covered by that run: it is itself the catch-up run, so the
/// next run is the first occurrence after it started, not another immediate one.
///
/// Returns `None` when the job has no valid cron schedule or no next occurrence.
pub(crate) fn next_cron_run(job: &Job, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let schedule = job.get_cron_schedule()?.ok()?;
    // `next_run_at` is the slot; a retried run has a later `scheduled_at`, and a run
    // started early by hand an earlier one.
    let slot = job
        .next_run_at
        .map_or(job.scheduled_at, |slot| slot.min(job.scheduled_at));
    // A run that started after its slot already covers the occurrences missed before it
    // started; count from the start so they do not trigger a second, immediate run.
    let from = job
        .started_at
        .filter(|started| *started > slot)
        .unwrap_or(slot);
    let next = schedule.next_execution(from)?;
    if next > now {
        return Some(next);
    }
    let mut latest = next;
    for _ in 0..CRON_CATCH_UP_SCAN_LIMIT {
        match schedule.next_execution(latest) {
            Some(occurrence) if occurrence <= now => latest = occurrence,
            _ => return Some(latest),
        }
    }
    // Too many missed occurrences to walk (e.g. a per-second schedule after a long
    // outage): run the catch-up now.
    Some(now)
}

/// The state of a dependency of a job being enqueued, as far as the new job is
/// concerned. See [`settle_new_dependents`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ParentState {
    /// The dependency completed: it no longer blocks.
    Completed,
    /// The dependency failed terminally and its workflow's failure policy fails the
    /// jobs that depend on it.
    Failed,
    /// Anything else: not finished, failed under the `Manual` policy, or unknown.
    Unfinished,
}

impl ParentState {
    /// The state of a stored dependency with job status `status`, in a workflow with
    /// failure policy `policy` (the `failure_policy` column, `None` outside workflows).
    ///
    /// Mirrors what happens when a dependency finishes while its dependents already
    /// exist: completion releases them; a terminal failure fails them unless the
    /// workflow's policy is `Manual`, which leaves them waiting for an operator.
    pub fn from_db(status: &str, policy: Option<&str>) -> Self {
        let manual = policy.is_some_and(|policy| {
            crate::workflow::FailurePolicy::parse_from_db(policy)
                .is_ok_and(|policy| policy == crate::workflow::FailurePolicy::Manual)
        });
        match job_status_from_db(status) {
            Some(JobStatus::Completed) => Self::Completed,
            Some(JobStatus::Failed | JobStatus::Dead | JobStatus::TimedOut) if !manual => {
                Self::Failed
            }
            _ => Self::Unfinished,
        }
    }

    /// The state of an archived dependency. Archived jobs keep no workflow, so a
    /// terminally failed one fails its dependents.
    pub fn from_archived(status: &str) -> Self {
        Self::from_db(status, None)
    }
}

/// The dependency status of a job that is put back to `Pending` by hand
/// ([`retry_job`](super::DatabaseQueue::retry_job) or
/// [`restore_archived_job`](super::DatabaseQueue::restore_archived_job); `action` is
/// "retried" or "restored"), re-evaluated from the current statuses of its
/// dependencies (`states`, from `hammerwork_jobs` or else the archive):
///
/// - no dependencies: `none`;
/// - every dependency `Completed`: `satisfied`;
/// - a dependency that failed terminally (`Failed`, `Dead`, `TimedOut`, which includes a
///   cancelled workflow's jobs) or that no longer exists: an error, because the job
///   could never run;
/// - otherwise: `waiting`, released when the remaining dependencies complete.
pub(crate) fn reevaluate_dependencies(
    job: &Job,
    states: &std::collections::HashMap<JobId, JobStatus>,
    action: &str,
) -> Result<crate::workflow::DependencyStatus> {
    use crate::workflow::DependencyStatus;

    if job.depends_on.is_empty() {
        return Ok(DependencyStatus::None);
    }
    let mut all_completed = true;
    for dependency in &job.depends_on {
        let problem = match states.get(dependency) {
            Some(JobStatus::Completed) => continue,
            Some(status @ (JobStatus::Failed | JobStatus::Dead | JobStatus::TimedOut)) => {
                format!("is {}", status.as_str())
            }
            Some(_) => {
                all_completed = false;
                continue;
            }
            None => "no longer exists".to_string(),
        };
        return Err(crate::HammerworkError::Workflow {
            message: format!(
                "Job {} cannot be {action}: its dependency {dependency} {problem}; retry or \
                 restore the dependency first",
                job.id
            ),
        });
    }
    Ok(if all_completed {
        DependencyStatus::Satisfied
    } else {
        DependencyStatus::Waiting
    })
}

/// The dependencies of `jobs` (about to be inserted) that are not themselves in `jobs`.
pub(crate) fn external_dependencies(jobs: &[Job]) -> Vec<JobId> {
    let new: std::collections::HashSet<JobId> = jobs.iter().map(|job| job.id).collect();
    let mut parents: Vec<JobId> = jobs
        .iter()
        .filter(|job| job.dependency_status == crate::workflow::DependencyStatus::Waiting)
        .flat_map(|job| job.depends_on.iter().copied())
        .filter(|id| !new.contains(id))
        .collect();
    parents.sort_unstable();
    parents.dedup();
    parents
}

/// Settle the dependency state of jobs about to be inserted whose dependencies may
/// already have finished.
///
/// Dependents are normally released (or failed) when a dependency finishes, which
/// only reaches jobs that already exist. A job enqueued after its dependencies
/// finished would otherwise wait forever. Given the (locked) states of the
/// dependencies outside `jobs`:
///
/// - a waiting job whose dependencies all completed becomes `satisfied`;
/// - a waiting job with a failed dependency is inserted as `Failed` (dependency status
///   `failed`), and so are the jobs in `jobs` that (transitively) depend on it;
/// - everything else keeps waiting.
///
/// Returns the ids of the jobs that were failed.
pub(crate) fn settle_new_dependents(
    jobs: &mut [Job],
    parents: &std::collections::HashMap<JobId, ParentState>,
    now: DateTime<Utc>,
) -> Vec<JobId> {
    use crate::workflow::DependencyStatus;

    let index: std::collections::HashMap<JobId, usize> = jobs
        .iter()
        .enumerate()
        .map(|(i, job)| (job.id, i))
        .collect();
    let mut failed = Vec::new();

    // Failures propagate along dependency chains inside `jobs`, so repeat until nothing
    // changes; each pass fails at least one more job or ends the loop.
    loop {
        let mut changed = false;
        for i in 0..jobs.len() {
            let job = &jobs[i];
            if job.dependency_status != DependencyStatus::Waiting || job.depends_on.is_empty() {
                continue;
            }
            let state_of = |parent: &JobId| match index.get(parent) {
                Some(&j) => match jobs[j].status {
                    JobStatus::Completed => ParentState::Completed,
                    JobStatus::Failed | JobStatus::Dead | JobStatus::TimedOut => {
                        ParentState::Failed
                    }
                    _ => ParentState::Unfinished,
                },
                None => parents
                    .get(parent)
                    .copied()
                    .unwrap_or(ParentState::Unfinished),
            };
            if let Some(&failed_parent) = job
                .depends_on
                .iter()
                .find(|parent| state_of(parent) == ParentState::Failed)
            {
                let job = &mut jobs[i];
                job.status = JobStatus::Failed;
                job.dependency_status = DependencyStatus::Failed;
                job.failed_at = Some(now);
                job.error_message = Some(dependency_failed_message(failed_parent));
                failed.push(job.id);
                changed = true;
            } else if job
                .depends_on
                .iter()
                .all(|parent| state_of(parent) == ParentState::Completed)
            {
                jobs[i].dependency_status = DependencyStatus::Satisfied;
            }
        }
        if !changed {
            return failed;
        }
    }
}

/// The locked `hammerwork_workflows` row of a job's workflow.
pub(crate) struct LockedWorkflow {
    pub id: crate::workflow::WorkflowId,
    pub policy: FailurePolicy,
    pub status: crate::workflow::WorkflowStatus,
    pub total_jobs: usize,
}

/// The statements a database backend runs for a job lifecycle transition.
///
/// The transition itself (lock the job, check the guard, write the target, then apply
/// the side effects of a terminal status) is backend-independent and implemented once,
/// by [`transition_in_tx`] and the functions it calls; each backend implements these
/// statements in its SQL dialect. Every statement runs on the transaction's connection.
pub(crate) trait TransitionStore: Sync {
    /// The connection a transition's statements run on.
    type Conn: Send;

    /// Apply `target` to a job if `guard` admits its current state, together with the
    /// side effects of the new state, in one transaction (retried on conflicts).
    fn transition_job(
        &self,
        job_id: JobId,
        guard: Guard<'_>,
        target: &Target,
        reschedule_recurring: bool,
    ) -> impl Future<Output = Result<TransitionResult>> + Send;

    /// The job `job_id`, locked for update (`None` if it does not exist).
    fn lock_job(
        conn: &mut Self::Conn,
        job_id: JobId,
    ) -> impl Future<Output = Result<Option<Job>>> + Send;

    /// The current time by the database clock.
    fn db_now(conn: &mut Self::Conn) -> impl Future<Output = Result<DateTime<Utc>>> + Send;

    /// Write the new status and its timestamps. Clears the lease: the run is over.
    fn write_target(
        conn: &mut Self::Conn,
        job_id: JobId,
        target: &Target,
        now: DateTime<Utc>,
    ) -> impl Future<Output = Result<()>> + Send;

    /// Whether `next_run_at` can be stored as a recurring job's next run.
    fn can_schedule_at(_next_run_at: DateTime<Utc>) -> bool {
        true
    }

    /// Put a recurring job back to `Pending` for its next run. A failed run's error and
    /// failure time stay visible until the next run; a successful run clears them.
    fn reschedule_after_run(
        conn: &mut Self::Conn,
        job_id: JobId,
        next_run_at: DateTime<Utc>,
        run_status: JobStatus,
    ) -> impl Future<Output = Result<()>> + Send;

    /// The statuses of the jobs `ids`, read from `hammerwork_jobs` (locked `FOR SHARE`)
    /// or else from the archive (the status they were archived with). Missing jobs are
    /// left out.
    fn dependency_states(
        conn: &mut Self::Conn,
        ids: &[JobId],
    ) -> impl Future<Output = Result<std::collections::HashMap<JobId, JobStatus>>> + Send;

    /// Set the `dependency_status` of job `job_id`.
    fn write_dependency_status(
        conn: &mut Self::Conn,
        job_id: JobId,
        status: crate::workflow::DependencyStatus,
    ) -> impl Future<Output = Result<()>> + Send;

    /// Lock a workflow's row (`None` if it does not exist).
    fn lock_workflow(
        conn: &mut Self::Conn,
        workflow_id: crate::workflow::WorkflowId,
    ) -> impl Future<Output = Result<Option<LockedWorkflow>>> + Send;

    /// Mark `waiting` dependents of `completed_job_id` whose dependencies have all
    /// completed as `satisfied`, so they can be dequeued. Returns them.
    fn resolve_dependents(
        conn: &mut Self::Conn,
        completed_job_id: JobId,
    ) -> impl Future<Output = Result<Vec<JobId>>> + Send;

    /// Fail every pending job that (transitively) depends on `failed_job_id`.
    fn fail_dependents(
        conn: &mut Self::Conn,
        failed_job_id: JobId,
        now: DateTime<Utc>,
    ) -> impl Future<Output = Result<Vec<JobId>>> + Send;

    /// Fail the pending jobs of a fail-fast workflow after `failed_job_id` failed.
    fn fail_workflow_pending(
        conn: &mut Self::Conn,
        workflow_id: crate::workflow::WorkflowId,
        failed_job_id: JobId,
        now: DateTime<Utc>,
    ) -> impl Future<Output = Result<Vec<JobId>>> + Send;

    /// Recompute a workflow's counters and status from its jobs.
    fn refresh_workflow(
        conn: &mut Self::Conn,
        workflow: &LockedWorkflow,
        now: DateTime<Utc>,
    ) -> impl Future<Output = Result<()>> + Send;

    /// Apply a batch job's terminal `status` to its batch: a `FailFast` batch fails its
    /// pending jobs on the first failure, and the batch's counters and status are
    /// persisted once the batch has finished (or failed fast). Returns the jobs failed.
    fn apply_batch_outcome(
        conn: &mut Self::Conn,
        batch_id: crate::batch::BatchId,
        job_id: JobId,
        status: JobStatus,
        now: DateTime<Utc>,
    ) -> impl Future<Output = Result<Vec<JobId>>> + Send;
}

/// Apply a manual transition (see [`JobTransition`]) to `target`.
pub(crate) async fn manual_transition<S: TransitionStore>(
    store: &S,
    job_id: JobId,
    transition: JobTransition,
    target: Target,
) -> Result<()> {
    store
        .transition_job(job_id, Guard::Manual(transition), &target, false)
        .await?
        .into_manual(job_id, transition)?;
    Ok(())
}

/// Record the outcome of the run `run` ([`DatabaseQueue::finish_job_run`]).
///
/// [`DatabaseQueue::finish_job_run`]: super::DatabaseQueue::finish_job_run
pub(crate) async fn finish_run<S: TransitionStore>(
    store: &S,
    run: &Job,
    outcome: JobOutcome,
) -> Result<Option<RecordedOutcome>> {
    let target = Target::from(outcome);
    Ok(store
        .transition_job(run.id, Guard::Run(run), &target, true)
        .await?
        .into_run())
}

/// Apply `target` to a job if `guard` admits its current state, together with the side
/// effects of the new state, inside the caller's transaction.
pub(crate) async fn transition_in_tx<S: TransitionStore>(
    conn: &mut S::Conn,
    job_id: JobId,
    guard: Guard<'_>,
    target: &Target,
    reschedule_recurring: bool,
) -> Result<TransitionResult> {
    let Some(job) = S::lock_job(conn, job_id).await? else {
        return Ok(TransitionResult::NotFound);
    };
    if !guard.admits(&job) {
        return Ok(TransitionResult::Rejected(job.status));
    }

    // A job retried by hand may have been failed because a dependency failed (its
    // dependency status is then `failed` and it would never be dequeued again):
    // re-evaluate its dependencies, refusing when one of them can no longer complete.
    if let (Guard::Manual(_), Target::Retry { .. }) = (&guard, target)
        && !job.depends_on.is_empty()
    {
        let states = S::dependency_states(conn, &job.depends_on).await?;
        let status = reevaluate_dependencies(&job, &states, "retried")?;
        if status != job.dependency_status {
            S::write_dependency_status(conn, job_id, status).await?;
        }
    }

    let now = S::db_now(conn).await?;
    let target = target.on_db_clock(&guard, now);
    S::write_target(conn, job_id, &target, now).await?;
    let recorded = match target.terminal_status() {
        Some(status) => after_terminal::<S>(conn, &job, status, now, reschedule_recurring).await?,
        None => RecordedOutcome::new(JobStatus::Pending),
    };
    Ok(TransitionResult::Applied(recorded))
}

/// Side effects of `job` reaching the terminal `status`.
///
/// A recurring job (when `reschedule_recurring`) is rescheduled for its next run
/// instead. Otherwise dependents are resolved or failed, the workflow policy and
/// counters are applied, and the batch's failure mode and status.
pub(crate) async fn after_terminal<S: TransitionStore>(
    conn: &mut S::Conn,
    job: &Job,
    status: JobStatus,
    now: DateTime<Utc>,
    reschedule_recurring: bool,
) -> Result<RecordedOutcome> {
    if reschedule_recurring && job.recurring {
        if let Some(next_run_at) = next_cron_run(job, now).filter(|at| S::can_schedule_at(*at)) {
            S::reschedule_after_run(conn, job.id, next_run_at, status).await?;
            let mut recorded = RecordedOutcome::new(JobStatus::Pending);
            recorded.next_run_at = Some(next_run_at);
            return Ok(recorded);
        }
        tracing::warn!(
            "Could not calculate the next run time for recurring job {}; it stays {}",
            job.id,
            status.as_str()
        );
    }

    let mut recorded = RecordedOutcome::new(status);

    // Lock the workflow row first: it serializes terminal transitions within a
    // workflow, so concurrent completions cannot both miss each other.
    let workflow = match job.workflow_id {
        Some(id) => S::lock_workflow(conn, id).await?,
        None => None,
    };

    if status == JobStatus::Completed {
        recorded.unblocked = S::resolve_dependents(conn, job.id).await?;
    } else {
        recorded.cancelled =
            apply_failure_policy::<S>(conn, workflow.as_ref(), job.id, now).await?;
    }

    if let Some(workflow) = &workflow {
        S::refresh_workflow(conn, workflow, now).await?;
    }
    if let Some(batch_id) = job.batch_id {
        let cancelled = S::apply_batch_outcome(conn, batch_id, job.id, status, now).await?;
        recorded.cancelled.extend(cancelled);
    }
    Ok(recorded)
}

/// Apply the [`FailurePolicy`] of `workflow` (the failed job's locked workflow, if any)
/// to the failure of `failed_job_id`. Returns the jobs that were failed.
///
/// - `FailFast` fails the workflow's other `Pending` and `Retrying` jobs, then the jobs
///   that (transitively) depend on the failed one;
/// - `ContinueOnFailure`, and jobs with dependencies outside any workflow, fail only
///   the jobs that (transitively) depend on the failed one;
/// - `Manual` leaves the dependents waiting for an operator.
async fn apply_failure_policy<S: TransitionStore>(
    conn: &mut S::Conn,
    workflow: Option<&LockedWorkflow>,
    failed_job_id: JobId,
    now: DateTime<Utc>,
) -> Result<Vec<JobId>> {
    match workflow {
        Some(w) if w.policy == FailurePolicy::Manual => Ok(Vec::new()),
        Some(w) if w.policy == FailurePolicy::FailFast => {
            let mut failed = S::fail_workflow_pending(conn, w.id, failed_job_id, now).await?;
            for id in S::fail_dependents(conn, failed_job_id, now).await? {
                if !failed.contains(&id) {
                    failed.push(id);
                }
            }
            Ok(failed)
        }
        _ => S::fail_dependents(conn, failed_job_id, now).await,
    }
}

/// [`DatabaseQueue::fail_job_dependencies`] inside the caller's transaction, once the
/// failed job is locked: lock its workflow (`workflow_id`, in the same order as a
/// terminal transition), apply the workflow's failure policy and update the workflow's
/// counters and status.
///
/// [`DatabaseQueue::fail_job_dependencies`]: super::DatabaseQueue::fail_job_dependencies
pub(crate) async fn fail_job_dependencies<S: TransitionStore>(
    conn: &mut S::Conn,
    failed_job_id: JobId,
    workflow_id: Option<crate::workflow::WorkflowId>,
) -> Result<Vec<JobId>> {
    let workflow = match workflow_id {
        Some(id) => S::lock_workflow(conn, id).await?,
        None => None,
    };
    let now = S::db_now(conn).await?;
    let failed = apply_failure_policy::<S>(conn, workflow.as_ref(), failed_job_id, now).await?;
    if let Some(workflow) = &workflow {
        S::refresh_workflow(conn, workflow, now).await?;
    }
    Ok(failed)
}

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

    fn daily_job(slot: DateTime<Utc>) -> Job {
        let mut job = job_in(JobStatus::Running);
        job.cron_schedule = Some("0 0 0 * * *".to_string());
        job.recurring = true;
        job.scheduled_at = slot;
        job.next_run_at = Some(slot);
        job
    }

    fn at(rfc3339: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(rfc3339).unwrap().into()
    }

    #[test]
    fn next_cron_run_is_computed_from_the_slot() {
        // On time: the next slot
        let job = daily_job(at("2026-03-10T00:00:00Z"));
        assert_eq!(
            next_cron_run(&job, at("2026-03-10T00:05:00Z")),
            Some(at("2026-03-11T00:00:00Z"))
        );
        // Overran into the next slot: that slot still runs, immediately
        assert_eq!(
            next_cron_run(&job, at("2026-03-11T00:30:00Z")),
            Some(at("2026-03-11T00:00:00Z"))
        );
        // Days behind with no recorded start: one catch-up run at the latest missed
        // slot, no pile-up (a worker's run always has a start; see below)
        assert_eq!(
            next_cron_run(&job, at("2026-03-15T12:00:00Z")),
            Some(at("2026-03-15T00:00:00Z"))
        );
        // A retried run keeps its slot
        let mut job = daily_job(at("2026-03-10T00:00:00Z"));
        job.scheduled_at = at("2026-03-10T00:03:00Z");
        assert_eq!(
            next_cron_run(&job, at("2026-03-10T00:05:00Z")),
            Some(at("2026-03-11T00:00:00Z"))
        );
        // A run started early by hand does not skip the upcoming slot
        let mut job = daily_job(at("2026-03-11T00:00:00Z"));
        job.scheduled_at = at("2026-03-10T12:00:00Z");
        assert_eq!(
            next_cron_run(&job, at("2026-03-10T12:05:00Z")),
            Some(at("2026-03-11T00:00:00Z"))
        );
        // Old rows without next_run_at use scheduled_at
        let mut job = daily_job(at("2026-03-10T00:00:00Z"));
        job.next_run_at = None;
        assert_eq!(
            next_cron_run(&job, at("2026-03-10T00:05:00Z")),
            Some(at("2026-03-11T00:00:00Z"))
        );
        // No schedule
        let mut job = daily_job(at("2026-03-10T00:00:00Z"));
        job.cron_schedule = None;
        assert_eq!(next_cron_run(&job, at("2026-03-10T00:05:00Z")), None);
    }

    #[test]
    fn a_late_run_is_the_catch_up_run() {
        // Workers were down for days; the run for the stale slot starts late and is the
        // catch-up, so the next run is the next regular slot, not another one at once.
        let mut job = daily_job(at("2026-03-10T00:00:00Z"));
        job.started_at = Some(at("2026-03-15T12:00:00Z"));
        assert_eq!(
            next_cron_run(&job, at("2026-03-15T12:00:01Z")),
            Some(at("2026-03-16T00:00:00Z"))
        );
        // A per-five-seconds schedule picked up 30 seconds late runs once, then resumes.
        let mut job = daily_job(at("2026-03-10T00:00:00Z"));
        job.cron_schedule = Some("*/5 * * * * *".to_string());
        job.started_at = Some(at("2026-03-10T00:00:30.950Z"));
        assert_eq!(
            next_cron_run(&job, at("2026-03-10T00:00:30.960Z")),
            Some(at("2026-03-10T00:00:35Z"))
        );
        // An occurrence that passes while the run is still going still gets its run.
        let mut job = daily_job(at("2026-03-10T00:00:00Z"));
        job.started_at = Some(at("2026-03-10T00:00:01Z"));
        assert_eq!(
            next_cron_run(&job, at("2026-03-11T00:30:00Z")),
            Some(at("2026-03-11T00:00:00Z"))
        );
        // A run that starts on time behaves as before.
        let mut job = daily_job(at("2026-03-10T00:00:00Z"));
        job.started_at = Some(at("2026-03-10T00:00:00.040Z"));
        assert_eq!(
            next_cron_run(&job, at("2026-03-10T00:05:00Z")),
            Some(at("2026-03-11T00:00:00Z"))
        );
    }

    #[test]
    fn next_cron_run_bounds_the_catch_up_scan() {
        let mut job = daily_job(at("2026-03-10T00:00:00Z"));
        job.cron_schedule = Some("* * * * * *".to_string());
        let now = at("2026-03-20T00:00:00Z");
        assert_eq!(next_cron_run(&job, now), Some(now));
    }

    #[test]
    fn parent_state_follows_status_and_policy() {
        assert_eq!(
            ParentState::from_db("Completed", None),
            ParentState::Completed
        );
        assert_eq!(ParentState::from_db("Dead", None), ParentState::Failed);
        assert_eq!(
            ParentState::from_db("TimedOut", Some("fail_fast")),
            ParentState::Failed
        );
        assert_eq!(
            ParentState::from_db("Failed", Some("manual")),
            ParentState::Unfinished
        );
        assert_eq!(
            ParentState::from_db("Running", None),
            ParentState::Unfinished
        );
        assert_eq!(
            ParentState::from_archived("\"Completed\""),
            ParentState::Completed
        );
    }

    /// #102: a job put back to `Pending` by hand gets its dependency status from its
    /// dependencies' current statuses, and is refused when one can never complete.
    #[test]
    fn retried_jobs_reevaluate_their_dependencies() {
        use crate::workflow::DependencyStatus;
        use std::collections::HashMap;

        let (a, b) = (uuid::Uuid::new_v4(), uuid::Uuid::new_v4());
        let mut job = job_in(JobStatus::Failed);
        assert_eq!(
            reevaluate_dependencies(&job, &HashMap::new(), "retried").unwrap(),
            DependencyStatus::None
        );

        job.depends_on = vec![a, b];
        job.dependency_status = DependencyStatus::Failed;
        let states = |sa, sb| HashMap::from([(a, sa), (b, sb)]);
        assert_eq!(
            reevaluate_dependencies(
                &job,
                &states(JobStatus::Completed, JobStatus::Completed),
                "retried"
            )
            .unwrap(),
            DependencyStatus::Satisfied
        );
        for unfinished in [JobStatus::Pending, JobStatus::Running, JobStatus::Retrying] {
            assert_eq!(
                reevaluate_dependencies(&job, &states(JobStatus::Completed, unfinished), "x")
                    .unwrap(),
                DependencyStatus::Waiting
            );
        }
        for failed in [JobStatus::Failed, JobStatus::Dead, JobStatus::TimedOut] {
            let error =
                reevaluate_dependencies(&job, &states(JobStatus::Pending, failed), "restored")
                    .unwrap_err()
                    .to_string();
            assert!(error.contains("cannot be restored"), "{error}");
            assert!(error.contains(&b.to_string()), "{error}");
        }
        let missing = HashMap::from([(a, JobStatus::Completed)]);
        let error = reevaluate_dependencies(&job, &missing, "retried")
            .unwrap_err()
            .to_string();
        assert!(error.contains("no longer exists"), "{error}");
    }

    #[test]
    fn new_dependents_are_settled_from_their_dependencies() {
        use crate::workflow::DependencyStatus;
        use std::collections::HashMap;

        let (done, failed, running) = (
            uuid::Uuid::new_v4(),
            uuid::Uuid::new_v4(),
            uuid::Uuid::new_v4(),
        );
        let parents = HashMap::from([
            (done, ParentState::Completed),
            (failed, ParentState::Failed),
            (running, ParentState::Unfinished),
        ]);
        let new = |deps: Vec<JobId>| Job::new("q".to_string(), json!({})).depends_on_jobs(&deps);

        let satisfied = new(vec![done]);
        let waiting = new(vec![done, running]);
        let doomed = new(vec![done, failed]);
        // Depends on `doomed`, which is in the same insert
        let downstream = new(vec![doomed.id]);
        let mut jobs = vec![downstream, satisfied, waiting, doomed];
        let now = Utc::now();
        let mut external = vec![done, failed, running];
        external.sort_unstable();
        assert_eq!(external_dependencies(&jobs), external);

        let failed_ids = settle_new_dependents(&mut jobs, &parents, now);

        assert_eq!(jobs[1].dependency_status, DependencyStatus::Satisfied);
        assert_eq!(jobs[1].status, JobStatus::Pending);
        assert_eq!(jobs[2].dependency_status, DependencyStatus::Waiting);
        for job in [&jobs[3], &jobs[0]] {
            assert_eq!(job.status, JobStatus::Failed);
            assert_eq!(job.dependency_status, DependencyStatus::Failed);
            assert_eq!(job.failed_at, Some(now));
        }
        assert_eq!(
            jobs[3].error_message.as_deref(),
            Some(dependency_failed_message(failed).as_str())
        );
        assert_eq!(failed_ids.len(), 2);
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
