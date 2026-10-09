# Job Dependencies & Workflows

Hammerwork's workflow system lets you build data processing pipelines from jobs that depend on one another: sequential chains, parallel fan-out with a synchronization point, and configurable behavior when a job fails.

## Overview

The workflow system enables you to:
- Make jobs wait for other jobs to complete
- Build sequential pipelines (job1 → job2 → job3)
- Run jobs in parallel and wait for all of them
- Choose how a workflow reacts to a failed job
- Validate a workflow for missing dependencies and cycles before enqueuing it
- Inspect and cancel workflows from code or the CLI

## Core Concepts

### Job Dependencies

A job can depend on other jobs with `Job::depends_on` (one at a time) or `Job::depends_on_jobs` (a slice). Both mark the job as waiting for its dependencies.

```rust,no_run
# #[allow(unused_imports)] use hammerwork::queue::DatabaseQueue;
# #[allow(dead_code)] fn job(name: &str) -> hammerwork::Job { hammerwork::Job::new(name.to_string(), serde_json::json!({})) }
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler, payload: serde_json::Value, workflow_id: hammerwork::WorkflowId) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::{Job, queue::DatabaseQueue};
use serde_json::json;

let job1 = Job::new("data_processing".to_string(), json!({
    "input_file": "raw_data.csv"
}));

let job2 = Job::new("data_transformation".to_string(), json!({
    "format": "parquet"
}))
.depends_on(&job1.id);  // job2 waits for job1

let job3 = Job::new("data_export".to_string(), json!({
    "destination": "s3://bucket/processed/"
}))
.depends_on(&job2.id);  // job3 waits for job2

assert!(job3.has_dependencies());

// Enqueue the jobs - they execute in dependency order
queue.enqueue(job1).await?;
queue.enqueue(job2).await?;
queue.enqueue(job3).await?;
# Ok(())
# }
```

### Dependency Status

Jobs track their dependency state in `Job::dependency_status` (a `DependencyStatus`):
- `None` - the job has no dependencies and can run immediately
- `Waiting` - the job is waiting for dependencies to complete
- `Satisfied` - all dependencies have completed successfully
- `Failed` - one or more dependencies failed

A job may be enqueued after its dependencies have finished. Its dependency state is
then settled when it is enqueued: `Satisfied` if every dependency completed (also when
they have been archived), and `Failed` (the job is inserted as `Failed`, with the
failed dependency in its `error_message`) if a dependency failed, died or timed out,
unless that dependency's workflow uses the `Manual` failure policy, in which case the
job waits like any other dependent. The dependencies are locked while the job is
inserted, so a dependency that finishes at the same moment cannot leave it waiting.

## JobGroup and Workflow Creation

A `JobGroup` collects jobs under a workflow ID and a name, with a failure policy.
`JobGroup::add_job` and `add_parallel_jobs` add jobs without dependencies. `JobGroup::then`
adds a job that depends on **every job already in the group**, replacing any `depends_on`
the job had. Enqueue the group with `DatabaseQueue::enqueue_workflow`.

### Sequential Workflows

```rust,no_run
# #[allow(unused_imports)] use hammerwork::queue::DatabaseQueue;
# #[allow(dead_code)] fn job(name: &str) -> hammerwork::Job { hammerwork::Job::new(name.to_string(), serde_json::json!({})) }
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler, payload: serde_json::Value, workflow_id: hammerwork::WorkflowId) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::{FailurePolicy, Job, JobGroup, queue::DatabaseQueue};
use serde_json::json;

let extract_job = Job::new("extract".to_string(), json!({
    "source": "database_table",
    "query": "SELECT * FROM customers WHERE created_at > ?"
}));

let transform_job = Job::new("transform".to_string(), json!({
    "operations": ["normalize_phone", "validate_email", "enrich_location"]
}));

let load_job = Job::new("load".to_string(), json!({
    "destination": "data_warehouse",
    "table": "dim_customers"
}));

let workflow = JobGroup::new("etl_pipeline")
    .add_job(extract_job)
    .then(transform_job)  // transform depends on extract
    .then(load_job)       // load depends on extract and transform
    .with_failure_policy(FailurePolicy::FailFast);

workflow.validate()?;
let workflow_id = queue.enqueue_workflow(workflow).await?;
# Ok(())
# }
```

