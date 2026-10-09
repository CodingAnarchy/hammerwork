# Database Migrations

Hammerwork provides a comprehensive migration system that allows you to progressively update your database schema while maintaining backward compatibility. This system replaces the old `create_tables()` method with a more robust, version-controlled approach.

## Overview

The migration system consists of:

- **Migration Framework**: Tracks which migrations have been executed
- **Versioned Migrations**: Each feature addition is a separate, numbered migration
- **Database-Specific SQL**: Separate SQL files for PostgreSQL and MySQL optimizations
- **Migration Binary**: Command-line tool for running migrations
- **Programmatic API**: Integrate migrations into your application

## Migration Structure

Migrations are organized chronologically and represent the evolution of Hammerwork's features:

1. **001_initial_schema** - Create initial hammerwork_jobs table
2. **002_add_priority** - Add priority field and indexes for job prioritization
3. **003_add_timeouts** - Add timeout_seconds and timed_out_at fields
4. **004_add_cron** - Add cron scheduling fields and indexes
5. **005_add_batches** - Add batch processing table and job batch_id field
6. **006_add_result_storage** - Add result storage fields for job execution results
7. **007_add_dependencies** - Add job dependencies and workflow support
8. **008_add_result_config** - Add result configuration storage fields
9. **009_add_tracing** - Add distributed tracing and correlation fields
10. **010_add_archival** - Add job archival support and archive table
11. **011_add_encryption** - Add encryption fields and key storage
12. **012_optimize_dependencies** - Optimize dependency lookups
13. **013_add_key_audit** - Add encryption key audit table
14. **014_add_queue_pause** - Add queue pause state table
15. **015_add_job_leases** - Add `last_heartbeat_at` and `lease_expires_at` for job leases and stale job recovery
16. **016_versioned_encryption_keys** - Keep every encryption key version
17. **017_add_kms_data_keys** - Store KMS-wrapped data keys for AWS/GCP key sources
18. **018_add_job_retry_strategy** - Add `retry_strategy` (JSON) so `Job::with_retry_strategy` is stored, and an index on `(batch_id, status)` for batch progress checks. Required: every job query reads the new column.

## Running Migrations

### Using the Cargo Subcommand (Recommended)

The easiest way to run migrations is using the cargo subcommand after building Hammerwork:

```bash
# Build the cargo subcommand
cargo build --bin cargo-hammerwork --features postgres

# Run migrations
cargo hammerwork migration run --database-url postgresql://localhost/hammerwork
cargo hammerwork migration run --database-url mysql://localhost/hammerwork

# Run migrations with drop (removes existing tables first)
cargo hammerwork migration run --database-url postgresql://localhost/hammerwork --drop

# Check migration status
cargo hammerwork migration status --database-url postgresql://localhost/hammerwork
```

### Application Integration

Once migrations are complete, your application can connect directly to the database:

```rust,no_run
use hammerwork::{Job, JobQueue, queue::DatabaseQueue};
use serde_json::json;
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Connect to database (schema already set up by migrations)
    let pool = sqlx::PgPool::connect("postgresql://localhost/hammerwork").await?;
    let queue = Arc::new(JobQueue::new(pool));
    
    // Start using the queue immediately - no setup required
    let job = Job::new("default".to_string(), json!({"task": "send_email"}));
    queue.enqueue(job).await?;
    
    Ok(())
}
```

### Example Usage

All Hammerwork examples now expect the database to be set up via migrations:

```bash
# First, run migrations
cargo hammerwork migration run --database-url postgresql://localhost/hammerwork

# Then run any example
cargo run --example postgres_example --features postgres
cargo run --example batch_example --features postgres
```

## Migration Safety

### Idempotent Operations

All migrations are designed to be idempotent - you can run them multiple times safely:

- Uses `CREATE TABLE IF NOT EXISTS` for table creation
- Uses `ADD COLUMN IF NOT EXISTS` for PostgreSQL column additions
- Checks for existing columns before adding them in MySQL

### Concurrent Runs

Several processes may migrate the same database at once: every replica of a service
starting with `auto_migrate`, or `cargo hammerwork migration run` while the application
starts. `MigrationManager::run_migrations` serializes them with a database lock held
for the whole run:

- PostgreSQL: a session-level `pg_advisory_lock` (key `0x686d72776b6d6967`, per
  database);
- MySQL: `GET_LOCK` on `hammerwork_migrations:` followed by the SHA-1 of the database
  name (lock names are server-wide), waiting up to 10 minutes.

The lock is taken on a dedicated connection that also runs every statement of the
run, and the list of applied migrations is read only after it is acquired, so a run
that waited sees what the previous one applied and runs nothing twice. The lock is
released when the run ends, also after an error; if the run is abandoned the
connection is closed, which releases it too. Recording a migration also ignores a row
that already exists (`ON CONFLICT DO NOTHING` / `INSERT IGNORE`), for runs by older
Hammerwork versions that take no lock.

A custom `MigrationRunner` can take part by implementing `acquire_migration_lock` and
`release_migration_lock`; the default implementations take no lock.

### Tracking

The migration system creates a `hammerwork_migrations` table to track which migrations have been executed:

```sql
-- PostgreSQL
CREATE TABLE hammerwork_migrations (
    migration_id VARCHAR NOT NULL PRIMARY KEY,
    executed_at TIMESTAMPTZ NOT NULL,
    execution_time_ms BIGINT NOT NULL
);

-- MySQL  
CREATE TABLE hammerwork_migrations (
    migration_id VARCHAR(255) NOT NULL PRIMARY KEY,
    executed_at TIMESTAMP(6) NOT NULL,
    execution_time_ms BIGINT NOT NULL
);
```

### Backward Compatibility

