# Job Archiving & Retention

Hammerwork provides a comprehensive job archiving system with policy-driven archival, configurable retention periods, payload compression, and automated cleanup for compliance and performance optimization.

## Overview

The archiving system enables you to:
- Automatically archive completed, failed, and timed-out jobs based on configurable policies
- Compress job payloads to reduce storage requirements
- Maintain compliance with data retention requirements
- Restore archived jobs when needed
- Permanently purge old archived data
- Monitor archival statistics and compression ratios
- Integrate archival operations into your operational workflows

## Architecture

### Archive Tables

Hammerwork maintains separate archive tables for performance:
- **`hammerwork_jobs`** - Active jobs table for high-performance operations
- **`hammerwork_jobs_archive`** - Archive table with compressed payloads and full metadata
- **`hammerwork_migrations`** - Tracks archival feature migration status

### Key Benefits

- **Performance**: Keeps the main jobs table small and fast
- **Compliance**: Meets data retention and audit requirements
- **Storage Efficiency**: Compresses payloads to reduce storage costs
- **Flexibility**: Configurable policies per queue or globally
- **Recoverability**: Archived jobs can be restored if needed

## Archival Policies

### Creating Archival Policies

```rust
use hammerwork::archive::{ArchivalPolicy, ArchivalConfig, ArchivalReason};
use chrono::Duration;

// Create a comprehensive archival policy
let policy = ArchivalPolicy::new()
    .archive_completed_after(Duration::days(7))      // Archive completed jobs after 7 days
    .archive_failed_after(Duration::days(30))        // Keep failed jobs for debugging
    .archive_dead_after(Duration::days(14))         // Archive dead jobs after 2 weeks
    .archive_timed_out_after(Duration::days(21))    // Archive timed out jobs after 3 weeks
    .purge_archived_after(Duration::days(365))      // Purge archived jobs after 1 year
    .compress_archived_payloads(true)               // Enable payload compression
    .with_batch_size(1000)                          // Process 1000 jobs per batch
    .enabled(true);                                 // Enable the policy

// Configure compression settings
let config = ArchivalConfig::new()
    .with_compression_level(6)                      // Balanced compression (0-9)
    .with_compression_verification(true)            // Verify compression integrity
    .with_min_payload_size_for_compression(1024);  // Only compress payloads > 1KB
```

### Policy Configuration Options

```rust
// Fine-grained archival control
let policy = ArchivalPolicy::new()
    // Status-based retention periods
    .archive_completed_after(Duration::days(3))     // Quick archival for completed jobs
    .archive_failed_after(Duration::days(90))       // Keep failed jobs longer for analysis
    .archive_dead_after(Duration::days(7))         // Dead jobs archived quickly
    .archive_timed_out_after(Duration::days(14))   // Timeout jobs need investigation
    
    // Global settings
    .purge_archived_after(Duration::days(2555))    // 7 years for compliance (e.g., GDPR)
    .with_batch_size(500)                          // Smaller batches for incremental processing
    .compress_archived_payloads(true)              // Always compress to save space
    
    // Queue-specific policies
    .with_queue_specific_policy("critical_queue", Duration::days(180)) // Keep critical jobs longer
    .with_queue_specific_policy("logs_queue", Duration::days(1))       // Archive logs quickly
    
    // Control settings
    .enabled(true)                                 // Enable automatic archival
    .dry_run(false);                              // Set to true for testing policies
```

## Running Archival Operations

### Manual Archival

```rust
use hammerwork::{archive::{ArchivalReason, ArchivalStats}, queue::DatabaseQueue};

// Run archival for all queues
let stats = queue.archive_jobs(
    None,                           // Archive all queues (or specify queue name)
    &policy,                        // Use the archival policy
    &config,                        // Use compression config
    ArchivalReason::Manual,         // Manual execution
    Some("admin_user")              // Who initiated the archival
).await?;

// Print archival results
println!("Archival completed:");
println!("  Jobs archived: {}", stats.jobs_archived);
println!("  Bytes saved: {} ({:.2}% compression)", 
         stats.bytes_saved, stats.compression_ratio * 100.0);
println!("  Execution time: {:?}", stats.execution_time);
```

### Queue-specific Archival

