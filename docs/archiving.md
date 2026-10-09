# Job Archiving & Retention

Hammerwork can move finished jobs out of the active jobs table into an archive table, compress their payloads, restore them, and permanently purge old archived data. This keeps the hot table small and helps with data retention requirements.

## Overview

The archiving system lets you:
- Archive completed, failed, dead and timed-out jobs after a configurable age
- Gzip-compress archived payloads
- Restore archived jobs to the active queue
- Permanently purge old archived jobs
- Read archival statistics (jobs archived, bytes stored, compression ratio)
- Drive all of this from code, the `cargo hammerwork archive` CLI or the web API

## Architecture

### Archive Tables

- **`hammerwork_jobs`**: the active jobs table
- **`hammerwork_jobs_archive`**: archived jobs, with the payload stored (optionally compressed) next to the job metadata
- **`hammerwork_migrations`**: tracks which migrations have been applied (run the archive migration first)

### Key Benefits

- **Performance**: keeps the main jobs table small and fast
- **Storage efficiency**: payloads are gzip-compressed when that makes them smaller
- **Recoverability**: archived jobs can be restored
- **Auditability**: every archived row records when it was archived, why and by whom

## Archival Policies

### Creating Archival Policies

An `ArchivalPolicy` says how old a job in each terminal status must be before it is archived. A status whose retention is `None` is never archived.

```rust,no_run
# #[allow(unused_imports)] use hammerwork::queue::DatabaseQueue;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler, payload: serde_json::Value, job: hammerwork::Job) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::archive::{ArchivalConfig, ArchivalPolicy};
use chrono::Duration;

let policy = ArchivalPolicy::new()
    .archive_completed_after(Duration::days(7))   // Archive completed jobs after 7 days
    .archive_failed_after(Duration::days(30))     // Keep failed jobs for debugging
    .archive_dead_after(Duration::days(14))       // Archive dead jobs after 2 weeks
    .archive_timed_out_after(Duration::days(21))  // Archive timed out jobs after 3 weeks
    .purge_archived_after(Duration::days(365))    // Retention hint for purging (see below)
    .compress_archived_payloads(true)             // Gzip payloads
    .with_batch_size(1000)                        // At most 1000 jobs per archival pass
    .enabled(true);

// Compression settings
let config = ArchivalConfig::new().with_compression_level(6); // 0-9, 6 is the default
# Ok(())
# }
```

`ArchivalPolicy::new()` starts from the defaults: completed jobs after 30 days, failed, dead and timed-out jobs after 90 days, payload compression on, batch size 1000, enabled.

`should_archive` tells you whether a job of a given status and age is eligible:

```rust,no_run
# #[allow(unused_imports)] use hammerwork::queue::DatabaseQueue;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler, payload: serde_json::Value, job: hammerwork::Job) -> std::result::Result<(), Box<dyn std::error::Error>> {
use chrono::Duration;
use hammerwork::{JobStatus, archive::ArchivalPolicy};

let policy = ArchivalPolicy::new()
    .archive_completed_after(Duration::days(7))
    .archive_failed_after(Duration::days(30));

assert!(!policy.should_archive(&JobStatus::Completed, Duration::days(5)));
assert!(policy.should_archive(&JobStatus::Failed, Duration::days(40)));
assert!(!policy.should_archive(&JobStatus::Pending, Duration::days(100)));
# Ok(())
# }
```

`purge_archived_after` is stored on the policy, but archiving does not purge on its own; call `purge_archived_jobs` (below) with the cutoff you want, for example from a scheduled job.

### Compression Configuration

```rust,no_run
# #[allow(unused_imports)] use hammerwork::queue::DatabaseQueue;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler, payload: serde_json::Value, job: hammerwork::Job) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::archive::ArchivalConfig;

// Maximum compression for long-term storage
let high_compression = ArchivalConfig::new().with_compression_level(9);

// Fast compression for frequent archival
let fast_compression = ArchivalConfig::new().with_compression_level(1);

assert_eq!(high_compression.compression_level, 9);
# Ok(())
# }
```

