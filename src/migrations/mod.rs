//! Database migration system for Hammerwork.
//!
//! This module provides a migration framework that allows progressive schema updates
//! while maintaining backward compatibility. Migrations are versioned and tracked
//! to ensure each migration runs only once.
//!
//! ## Quick Start
//!
//! The easiest way to run migrations is using the cargo subcommand:
//!
//! ```bash
//! # Build the subcommand
//! cargo build --bin cargo-hammerwork --features postgres
//!
//! # Run migrations
//! cargo hammerwork migrate --database-url postgresql://localhost/hammerwork
//!
//! # Check status
//! cargo hammerwork status --database-url postgresql://localhost/hammerwork
//! ```
//!
//! ## Application Usage
//!
//! Once migrations are complete, your application simply connects to the database:
//!
//! ```rust,no_run
//! use hammerwork::{Job, JobQueue, queue::DatabaseQueue};
//! use serde_json::json;
//! use std::sync::Arc;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     // Database schema is already set up by cargo hammerwork migrate
//!     let pool = sqlx::PgPool::connect("postgresql://localhost/hammerwork").await?;
//!     let queue = Arc::new(JobQueue::new(pool));
//!     
//!     // Start using immediately
//!     let job = Job::new("queue".to_string(), json!({"task": "work"}));
//!     queue.enqueue(job).await?;
//!     Ok(())
//! }
//! ```

#[cfg(feature = "postgres")]
pub mod postgres;

#[cfg(feature = "mysql")]
pub mod mysql;

use crate::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::Database;
use std::collections::HashMap;
use tracing::info;

/// Migration identifier and metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Migration {
    /// Unique migration identifier (e.g., "001_initial_schema")
    pub id: String,
    /// Human-readable description of what this migration does
    pub description: String,
    /// Version number for ordering (e.g., 1, 2, 3...)
    pub version: u32,
    /// When this migration was created
    pub created_at: DateTime<Utc>,
}

/// Migration execution record
#[derive(Debug, Clone)]
pub struct MigrationRecord {
    pub migration_id: String,
    pub executed_at: DateTime<Utc>,
    pub execution_time_ms: u64,
}

/// Migration runner trait for database-specific implementations
#[async_trait::async_trait]
pub trait MigrationRunner<DB: Database> {
    /// Run a specific migration SQL for this database type
    async fn run_migration(&self, migration: &Migration, sql: &str) -> Result<()>;

    /// Check if the migration tracking table exists
    async fn migration_table_exists(&self) -> Result<bool>;

    /// Create the migration tracking table
    async fn create_migration_table(&self) -> Result<()>;

    /// Get list of migrations that have already been executed
    async fn get_executed_migrations(&self) -> Result<Vec<MigrationRecord>>;

    /// Record that a migration has been executed
    async fn record_migration(&self, migration: &Migration, execution_time_ms: u64) -> Result<()>;
}

/// Main migration manager
pub struct MigrationManager<DB: Database> {
    runner: Box<dyn MigrationRunner<DB> + Send + Sync>,
    migrations: HashMap<String, (Migration, String, String)>, // id -> (metadata, postgres_sql, mysql_sql)
}

impl<DB: Database> MigrationManager<DB> {
    /// Create a new migration manager
    pub fn new(runner: Box<dyn MigrationRunner<DB> + Send + Sync>) -> Self {
        let mut manager = Self {
            runner,
            migrations: HashMap::new(),
        };

        // Register all built-in migrations
        manager.register_builtin_migrations();
        manager
    }

    /// Register a migration with both PostgreSQL and MySQL SQL
    pub fn register_migration(
        &mut self,
        migration: Migration,
        postgres_sql: String,
        mysql_sql: String,
    ) {
        self.migrations
            .insert(migration.id.clone(), (migration, postgres_sql, mysql_sql));
    }