### Parallel Workflows

```rust,no_run
# #[allow(unused_imports)] use hammerwork::queue::DatabaseQueue;
# #[allow(dead_code)] fn job(name: &str) -> hammerwork::Job { hammerwork::Job::new(name.to_string(), serde_json::json!({})) }
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler, payload: serde_json::Value, workflow_id: hammerwork::WorkflowId) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::{FailurePolicy, Job, JobGroup, queue::DatabaseQueue};
use serde_json::json;

let region_jobs = vec![
    Job::new("process_region".to_string(), json!({"region": "us-east"})),
    Job::new("process_region".to_string(), json!({"region": "us-west"})),
    Job::new("process_region".to_string(), json!({"region": "eu-west"})),
    Job::new("process_region".to_string(), json!({"region": "ap-south"})),
];

let summary_job = Job::new("create_summary".to_string(), json!({
    "output_file": "global_summary.json",
    "include_all_regions": true
}));

let workflow = JobGroup::new("parallel_processing")
    .add_parallel_jobs(region_jobs)  // These run concurrently
    .then(summary_job)               // This waits for all of them
    .with_failure_policy(FailurePolicy::ContinueOnFailure);

queue.enqueue_workflow(workflow).await?;
# Ok(())
# }
```

### Fan-out and Fan-in Patterns

```rust,no_run
# #[allow(unused_imports)] use hammerwork::queue::DatabaseQueue;
# #[allow(dead_code)] fn job(name: &str) -> hammerwork::Job { hammerwork::Job::new(name.to_string(), serde_json::json!({})) }
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler, payload: serde_json::Value, workflow_id: hammerwork::WorkflowId) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::{Job, JobGroup, queue::DatabaseQueue};
use serde_json::json;

// Initial job that generates work
let split_job = Job::new("split_data".to_string(), json!({
    "input_file": "large_dataset.csv",
    "chunk_size": 10000
}));

// Multiple processing jobs (fan-out), each depending on the split job
let processing_jobs: Vec<Job> = (0..10)
    .map(|i| {
        Job::new("process_chunk".to_string(), json!({
            "chunk_id": i,
            "algorithm": "ml_classification"
        }))
        .depends_on(&split_job.id)
    })
    .collect();

// Aggregation job (fan-in) depending on every processing job
let processing_ids: Vec<_> = processing_jobs.iter().map(|job| job.id).collect();
let aggregate_job = Job::new("aggregate_results".to_string(), json!({
    "output_format": "final_results.json"
}))
.depends_on_jobs(&processing_ids);

let workflow = JobGroup::new("fan_out_fan_in")
    .add_job(split_job)
    .add_parallel_jobs(processing_jobs)
    .add_job(aggregate_job);

workflow.validate()?;
queue.enqueue_workflow(workflow).await?;
# Ok(())
# }
```

`add_job` keeps the dependencies you set on the job, which is what you want here; using
`then` for the last job would also work but would make it depend on the split job too.

## Advanced Dependency Patterns

### Multiple Dependencies

A job can depend on several jobs and only runs once all of them have completed:

```rust,no_run
# #[allow(unused_imports)] use hammerwork::queue::DatabaseQueue;
# #[allow(dead_code)] fn job(name: &str) -> hammerwork::Job { hammerwork::Job::new(name.to_string(), serde_json::json!({})) }
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler, payload: serde_json::Value, workflow_id: hammerwork::WorkflowId) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::Job;
use serde_json::json;

let data_job = Job::new("fetch_data".to_string(), json!({"source": "api"}));
let config_job = Job::new("load_config".to_string(), json!({"env": "prod"}));
let auth_job = Job::new("authenticate".to_string(), json!({"service": "external"}));

let process_job = Job::new("process_with_deps".to_string(), json!({
    "operation": "complex_processing"
}))
.depends_on(&data_job.id)
.depends_on(&config_job.id)
.depends_on(&auth_job.id);

assert_eq!(process_job.depends_on.len(), 3);

// Equivalent, in one call
let process_job = Job::new("process_with_deps".to_string(), json!({}))
    .depends_on_jobs(&[data_job.id, config_job.id, auth_job.id]);
# Ok(())
# }
```

