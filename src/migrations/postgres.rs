//! PostgreSQL-specific migration runner implementation.

use super::split::{Dialect, split_statements};
use super::{Migration, MigrationRecord, MigrationRunner};
use crate::Result;
use chrono::Utc;
use sqlx::{PgPool, Row};
use tracing::{debug, error, info};

/// PostgreSQL migration runner
pub struct PostgresMigrationRunner {
    pool: PgPool,
}

impl PostgresMigrationRunner {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl MigrationRunner<sqlx::Postgres> for PostgresMigrationRunner {
    async fn run_migration(&self, migration: &Migration, sql: &str) -> Result<()> {
        debug!("Executing PostgreSQL migration: {}", migration.id);

        let mut tx = self.pool.begin().await?;

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
        let row = sqlx::query(
            "SELECT EXISTS (
                SELECT FROM information_schema.tables 
                WHERE table_schema = 'public' 
                AND table_name = 'hammerwork_migrations'
            )",
        )
        .fetch_one(&self.pool)
        .await?;

        Ok(row.try_get::<bool, _>(0)?)
    }

    async fn create_migration_table(&self) -> Result<()> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS hammerwork_migrations (
                migration_id VARCHAR NOT NULL PRIMARY KEY,
                executed_at TIMESTAMPTZ NOT NULL,
                execution_time_ms BIGINT NOT NULL
            )
            "#,
        )
        .execute(&self.pool)
        .await?;

        info!("Created PostgreSQL migration tracking table");
        Ok(())
    }

    async fn get_executed_migrations(&self) -> Result<Vec<MigrationRecord>> {
        let rows = sqlx::query(
            "SELECT migration_id, executed_at, execution_time_ms 
             FROM hammerwork_migrations 
             ORDER BY executed_at",
        )
        .fetch_all(&self.pool)
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
        sqlx::query(
            "INSERT INTO hammerwork_migrations (migration_id, executed_at, execution_time_ms) 
             VALUES ($1, $2, $3)",
        )
        .bind(&migration.id)
        .bind(Utc::now())
        .bind(execution_time_ms as i64)
        .execute(&self.pool)
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