    /// Run all pending migrations
    pub async fn run_migrations(&self) -> Result<()> {
        info!("Starting migration process...");

        // Ensure migration table exists
        if !self.runner.migration_table_exists().await? {
            info!("Creating migration tracking table...");
            self.runner.create_migration_table().await?;
        }

        // Get executed migrations
        let executed = self.runner.get_executed_migrations().await?;
        let executed_ids: std::collections::HashSet<String> =
            executed.iter().map(|r| r.migration_id.clone()).collect();

        // Sort migrations by version
        let mut pending_migrations: Vec<_> = self
            .migrations
            .values()
            .filter(|(migration, _, _)| !executed_ids.contains(&migration.id))
            .collect();
        pending_migrations.sort_by_key(|(migration, _, _)| migration.version);

        if pending_migrations.is_empty() {
            info!("No pending migrations to run");
            return Ok(());
        }

        info!("Found {} pending migrations", pending_migrations.len());

        // Run each pending migration
        for (migration, postgres_sql, mysql_sql) in pending_migrations {
            info!(
                "Running migration: {} - {}",
                migration.id, migration.description
            );

            let start_time = std::time::Instant::now();

            // Choose SQL based on database type
            let sql = if std::any::type_name::<DB>().contains("Postgres") {
                postgres_sql
            } else {
                mysql_sql
            };

            // Execute the migration
            self.runner.run_migration(migration, sql).await?;

            let execution_time_ms = start_time.elapsed().as_millis() as u64;

            // Record successful execution
            self.runner
                .record_migration(migration, execution_time_ms)
                .await?;

            info!(
                "Completed migration {} in {}ms",
                migration.id, execution_time_ms
            );
        }

        info!("All migrations completed successfully");
        Ok(())
    }

    /// Get status of all migrations
    pub async fn get_migration_status(&self) -> Result<Vec<(Migration, bool)>> {
        let executed = self.runner.get_executed_migrations().await?;
        let executed_ids: std::collections::HashSet<String> =
            executed.iter().map(|r| r.migration_id.clone()).collect();

        let mut status: Vec<_> = self
            .migrations
            .values()
            .map(|(migration, _, _)| (migration.clone(), executed_ids.contains(&migration.id)))
            .collect();
        status.sort_by_key(|(migration, _)| migration.version);

        Ok(status)
    }