There is no conditional dependency (running a job only if a dependency produced a particular
result). To branch, have the dependency's handler enqueue the follow-up job it wants, for
example with [dynamic job spawning](job-spawning.md).

## Failure Policies

Control how workflows handle failures. A policy applies when a job fails terminally
(its last attempt failed or timed out, or it was failed manually); failed attempts that
will be retried do not trigger it. Dependency resolution and the policy run in the same
transaction as the job's status change, and the workflow's `completed_jobs`,
`failed_jobs` and `status` are updated with it. A workflow is `completed` when every
job completed, and `failed` as soon as a job fails under `FailFast`, or once every job
has finished with at least one failure under the other policies.

### FailFast (Default)

Stop the entire workflow when any job fails:

```rust,no_run
# #[allow(unused_imports)] use hammerwork::queue::DatabaseQueue;
# #[allow(dead_code)] fn job(name: &str) -> hammerwork::Job { hammerwork::Job::new(name.to_string(), serde_json::json!({})) }
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler, payload: serde_json::Value, workflow_id: hammerwork::WorkflowId) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::{FailurePolicy, JobGroup};

let workflow = JobGroup::new("critical_pipeline")
    .add_job(job("step1"))
    .then(job("step2"))
    .then(job("step3"))
    .with_failure_policy(FailurePolicy::FailFast);
// If step1 fails, step2 and step3 will not execute
# Ok(())
# }
```

### ContinueOnFailure

Continue executing jobs that don't depend on failed jobs:

```rust,no_run
# #[allow(unused_imports)] use hammerwork::queue::DatabaseQueue;
# #[allow(dead_code)] fn job(name: &str) -> hammerwork::Job { hammerwork::Job::new(name.to_string(), serde_json::json!({})) }
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler, payload: serde_json::Value, workflow_id: hammerwork::WorkflowId) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::{FailurePolicy, JobGroup};

let workflow = JobGroup::new("resilient_pipeline")
    .add_parallel_jobs(vec![job("a"), job("b"), job("c")])
    .then(job("final"))
    .with_failure_policy(FailurePolicy::ContinueOnFailure);
// If job a fails, b and c continue, but final won't execute
# Ok(())
# }
```

### Manual

Require manual intervention for failure handling:

```rust,no_run
# #[allow(unused_imports)] use hammerwork::queue::DatabaseQueue;
# #[allow(dead_code)] fn job(name: &str) -> hammerwork::Job { hammerwork::Job::new(name.to_string(), serde_json::json!({})) }
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler, payload: serde_json::Value, workflow_id: hammerwork::WorkflowId) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::{FailurePolicy, JobGroup};

let workflow = JobGroup::new("manual_review_pipeline")
    .add_job(job("critical"))
    .then(job("dependent"))
    .with_failure_policy(FailurePolicy::Manual);
// If critical fails, its dependents wait for a manual decision
# Ok(())
# }
```

With `Manual`, the dependents of the failed job stay `waiting` and the workflow stays
`running`. Re-run the failed job with `queue.retry_dead_job(job_id)` (a `Dead` job) or
`queue.retry_job(job_id, at)` (a `Failed` or `TimedOut` one); when it completes, its
dependents become runnable. To give up instead, call `cancel_workflow`.

With `FailFast`, every job of the workflow that has not started yet is marked `Failed`
(jobs already running finish normally). With `ContinueOnFailure`, the jobs that depend
on the failed job, directly or transitively, are marked `Failed` with
`dependency_status = failed`; independent jobs keep running. Jobs that use
`Job::depends_on` outside a workflow behave like `ContinueOnFailure`.