`ArchivalConfig` also has `max_payload_size` and `verify_compression` fields (with builders), but the built-in queue implementations do not currently read them. What matters in practice is the compression level, and the policy's `compress_payloads` switch: with compression off, payloads are stored as plain JSON. With compression on, a payload is stored compressed only if that is actually smaller.

### Per-Queue Policies

`JobArchiver` keeps a policy per queue plus a global configuration, and runs archival with a progress callback or an event stream. A queue without its own policy uses `ArchivalPolicy::default()`.

```rust,no_run
# #[allow(unused_imports)] use hammerwork::queue::DatabaseQueue;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler, payload: serde_json::Value, job: hammerwork::Job) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::archive::{ArchivalConfig, ArchivalPolicy, ArchivalReason, JobArchiver};
use chrono::Duration;

let mut archiver = JobArchiver::new(queue.pool.clone());
archiver.set_policy("critical_queue", ArchivalPolicy::new().archive_completed_after(Duration::days(180)));
archiver.set_policy("logs_queue", ArchivalPolicy::new().archive_completed_after(Duration::days(1)));
archiver.set_config(ArchivalConfig::new().with_compression_level(9));

assert!(archiver.get_policy("logs_queue").is_some());

let (operation_id, stats) = archiver
    .archive_jobs_with_progress(
        queue.as_ref(),
        Some("logs_queue"),
        ArchivalReason::Maintenance,
        Some("maintenance_window"),
        Some(Box::new(|processed, total| {
            println!("Archived {} of about {} jobs", processed, total);
        })),
    )
    .await?;
println!("Operation {} archived {} jobs", operation_id, stats.jobs_archived);
# Ok(())
# }
```

`archive_jobs_with_events` is the same but reports `ArchiveEvent`s (`BulkArchiveStarted`, `BulkArchiveProgress`, `BulkArchiveCompleted`) to a callback. Both keep archiving batch after batch until no eligible jobs remain (with an upper bound on the number of batches per call).

## Running Archival Operations

### Manual Archival

`DatabaseQueue::archive_jobs` runs one archival pass with a policy and configuration:

```rust,no_run
# #[allow(unused_imports)] use hammerwork::queue::DatabaseQueue;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler, payload: serde_json::Value, job: hammerwork::Job) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::archive::{ArchivalConfig, ArchivalPolicy, ArchivalReason};
use hammerwork::queue::DatabaseQueue;
use chrono::Duration;

let policy = ArchivalPolicy::new().archive_completed_after(Duration::days(7));
let config = ArchivalConfig::new();

let stats = queue
    .archive_jobs(
        None,                    // all queues, or Some("queue_name")
        &policy,
        &config,
        ArchivalReason::Manual,
        Some("admin_user"),      // who initiated the archival
    )
    .await?;

println!("Jobs archived: {}", stats.jobs_archived);
println!("Bytes stored: {}", stats.bytes_archived);
println!("Compression ratio: {:.2}", stats.compression_ratio);
println!("Took: {:?}", stats.operation_duration);
# Ok(())
# }
```

`ArchivalReason` is one of `Automatic`, `Manual`, `Compliance` and `Maintenance`. One call to `archive_jobs` archives at most `policy.batch_size` jobs.

### Queue-specific Archival

```rust,no_run
# #[allow(unused_imports)] use hammerwork::queue::DatabaseQueue;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler, payload: serde_json::Value, job: hammerwork::Job) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::archive::{ArchivalConfig, ArchivalPolicy, ArchivalReason};
use hammerwork::queue::DatabaseQueue;
use chrono::Duration;

let email_policy = ArchivalPolicy::new().archive_completed_after(Duration::days(3));
let batch_policy = ArchivalPolicy::new()
    .archive_completed_after(Duration::days(1))
    .with_batch_size(5000);
let config = ArchivalConfig::new();

let email_stats = queue
    .archive_jobs(Some("email_queue"), &email_policy, &config, ArchivalReason::Automatic, Some("cron_scheduler"))
    .await?;

let batch_stats = queue
    .archive_jobs(Some("batch_processing_queue"), &batch_policy, &config, ArchivalReason::Maintenance, Some("maintenance_window"))
    .await?;
# Ok(())
# }
```

