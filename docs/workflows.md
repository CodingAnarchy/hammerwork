# Job Dependencies & Workflows

Hammerwork provides a comprehensive workflow system that allows you to create complex data processing pipelines with job dependencies, sequential chains, and parallel processing with synchronization barriers.

## Overview

The workflow system enables you to:
- Create job dependencies where jobs wait for other jobs to complete
- Build sequential processing pipelines (job1 → job2 → job3)
- Execute jobs in parallel with synchronization points
- Handle failure scenarios with configurable policies
- Visualize complex dependency graphs
- Prevent circular dependencies with validation

## Core Concepts

### Job Dependencies

Jobs can depend on other jobs using the `depends_on` field:

```rust
use hammerwork::{Job, queue::DatabaseQueue};
use serde_json::json;

// Create jobs with dependencies
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

// Enqueue jobs - they'll execute in dependency order
queue.enqueue(job1).await?;
queue.enqueue(job2).await?;
queue.enqueue(job3).await?;
```

### Dependency Status

Jobs track their dependency state:
- `None` - Job has no dependencies and can run immediately
- `Waiting` - Job is waiting for dependencies to complete
- `Satisfied` - All dependencies have completed successfully
- `Failed` - One or more dependencies failed

## JobGroup and Workflow Creation

### Sequential Workflows

Create linear processing pipelines:

```rust
use hammerwork::{Job, JobGroup, FailurePolicy};

// Create a sequential data processing pipeline
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

// Create workflow - each job depends on the previous one
let workflow = JobGroup::new("etl_pipeline")
    .add_job(extract_job)
    .then(transform_job)  // transform depends on extract
    .then(load_job)       // load depends on transform
    .with_failure_policy(FailurePolicy::FailFast);

// Enqueue the entire workflow
queue.enqueue_workflow(workflow).await?;
```

### Parallel Workflows

Execute multiple jobs concurrently:

```rust
// Create parallel processing jobs
let region_jobs = vec![
    Job::new("process_region".to_string(), json!({"region": "us-east"})),
    Job::new("process_region".to_string(), json!({"region": "us-west"})),
    Job::new("process_region".to_string(), json!({"region": "eu-west"})),
    Job::new("process_region".to_string(), json!({"region": "ap-south"})),
];

// Create a job that waits for all parallel jobs
let summary_job = Job::new("create_summary".to_string(), json!({
    "output_file": "global_summary.json",
    "include_all_regions": true
}));

// Create workflow with parallel execution and synchronization
let workflow = JobGroup::new("parallel_processing")
    .add_parallel_jobs(region_jobs)  // These run concurrently
    .then(summary_job)               // This waits for all parallel jobs
    .with_failure_policy(FailurePolicy::ContinueOnFailure);

queue.enqueue_workflow(workflow).await?;
```

### Fan-out and Fan-in Patterns

Create complex patterns with branching and merging:

```rust
// Initial job that generates work
let split_job = Job::new("split_data".to_string(), json!({
    "input_file": "large_dataset.csv",
    "chunk_size": 10000
}));

// Multiple processing jobs (fan-out)
let mut processing_jobs = Vec::new();
for i in 0..10 {
    let job = Job::new("process_chunk".to_string(), json!({
        "chunk_id": i,
        "algorithm": "ml_classification"
    }))
    .depends_on(&split_job.id);
    
    processing_jobs.push(job);
}

// Aggregation job (fan-in)
let aggregate_job = Job::new("aggregate_results".to_string(), json!({
    "output_format": "final_results.json"
}));

// Add dependencies from all processing jobs to aggregation
for job in &processing_jobs {
    aggregate_job.depends_on(&job.id);
}

// Create and enqueue workflow
let workflow = JobGroup::new("fan_out_fan_in")
    .add_job(split_job)
    .add_parallel_jobs(processing_jobs)
    .then(aggregate_job);

queue.enqueue_workflow(workflow).await?;
```

## Advanced Dependency Patterns

### Multiple Dependencies

Jobs can depend on multiple other jobs:

```rust
let data_job = Job::new("fetch_data".to_string(), json!({"source": "api"}));
let config_job = Job::new("load_config".to_string(), json!({"env": "prod"}));
let auth_job = Job::new("authenticate".to_string(), json!({"service": "external"}));

// Job that depends on all three completing
let process_job = Job::new("process_with_deps".to_string(), json!({
    "operation": "complex_processing"
}))
.depends_on(&data_job.id)
.depends_on(&config_job.id)
.depends_on(&auth_job.id);
```

### Conditional Dependencies

Create workflows with conditional paths:

```rust
let validation_job = Job::new("validate_input".to_string(), json!({
    "input_file": "data.csv",
    "validation_rules": "strict"
}));

// Different paths based on validation result
let success_job = Job::new("process_valid_data".to_string(), json!({
    "algorithm": "standard_processing"
}))
.depends_on(&validation_job.id)
.with_condition("validation_status == 'valid'");

let error_job = Job::new("handle_invalid_data".to_string(), json!({
    "action": "quarantine_and_notify"
}))
.depends_on(&validation_job.id)
.with_condition("validation_status == 'invalid'");
```

