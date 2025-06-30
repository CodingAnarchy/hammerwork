//! Workflow and job dependency management.
//!
//! This module provides the types and logic for managing job dependencies and workflows.
//! Jobs can depend on other jobs completing successfully before they are executed,
//! enabling complex data pipelines and business workflows.

use crate::{HammerworkError, Job, JobId, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

/// Unique identifier for a workflow.
pub type WorkflowId = Uuid;

/// Status of job dependency resolution.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum DependencyStatus {
    /// Job has no dependencies
    None,
    /// Job is waiting for dependencies to complete
    Waiting,
    /// All dependencies have been satisfied
    Satisfied,
    /// One or more dependencies failed
    Failed,
}

impl Default for DependencyStatus {
    fn default() -> Self {
        Self::None
    }
}

impl DependencyStatus {
    /// Convert to string for database storage
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Waiting => "waiting",
            Self::Satisfied => "satisfied",
            Self::Failed => "failed",
        }
    }

    /// Parse from string from database
    pub fn parse_from_db(s: &str) -> Result<Self> {
        match s {
            "none" => Ok(Self::None),
            "waiting" => Ok(Self::Waiting),
            "satisfied" => Ok(Self::Satisfied),
            "failed" => Ok(Self::Failed),
            _ => Err(HammerworkError::Workflow {
                message: format!("Invalid dependency status: {}", s),
            }),
        }
    }
}

/// Status of a workflow execution.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum WorkflowStatus {
    /// Workflow is currently running
    Running,
    /// Workflow completed successfully
    Completed,
    /// Workflow failed
    Failed,
    /// Workflow was cancelled
    Cancelled,
}

impl Default for WorkflowStatus {
    fn default() -> Self {
        Self::Running
    }
}

impl WorkflowStatus {
    /// Convert to string for database storage
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    /// Parse from string from database
    pub fn parse_from_db(s: &str) -> Result<Self> {
        match s {
            "running" => Ok(Self::Running),
            "completed" => Ok(Self::Completed),
            "failed" => Ok(Self::Failed),
            "cancelled" => Ok(Self::Cancelled),
            _ => Err(HammerworkError::Workflow {
                message: format!("Invalid workflow status: {}", s),
            }),
        }
    }
}

/// Policy for handling failures in workflows.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum FailurePolicy {
    /// Stop the entire workflow when any job fails
    FailFast,
    /// Continue executing jobs that don't depend on failed jobs
    ContinueOnFailure,
    /// Require manual intervention to decide how to handle failures
    Manual,
}

impl Default for FailurePolicy {
    fn default() -> Self {
        Self::FailFast
    }
}

impl FailurePolicy {
    /// Convert to string for database storage
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::FailFast => "fail_fast",
            Self::ContinueOnFailure => "continue_on_failure",
            Self::Manual => "manual",
        }
    }

    /// Parse from string from database
    pub fn parse_from_db(s: &str) -> Result<Self> {
        match s {
            "fail_fast" => Ok(Self::FailFast),
            "continue_on_failure" => Ok(Self::ContinueOnFailure),
            "manual" => Ok(Self::Manual),
            _ => Err(HammerworkError::Workflow {
                message: format!("Invalid failure policy: {}", s),
            }),
        }
    }
}