### Scheduled Archival

Run archival on a schedule with a recurring job whose handler calls `archive_jobs`:

```rust,no_run
# #[allow(unused_imports)] use hammerwork::queue::DatabaseQueue;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler, payload: serde_json::Value, job: hammerwork::Job) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::archive::{ArchivalConfig, ArchivalPolicy, ArchivalReason};
use hammerwork::cron::CronSchedule;
use hammerwork::queue::DatabaseQueue;
use hammerwork::worker::JobHandler;
use hammerwork::{Job, JobPriority, Worker};
use chrono::Duration;
use serde_json::json;
use std::sync::Arc;

// A daily archival job at 2 AM UTC
let archival_job = Job::with_cron_schedule(
    "system_maintenance".to_string(),
    json!({"operation": "archive_jobs"}),
    CronSchedule::new("0 0 2 * * *")?,
)?
.with_priority(JobPriority::Low)
.with_timeout(std::time::Duration::from_secs(2 * 60 * 60));

queue.enqueue_cron_job(archival_job).await?;

// The handler captures the queue and runs the archival
let archival_queue = queue.clone();
let archival_handler: JobHandler = Arc::new(move |_job: Job| {
    let queue = archival_queue.clone();
    Box::pin(async move {
        let policy = ArchivalPolicy::new()
            .archive_completed_after(Duration::days(7))
            .archive_failed_after(Duration::days(30));
        let stats = queue
            .archive_jobs(None, &policy, &ArchivalConfig::new(), ArchivalReason::Automatic, Some("scheduler"))
            .await?;

        ::tracing::info!(
            jobs_archived = stats.jobs_archived,
            compression_ratio = stats.compression_ratio,
            "Daily archival completed"
        );
        Ok(())
    })
});

let worker = Worker::new(queue.clone(), "system_maintenance".to_string(), archival_handler);
# Ok(())
# }
```

## Compression and Storage

### Archival Statistics

`get_archival_stats` reports totals for the archive, optionally for one queue:

```rust,no_run
# #[allow(unused_imports)] use hammerwork::queue::DatabaseQueue;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler, payload: serde_json::Value, job: hammerwork::Job) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::queue::DatabaseQueue;

let stats = queue.get_archival_stats(None).await?;

println!("Archived jobs: {}", stats.jobs_archived);
println!("Bytes stored: {}", stats.bytes_archived);
println!("Compression ratio (original / stored): {:.2}", stats.compression_ratio);
println!("Last archived at: {}", stats.last_run_at);

let email_stats = queue.get_archival_stats(Some("email_queue")).await?;
# Ok(())
# }
```

### What Archiving Does to a Job

Archiving **moves** each eligible job: in one transaction the row is copied into
`hammerwork_jobs_archive` (payload compressed when the policy asks for it) and deleted
from `hammerwork_jobs`. Candidate rows are locked with `FOR UPDATE SKIP LOCKED`
(PostgreSQL, and MySQL 8.0+), so concurrent archivers never archive the same job twice.

`queue.get_job(id)` still finds an archived job: it falls back to the archive table and
returns the job as it was archived, with `status == JobStatus::Archived`.
`list_archived_jobs` reports the job's status *before* it was archived.

`queue.archive_jobs(...)` processes at most `batch_size` jobs per call.
`JobArchiver::archive_jobs_with_progress` / `archive_jobs_with_events` call it repeatedly
until no eligible jobs remain, reporting progress after each batch.

## Restoring Archived Jobs

### Individual Job Restoration

