//! Database connection and migration utilities.
//!
//! This module provides abstractions for working with both PostgreSQL and MySQL
//! databases in the Hammerwork CLI. It handles connection pooling, automatic
//! database type detection, and migration management.
//!
//! # Examples
//!
//! ## Connecting to a Database
//!
//! ```rust,no_run
//! use cargo_hammerwork::utils::database::DatabasePool;
//!
//! # #[tokio::main]
//! # async fn main() -> Result<(), Box<dyn std::error::Error>> {
//! // Connect to PostgreSQL
//! let pg_pool = DatabasePool::connect(
//!     "postgresql://localhost/hammerwork",
//!     5  // connection pool size
//! ).await?;
//!
//! // Connect to MySQL
//! let mysql_pool = DatabasePool::connect(
//!     "mysql://localhost/hammerwork", 
//!     5
//! ).await?;
//! # Ok(())
//! # }
//! ```
//!
//! ## Running Migrations
//!
//! ```rust,no_run
//! use cargo_hammerwork::utils::database::DatabasePool;
//!
//! # #[tokio::main]
//! # async fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let pool = DatabasePool::connect("postgresql://localhost/hammerwork", 5).await?;
//!
//! // Run all pending migrations
//! pool.migrate(false).await?;
//!
//! // Drop existing tables and run migrations from scratch
//! pool.migrate(true).await?;
//! # Ok(())
//! # }
//! ```
//!
//! ## Creating a Job Queue
//!
//! ```rust,no_run
//! use cargo_hammerwork::utils::database::DatabasePool;
//!
//! # #[tokio::main]
//! # async fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let pool = DatabasePool::connect("postgresql://localhost/hammerwork", 5).await?;
//! let job_queue = pool.create_job_queue();
//!
//! // Now you can use the job queue for operations
//! // The wrapper automatically handles PostgreSQL vs MySQL differences
//! # Ok(())
//! # }
//! ```

use anyhow::Result;
use hammerwork::{
    JobQueue,
    migrations::{
        MigrationManager, mysql::MySqlMigrationRunner, postgres::PostgresMigrationRunner,
    },
};
use sqlx::{MySqlPool, PgPool};
use tracing::info;

/// Database connection pool abstraction.
///
/// This enum wraps either a PostgreSQL or MySQL connection pool,
/// providing a unified interface for database operations.
///
/// # Database URL Format
///
/// - PostgreSQL: `postgres://` or `postgresql://`
/// - MySQL: `mysql://`
///
/// # Examples
///
/// ```rust,no_run
/// use cargo_hammerwork::utils::database::DatabasePool;
///
/// # #[tokio::main]
/// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
/// // The database type is automatically detected from the URL
/// let pool = DatabasePool::connect(
///     "postgresql://user:pass@localhost:5432/hammerwork",
///     10  // max connections
/// ).await?;
/// # Ok(())
/// # }
/// ```
pub enum DatabasePool {
    Postgres(PgPool),
    MySQL(MySqlPool),
}

