//! MySQL-specific migration runner implementation.

use super::{Migration, MigrationRecord, MigrationRunner};
use crate::Result;
use chrono::Utc;
use sqlx::{MySqlPool, Row};
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
        
        // Execute the migration SQL
        sqlx::query(sql).execute(&mut *tx).await?;
        
        tx.commit().await?;
        
        info!("Successfully executed MySQL migration: {}", migration.id);
        Ok(())
    }
    
    async fn migration_table_exists(&self) -> Result<bool> {
        let row = sqlx::query(
            "SELECT COUNT(*) as count FROM information_schema.tables 
             WHERE table_schema = DATABASE() 
             AND table_name = 'hammerwork_migrations'"
        )
        .fetch_one(&self.pool)
        .await?;
        
        Ok(row.get::<i64, _>("count") > 0)
    }
    
    async fn create_migration_table(&self) -> Result<()> {
        sqlx::query(
            r#"
            CREATE TABLE hammerwork_migrations (
                migration_id VARCHAR(255) NOT NULL PRIMARY KEY,
                executed_at TIMESTAMP(6) NOT NULL,
                execution_time_ms BIGINT NOT NULL
            )
            "#
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
             ORDER BY executed_at"
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
             VALUES (?, ?, ?)"
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