```rust,no_run
# #[allow(unused_imports)] use hammerwork::queue::DatabaseQueue;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler, payload: serde_json::Value, job: hammerwork::Job) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::queue::DatabaseQueue;

let job_id = uuid::Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000")?;
let restored_job = queue.restore_archived_job(job_id).await?;

println!("Restored job {} to queue {}", restored_job.id, restored_job.queue_name);
// The job is back in the main jobs table, Pending, and can be processed normally
# Ok(())
# }
```

Restoring moves the row back in one transaction. Restoring an id that is not in the
archive returns `HammerworkError::JobNotFound`.

### Bulk Restoration

`list_archived_jobs` returns `ArchivedJob` records (metadata only, no payload): `id`, `queue_name`, the original `status`, `created_at`, `archived_at`, `archival_reason`, `original_payload_size`, `payload_compressed` and `archived_by`. Combine it with `restore_archived_job` to restore in bulk:

```rust,no_run
# #[allow(unused_imports)] use hammerwork::queue::DatabaseQueue;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler, payload: serde_json::Value, job: hammerwork::Job) -> std::result::Result<(), Box<dyn std::error::Error>> {
use chrono::{Duration, Utc};
use hammerwork::{JobStatus, queue::DatabaseQueue};

let archived_jobs = queue
    .list_archived_jobs(
        Some("critical_queue"), // from a specific queue (None for all)
        Some(100),              // limit
        Some(0),                // offset
    )
    .await?;

let mut restored_count = 0;
for archived_job in archived_jobs {
    // Restore failed jobs that were archived in the last 24 hours
    if archived_job.status == JobStatus::Failed
        && archived_job.archived_at > Utc::now() - Duration::hours(24)
    {
        queue.restore_archived_job(archived_job.id).await?;
        restored_count += 1;
    }
}

println!("Restored {} jobs from the archive", restored_count);
# Ok(())
# }
```

## Querying Archived Jobs

### List Archived Jobs

```rust,no_run
# #[allow(unused_imports)] use hammerwork::queue::DatabaseQueue;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler, payload: serde_json::Value, job: hammerwork::Job) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::queue::DatabaseQueue;

// Page 3 of 50 jobs per page, across all queues
let archived_jobs = queue.list_archived_jobs(None, Some(50), Some(100)).await?;

for job in archived_jobs {
    println!(
        "Job: {} | Queue: {} | Archived: {} | Status: {:?} | Reason: {}",
        job.id, job.queue_name, job.archived_at, job.status, job.archival_reason
    );
}
# Ok(())
# }
```

There is no payload search or arbitrary filtering of the archive from code; filter the
returned `ArchivedJob` records yourself, or query `hammerwork_jobs_archive` directly.

## Purging Archived Jobs

### Purging by Age

`purge_archived_jobs` permanently deletes archived jobs archived before the given instant and returns how many were deleted. This cannot be undone.

```rust,no_run
# #[allow(unused_imports)] use hammerwork::queue::DatabaseQueue;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler, payload: serde_json::Value, job: hammerwork::Job) -> std::result::Result<(), Box<dyn std::error::Error>> {
use chrono::{Duration, Utc};
use hammerwork::queue::DatabaseQueue;

// Delete jobs archived more than a year ago
let purged_count = queue
    .purge_archived_jobs(Utc::now() - Duration::days(365))
    .await?;

println!("Permanently purged {} archived jobs", purged_count);
# Ok(())
# }
```

There is no per-queue or per-status purge and no purge preview in the library API. The CLI
(`archive purge --dry-run`) can show what a purge would delete.

## CLI Archival Commands

```bash
# Archive jobs, optionally for one queue, with per-status ages in days
cargo hammerwork archive run --queue-name email_queue \
  --completed-after-days 7 --failed-after-days 30 \
  --dead-after-days 30 --timed-out-after-days 30

# Preview what would be archived
cargo hammerwork archive run --dry-run

# Custom compression, batch size and bookkeeping
cargo hammerwork archive run \
  --compression-level 9 \
  --batch-size 500 \
  --reason maintenance \
  --archived-by admin

# Restore a job
cargo hammerwork archive restore JOB_ID

# List archived jobs (table, json or csv)
cargo hammerwork archive list --queue-name payment_queue --limit 100 --format json

# Archive statistics (table or json)
cargo hammerwork archive stats --queue-name batch_processing

# Preview and run a purge
cargo hammerwork archive purge --older-than-days 365 --dry-run
cargo hammerwork archive purge --older-than-days 730 --confirm
```