```rust
// Archive only specific queues
let email_stats = queue.archive_jobs(
    Some("email_queue"),            // Only archive email queue
    &email_policy,                  // Email-specific policy
    &config,
    ArchivalReason::Automatic,
    Some("cron_scheduler")
).await?;

let batch_stats = queue.archive_jobs(
    Some("batch_processing_queue"), // Only archive batch processing queue
    &batch_policy,                  // Batch-specific policy  
    &config,
    ArchivalReason::Maintenance,
    Some("maintenance_window")
).await?;
```

### Scheduled Archival

Set up automatic archival with cron jobs:

```rust
use hammerwork::{Job, JobPriority};

// Create a daily archival job
let archival_job = Job::new("system_maintenance".to_string(), json!({
    "operation": "archive_jobs",
    "policy": "default_policy",
    "queues": ["email_queue", "batch_queue", "analytics_queue"]
}))
.with_cron_schedule("0 2 * * *")              // Run at 2 AM daily
.with_priority(JobPriority::Low)              // Low priority to not interfere
.with_timeout(Duration::from_hours(2));       // 2-hour timeout for archival

queue.enqueue(archival_job).await?;

// Job handler for archival
let archival_handler = Arc::new(|job: Job| {
    Box::pin(async move {
        let queue = get_queue_from_context(&job).await?;
        
        // Load archival policy from configuration
        let policy = load_archival_policy("default_policy")?;
        let config = ArchivalConfig::default();
        
        // Run archival
        let stats = queue.archive_jobs(
            None,                           // All queues
            &policy,
            &config,
            ArchivalReason::Automatic,
            Some("scheduler")
        ).await?;
        
        // Log results
        tracing::info!(
            jobs_archived = stats.jobs_archived,
            compression_ratio = stats.compression_ratio,
            "Daily archival completed"
        );
        
        Ok(())
    })
});
```

## Compression and Storage

### Compression Configuration

```rust
// High compression for long-term storage
let high_compression_config = ArchivalConfig::new()
    .with_compression_level(9)                      // Maximum compression
    .with_compression_verification(true)            // Always verify
    .with_min_payload_size_for_compression(512);   // Compress payloads > 512 bytes

// Fast compression for frequent archival
let fast_compression_config = ArchivalConfig::new()
    .with_compression_level(1)                      // Fast compression
    .with_compression_verification(false)           // Skip verification for speed
    .with_min_payload_size_for_compression(2048);  // Only compress larger payloads

// No compression for debugging
let no_compression_config = ArchivalConfig::new()
    .with_compression_level(0)                      // No compression
    .with_compression_verification(false);          // No verification needed
```

### Storage Optimization

```rust
// Calculate storage savings
let stats = queue.get_archival_stats().await?;

println!("Storage optimization:");
println!("  Total archived jobs: {}", stats.total_archived_jobs);
println!("  Original size: {} MB", stats.original_total_size / 1_048_576);
println!("  Compressed size: {} MB", stats.compressed_total_size / 1_048_576);
println!("  Space saved: {} MB ({:.1}% compression)", 
         (stats.original_total_size - stats.compressed_total_size) / 1_048_576,
         stats.average_compression_ratio * 100.0);
```

## Restoring Archived Jobs

### Individual Job Restoration

```rust
// Restore a specific archived job
let job_id = uuid::Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000")?;
let restored_job = queue.restore_archived_job(job_id).await?;

println!("Restored job: {} from queue: {}", 
         restored_job.id, restored_job.queue_name);

// The job is now back in the main jobs table with 'pending' status
// and can be processed normally
```

### Bulk Restoration

```rust
// Restore multiple jobs based on criteria
let archived_jobs = queue.list_archived_jobs(
    Some("critical_queue"),         // From specific queue
    Some(100),                      // Limit to 100 jobs
    Some(0)                         // Starting from offset 0
).await?;

let mut restored_count = 0;
for archived_job in archived_jobs {
    // Only restore jobs from the last 24 hours
    if archived_job.archived_at > Utc::now() - Duration::hours(24) {
        queue.restore_archived_job(archived_job.job_id).await?;
        restored_count += 1;
    }
}

println!("Restored {} jobs from archive", restored_count);
```

### Conditional Restoration

