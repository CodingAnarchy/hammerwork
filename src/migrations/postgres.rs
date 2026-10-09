//! PostgreSQL-specific migration runner implementation.

use super::split::{Dialect, split_statements};
use super::{Migration, MigrationRecord, MigrationRunner};
use crate::Result;
use chrono::Utc;
use sqlx::{PgConnection, PgPool, Postgres, Row, pool::PoolConnection};
use tokio::sync::{Mutex, MutexGuard};
use tracing::{debug, error, info};

/// Key of the session-level advisory lock that serializes migration runs on a database
/// (`pg_advisory_lock`). The ASCII bytes of "hmrwkmig".
pub const MIGRATION_LOCK_KEY: i64 = 0x686d_7277_6b6d_6967;

/// PostgreSQL migration runner
pub struct PostgresMigrationRunner {
    pool: PgPool,
    /// The connection holding the migration lock while a run is in progress. Every
    /// statement of the run goes through it.
    locked: Mutex<Option<PoolConnection<Postgres>>>,
}

impl PostgresMigrationRunner {
    pub fn new(pool: PgPool) -> Self {
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
    Locked(MutexGuard<'a, Option<PoolConnection<Postgres>>>),
    Pooled(PoolConnection<Postgres>),
}

impl Conn<'_> {
    fn get(&mut self) -> Result<&mut PgConnection> {
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
impl MigrationRunner<sqlx::Postgres> for PostgresMigrationRunner {
    async fn acquire_migration_lock(&self) -> Result<()> {
        let mut conn = self.pool.acquire().await?;
        // If the run is abandoned (an error, or its future is dropped) the connection is
        // closed instead of going back to the pool, which releases the session lock.
        conn.close_on_drop();
        debug!("Waiting for the PostgreSQL migration lock");
        sqlx::query("SELECT pg_advisory_lock($1)")
            .bind(MIGRATION_LOCK_KEY)
            .execute(&mut *conn)
            .await?;
        debug!("Acquired the PostgreSQL migration lock");
        *self.locked.lock().await = Some(conn);
        Ok(())
    }

    async fn release_migration_lock(&self) -> Result<()> {
        let Some(mut conn) = self.locked.lock().await.take() else {
            return Ok(());
        };
        // Dropping the connection closes it (close_on_drop), so the lock is released
        // even if the unlock fails.
        sqlx::query("SELECT pg_advisory_unlock($1)")
            .bind(MIGRATION_LOCK_KEY)
            .execute(&mut *conn)
            .await?;
        debug!("Released the PostgreSQL migration lock");
        Ok(())
    }

    async fn run_migration(&self, migration: &Migration, sql: &str) -> Result<()> {
        debug!("Executing PostgreSQL migration: {}", migration.id);

        let mut conn = self.connection().await?;
        let mut tx = sqlx::Connection::begin(conn.get()?).await?;

        // Statements run exactly as written; the splitter only finds top-level `;`.
        let statements = split_statements(sql, Dialect::Postgres);

        debug!(
            "Parsed {} statements for migration {}",
            statements.len(),
            migration.id
        );
        for (i, statement) in statements.iter().enumerate() {
            debug!("Statement {}: '{}'", i + 1, statement.trim());
        }

        for (i, statement) in statements.iter().enumerate() {
            // Add semicolon back if it was removed by split
            let full_statement = if statement.ends_with(';') {
                statement.to_string()
            } else {
                format!("{};", statement)
            };

            debug!(
                "Executing statement {} of {} for migration {}: {}",
                i + 1,
                statements.len(),
                migration.id,
                full_statement
            );

            if let Err(e) = sqlx::query(&full_statement).execute(&mut *tx).await {
                error!(
                    "Failed to execute statement {}: {} - Error: {}",
                    i + 1,
                    full_statement,
                    e
                );
                return Err(e.into());
            }
        }

        tx.commit().await?;

        info!(
            "Successfully executed PostgreSQL migration: {} ({} statements)",
            migration.id,
            statements.len()
        );
        Ok(())
    }

    async fn migration_table_exists(&self) -> Result<bool> {
        let mut conn = self.connection().await?;
        // The schema the tables are created in: the first schema of the search path.
        let row = sqlx::query(
            "SELECT EXISTS (
                SELECT FROM information_schema.tables
                WHERE table_schema = current_schema()
                AND table_name = 'hammerwork_migrations'
            )",
        )
        .fetch_one(conn.get()?)
        .await?;

        Ok(row.try_get::<bool, _>(0)?)
    }

    async fn create_migration_table(&self) -> Result<()> {
        let mut conn = self.connection().await?;
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS hammerwork_migrations (
                migration_id VARCHAR NOT NULL PRIMARY KEY,
                executed_at TIMESTAMPTZ NOT NULL,
                execution_time_ms BIGINT NOT NULL
            )
            "#,
        )
        .execute(conn.get()?)
        .await?;

        info!("Created PostgreSQL migration tracking table");
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
            "INSERT INTO hammerwork_migrations (migration_id, executed_at, execution_time_ms) 
             VALUES ($1, $2, $3) ON CONFLICT (migration_id) DO NOTHING",
        )
        .bind(&migration.id)
        .bind(Utc::now())
        .bind(i64::try_from(execution_time_ms).unwrap_or(i64::MAX))
        .execute(conn.get()?)
        .await?;

        debug!("Recorded PostgreSQL migration: {}", migration.id);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::split::{Dialect, split_statements};

    #[test]
    fn migration_014_trigger_keeps_its_argument_list() {
        let statements = split_statements(
            include_str!("014_add_queue_pause.postgres.sql"),
            Dialect::Postgres,
        );
        let trigger = statements
            .iter()
            .find(|s| s.contains("CREATE TRIGGER"))
            .expect("migration 014 creates a trigger");
        // Executed verbatim, so PostgreSQL gets the `()` it requires.
        assert!(trigger.contains("EXECUTE FUNCTION"), "{trigger}");
        assert!(trigger.trim_end().ends_with(')'), "{trigger}");
        let function = statements
            .iter()
            .find(|s| s.contains("CREATE OR REPLACE FUNCTION"))
            .expect("migration 014 creates a function");
        assert!(
            function.contains("RETURN NEW"),
            "function body must stay whole"
        );
    }

    #[test]
    fn migration_012_do_blocks_are_single_statements() {
        let statements = split_statements(
            include_str!("012_optimize_dependencies.postgres.sql"),
            Dialect::Postgres,
        );
        for statement in statements
            .iter()
            .filter(|s| s.trim_start().starts_with("DO"))
        {
            assert!(
                statement.trim_end().ends_with('$'),
                "DO block was split: {statement}"
            );
        }
    }
}