`queue.fail_job_dependencies(job_id)` applies the same policy explicitly (and updates the
workflow's counters and status), and returns the jobs it failed. Every terminal
transition (`fail_job`, `mark_job_dead`, `mark_job_timed_out`, a worker's
`finish_job_run` and a stale job the reaper marks `Dead`) already applies it, so after
one it finds nothing left to do. `TestQueue` applies the policies the same way as
PostgreSQL and MySQL.

## Workflow Management

### Creating and Managing Workflows

`JobGroup::with_metadata` attaches arbitrary JSON to the workflow. `get_workflow_status` returns the stored workflow, including its jobs and counters, or `None` if there is no such workflow.

```rust,no_run
# #[allow(unused_imports)] use hammerwork::queue::DatabaseQueue;
# #[allow(dead_code)] fn job(name: &str) -> hammerwork::Job { hammerwork::Job::new(name.to_string(), serde_json::json!({})) }
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler, payload: serde_json::Value, workflow_id: hammerwork::WorkflowId) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::{FailurePolicy, JobGroup, WorkflowStatus, queue::DatabaseQueue};
use serde_json::json;

let workflow = JobGroup::new("daily_report_generation")
    .with_metadata(json!({"description": "Generate daily sales and analytics reports"}))
    .add_job(job("extract_sales_data"))
    .then(job("generate_report"))
    .then(job("send_report_email"))
    .with_failure_policy(FailurePolicy::FailFast);

let workflow_id = queue.enqueue_workflow(workflow).await?;

// Check workflow status
if let Some(workflow) = queue.get_workflow_status(workflow_id).await? {
    match workflow.status {
        WorkflowStatus::Running => println!("Workflow is executing"),
        WorkflowStatus::Completed => println!("Workflow completed successfully"),
        WorkflowStatus::Failed => println!("Workflow failed"),
        WorkflowStatus::Cancelled => println!("Workflow was cancelled"),
    }
}

// List the jobs of a workflow
let jobs = queue.get_workflow_jobs(workflow_id).await?;
println!("{} jobs in the workflow", jobs.len());

// Cancel a running workflow
queue.cancel_workflow(workflow_id).await?;
# Ok(())
# }
```

`cancel_workflow` marks every unfinished job of the workflow (`Pending`, `Retrying`
and `Running`) as `Failed` with the error "Workflow cancelled", and the workflow as
`Cancelled`. A job that is already running cannot be interrupted: its handler runs to
the end, but its outcome is discarded. `finish_job_run` only records outcomes for jobs
that are still `Running` the same run, so the job stays `Failed`, and the worker's
heartbeats stop extending its lease. Side effects the handler performed are not undone.

### Workflow Progress

The `JobGroup` returned by `get_workflow_status` carries the counters:

```rust,no_run
# #[allow(unused_imports)] use hammerwork::queue::DatabaseQueue;
# #[allow(dead_code)] fn job(name: &str) -> hammerwork::Job { hammerwork::Job::new(name.to_string(), serde_json::json!({})) }
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler, payload: serde_json::Value, workflow_id: hammerwork::WorkflowId) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::queue::DatabaseQueue;

if let Some(workflow) = queue.get_workflow_status(workflow_id).await? {
    println!("Total jobs: {}", workflow.total_jobs);
    println!("Completed: {}", workflow.completed_jobs);
    println!("Failed: {}", workflow.failed_jobs);
    if workflow.total_jobs > 0 {
        let pct = workflow.completed_jobs as f64 / workflow.total_jobs as f64 * 100.0;
        println!("Completion: {:.1}%", pct);
    }
}
# Ok(())
# }
```

## Database Implementation

### Dependency Storage

Dependencies are stored in the `depends_on` column of `hammerwork_jobs`, next to
`dependency_status`:

```sql
-- PostgreSQL example
SELECT id, queue_name, depends_on, dependency_status
FROM hammerwork_jobs
WHERE dependency_status = 'waiting';
```

### Efficient Dependency Queries

The system uses optimized queries to find ready jobs:

```sql
-- Only jobs with satisfied dependencies are eligible for dequeue
SELECT * FROM hammerwork_jobs
WHERE queue_name = ?
  AND status = 'Pending'
  AND scheduled_at <= NOW()
  AND dependency_status IN ('none', 'satisfied')
ORDER BY priority DESC, scheduled_at ASC;
```

When a job completes, its waiting dependents are locked and checked with a constant
number of queries, however many there are. On PostgreSQL `depends_on` is a `UUID[]`
with a GIN index, which serves the dependent lookup. On MySQL `depends_on` is a JSON
array, and the lookup (`JSON_CONTAINS`) cannot use an index, so it scans the waiting
jobs; keep the number of jobs waiting on dependencies moderate on MySQL. Failing the
dependents of a failed job walks the dependency graph one job at a time.

## CLI Workflow Commands

The `cargo-hammerwork` CLI inspects and manages workflows:

```bash
# List workflows (optionally --running, --completed or --failed)
cargo hammerwork workflow list --limit 20

# Show workflow details, optionally with the dependency graph
cargo hammerwork workflow show <workflow_id> --dependencies

# Create a workflow and enqueue its jobs in one transaction. jobs.json is a JSON array of
# {"queue", "payload", "priority"?, "depends_on"?} objects; depends_on lists the indexes of
# earlier jobs, e.g. [{"queue": "etl", "payload": {"step": "extract"}},
#                     {"queue": "etl", "payload": {"step": "load"}, "depends_on": [0]}]
cargo hammerwork workflow create --name nightly_etl --jobs-file jobs.json --failure-policy continue_on_failure

# Show a job's dependencies (--tree for the full tree, --dependents for jobs that wait on it)
cargo hammerwork workflow dependencies <job_id> --tree

# Visualize a workflow as a dependency graph
cargo hammerwork workflow graph <workflow_id> --format dot
cargo hammerwork workflow graph <workflow_id> --format mermaid
cargo hammerwork workflow graph <workflow_id> --format json

# Cancel a workflow
cargo hammerwork workflow cancel <workflow_id>
```

Workflows with jobs are built from code with `JobGroup` and `enqueue_workflow`; the CLI
does not load workflow definition files.

## Validation and Error Handling

### Circular Dependency Detection

`JobGroup::validate` rejects missing dependencies and cycles. The `Job::depends_on`
builder cannot easily produce a cycle, but the `depends_on` field is public, so a cycle can be
introduced by editing it directly:

```rust,no_run
# #[allow(unused_imports)] use hammerwork::queue::DatabaseQueue;
# #[allow(dead_code)] fn job(name: &str) -> hammerwork::Job { hammerwork::Job::new(name.to_string(), serde_json::json!({})) }
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler, payload: serde_json::Value, workflow_id: hammerwork::WorkflowId) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::JobGroup;

let mut job_a = job("a");
let job_b = job("b").depends_on(&job_a.id);
let job_c = job("c").depends_on(&job_b.id);

// A -> C -> B -> A would be a cycle: make A depend on C
job_a.depends_on = vec![job_c.id];

let workflow = JobGroup::new("test")
    .add_job(job_a)
    .add_job(job_b)
    .add_job(job_c);

assert!(workflow.validate().is_err());
# Ok(())
# }
```

### Dependency Validation

All dependencies must be part of the workflow:

```rust,no_run
# #[allow(unused_imports)] use hammerwork::queue::DatabaseQueue;
# #[allow(dead_code)] fn job(name: &str) -> hammerwork::Job { hammerwork::Job::new(name.to_string(), serde_json::json!({})) }
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler, payload: serde_json::Value, workflow_id: hammerwork::WorkflowId) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::JobGroup;

let outsider = job("not-in-the-workflow");
let workflow = JobGroup::new("test_workflow")
    .add_job(job("job1"))
    .add_job(job("job2").depends_on(&outsider.id));

assert!(workflow.validate().is_err()); // missing dependency
# Ok(())
# }
```

## Integration with Other Features

### Priorities and Dependencies

Dependencies are checked before priority ordering:

```rust,no_run
# #[allow(unused_imports)] use hammerwork::queue::DatabaseQueue;
# #[allow(dead_code)] fn job(name: &str) -> hammerwork::Job { hammerwork::Job::new(name.to_string(), serde_json::json!({})) }
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler, payload: serde_json::Value, workflow_id: hammerwork::WorkflowId) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::{Job, JobPriority};
use serde_json::json;

let low_priority_job = Job::new("queue".to_string(), json!({})).as_low_priority();
let high_priority_job = Job::new("queue".to_string(), json!({}))
    .with_priority(JobPriority::High)
    .depends_on(&low_priority_job.id);

// high_priority_job won't run until low_priority_job completes,
// regardless of priority levels
# Ok(())
# }
```

### Workflow Jobs and Tracing

Jobs in a workflow can carry trace and correlation IDs like any other job. `JobGroup` has no tracing helpers, so set them on each job:

```rust,no_run
# #[allow(unused_imports)] use hammerwork::queue::DatabaseQueue;
# #[allow(dead_code)] fn job(name: &str) -> hammerwork::Job { hammerwork::Job::new(name.to_string(), serde_json::json!({})) }
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler, payload: serde_json::Value, workflow_id: hammerwork::WorkflowId) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::{JobGroup, tracing::{CorrelationId, TraceId}};

let trace_id = TraceId::new();
let correlation_id = CorrelationId::from_string("daily-report-001");

let traced = |name: &str| {
    job(name)
        .with_trace_id(trace_id.to_string())
        .with_correlation_id(correlation_id.to_string())
};

let workflow = JobGroup::new("traced_workflow")
    .add_job(traced("job1"))
    .then(traced("job2"))
    .then(traced("job3"));
# Ok(())
# }
```

## Best Practices

### 1. Keep Dependencies Simple

Prefer linear chains and simple fan-out/fan-in over dense dependency webs:

```text
Good:   job1 -> job2 -> job3

Avoid:  job1 -> job2 -> job4
          \-> job3 -> job5
                  \-> job6
```

### 2. Use Meaningful Names

```rust,no_run
# #[allow(unused_imports)] use hammerwork::queue::DatabaseQueue;
# #[allow(dead_code)] fn job(name: &str) -> hammerwork::Job { hammerwork::Job::new(name.to_string(), serde_json::json!({})) }
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler, payload: serde_json::Value, workflow_id: hammerwork::WorkflowId) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::JobGroup;

// Good: descriptive workflow and job names
let workflow = JobGroup::new("customer_onboarding_pipeline")
    .add_job(job("validate_customer_data"))
    .then(job("create_customer_account"))
    .then(job("send_welcome_email"));
# Ok(())
# }
```

### 3. Handle Failures Deliberately

Retries are configured per job (and per worker), and the failure policy decides what happens once a job has exhausted them:

```rust,no_run
# #[allow(unused_imports)] use hammerwork::queue::DatabaseQueue;
# #[allow(dead_code)] fn job(name: &str) -> hammerwork::Job { hammerwork::Job::new(name.to_string(), serde_json::json!({})) }
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler, payload: serde_json::Value, workflow_id: hammerwork::WorkflowId) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::{FailurePolicy, JobGroup};

let workflow = JobGroup::new("data_processing")
    .add_job(job("critical_job").with_max_attempts(5))
    .then(job("optional_job"))
    .with_failure_policy(FailurePolicy::ContinueOnFailure);
# Ok(())
# }
```

### 4. Use Appropriate Granularity

```text
Good:   extract_customer_data -> transform_customer_data -> load_customer_data
Avoid:  read_file -> parse_header -> validate_row1 -> validate_row2 -> ...
```

## Performance Considerations

- Dependencies are checked during job dequeue operations
- Use indexes on `dependency_status` and `depends_on` fields
- Use parallel execution where possible to reduce total workflow time
- Keep the dependency graph of a single workflow reasonably small

## Troubleshooting

1. **Jobs not starting**: check `dependency_status` and make sure the jobs they depend on completed.
2. **Circular or missing dependencies**: call `JobGroup::validate` before `enqueue_workflow`.
3. **Stuck workflow under `Manual`**: retry the failed job with `retry_dead_job`, or cancel the workflow.

### Debugging Workflows

```bash
# Check a job's dependencies and dependents
cargo hammerwork workflow dependencies JOB_ID --tree --dependents

# Visualize the workflow graph
cargo hammerwork workflow graph WORKFLOW_ID --format mermaid

# Inspect the workflow and its state
cargo hammerwork workflow show WORKFLOW_ID --dependencies
```