## Failure Policies

Control how workflows handle failures:

### FailFast (Default)

Stop the entire workflow when any job fails:

```rust
let workflow = JobGroup::new("critical_pipeline")
    .add_job(job1)
    .then(job2)
    .then(job3)
    .with_failure_policy(FailurePolicy::FailFast);
// If job1 fails, job2 and job3 will not execute
```

### ContinueOnFailure

Continue executing jobs that don't depend on failed jobs:

```rust
let workflow = JobGroup::new("resilient_pipeline")
    .add_parallel_jobs(vec![job_a, job_b, job_c])
    .then(final_job)
    .with_failure_policy(FailurePolicy::ContinueOnFailure);
// If job_a fails, job_b and job_c continue, but final_job won't execute
```

### Manual

Require manual intervention for failure handling:

```rust
let workflow = JobGroup::new("manual_review_pipeline")
    .add_job(critical_job)
    .then(dependent_job)
    .with_failure_policy(FailurePolicy::Manual);
// If critical_job fails, workflow pauses for manual decision
```

## Workflow Management

### Creating and Managing Workflows

```rust
use hammerwork::{JobGroup, WorkflowStatus};

// Create workflow with metadata
let workflow = JobGroup::new("daily_report_generation")
    .with_description("Generate daily sales and analytics reports")
    .with_timeout(Duration::from_hours(2))
    .with_priority(JobPriority::High)
    .add_job(extract_sales_data)
    .then(generate_report)
    .then(send_report_email);

// Enqueue workflow
let workflow_id = queue.enqueue_workflow(workflow).await?;

// Check workflow status
let status = queue.get_workflow_status(workflow_id).await?;
match status {
    WorkflowStatus::Running => println!("Workflow is executing"),
    WorkflowStatus::Completed => println!("Workflow completed successfully"),
    WorkflowStatus::Failed => println!("Workflow failed"),
    WorkflowStatus::Cancelled => println!("Workflow was cancelled"),
}

// Cancel a running workflow
queue.cancel_workflow(workflow_id).await?;
```

### Workflow Statistics

Monitor workflow execution:

```rust
let stats = queue.get_workflow_stats(workflow_id).await?;
println!("Total jobs: {}", stats.total_jobs);
println!("Completed: {}", stats.completed_jobs);
println!("Failed: {}", stats.failed_jobs);
println!("Pending: {}", stats.pending_jobs);
println!("Running: {}", stats.running_jobs);
println!("Completion: {:.1}%", stats.completion_percentage());
```

## Database Implementation

### Dependency Storage

Dependencies are stored as JSON arrays in the database:

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
  AND status = 'pending'
  AND scheduled_at <= NOW()
  AND (dependency_status = 'none' OR dependency_status = 'satisfied')
ORDER BY priority DESC, scheduled_at ASC;
```

## CLI Workflow Commands

The `cargo-hammerwork` CLI provides comprehensive workflow management:

```bash
# List all workflows
cargo hammerwork workflow list

# Show workflow details
cargo hammerwork workflow show <workflow_id>

# Create a new workflow from JSON/YAML
cargo hammerwork workflow create --file workflow.json

# Show job dependencies
cargo hammerwork workflow dependencies <job_id>

# Visualize workflow as dependency graph
cargo hammerwork workflow graph <workflow_id> --format dot
cargo hammerwork workflow graph <workflow_id> --format mermaid
cargo hammerwork workflow graph <workflow_id> --format json

# Cancel a workflow
cargo hammerwork workflow cancel <workflow_id>

# Retry failed workflows
cargo hammerwork workflow retry <workflow_id>
```

### Workflow Configuration File

Define workflows in JSON or YAML:

```json
{
  "name": "data_pipeline",
  "description": "Daily data processing pipeline",
  "failure_policy": "continue_on_failure",
  "jobs": [
    {
      "name": "extract",
      "queue": "etl_queue",
      "payload": {"source": "production_db"},
      "depends_on": []
    },
    {
      "name": "transform",
      "queue": "etl_queue", 
      "payload": {"rules": "business_logic.json"},
      "depends_on": ["extract"]
    },
    {
      "name": "load",
      "queue": "etl_queue",
      "payload": {"destination": "data_warehouse"},
      "depends_on": ["transform"]
    }
  ]
}
```

## Validation and Error Handling

### Circular Dependency Detection

The system prevents circular dependencies:

```rust
let job_a = Job::new("queue".to_string(), json!({}));
let job_b = Job::new("queue".to_string(), json!({})).depends_on(&job_a.id);
let job_c = Job::new("queue".to_string(), json!({})).depends_on(&job_b.id);

// This would be detected and rejected
job_a.depends_on(&job_c.id); // Creates A -> B -> C -> A cycle

let workflow = JobGroup::new("test")
    .add_job(job_a)
    .add_job(job_b)
    .add_job(job_c);