/// A collection of jobs with dependency relationships and execution policies.
///
/// JobGroup provides workflow orchestration capabilities, allowing you to:
/// - Define sequential job chains where jobs depend on previous jobs
/// - Create parallel job groups that synchronize at barrier jobs
/// - Configure failure handling policies for complex workflows
///
/// # Examples
///
/// ## Sequential Pipeline
///
/// ```rust
/// use hammerwork::workflow::JobGroup;
/// use hammerwork::Job;
/// use serde_json::json;
///
/// let job1 = Job::new("process_data".to_string(), json!({"step": 1}));
/// let job2 = Job::new("transform_data".to_string(), json!({"step": 2}));
/// let job3 = Job::new("export_data".to_string(), json!({"step": 3}));
///
/// let workflow = JobGroup::new("data_pipeline")
///     .add_job(job1)
///     .then(job2)
///     .then(job3);
/// ```
///
/// ## Parallel Processing with Synchronization
///
/// ```rust
/// use hammerwork::workflow::JobGroup;
/// use hammerwork::Job;
/// use serde_json::json;
///
/// let job_a = Job::new("process_a".to_string(), json!({"data": "a"}));
/// let job_b = Job::new("process_b".to_string(), json!({"data": "b"}));
/// let job_c = Job::new("process_c".to_string(), json!({"data": "c"}));
/// let final_job = Job::new("combine_results".to_string(), json!({}));
///
/// let workflow = JobGroup::new("parallel_pipeline")
///     .add_parallel_jobs(vec![job_a, job_b, job_c])
///     .then(final_job);
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobGroup {
    /// Unique identifier for this workflow
    pub id: WorkflowId,
    /// Human-readable name for the workflow
    pub name: String,
    /// Current status of the workflow
    pub status: WorkflowStatus,
    /// When the workflow was created
    pub created_at: DateTime<Utc>,
    /// When the workflow completed (if it has)
    pub completed_at: Option<DateTime<Utc>>,
    /// When the workflow failed (if it has)
    pub failed_at: Option<DateTime<Utc>>,
    /// Policy for handling job failures
    pub failure_policy: FailurePolicy,
    /// Jobs in this workflow
    pub jobs: Vec<Job>,
    /// Dependency graph: job_id -> Vec<dependency_job_ids>
    pub dependencies: HashMap<JobId, Vec<JobId>>,
    /// Statistics about workflow execution
    pub total_jobs: usize,
    pub completed_jobs: usize,
    pub failed_jobs: usize,
    /// Additional metadata for the workflow
    pub metadata: serde_json::Value,
}