    /// Register all built-in migrations
    fn register_builtin_migrations(&mut self) {
        // Migration 001: Initial schema with basic job table
        self.register_migration(
            Migration {
                id: "001_initial_schema".to_string(),
                description: "Create initial hammerwork_jobs table".to_string(),
                version: 1,
                created_at: chrono::DateTime::parse_from_rfc3339("2025-01-01T00:00:00Z")
                    .unwrap()
                    .with_timezone(&Utc),
            },
            include_str!("001_initial_schema.postgres.sql").to_string(),
            include_str!("001_initial_schema.mysql.sql").to_string(),
        );

        // Migration 002: Add priority system
        self.register_migration(
            Migration {
                id: "002_add_priority".to_string(),
                description: "Add priority field and indexes for job prioritization".to_string(),
                version: 2,
                created_at: chrono::DateTime::parse_from_rfc3339("2025-02-01T00:00:00Z")
                    .unwrap()
                    .with_timezone(&Utc),
            },
            include_str!("002_add_priority.postgres.sql").to_string(),
            include_str!("002_add_priority.mysql.sql").to_string(),
        );

        // Migration 003: Add timeout functionality
        self.register_migration(
            Migration {
                id: "003_add_timeouts".to_string(),
                description: "Add timeout_seconds and timed_out_at fields".to_string(),
                version: 3,
                created_at: chrono::DateTime::parse_from_rfc3339("2025-03-01T00:00:00Z")
                    .unwrap()
                    .with_timezone(&Utc),
            },
            include_str!("003_add_timeouts.postgres.sql").to_string(),
            include_str!("003_add_timeouts.mysql.sql").to_string(),
        );

        // Migration 004: Add cron scheduling
        self.register_migration(
            Migration {
                id: "004_add_cron".to_string(),
                description: "Add cron scheduling fields and indexes".to_string(),
                version: 4,
                created_at: chrono::DateTime::parse_from_rfc3339("2025-04-01T00:00:00Z")
                    .unwrap()
                    .with_timezone(&Utc),
            },
            include_str!("004_add_cron.postgres.sql").to_string(),
            include_str!("004_add_cron.mysql.sql").to_string(),
        );

        // Migration 005: Add batch processing
        self.register_migration(
            Migration {
                id: "005_add_batches".to_string(),
                description: "Add batch processing table and job batch_id field".to_string(),
                version: 5,
                created_at: chrono::DateTime::parse_from_rfc3339("2025-05-01T00:00:00Z")
                    .unwrap()
                    .with_timezone(&Utc),
            },
            include_str!("005_add_batches.postgres.sql").to_string(),
            include_str!("005_add_batches.mysql.sql").to_string(),
        );

        // Migration 006: Add result storage
        self.register_migration(
            Migration {
                id: "006_add_result_storage".to_string(),
                description: "Add result storage fields for job execution results".to_string(),
                version: 6,
                created_at: chrono::DateTime::parse_from_rfc3339("2025-06-01T00:00:00Z")
                    .unwrap()
                    .with_timezone(&Utc),
            },
            include_str!("006_add_result_storage.postgres.sql").to_string(),
            include_str!("006_add_result_storage.mysql.sql").to_string(),
        );

        // Migration 007: Add job dependencies for workflow support
        self.register_migration(
            Migration {
                id: "007_add_dependencies".to_string(),
                description: "Add job dependencies and workflow support".to_string(),
                version: 7,
                created_at: chrono::DateTime::parse_from_rfc3339("2025-07-01T00:00:00Z")
                    .unwrap()
                    .with_timezone(&Utc),
            },
            include_str!("007_add_dependencies.postgres.sql").to_string(),
            include_str!("007_add_dependencies.mysql.sql").to_string(),
        );

        // Migration 008: Add result configuration storage
        self.register_migration(
            Migration {
                id: "008_add_result_config".to_string(),
                description: "Add result configuration storage fields".to_string(),
                version: 8,
                created_at: chrono::DateTime::parse_from_rfc3339("2025-08-01T00:00:00Z")
                    .unwrap()
                    .with_timezone(&Utc),
            },
            include_str!("008_add_result_config.postgres.sql").to_string(),
            include_str!("008_add_result_config.mysql.sql").to_string(),
        );

        // Migration 009: Add job tracing and correlation support
        self.register_migration(
            Migration {
                id: "009_add_tracing".to_string(),
                description: "Add distributed tracing and correlation fields".to_string(),
                version: 9,
                created_at: chrono::DateTime::parse_from_rfc3339("2025-09-01T00:00:00Z")
                    .unwrap()
                    .with_timezone(&Utc),
            },
            include_str!("009_add_tracing.postgres.sql").to_string(),
            include_str!("009_add_tracing.mysql.sql").to_string(),
        );

        // Migration 010: Add job archival support and archive table
        self.register_migration(
            Migration {
                id: "010_add_archival".to_string(),
                description: "Add job archival support and archive table".to_string(),
                version: 10,
                created_at: chrono::DateTime::parse_from_rfc3339("2025-10-01T00:00:00Z")
                    .unwrap()
                    .with_timezone(&Utc),
            },
            include_str!("010_add_archival.postgres.sql").to_string(),
            include_str!("010_add_archival.mysql.sql").to_string(),
        );

        // Migration 011: Add encryption support for job payloads
        self.register_migration(
            Migration {
                id: "011_add_encryption".to_string(),
                description: "Add job payload encryption and key management".to_string(),
                version: 11,
                created_at: chrono::DateTime::parse_from_rfc3339("2025-11-01T00:00:00Z")
                    .unwrap()
                    .with_timezone(&Utc),
            },
            include_str!("011_add_encryption.postgres.sql").to_string(),
            include_str!("011_add_encryption.mysql.sql").to_string(),
        );

        // Migration 012: Optimize dependencies using native PostgreSQL arrays
        self.register_migration(
            Migration {
                id: "012_optimize_dependencies".to_string(),
                description: "Optimize job dependencies using native PostgreSQL UUID arrays"
                    .to_string(),
                version: 12,
                created_at: chrono::DateTime::parse_from_rfc3339("2025-12-01T00:00:00Z")
                    .unwrap()
                    .with_timezone(&Utc),
            },
            include_str!("012_optimize_dependencies.postgres.sql").to_string(),
            include_str!("012_optimize_dependencies.mysql.sql").to_string(),
        );
    }
}
