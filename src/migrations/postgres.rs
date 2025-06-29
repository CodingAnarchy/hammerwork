//! PostgreSQL-specific migration runner implementation.

use super::{Migration, MigrationRecord, MigrationRunner};
use crate::Result;
use chrono::Utc;
use sqlx::{PgPool, Row};
use tracing::{debug, info};

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

        // Split SQL into individual statements and execute each one
        // This is a simple split that handles most cases - splits on semicolon followed by newline
        let statements: Vec<&str> = sql
            .split(";\n")
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect();

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

            sqlx::query(&full_statement).execute(&mut *tx).await?;
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

        Ok(row.get::<bool, _>(0))
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
                migration_id: row.get("migration_id"),
                executed_at: row.get("executed_at"),
                execution_time_ms: row.get::<i64, _>("execution_time_ms") as u64,
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
    use super::*;
    
    #[test]
    fn test_sql_statement_splitting() {
        let multi_statement_sql = r#"
-- Comment line
CREATE TABLE test_table (
    id INTEGER PRIMARY KEY
);

-- Another comment
ALTER TABLE test_table ADD COLUMN name VARCHAR(50);

CREATE INDEX idx_test ON test_table (name);
"#;
        
        let statements: Vec<&str> = multi_statement_sql
            .split(";\n")
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect();
        
        // Should split into 3 non-empty statements
        assert_eq!(statements.len(), 3);
        
        // Verify each statement contains expected keywords
        assert!(statements[0].contains("CREATE TABLE"));
        assert!(statements[1].contains("ALTER TABLE"));
        assert!(statements[2].contains("CREATE INDEX"));
    }
}
