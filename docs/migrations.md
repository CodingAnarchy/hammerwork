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

```rust
use hammerwork::{Job, JobQueue, Worker, WorkerPool};
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

## Migration Development

### Adding New Migrations

When adding new features to Hammerwork:

1. **Create Migration Files**: Add both PostgreSQL and MySQL versions
   ```
   src/migrations/011_new_feature.postgres.sql
   src/migrations/011_new_feature.mysql.sql
   ```

2. **Register in Framework**: Add to `register_builtin_migrations()` in `src/migrations/mod.rs`
   ```rust
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