```rust
// Restore jobs based on business logic
let failed_payment_jobs = queue.list_archived_jobs(
    Some("payment_queue"),
    Some(1000),
    Some(0)
).await?;

for archived_job in failed_payment_jobs {
    // Parse the job payload to check conditions
    let payload: serde_json::Value = serde_json::from_str(&archived_job.payload)?;
    
    // Restore high-value failed payments for retry
    if archived_job.status == "failed" {
        if let Some(amount) = payload.get("amount").and_then(|a| a.as_f64()) {
            if amount > 1000.0 {  // High value transactions
                let restored = queue.restore_archived_job(archived_job.job_id).await?;
                println!("Restored high-value payment job: ${}", amount);
            }
        }
    }
}
```

## Querying Archived Jobs

### List Archived Jobs

```rust
// Basic listing with pagination
let archived_jobs = queue.list_archived_jobs(
    None,                           // All queues
    Some(50),                       // 50 jobs per page
    Some(100)                       // Skip first 100 (page 3)
).await?;

for job in archived_jobs {
    println!("Job: {} | Queue: {} | Archived: {} | Status: {}", 
             job.job_id, job.queue_name, job.archived_at, job.status);
}
```

### Advanced Filtering

```rust
// Query with advanced filters
let recent_failed_jobs = queue.query_archived_jobs()
    .queue("payment_processing")
    .status(JobStatus::Failed)
    .archived_after(Utc::now() - Duration::days(7))
    .archived_before(Utc::now() - Duration::days(1))
    .limit(100)
    .execute()
    .await?;
```

### Search by Content

```rust
// Search archived jobs by payload content
let jobs_with_customer = queue.search_archived_jobs(
    "customer_id",                  // Search key
    "12345",                        // Search value
    Some("orders_queue"),           // Optional queue filter
    Some(20),                       // Limit results
).await?;

for job in jobs_with_customer {
    println!("Found job {} containing customer_id: 12345", job.job_id);
}
```

## Purging Archived Jobs

### Automatic Purging

```rust
// Purge jobs older than policy allows
let purged_count = queue.purge_archived_jobs(
    Utc::now() - Duration::days(365)  // Delete jobs archived over 1 year ago
).await?;

println!("Permanently purged {} archived jobs", purged_count);
```

### Selective Purging

```rust
// Purge specific queues or job types
let test_jobs_purged = queue.purge_archived_jobs_by_queue(
    "test_queue",                     // Only purge test queue
    Utc::now() - Duration::days(30)   // Delete test jobs older than 30 days
).await?;

let failed_jobs_purged = queue.purge_archived_jobs_by_status(
    JobStatus::Failed,                // Only purge failed jobs
    Utc::now() - Duration::days(90)   // Delete failed jobs older than 90 days
).await?;

println!("Purged {} test jobs and {} failed jobs", 
         test_jobs_purged, failed_jobs_purged);
```

### Dry Run Purging

```rust
// Test purging without actually deleting
let purge_stats = queue.preview_purge_operation(
    Utc::now() - Duration::days(365)
).await?;

println!("Purge preview:");
println!("  Jobs to be purged: {}", purge_stats.jobs_to_purge);
println!("  Storage to be freed: {} MB", purge_stats.storage_to_free / 1_048_576);
println!("  Queues affected: {:?}", purge_stats.affected_queues);

// If the preview looks good, run the actual purge
if purge_stats.jobs_to_purge < 10000 {  // Safety check
    let actual_purged = queue.purge_archived_jobs(
        Utc::now() - Duration::days(365)
    ).await?;
    println!("Actually purged: {} jobs", actual_purged);
}
```

## CLI Archival Commands

The `cargo-hammerwork` CLI provides comprehensive archival management:

### Basic Archive Operations

```bash
# Run archival with default policy
cargo hammerwork archive run --database-url postgres://localhost/hammerwork

# Run archival for specific queue
cargo hammerwork archive run --queue email_queue --policy email_policy.json

# Dry run to preview archival
cargo hammerwork archive run --dry-run --verbose

# Archive with custom settings
cargo hammerwork archive run \
  --compression-level 9 \
  --batch-size 500 \
  --reason maintenance \
  --initiated-by admin
```

### Restoration Commands