- All existing databases will work without changes
- The old `create_tables()` method is still available but deprecated
- New installations should use the migration system
- Migrations add features incrementally without breaking existing functionality

## Database Differences

The migration system handles differences between PostgreSQL and MySQL:

### PostgreSQL Optimizations
- Native UUID support
- JSONB for better performance and indexing
- Partial indexes with WHERE clauses for efficiency
- Timezone-aware timestamps (TIMESTAMPTZ)

### MySQL Adaptations
- String-based UUID storage (CHAR(36))
- Standard JSON columns
- Regular indexes (no partial index support)
- Microsecond precision timestamps

### Clocks and Time Zones

Due times are checked against the **database clock**: the dequeue (`scheduled_at <=
NOW()`), cron due checks, lease expiry and the stale-job reaper all compare with the
database's current time, and the timestamps of a run (`started_at`, `completed_at`,
`failed_at`, `timed_out_at`, heartbeats and leases) and the next cron run are taken
from it. Clock skew between application servers therefore no longer shifts
scheduling. A worker's retry backoff keeps its length on the database clock.
Absolute times the application sets (`Job::scheduled_at`, `with_delay`, a `retry_at`
passed to `retry_job`) are stored as given, so keep application and database clocks
synchronized (NTP) if you rely on them to the second.

On MySQL every time is written and compared as UTC (`UTC_TIMESTAMP(6)`, never the
session-local `NOW()`), so the session `time_zone` does not matter. sqlx sets it to
`+00:00` by default.

### MySQL `TIMESTAMP` Range (Year 2038)

The MySQL schema stores times in `TIMESTAMP(6)` columns, which hold
1970-01-01 00:00:01 to **2038-01-19 03:14:07 UTC**. The MySQL backend rejects times
outside that range with `HammerworkError::InvalidJobPayload` instead of a database
error: enqueueing a job scheduled after it, `retry_job`, `reschedule_cron_job` and
`store_job_result` with such a time. Times computed by the library are capped instead:
a worker's retry backoff that would end after it retries at the latest storable time,
a lease is capped, and a recurring job whose next occurrence falls after it is not
rescheduled (it keeps its final status, with a warning). PostgreSQL (`TIMESTAMPTZ`) has
no such limit.

Converting the columns to `DATETIME(6)` would lift the limit, but MySQL performs that
change by rebuilding each table (`ALTER TABLE ... MODIFY`, `ALGORITHM=COPY`), which
blocks writes to `hammerwork_jobs` for the duration. Because migrations run
automatically (`run_migrations`, `auto_migrate`), shipping it as a regular migration
would turn an upgrade into an unplanned write outage on large queues, so Hammerwork
does not do it.

## Migration Development

### Adding New Migrations

When adding new features to Hammerwork:

1. **Create Migration Files**: Add both PostgreSQL and MySQL versions
   ```text
   src/migrations/011_new_feature.postgres.sql
   src/migrations/011_new_feature.mysql.sql
   ```

2. **Register in Framework**: Add to `register_builtin_migrations()` in `src/migrations/mod.rs`
   ```text
   // Migration 011: Add new feature
   self.register_migration(
       Migration {
           id: "011_new_feature".to_string(),
           description: "Add new feature description".to_string(),
           version: 11,
           created_at: chrono::DateTime::parse_from_rfc3339("2025-11-01T00:00:00Z")
               .unwrap()
               .with_timezone(&Utc),
       },
       include_str!("011_new_feature.postgres.sql").to_string(),
       include_str!("011_new_feature.mysql.sql").to_string(),
   );
   ```

3. **Test Both Databases**: Ensure the migration works with both PostgreSQL and MySQL

### Migration SQL Guidelines

- Use database-specific optimizations when beneficial
- Ensure operations are reversible if needed
- Add appropriate indexes for performance
- Use standard SQL when possible for consistency

## Troubleshooting

### Migration Failures

If a migration fails:

1. Check the database logs for specific error messages
2. Ensure the database user has sufficient privileges
3. Verify the database connection URL is correct
4. Check that the feature flags match your database type

### Partial Migrations

The migration system is atomic - if any migration fails, the transaction is rolled back. This ensures your database doesn't end up in an inconsistent state.

### Rollbacks

Currently, the migration system doesn't support automatic rollbacks. If you need to rollback:

1. Restore from a database backup
2. Manually reverse the schema changes
3. Remove the migration record from `hammerwork_migrations`

## Integration with CI/CD

### Docker Deployments

```dockerfile
# Run migrations as part of container startup
RUN cargo build --release --bin cargo-hammerwork --features postgres
CMD ["sh", "-c", "/app/target/release/cargo-hammerwork migration run --database-url $DATABASE_URL && /app/target/release/myapp"]
```

### Kubernetes Jobs

```yaml
apiVersion: batch/v1
kind: Job
metadata:
  name: hammerwork-migrate
spec:
  template:
    spec:
      containers:
      - name: migrate
        image: myapp:latest
        command: ["/app/target/release/cargo-hammerwork"]
        args: ["migration", "run", "--database-url", "$(DATABASE_URL)"]
        env:
        - name: DATABASE_URL
          valueFrom:
            secretKeyRef:
              name: database-secret
              key: url
      restartPolicy: OnFailure
```

## Performance Considerations

- Migrations are typically run during deployment, not in production traffic
- Large table alterations may require maintenance windows
- Index creation can be time-consuming on large datasets
- Consider the impact of migrations on application startup time

## Best Practices

1. **Run Migrations Early**: Execute migrations before starting your application
2. **Test Migrations**: Always test migrations on a copy of production data
3. **Monitor Execution**: Use the migration status command to verify completion
4. **Backup First**: Take database backups before running migrations in production
5. **Use the Binary**: The migration binary provides better error handling and logging than programmatic execution