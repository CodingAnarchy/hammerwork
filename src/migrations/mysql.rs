//! MySQL-specific migration runner implementation.

use super::split::{Dialect, split_statements};
use super::{Migration, MigrationRecord, MigrationRunner};
use crate::Result;
use chrono::Utc;
use sqlx::{Executor, MySql, MySqlConnection, MySqlPool, Row, pool::PoolConnection};
use tokio::sync::{Mutex, MutexGuard};
use tracing::{debug, info};

/// Prefix of the `GET_LOCK` name that serializes migration runs. The SHA-1 of the
/// database name is appended, because MySQL lock names are server-wide and at most 64
/// characters long.
pub const MIGRATION_LOCK_PREFIX: &str = "hammerwork_migrations:";

/// How long a run waits for another run's migration lock before failing.
pub const MIGRATION_LOCK_TIMEOUT_SECS: u32 = 600;

/// MySQL migration runner
pub struct MySqlMigrationRunner {
    pool: MySqlPool,
    /// The connection holding the migration lock while a run is in progress. Every
    /// statement of the run goes through it.
    locked: Mutex<Option<PoolConnection<MySql>>>,
}

impl MySqlMigrationRunner {
    pub fn new(pool: MySqlPool) -> Self {
        Self {
            pool,
            locked: Mutex::new(None),
        }
    }

    /// The connection to run a statement on: the one holding the migration lock, or
    /// else a pooled connection.
    async fn connection(&self) -> Result<Conn<'_>> {
        let guard = self.locked.lock().await;
        if guard.is_some() {
            return Ok(Conn::Locked(guard));
        }
        drop(guard);
        Ok(Conn::Pooled(self.pool.acquire().await?))
    }
}

/// A connection for one runner operation.
enum Conn<'a> {
    Locked(MutexGuard<'a, Option<PoolConnection<MySql>>>),
    Pooled(PoolConnection<MySql>),
}

impl Conn<'_> {
    fn get(&mut self) -> Result<&mut MySqlConnection> {
        match self {
            Self::Locked(guard) => guard.as_mut().map(|conn| &mut **conn).ok_or_else(|| {
                crate::HammerworkError::Queue {
                    message: "the migration lock connection is gone".to_string(),
                }
            }),
            Self::Pooled(conn) => Ok(&mut **conn),
        }
    }
}

#[async_trait::async_trait]
impl MigrationRunner<sqlx::MySql> for MySqlMigrationRunner {
    async fn acquire_migration_lock(&self) -> Result<()> {
        let mut conn = self.pool.acquire().await?;
        // If the run is abandoned (an error, or its future is dropped) the connection is
        // closed instead of going back to the pool, which releases the lock.
        conn.close_on_drop();
        debug!("Waiting for the MySQL migration lock");
        let acquired: Option<i64> =
            sqlx::query_scalar("SELECT GET_LOCK(CONCAT(?, SHA1(DATABASE())), ?)")
                .bind(MIGRATION_LOCK_PREFIX)
                .bind(MIGRATION_LOCK_TIMEOUT_SECS)
                .fetch_one(&mut *conn)
                .await?;
        if acquired != Some(1) {
            return Err(crate::HammerworkError::Queue {
                message: format!(
                    "timed out after {MIGRATION_LOCK_TIMEOUT_SECS}s waiting for another \
                     migration run to finish (MySQL lock {MIGRATION_LOCK_PREFIX}<SHA-1 of the database name>)"
                ),
            });
        }
        debug!("Acquired the MySQL migration lock");
        *self.locked.lock().await = Some(conn);
        Ok(())
    }

    async fn release_migration_lock(&self) -> Result<()> {
        let Some(mut conn) = self.locked.lock().await.take() else {
            return Ok(());
        };
        // Dropping the connection closes it (close_on_drop), so the lock is released
        // even if RELEASE_LOCK fails.
        sqlx::query("SELECT RELEASE_LOCK(CONCAT(?, SHA1(DATABASE())))")
            .bind(MIGRATION_LOCK_PREFIX)
            .execute(&mut *conn)
            .await?;
        debug!("Released the MySQL migration lock");
        Ok(())
    }