```bash
# Restore a specific job
cargo hammerwork archive restore JOB_ID

# List archived jobs
cargo hammerwork archive list --queue payment_queue --limit 100

# List with filtering
cargo hammerwork archive list \
  --status failed \
  --archived-after "2024-01-01" \
  --archived-before "2024-01-31" \
  --format json

# Restore multiple jobs
cargo hammerwork archive restore --queue critical_queue --status failed --count 10
```

### Statistics and Monitoring

```bash
# Show archival statistics
cargo hammerwork archive stats

# Show detailed statistics for specific queue
cargo hammerwork archive stats --queue batch_processing --detailed

# Monitor archival operations
cargo hammerwork archive stats --watch --interval 5s
```

### Purge Operations

```bash
# Preview purge operation
cargo hammerwork archive purge --older-than "365 days" --dry-run

# Purge old archived jobs
cargo hammerwork archive purge --older-than "2 years" --confirm

# Purge specific queue
cargo hammerwork archive purge --queue test_queue --older-than "30 days" --confirm
```

### Policy Management

```bash
# Show current archival policy
cargo hammerwork archive get-policy

# Set new archival policy from file
cargo hammerwork archive set-policy --file archival_policy.json

# Remove archival policy (disable automatic archival)
cargo hammerwork archive remove-policy --confirm
```

## Web Dashboard Integration

The Hammerwork web dashboard includes archival management:

### Archive Dashboard

Access archival features at `http://localhost:8080/archive`:
- View archival statistics and trends
- Monitor compression ratios and storage savings
- Browse archived jobs with advanced filtering
- Restore individual jobs or bulk operations
- Configure archival policies
- Schedule archival operations

### REST API Endpoints

```bash
# Get archival statistics
curl http://localhost:8080/api/archive/stats

# List archived jobs
curl "http://localhost:8080/api/archive/jobs?queue=email_queue&limit=50"

# Restore a job
curl -X POST http://localhost:8080/api/archive/jobs/JOB_ID/restore

# Run archival
curl -X POST http://localhost:8080/api/archive/run \
  -H "Content-Type: application/json" \
  -d '{"queue": "batch_queue", "reason": "manual", "dry_run": false}'

# Purge old jobs
curl -X DELETE "http://localhost:8080/api/archive/purge?older_than=365"
```

## Integration Examples

### Automated Compliance Archival

```rust
// GDPR-compliant archival for EU customers
let gdpr_policy = ArchivalPolicy::new()
    .archive_completed_after(Duration::days(30))   // Archive completed jobs quickly
    .purge_archived_after(Duration::days(2555))    // 7 years retention for GDPR
    .compress_archived_payloads(true)              // Minimize storage
    .with_gdpr_compliance(true)                    // Enable GDPR-specific handling
    .enabled(true);

// Run daily GDPR archival
let gdpr_job = Job::new("gdpr_compliance".to_string(), json!({
    "operation": "gdpr_archival",
    "data_subject_regions": ["EU", "UK"],
    "retention_policy": "7_years"
}))
.with_cron_schedule("0 3 * * *")  // 3 AM daily
.with_priority(JobPriority::High); // High priority for compliance
```

### Development Environment Cleanup

```rust
// Aggressive archival for development environments
let dev_policy = ArchivalPolicy::new()
    .archive_completed_after(Duration::hours(1))    // Archive completed jobs quickly
    .archive_failed_after(Duration::hours(24))      // Keep failed jobs for debugging
    .purge_archived_after(Duration::days(7))        // Short retention in dev
    .compress_archived_payloads(false)              // Skip compression for speed
    .with_batch_size(2000)                          // Larger batches for speed
    .enabled(true);

// Clean up development data hourly
let cleanup_job = Job::new("dev_cleanup".to_string(), json!({
    "environment": "development",
    "operation": "cleanup_old_jobs"
}))
.with_cron_schedule("0 * * * *");  // Every hour
```

### Cost Optimization Strategy

```rust
// Cost-optimized archival for cloud storage
let cost_optimized_policy = ArchivalPolicy::new()
    .archive_completed_after(Duration::days(1))     // Quick archival to save storage
    .archive_failed_after(Duration::days(7))        // Keep failures longer
    .purge_archived_after(Duration::days(90))       // Balance cost vs. utility
    .compress_archived_payloads(true)               // Maximum compression
    .with_batch_size(5000)                          // Large batches for efficiency
    .enabled(true);

let cost_config = ArchivalConfig::new()
    .with_compression_level(9)                      // Maximum compression
    .with_compression_verification(true)            // Ensure integrity
    .with_storage_class("GLACIER")                  // Use cheaper storage class
    .with_lifecycle_management(true);               // Enable automatic transitions
```

