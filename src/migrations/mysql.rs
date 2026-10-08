//! MySQL-specific migration runner implementation.

use super::split::{Dialect, split_statements};
use super::{Migration, MigrationRecord, MigrationRunner};
use crate::Result;
use chrono::Utc;
use sqlx::{Executor, MySqlPool, Row};
use tracing::{debug, info};

/// MySQL migration runner
pub struct MySqlMigrationRunner {
    pool: MySqlPool,
}

impl MySqlMigrationRunner {
    pub fn new(pool: MySqlPool) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl MigrationRunner<sqlx::MySql> for MySqlMigrationRunner {
    async fn run_migration(&self, migration: &Migration, sql: &str) -> Result<()> {
        debug!("Executing MySQL migration: {}", migration.id);

        let mut tx = self.pool.begin().await?;

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
        let row = sqlx::query(
            "SELECT COUNT(*) as count FROM information_schema.tables 
             WHERE table_schema = DATABASE() 
             AND table_name = 'hammerwork_migrations'",
        )
        .fetch_one(&self.pool)
        .await?;

        Ok(row.try_get::<i64, _>("count")? > 0)
    }

    async fn create_migration_table(&self) -> Result<()> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS hammerwork_migrations (
                migration_id VARCHAR(255) NOT NULL PRIMARY KEY,
                executed_at TIMESTAMP(6) NOT NULL,
                execution_time_ms BIGINT NOT NULL
            )
            "#,
        )
        .execute(&self.pool)
        .await?;

        info!("Created MySQL migration tracking table");
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
             VALUES (?, ?, ?)",
        )
        .bind(&migration.id)
        .bind(Utc::now())
        .bind(execution_time_ms as i64)
        .execute(&self.pool)
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