// Validation will fail with circular dependency error
match workflow.validate() {
    Ok(_) => println!("Workflow is valid"),
    Err(e) => eprintln!("Validation error: {}", e),
}
```

### Dependency Validation

Ensure all dependencies exist:

```rust
let workflow = JobGroup::new("test_workflow")
    .add_job(job1)
    .add_job(job2.depends_on(&non_existent_id)); // This will fail validation

workflow.validate()?; // Returns error for missing dependency
```

## Integration with Other Features

### Priorities and Dependencies

Dependencies are checked before priority ordering:

```rust
let high_priority_job = Job::new("queue".to_string(), json!({}))
    .with_priority(JobPriority::High)
    .depends_on(&low_priority_job.id);

// high_priority_job won't run until low_priority_job completes,
// regardless of priority levels
```

### Cron Jobs with Dependencies

Create recurring workflows:

```rust
let daily_extract = Job::new("extract".to_string(), json!({}))
    .with_cron_schedule("0 2 * * *"); // 2 AM daily

let daily_report = Job::new("report".to_string(), json!({}))
    .depends_on(&daily_extract.id)
    .with_cron_schedule("0 8 * * *"); // 8 AM daily

// Both jobs will be created daily, with report waiting for extract
```

### Batch Operations with Workflows

Create workflows that include batch jobs:

```rust
let batch_job = Job::new_batch("process_batch".to_string(), 
    vec![payload1, payload2, payload3]);

let summary_job = Job::new("summarize".to_string(), json!({}))
    .depends_on(&batch_job.id);

let workflow = JobGroup::new("batch_workflow")
    .add_job(batch_job)
    .then(summary_job);
```

### Distributed Tracing in Workflows

Trace entire workflows:

```rust
let trace_id = TraceId::new();
let correlation_id = CorrelationId::from_business_id("daily-report-001");

let workflow = JobGroup::new("traced_workflow")
    .with_trace_id(trace_id.to_string())
    .with_correlation_id(correlation_id.to_string())
    .add_job(job1.with_trace_id(trace_id.to_string()))
    .then(job2.with_trace_id(trace_id.to_string()))
    .then(job3.with_trace_id(trace_id.to_string()));
```

## Best Practices

### 1. Keep Dependencies Simple

```rust
// Good: Linear dependency chain
job1 -> job2 -> job3

// Avoid: Complex dependency webs
job1 -> job2 -> job4
  |  -> job3 -> job5
           |  -> job6
```

### 2. Use Meaningful Names

```rust
// Good: Descriptive workflow and job names
let workflow = JobGroup::new("customer_onboarding_pipeline")
    .add_job(validate_customer_data)
    .then(create_customer_account)
    .then(send_welcome_email);

// Avoid: Generic names
let workflow = JobGroup::new("workflow1")
    .add_job(job1)
    .then(job2);
```

### 3. Handle Failures Gracefully

```rust
// Consider what happens when jobs fail
let workflow = JobGroup::new("data_processing")
    .add_job(critical_job)
    .then(optional_job)
    .with_failure_policy(FailurePolicy::ContinueOnFailure)
    .with_retry_policy(RetryPolicy::ExponentialBackoff {
        initial_delay: Duration::from_secs(30),
        max_delay: Duration::from_secs(300),
        multiplier: 2.0,
    });
```

### 4. Monitor Workflow Health

```rust
// Set up monitoring for long-running workflows
let workflow = JobGroup::new("daily_batch_processing")
    .with_timeout(Duration::from_hours(4))
    .with_health_check_interval(Duration::from_minutes(15))
    .add_job(extract_job)
    .then(transform_job)
    .then(load_job);
```

### 5. Use Appropriate Granularity

```rust
// Good: Logical job boundaries
extract_customer_data -> transform_customer_data -> load_customer_data

// Avoid: Too fine-grained
read_file -> parse_header -> validate_row1 -> validate_row2 -> ...
```

## Performance Considerations

- Dependencies are checked during job dequeue operations
- Use indexes on `dependency_status` and `depends_on` fields
- Consider batch processing for workflows with many small jobs
- Monitor dependency graph complexity in large workflows
- Use parallel execution where possible to reduce total workflow time

## Troubleshooting

### Common Issues

1. **Jobs Not Starting**: Check dependency status and ensure parent jobs completed
2. **Circular Dependencies**: Use workflow validation to detect cycles
3. **Orphaned Jobs**: Clean up jobs whose dependencies were deleted
4. **Performance Issues**: Monitor dependency query performance with complex graphs

### Debugging Workflows

```bash
# Check job dependencies
cargo hammerwork job show JOB_ID --include-dependencies

# Visualize workflow graph
cargo hammerwork workflow graph WORKFLOW_ID --format mermaid

# Monitor workflow progress
cargo hammerwork workflow status WORKFLOW_ID --watch

# Check for stuck jobs
cargo hammerwork job list --status waiting --dependency-status waiting
```