`archive run` takes its retention periods from flags; the defaults are 7 days for completed jobs and 30 days for failed, dead and timed-out jobs.

`archive set-policy`, `get-policy` and `remove-policy` exist as commands but are placeholders: the CLI has no policy storage yet, so they only print a note and change nothing. Pass the retention flags to `archive run` instead, or keep the policy in your application (see the per-queue policies above).

## Web API

The web dashboard crate (`hammerwork-web`) exposes archive endpoints; see its README for authentication and parameters:

- `GET /api/archive/jobs`: list archived jobs
- `POST /api/archive/jobs`: run archival (the JSON body can set the queue, reason, policy, config and `dry_run`)
- `POST /api/archive/jobs/{id}/restore`: restore a job
- `DELETE /api/archive/purge`: purge old archived jobs (supports `dry_run`)
- `GET /api/archive/stats`: archive statistics

## Best Practices

### 1. Design Archival Policies Per Status

```rust,no_run
# #[allow(unused_imports)] use hammerwork::queue::DatabaseQueue;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler, payload: serde_json::Value, job: hammerwork::Job) -> std::result::Result<(), Box<dyn std::error::Error>> {
use chrono::Duration;
use hammerwork::archive::ArchivalPolicy;

let policy = ArchivalPolicy::new()
    .archive_completed_after(Duration::days(7))   // Quick archival for completed jobs
    .archive_failed_after(Duration::days(30))     // Longer retention for debugging
    .archive_dead_after(Duration::days(14))       // Medium retention for analysis
    .purge_archived_after(Duration::days(365));   // Retention for the purge job you schedule
# Ok(())
# }
```

### 2. Monitor Compression

```rust,no_run
# #[allow(unused_imports)] use hammerwork::queue::DatabaseQueue;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler, payload: serde_json::Value, job: hammerwork::Job) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::queue::DatabaseQueue;

let stats = queue.get_archival_stats(None).await?;
if stats.compression_ratio < 1.5 {
    ::tracing::warn!("Low compression ratio: {:.2}", stats.compression_ratio);
}
# Ok(())
# }
```

### 3. Preview Before Archiving Large Amounts

Use `ArchivalPolicy::should_archive` to reason about a policy and the CLI's `archive run --dry-run` to preview its effect before you run it for real.

### 4. Plan for Restoration

Archived jobs keep their payload, so a restore recreates the job as `Pending` with its original data. Restoring a job re-runs it, so make sure the handler is idempotent or that you restore only jobs that should run again.

## Performance Considerations

- **Batch size**: balance memory and transaction size against the number of passes
- **Compression**: higher levels use more CPU for smaller rows
- **Indexing**: the archive migration indexes the archive table; keep your own queries on indexed columns (`archived_at`, `queue_name`)
- **Scheduling**: run archival during low-traffic periods and give the job a low priority

## Security and Compliance

- Archived rows live in your database, so database access control, backups and encryption at rest apply to them
- Jobs encrypted with the `encryption` feature stay encrypted in the archive; see the [encryption guide](encryption.md)
- Every archived row records `archived_at`, `archival_reason` and `archived_by`
- Purging is the only way archived data is deleted; schedule it to match your retention requirements

## Troubleshooting

1. **Slow archival**: reduce the batch size, add capacity, or archive per queue
2. **High storage usage**: lower the retention periods and purge old archived jobs
3. **Nothing is archived**: check that the policy is `enabled`, that the status you expect has a retention set, and that jobs are old enough
4. **Restore fails with `JobNotFound`**: the job is not in the archive (it may have been purged)
