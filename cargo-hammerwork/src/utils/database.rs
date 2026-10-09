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
use sqlx::Connection;
use sqlx::{MySqlPool, PgPool};
use std::time::Duration;
use tracing::info;

/// Default time to wait for a database connection before giving up.
pub const DEFAULT_CONNECT_TIMEOUT_SECS: u64 = 10;

/// Await one connection attempt, closing the connection and mapping a timeout to a
/// readable error.
async fn probe<C, F>(attempt: F, timeout: Duration) -> Result<()>
where
    C: sqlx::Connection,
    F: std::future::Future<Output = std::result::Result<C, sqlx::Error>>,
{
    match tokio::time::timeout(timeout, attempt).await {
        Ok(Ok(conn)) => {
            let _ = conn.close().await;
            Ok(())
        }
        Ok(Err(e)) => Err(connect_error(e, timeout)),
        Err(_) => Err(connect_error(sqlx::Error::PoolTimedOut, timeout)),
    }
}

fn connect_error(err: sqlx::Error, timeout: Duration) -> anyhow::Error {
    match err {
        sqlx::Error::PoolTimedOut => anyhow::anyhow!(
            "Timed out after {}s connecting to the database; check the URL and that the server is reachable",
            timeout.as_secs_f32().max(0.0)
        ),
        other => anyhow::Error::new(other),
    }
}

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
#[derive(Clone)]
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
        Self::connect_with_timeout(
            database_url,
            pool_size,
            Duration::from_secs(DEFAULT_CONNECT_TIMEOUT_SECS),
        )
        .await
    }

    /// Like [`connect`](Self::connect), but fails with an error if no connection
    /// can be established within `connect_timeout`, instead of hanging on an
    /// unreachable host.
    pub async fn connect_with_timeout(
        database_url: &str,
        pool_size: u32,
        connect_timeout: Duration,
    ) -> Result<Self> {
        if database_url.starts_with("postgres://") || database_url.starts_with("postgresql://") {
            // sqlx's pool retries refused connections until its acquire timeout, so
            // probe with a single connection first: a refused port or bad
            // credentials fail immediately, and an unroutable host hits the timeout.
            probe(sqlx::PgConnection::connect(database_url), connect_timeout).await?;
            let pool = sqlx::postgres::PgPoolOptions::new()
                .max_connections(pool_size)
                .acquire_timeout(connect_timeout)
                .connect(database_url)
                .await
                .map_err(|e| connect_error(e, connect_timeout))?;
            Ok(DatabasePool::Postgres(pool))
        } else if database_url.starts_with("mysql://") {
            probe(
                sqlx::MySqlConnection::connect(database_url),
                connect_timeout,
            )
            .await?;
            let pool = sqlx::mysql::MySqlPoolOptions::new()
                .max_connections(pool_size)
                .acquire_timeout(connect_timeout)
                .connect(database_url)
                .await
                .map_err(|e| connect_error(e, connect_timeout))?;
            Ok(DatabasePool::MySQL(pool))
        } else {
            Err(anyhow::anyhow!(
                "Unsupported database URL format. Use postgres:// or mysql://"
            ))
        }
    }

    /// Connect using the pool size and connect timeout from the CLI configuration.
    pub async fn connect_with_config(
        database_url: &str,
        config: &crate::config::Config,
    ) -> Result<Self> {
        Self::connect_with_timeout(
            database_url,
            config.get_connection_pool_size(),
            Duration::from_secs(config.get_connect_timeout_secs()),
        )
        .await
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[tokio::test]
    async fn refused_connection_fails_fast() {
        let start = Instant::now();
        let result = DatabasePool::connect("postgres://u:p@127.0.0.1:1/x", 1).await;
        assert!(result.is_err());
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "{:?}",
            start.elapsed()
        );
    }

    #[tokio::test]
    async fn mysql_refused_connection_fails_fast() {
        let start = Instant::now();
        let result = DatabasePool::connect("mysql://u:p@127.0.0.1:1/x", 1).await;
        assert!(result.is_err());
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "{:?}",
            start.elapsed()
        );
    }

    /// 10.255.255.1 is normally a black hole (SYNs are dropped, not refused), which
    /// is what used to hang. Whatever the network does, the connect must give up
    /// within the configured timeout.
    #[tokio::test]
    async fn unroutable_address_respects_the_timeout() {
        let start = Instant::now();
        let result = DatabasePool::connect_with_timeout(
            "postgres://u:p@10.255.255.1:5432/x",
            1,
            Duration::from_secs(1),
        )
        .await;
        assert!(result.is_err());
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "{:?}",
            start.elapsed()
        );
    }

    #[tokio::test]
    async fn connect_with_config_uses_the_configured_timeout() {
        let config = crate::config::Config {
            connect_timeout_secs: Some(1),
            ..crate::config::Config::default()
        };
        let start = Instant::now();
        let result =
            DatabasePool::connect_with_config("postgres://u:p@10.255.255.1:5432/x", &config).await;
        assert!(result.is_err());
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "{:?}",
            start.elapsed()
        );
    }

    #[test]
    fn timeout_error_message_says_what_happened() {
        let err = connect_error(sqlx::Error::PoolTimedOut, Duration::from_secs(10));
        assert!(err.to_string().contains("Timed out after 10"), "{err}");
    }

    #[tokio::test]
    async fn unsupported_scheme_is_rejected() {
        assert!(DatabasePool::connect("sqlite://x", 1).await.is_err());
    }
}