    async fn run_migration(&self, migration: &Migration, sql: &str) -> Result<()> {
        debug!("Executing MySQL migration: {}", migration.id);

        let mut conn = self.connection().await?;
        let mut tx = sqlx::Connection::begin(conn.get()?).await?;

        // Statements run exactly as written; the splitter only finds top-level `;`.
        let statements = split_statements(sql, Dialect::MySql);

        for (i, statement) in statements.iter().enumerate() {
            // Add semicolon back if it was removed by split
            let full_statement = if statement.ends_with(';') {
                statement.to_string()
            } else {
                format!("{};", statement)
            };

            debug!(
                "Executing statement {} of {} for migration {}",
                i + 1,
                statements.len(),
                migration.id
            );

            // Use the text protocol: dynamic DDL (PREPARE/EXECUTE/DEALLOCATE) is not
            // supported through MySQL's prepared statement protocol.
            tx.execute(full_statement.as_str()).await?;
        }

        tx.commit().await?;

        info!(
            "Successfully executed MySQL migration: {} ({} statements)",
            migration.id,
            statements.len()
        );
        Ok(())
    }

    async fn migration_table_exists(&self) -> Result<bool> {
        let mut conn = self.connection().await?;
        let row = sqlx::query(
            "SELECT COUNT(*) as count FROM information_schema.tables
             WHERE table_schema = DATABASE()
             AND table_name = 'hammerwork_migrations'",
        )
        .fetch_one(conn.get()?)
        .await?;

        Ok(row.try_get::<i64, _>("count")? > 0)
    }

    async fn create_migration_table(&self) -> Result<()> {
        let mut conn = self.connection().await?;
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS hammerwork_migrations (
                migration_id VARCHAR(255) NOT NULL PRIMARY KEY,
                executed_at TIMESTAMP(6) NOT NULL,
                execution_time_ms BIGINT NOT NULL
            )
            "#,
        )
        .execute(conn.get()?)
        .await?;

        info!("Created MySQL migration tracking table");
        Ok(())
    }

    async fn get_executed_migrations(&self) -> Result<Vec<MigrationRecord>> {
        let mut conn = self.connection().await?;
        let rows = sqlx::query(
            "SELECT migration_id, executed_at, execution_time_ms
             FROM hammerwork_migrations
             ORDER BY executed_at",
        )
        .fetch_all(conn.get()?)
        .await?;

        let mut records = Vec::new();
        for row in rows {
            records.push(MigrationRecord {
                migration_id: row.try_get("migration_id")?,
                executed_at: row.try_get("executed_at")?,
                execution_time_ms: crate::queue::db_int(
                    row.try_get::<i64, _>("execution_time_ms")?,
                    "execution_time_ms",
                )?,
            });
        }

        Ok(records)
    }

    async fn record_migration(&self, migration: &Migration, execution_time_ms: u64) -> Result<()> {
        let mut conn = self.connection().await?;
        // A run that does not take the lock (an older version) may have recorded it too.
        sqlx::query(
            "INSERT IGNORE INTO hammerwork_migrations (migration_id, executed_at, execution_time_ms)
             VALUES (?, ?, ?)",
        )
        .bind(&migration.id)
        .bind(Utc::now())
        .bind(i64::try_from(execution_time_ms).unwrap_or(i64::MAX))
        .execute(conn.get()?)
        .await?;

        debug!("Recorded MySQL migration: {}", migration.id);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::split::{Dialect, split_statements};

    #[test]
    fn guarded_ddl_migrations_split_into_whole_statements() {
        let statements =
            split_statements(include_str!("011_add_encryption.mysql.sql"), Dialect::MySql);
        // Every PREPARE pairs with its EXECUTE and DEALLOCATE.
        let count = |kw: &str| statements.iter().filter(|s| s.starts_with(kw)).count();
        assert!(count("PREPARE") > 0);
        assert_eq!(count("PREPARE"), count("EXECUTE"));
        assert_eq!(count("PREPARE"), count("DEALLOCATE"));
        // Quoted DDL inside `SET @sql = IF(...)` is not split at its inner text.
        assert!(statements.iter().all(|s| !s.starts_with('\'')));
    }
}
