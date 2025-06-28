use anyhow::Result;
use hammerwork::JobQueue;
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
                }
                
                info!("Creating PostgreSQL tables...");
                create_postgres_tables(pool).await?;
                info!("PostgreSQL migrations completed successfully");
            }
            DatabasePool::MySQL(pool) => {
                if drop_tables {
                    info!("Dropping existing MySQL tables...");
                    sqlx::query("DROP TABLE IF EXISTS hammerwork_jobs")
                        .execute(pool)
                        .await?;
                }
                
                info!("Creating MySQL tables...");
                create_mysql_tables(pool).await?;
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

async fn create_postgres_tables(pool: &PgPool) -> Result<()> {
    // Create table
    sqlx::query(r#"
        CREATE TABLE IF NOT EXISTS hammerwork_jobs (
            id UUID PRIMARY KEY,
            queue_name VARCHAR NOT NULL,
            payload JSONB NOT NULL,
            status VARCHAR NOT NULL,
            priority VARCHAR NOT NULL DEFAULT 'normal',
            attempts INTEGER NOT NULL DEFAULT 0,
            max_attempts INTEGER NOT NULL DEFAULT 3,
            created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
            scheduled_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
            started_at TIMESTAMPTZ,
            completed_at TIMESTAMPTZ,
            failed_at TIMESTAMPTZ,
            timed_out_at TIMESTAMPTZ,
            error_message TEXT,
            result JSONB,
            timeout_seconds INTEGER,
            cron_schedule VARCHAR,
            next_run_at TIMESTAMPTZ,
            recurring BOOLEAN DEFAULT FALSE,
            timezone VARCHAR,
            batch_id UUID,
            batch_size INTEGER,
            batch_progress INTEGER DEFAULT 0
        )
    "#).execute(pool).await?;
    
    // Create indexes separately
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_hammerwork_queue_priority ON hammerwork_jobs(queue_name, priority DESC, scheduled_at ASC)")
        .execute(pool).await?;
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_hammerwork_status ON hammerwork_jobs(status)")
        .execute(pool).await?;
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_hammerwork_scheduled ON hammerwork_jobs(scheduled_at)")
        .execute(pool).await?;
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_hammerwork_cron ON hammerwork_jobs(next_run_at) WHERE cron_schedule IS NOT NULL")
        .execute(pool).await?;
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_hammerwork_batch ON hammerwork_jobs(batch_id) WHERE batch_id IS NOT NULL")
        .execute(pool).await?;
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_hammerwork_failed_at ON hammerwork_jobs(failed_at) WHERE failed_at IS NOT NULL")
        .execute(pool).await?;
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_hammerwork_completed_at ON hammerwork_jobs(completed_at) WHERE completed_at IS NOT NULL")
        .execute(pool).await?;
    
    Ok(())
}

async fn create_mysql_tables(pool: &MySqlPool) -> Result<()> {
    // Create table
    sqlx::query(r#"
        CREATE TABLE IF NOT EXISTS hammerwork_jobs (
            id CHAR(36) PRIMARY KEY,
            queue_name VARCHAR(255) NOT NULL,
            payload JSON NOT NULL,
            status VARCHAR(50) NOT NULL,
            priority VARCHAR(20) NOT NULL DEFAULT 'normal',
            attempts INTEGER NOT NULL DEFAULT 0,
            max_attempts INTEGER NOT NULL DEFAULT 3,
            created_at TIMESTAMP(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
            scheduled_at TIMESTAMP(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
            started_at TIMESTAMP(6),
            completed_at TIMESTAMP(6),
            failed_at TIMESTAMP(6),
            timed_out_at TIMESTAMP(6),
            error_message TEXT,
            result JSON,
            timeout_seconds INTEGER,
            cron_schedule VARCHAR(255),
            next_run_at TIMESTAMP(6),
            recurring BOOLEAN DEFAULT FALSE,
            timezone VARCHAR(50),
            batch_id CHAR(36),
            batch_size INTEGER,
            batch_progress INTEGER DEFAULT 0,
            
            INDEX idx_queue_priority (queue_name, priority, scheduled_at),
            INDEX idx_status (status),
            INDEX idx_scheduled (scheduled_at),
            INDEX idx_cron (next_run_at),
            INDEX idx_batch (batch_id),
            INDEX idx_failed_at (failed_at),
            INDEX idx_completed_at (completed_at)
        )
    "#).execute(pool).await?;
    
    Ok(())
}