impl DatabasePool {
    /// Connect to a database with the specified connection pool size.
    ///
    /// The database type is automatically detected from the URL scheme:
    /// - `postgres://` or `postgresql://` → PostgreSQL
    /// - `mysql://` → MySQL
    ///
    /// # Arguments
    ///
    /// * `database_url` - Database connection URL
    /// * `pool_size` - Maximum number of connections in the pool
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use cargo_hammerwork::utils::database::DatabasePool;
    ///
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// // PostgreSQL with 10 connections
    /// let pg_pool = DatabasePool::connect(
    ///     "postgresql://user:pass@localhost/mydb",
    ///     10
    /// ).await?;
    ///
    /// // MySQL with 5 connections
    /// let mysql_pool = DatabasePool::connect(
    ///     "mysql://root:pass@localhost/mydb",
    ///     5
    /// ).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn connect(database_url: &str, pool_size: u32) -> Result<Self> {
        if database_url.starts_with("postgres://") || database_url.starts_with("postgresql://") {
            let pool = sqlx::postgres::PgPoolOptions::new()
                .max_connections(pool_size)
                .connect(database_url)
                .await?;
            Ok(DatabasePool::Postgres(pool))
        } else if database_url.starts_with("mysql://") {
            let pool = sqlx::mysql::MySqlPoolOptions::new()
                .max_connections(pool_size)
                .connect(database_url)
                .await?;
            Ok(DatabasePool::MySQL(pool))
        } else {
            Err(anyhow::anyhow!(
                "Unsupported database URL format. Use postgres:// or mysql://"
            ))
        }
    }

    /// Create a job queue from this database pool.
    ///
    /// This consumes the pool and returns a wrapped JobQueue that
    /// automatically handles database-specific implementations.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use cargo_hammerwork::utils::database::DatabasePool;
    ///
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let pool = DatabasePool::connect("postgresql://localhost/hammerwork", 5).await?;
    /// let job_queue = pool.create_job_queue();
    /// // Use job_queue for enqueuing, processing, etc.
    /// # Ok(())
    /// # }
    /// ```
    pub fn create_job_queue(self) -> JobQueueWrapper {
        match self {
            DatabasePool::Postgres(pool) => JobQueueWrapper::Postgres(JobQueue::new(pool)),
            DatabasePool::MySQL(pool) => JobQueueWrapper::MySQL(JobQueue::new(pool)),
        }
    }

    /// Run database migrations.
    ///
    /// This method runs all pending migrations on the connected database.
    /// Optionally drops existing tables before running migrations.
    ///
    /// # Arguments
    ///
    /// * `drop_tables` - If true, drops all Hammerwork tables before running migrations
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use cargo_hammerwork::utils::database::DatabasePool;
    ///
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let pool = DatabasePool::connect("postgresql://localhost/hammerwork", 5).await?;
    ///
    /// // Run migrations on existing database
    /// pool.migrate(false).await?;
    ///
    /// // Drop tables and run fresh migrations
    /// pool.migrate(true).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn migrate(&self, drop_tables: bool) -> Result<()> {
        match self {
            DatabasePool::Postgres(pool) => {
                if drop_tables {
                    info!("Dropping existing PostgreSQL tables...");
                    sqlx::query("DROP TABLE IF EXISTS hammerwork_jobs CASCADE")
                        .execute(pool)
                        .await?;
                    sqlx::query("DROP TABLE IF EXISTS hammerwork_migrations CASCADE")
                        .execute(pool)
                        .await?;
                    sqlx::query("DROP TABLE IF EXISTS hammerwork_workflows CASCADE")
                        .execute(pool)
                        .await?;
                }

                info!("Running PostgreSQL migrations...");
                let runner = Box::new(PostgresMigrationRunner::new(pool.clone()));
                let manager = MigrationManager::new(runner);
                manager.run_migrations().await?;
                info!("PostgreSQL migrations completed successfully");
            }
            DatabasePool::MySQL(pool) => {
                if drop_tables {
                    info!("Dropping existing MySQL tables...");
                    sqlx::query("DROP TABLE IF EXISTS hammerwork_jobs")
                        .execute(pool)
                        .await?;
                    sqlx::query("DROP TABLE IF EXISTS hammerwork_migrations")
                        .execute(pool)
                        .await?;
                    sqlx::query("DROP TABLE IF EXISTS hammerwork_workflows")
                        .execute(pool)
                        .await?;
                }

                info!("Running MySQL migrations...");
                let runner = Box::new(MySqlMigrationRunner::new(pool.clone()));
                let manager = MigrationManager::new(runner);
                manager.run_migrations().await?;
                info!("MySQL migrations completed successfully");
            }
        }
        Ok(())
    }
}

/// Wrapper for database-specific JobQueue implementations.
///
/// This enum provides a unified interface for working with job queues
/// regardless of the underlying database type.
///
/// # Examples
///
/// ```rust,no_run
/// use cargo_hammerwork::utils::database::{DatabasePool, JobQueueWrapper};
///
/// # #[tokio::main]
/// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let pool = DatabasePool::connect("postgresql://localhost/hammerwork", 5).await?;
/// let job_queue = pool.create_job_queue();
///
/// // The wrapper handles database-specific operations internally
/// match job_queue {
///     JobQueueWrapper::Postgres(queue) => {
///         // PostgreSQL-specific operations
///     }
///     JobQueueWrapper::MySQL(queue) => {
///         // MySQL-specific operations
///     }
/// }
/// # Ok(())
/// # }
/// ```
pub enum JobQueueWrapper {
    Postgres(JobQueue<sqlx::Postgres>),
    MySQL(JobQueue<sqlx::MySql>),
}
