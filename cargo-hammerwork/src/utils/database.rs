use anyhow::Result;
use hammerwork::{JobQueue, migrations::{MigrationManager, postgres::PostgresMigrationRunner, mysql::MySqlMigrationRunner}};
use sqlx::{MySqlPool, PgPool};
use tracing::info;

pub enum DatabasePool {
    Postgres(PgPool),
    MySQL(MySqlPool),
}

impl DatabasePool {
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
            Err(anyhow::anyhow!("Unsupported database URL format. Use postgres:// or mysql://"))
        }
    }
    
    pub fn create_job_queue(self) -> JobQueueWrapper {
        match self {
            DatabasePool::Postgres(pool) => JobQueueWrapper::Postgres(JobQueue::new(pool)),
            DatabasePool::MySQL(pool) => JobQueueWrapper::MySQL(JobQueue::new(pool)),
        }
    }
    
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

pub enum JobQueueWrapper {
    Postgres(JobQueue<sqlx::Postgres>),
    MySQL(JobQueue<sqlx::MySql>),
}