impl JobGroup {
    /// Creates a new job group with the given name.
    ///
    /// # Arguments
    ///
    /// * `name` - Human-readable name for the workflow
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::workflow::JobGroup;
    ///
    /// let workflow = JobGroup::new("data_processing_pipeline");
    /// assert_eq!(workflow.name, "data_processing_pipeline");
    /// ```
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            name: name.into(),
            status: WorkflowStatus::Running,
            created_at: Utc::now(),
            completed_at: None,
            failed_at: None,
            failure_policy: FailurePolicy::default(),
            jobs: Vec::new(),
            dependencies: HashMap::new(),
            total_jobs: 0,
            completed_jobs: 0,
            failed_jobs: 0,
            metadata: serde_json::Value::Object(serde_json::Map::new()),
        }
    }

    /// Adds a single job to the workflow.
    ///
    /// The job will have no dependencies unless explicitly set later.
    ///
    /// # Arguments
    ///
    /// * `job` - Job to add to the workflow
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::{workflow::JobGroup, Job};
    /// use serde_json::json;
    ///
    /// let job = Job::new("process_data".to_string(), json!({"data": "test"}));
    /// let workflow = JobGroup::new("test_workflow").add_job(job);
    /// ```
    pub fn add_job(mut self, mut job: Job) -> Self {
        job.workflow_id = Some(self.id);
        job.workflow_name = Some(self.name.clone());
        self.jobs.push(job);
        self.total_jobs = self.jobs.len();
        self
    }

    /// Adds multiple jobs that can run in parallel.
    ///
    /// All jobs in the vector will be added with no dependencies between them,
    /// allowing them to execute concurrently.
    ///
    /// # Arguments
    ///
    /// * `jobs` - Vector of jobs to run in parallel
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::{workflow::JobGroup, Job};
    /// use serde_json::json;
    ///
    /// let jobs = vec![
    ///     Job::new("process_a".to_string(), json!({"data": "a"})),
    ///     Job::new("process_b".to_string(), json!({"data": "b"})),
    ///     Job::new("process_c".to_string(), json!({"data": "c"})),
    /// ];
    ///
    /// let workflow = JobGroup::new("parallel_workflow")
    ///     .add_parallel_jobs(jobs);
    /// ```
    pub fn add_parallel_jobs(mut self, jobs: Vec<Job>) -> Self {
        for mut job in jobs {
            job.workflow_id = Some(self.id);
            job.workflow_name = Some(self.name.clone());
            self.jobs.push(job);
        }
        self.total_jobs = self.jobs.len();
        self
    }

    /// Adds a job that depends on all previously added jobs.
    ///
    /// This creates a synchronization point where the new job will only
    /// execute after all existing jobs in the workflow have completed.
    ///
    /// # Arguments
    ///
    /// * `job` - Job that should wait for all previous jobs
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::{workflow::JobGroup, Job};
    /// use serde_json::json;
    ///
    /// let job1 = Job::new("step1".to_string(), json!({}));
    /// let job2 = Job::new("step2".to_string(), json!({}));
    /// let final_job = Job::new("final".to_string(), json!({}));
    ///
    /// let workflow = JobGroup::new("sequential_workflow")
    ///     .add_job(job1)
    ///     .add_job(job2)
    ///     .then(final_job); // Waits for job1 AND job2
    /// ```
    pub fn then(mut self, mut job: Job) -> Self {
        let dependencies: Vec<JobId> = self.jobs.iter().map(|j| j.id).collect();

        job.workflow_id = Some(self.id);
        job.workflow_name = Some(self.name.clone());
        job.depends_on = dependencies.clone();
        job.dependency_status = if dependencies.is_empty() {
            DependencyStatus::None
        } else {
            DependencyStatus::Waiting
        };

        let job_id = job.id;
        self.jobs.push(job);
        self.dependencies.insert(job_id, dependencies);
        self.total_jobs = self.jobs.len();
        self
    }

    /// Sets the failure policy for this workflow.
    ///
    /// # Arguments
    ///
    /// * `policy` - How to handle job failures in this workflow
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::{workflow::{JobGroup, FailurePolicy}};
    ///
    /// let workflow = JobGroup::new("resilient_workflow")
    ///     .with_failure_policy(FailurePolicy::ContinueOnFailure);
    /// ```
    pub fn with_failure_policy(mut self, policy: FailurePolicy) -> Self {
        self.failure_policy = policy;
        self
    }

    /// Sets custom metadata for this workflow.
    ///
    /// # Arguments
    ///
    /// * `metadata` - JSON metadata to associate with the workflow
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::workflow::JobGroup;
    /// use serde_json::json;
    ///
    /// let workflow = JobGroup::new("tracked_workflow")
    ///     .with_metadata(json!({
    ///         "owner": "data_team",
    ///         "environment": "production"
    ///     }));
    /// ```
    pub fn with_metadata(mut self, metadata: serde_json::Value) -> Self {
        self.metadata = metadata;
        self
    }

    /// Validates the workflow's dependency graph.
    ///
    /// Checks for circular dependencies and ensures all referenced job IDs exist.
    ///
    /// # Returns
    ///
    /// `Ok(())` if the workflow is valid, `Err` if there are issues.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::{workflow::JobGroup, Job};
    /// use serde_json::json;
    ///
    /// let job1 = Job::new("step1".to_string(), json!({}));
    /// let job2 = Job::new("step2".to_string(), json!({}));
    ///
    /// let workflow = JobGroup::new("valid_workflow")
    ///     .add_job(job1)
    ///     .then(job2);
    ///
    /// assert!(workflow.validate().is_ok());
    /// ```
    pub fn validate(&self) -> Result<()> {
        let job_ids: HashSet<JobId> = self.jobs.iter().map(|j| j.id).collect();

        // Check that all dependency references are valid
        for job in &self.jobs {
            for dep_id in &job.depends_on {
                if !job_ids.contains(dep_id) {
                    return Err(HammerworkError::Workflow {
                        message: format!("Job {} depends on non-existent job {}", job.id, dep_id),
                    });
                }
            }
        }

        // Check for circular dependencies using topological sort
        self.detect_cycles()?;

        Ok(())
    }

    /// Detects circular dependencies in the workflow using DFS.
    fn detect_cycles(&self) -> Result<()> {
        let mut visited = HashSet::new();
        let mut rec_stack = HashSet::new();

        for job in &self.jobs {
            if !visited.contains(&job.id)
                && self.has_cycle_dfs(job.id, &mut visited, &mut rec_stack)?
            {
                return Err(HammerworkError::Workflow {
                    message: "Circular dependency detected in workflow".to_string(),
                });
            }
        }

        Ok(())
    }

    /// DFS helper for cycle detection.
    fn has_cycle_dfs(
        &self,
        job_id: JobId,
        visited: &mut HashSet<JobId>,
        rec_stack: &mut HashSet<JobId>,
    ) -> Result<bool> {
        visited.insert(job_id);
        rec_stack.insert(job_id);

        // Find the job and check its dependencies
        if let Some(job) = self.jobs.iter().find(|j| j.id == job_id) {
            for &dep_id in &job.depends_on {
                if !visited.contains(&dep_id) {
                    if self.has_cycle_dfs(dep_id, visited, rec_stack)? {
                        return Ok(true);
                    }
                } else if rec_stack.contains(&dep_id) {
                    return Ok(true);
                }
            }
        }

        rec_stack.remove(&job_id);
        Ok(false)
    }

    /// Returns jobs that are ready to execute (have no unsatisfied dependencies).
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::{workflow::JobGroup, Job};
    /// use serde_json::json;
    ///
    /// let job1 = Job::new("independent".to_string(), json!({}));
    /// let job2 = Job::new("dependent".to_string(), json!({}));
    ///
    /// let workflow = JobGroup::new("test_workflow")
    ///     .add_job(job1)
    ///     .then(job2);
    ///
    /// let ready_jobs = workflow.get_ready_jobs();
    /// assert_eq!(ready_jobs.len(), 1); // Only job1 is ready initially
    /// ```
    pub fn get_ready_jobs(&self) -> Vec<&Job> {
        self.jobs
            .iter()
            .filter(|job| {
                job.dependency_status == DependencyStatus::None
                    || job.dependency_status == DependencyStatus::Satisfied
            })
            .collect()
    }

    /// Updates workflow statistics based on current job states.
    pub fn update_statistics(&mut self) {
        self.completed_jobs = self
            .jobs
            .iter()
            .filter(|job| matches!(job.status, crate::JobStatus::Completed))
            .count();

        self.failed_jobs = self
            .jobs
            .iter()
            .filter(|job| {
                matches!(
                    job.status,
                    crate::JobStatus::Failed | crate::JobStatus::Dead | crate::JobStatus::TimedOut
                )
            })
            .count();

        // Update workflow status based on job completion
        if self.completed_jobs == self.total_jobs {
            self.status = WorkflowStatus::Completed;
            if self.completed_at.is_none() {
                self.completed_at = Some(Utc::now());
            }
        } else if self.failed_jobs > 0 && self.failure_policy == FailurePolicy::FailFast {
            self.status = WorkflowStatus::Failed;
            if self.failed_at.is_none() {
                self.failed_at = Some(Utc::now());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_dependency_status_conversion() {
        assert_eq!(DependencyStatus::None.as_str(), "none");
        assert_eq!(DependencyStatus::Waiting.as_str(), "waiting");
        assert_eq!(DependencyStatus::Satisfied.as_str(), "satisfied");
        assert_eq!(DependencyStatus::Failed.as_str(), "failed");

        assert_eq!(
            DependencyStatus::parse_from_db("none").unwrap(),
            DependencyStatus::None
        );
        assert_eq!(
            DependencyStatus::parse_from_db("waiting").unwrap(),
            DependencyStatus::Waiting
        );
        assert_eq!(
            DependencyStatus::parse_from_db("satisfied").unwrap(),
            DependencyStatus::Satisfied
        );
        assert_eq!(
            DependencyStatus::parse_from_db("failed").unwrap(),
            DependencyStatus::Failed
        );

        assert!(DependencyStatus::parse_from_db("invalid").is_err());
    }

    #[test]
    fn test_workflow_status_conversion() {
        assert_eq!(WorkflowStatus::Running.as_str(), "running");
        assert_eq!(WorkflowStatus::Completed.as_str(), "completed");
        assert_eq!(WorkflowStatus::Failed.as_str(), "failed");
        assert_eq!(WorkflowStatus::Cancelled.as_str(), "cancelled");

        assert_eq!(
            WorkflowStatus::parse_from_db("running").unwrap(),
            WorkflowStatus::Running
        );
        assert_eq!(
            WorkflowStatus::parse_from_db("completed").unwrap(),
            WorkflowStatus::Completed
        );
        assert_eq!(
            WorkflowStatus::parse_from_db("failed").unwrap(),
            WorkflowStatus::Failed
        );
        assert_eq!(
            WorkflowStatus::parse_from_db("cancelled").unwrap(),
            WorkflowStatus::Cancelled
        );

        assert!(WorkflowStatus::parse_from_db("invalid").is_err());
    }

    #[test]
    fn test_failure_policy_conversion() {
        assert_eq!(FailurePolicy::FailFast.as_str(), "fail_fast");
        assert_eq!(
            FailurePolicy::ContinueOnFailure.as_str(),
            "continue_on_failure"
        );
        assert_eq!(FailurePolicy::Manual.as_str(), "manual");

        assert_eq!(
            FailurePolicy::parse_from_db("fail_fast").unwrap(),
            FailurePolicy::FailFast
        );
        assert_eq!(
            FailurePolicy::parse_from_db("continue_on_failure").unwrap(),
            FailurePolicy::ContinueOnFailure
        );
        assert_eq!(
            FailurePolicy::parse_from_db("manual").unwrap(),
            FailurePolicy::Manual
        );

        assert!(FailurePolicy::parse_from_db("invalid").is_err());
    }

    #[test]
    fn test_job_group_creation() {
        let workflow = JobGroup::new("test_workflow");
        assert_eq!(workflow.name, "test_workflow");
        assert_eq!(workflow.status, WorkflowStatus::Running);
        assert_eq!(workflow.failure_policy, FailurePolicy::FailFast);
        assert_eq!(workflow.jobs.len(), 0);
        assert_eq!(workflow.total_jobs, 0);
    }

    #[test]
    fn test_add_job() {
        let job = Job::new("test_queue".to_string(), json!({"data": "test"}));
        let workflow = JobGroup::new("test_workflow").add_job(job.clone());

        assert_eq!(workflow.jobs.len(), 1);
        assert_eq!(workflow.total_jobs, 1);
        assert_eq!(workflow.jobs[0].workflow_id, Some(workflow.id));
        assert_eq!(
            workflow.jobs[0].workflow_name,
            Some("test_workflow".to_string())
        );
    }

    #[test]
    fn test_add_parallel_jobs() {
        let jobs = vec![
            Job::new("queue1".to_string(), json!({"data": "1"})),
            Job::new("queue2".to_string(), json!({"data": "2"})),
            Job::new("queue3".to_string(), json!({"data": "3"})),
        ];

        let workflow = JobGroup::new("parallel_workflow").add_parallel_jobs(jobs);

        assert_eq!(workflow.jobs.len(), 3);
        assert_eq!(workflow.total_jobs, 3);

        // All jobs should have the workflow ID set
        for job in &workflow.jobs {
            assert_eq!(job.workflow_id, Some(workflow.id));
            assert_eq!(job.workflow_name, Some("parallel_workflow".to_string()));
        }
    }

    #[test]
    fn test_then_job() {
        let job1 = Job::new("step1".to_string(), json!({"step": 1}));
        let job2 = Job::new("step2".to_string(), json!({"step": 2}));
        let final_job = Job::new("final".to_string(), json!({"step": "final"}));

        let job1_id = job1.id;
        let job2_id = job2.id;

        let workflow = JobGroup::new("sequential_workflow")
            .add_job(job1)
            .add_job(job2)
            .then(final_job);

        assert_eq!(workflow.jobs.len(), 3);

        // The final job should depend on both previous jobs
        let final_job = &workflow.jobs[2];
        assert_eq!(final_job.depends_on.len(), 2);
        assert!(final_job.depends_on.contains(&job1_id));
        assert!(final_job.depends_on.contains(&job2_id));
        assert_eq!(final_job.dependency_status, DependencyStatus::Waiting);
    }

    #[test]
    fn test_workflow_validation() {
        let job1 = Job::new("step1".to_string(), json!({}));
        let job2 = Job::new("step2".to_string(), json!({}));

        let workflow = JobGroup::new("valid_workflow").add_job(job1).then(job2);

        assert!(workflow.validate().is_ok());
    }

    #[test]
    fn test_get_ready_jobs() {
        let job1 = Job::new("independent".to_string(), json!({}));
        let job2 = Job::new("dependent".to_string(), json!({}));

        let workflow = JobGroup::new("test_workflow").add_job(job1).then(job2);

        let ready_jobs = workflow.get_ready_jobs();
        assert_eq!(ready_jobs.len(), 1);
        assert_eq!(ready_jobs[0].queue_name, "independent");
    }

    #[test]
    fn test_workflow_with_metadata() {
        let metadata = json!({
            "owner": "data_team",
            "environment": "test"
        });

        let workflow = JobGroup::new("metadata_workflow").with_metadata(metadata.clone());

        assert_eq!(workflow.metadata, metadata);
    }

    #[test]
    fn test_workflow_with_failure_policy() {
        let workflow = JobGroup::new("resilient_workflow")
            .with_failure_policy(FailurePolicy::ContinueOnFailure);

        assert_eq!(workflow.failure_policy, FailurePolicy::ContinueOnFailure);
    }
}