## Best Practices

### 1. Design Archival Policies Carefully

```rust
// Good: Status-specific retention periods
let policy = ArchivalPolicy::new()
    .archive_completed_after(Duration::days(7))     // Quick archival for completed
    .archive_failed_after(Duration::days(30))       // Longer retention for debugging
    .archive_dead_after(Duration::days(14))         // Medium retention for analysis
    .purge_archived_after(Duration::days(365));     // Long-term compliance

// Avoid: One-size-fits-all approach
let bad_policy = ArchivalPolicy::new()
    .archive_all_after(Duration::days(1))           // Too aggressive
    .purge_archived_after(Duration::days(30));      // Too short for compliance
```

### 2. Monitor Archival Performance

```rust
// Track archival metrics
let stats = queue.get_archival_stats().await?;
if stats.average_compression_ratio < 0.3 {
    tracing::warn!("Low compression ratio: {:.2}%", 
                   stats.average_compression_ratio * 100.0);
}

if stats.archival_duration > Duration::from_hours(1) {
    tracing::warn!("Archival taking too long: {:?}", stats.archival_duration);
}
```

### 3. Test Archival Policies

```rust
// Always test with dry runs first
let test_policy = production_policy.clone().dry_run(true);
let preview_stats = queue.archive_jobs(
    Some("test_queue"),
    &test_policy,
    &config,
    ArchivalReason::Manual,
    Some("policy_test")
).await?;

println!("Dry run results: {} jobs would be archived", 
         preview_stats.jobs_that_would_be_archived);
```

### 4. Plan for Restoration

```rust
// Include restoration metadata
let policy = ArchivalPolicy::new()
    .with_restoration_metadata(true)                // Store restoration hints
    .with_business_context_preservation(true)       // Keep business metadata
    .archive_completed_after(Duration::days(7));

// Document restoration procedures
let restoration_job = Job::new("document_restoration".to_string(), json!({
    "procedure": "restoration_playbook.md",
    "contact": "data-team@company.com",
    "sla": "4 hours for critical jobs"
}));
```

### 5. Implement Monitoring and Alerting

```rust
// Set up archival monitoring
let monitoring_job = Job::new("archival_monitoring".to_string(), json!({
    "check_archival_health": true,
    "alert_on_failures": true,
    "metrics": ["compression_ratio", "archival_duration", "storage_savings"]
}))
.with_cron_schedule("*/15 * * * *");  // Check every 15 minutes

// Alert on archival issues
if archival_stats.failed_jobs > 0 {
    send_alert(&format!("Archival failed for {} jobs", archival_stats.failed_jobs)).await?;
}
```

## Performance Considerations

- **Batch Size**: Balance between memory usage and database efficiency
- **Compression**: Higher levels use more CPU but save more storage
- **Indexing**: Ensure proper indexes on `created_at`, `status`, and `queue_name`
- **Scheduling**: Run archival during low-traffic periods
- **Monitoring**: Track archival duration and success rates

## Security and Compliance

- **Data Encryption**: Archived data inherits encryption from database
- **Access Control**: Use database permissions to control archive access
- **Audit Trail**: All archival operations are logged with timestamps and initiators
- **Data Residency**: Consider data location requirements for archived jobs
- **GDPR/Privacy**: Implement right-to-deletion for personal data in archives

## Troubleshooting

### Common Issues

1. **Slow Archival**: Reduce batch size or improve database performance
2. **High Storage Usage**: Increase compression level or reduce retention periods
3. **Restoration Failures**: Check archived job integrity and compression verification
4. **Policy Conflicts**: Validate policy settings and queue-specific overrides

### Debugging

```bash
# Check archival logs
cargo hammerwork archive stats --verbose --debug

# Verify compression integrity
cargo hammerwork archive verify --queue problematic_queue

# Test restoration
cargo hammerwork archive restore JOB_ID --dry-run --verbose
```