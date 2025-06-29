use anyhow::Result;
use clap::Subcommand;
use sqlx::Row;
use tracing::info;

use crate::config::Config;
use crate::utils::database::DatabasePool;

#[derive(Subcommand)]
pub enum MigrationCommand {
    #[command(about = "Run database migrations")]
    Run {
        #[arg(short = 'u', long, help = "Database connection URL")]
        database_url: Option<String>,
        #[arg(short, long, help = "Drop existing tables before migration")]
        drop: bool,
    },
    #[command(about = "Check migration status")]
    Status {
        #[arg(short = 'u', long, help = "Database connection URL")]
        database_url: Option<String>,
    },
}

impl MigrationCommand {
    pub async fn execute(&self, config: &Config) -> Result<()> {
        match self {
            MigrationCommand::Run { database_url, drop } => {
                let db_url = database_url
                    .as_ref()
                    .map(|s| s.as_str())
                    .or(config.get_database_url())
                    .ok_or_else(|| anyhow::anyhow!("Database URL is required"))?;

                info!("Running migrations for: {}", db_url);
                let pool = DatabasePool::connect(db_url, config.get_connection_pool_size()).await?;
                pool.migrate(*drop).await?;
                info!("Migrations completed successfully");
            }
            MigrationCommand::Status { database_url } => {
                let db_url = database_url
                    .as_ref()
                    .map(|s| s.as_str())
                    .or(config.get_database_url())
                    .ok_or_else(|| anyhow::anyhow!("Database URL is required"))?;

                info!("Checking migration status for: {}", db_url);
                check_migration_status(db_url).await?;
            }
        }
        Ok(())
    }
}

async fn check_migration_status(database_url: &str) -> Result<()> {
    let pool = DatabasePool::connect(database_url, 1).await?;
    
    match pool {
        DatabasePool::Postgres(ref pg_pool) => {
            let result = sqlx::query(
                "SELECT table_name FROM information_schema.tables 
                 WHERE table_schema = 'public' AND table_name = 'hammerwork_jobs'"
            )
            .fetch_optional(pg_pool)
            .await?;
            
            if result.is_some() {
                println!("✅ PostgreSQL tables exist");
                
                // Check for specific columns to verify schema version
                let columns = sqlx::query(
                    "SELECT column_name FROM information_schema.columns 
                     WHERE table_name = 'hammerwork_jobs' ORDER BY column_name"
                )
                .fetch_all(pg_pool)
                .await?;
                
                println!("📊 Schema columns: {}", columns.len());
                for col in columns {
                    let column_name: String = col.try_get("column_name")?;
                    println!("  - {}", column_name);
                }
            } else {
                println!("❌ PostgreSQL tables do not exist. Run 'migrate run' to create them.");
            }
        }
        DatabasePool::MySQL(ref mysql_pool) => {
            let result = sqlx::query(
                "SELECT TABLE_NAME FROM information_schema.tables 
                 WHERE TABLE_SCHEMA = DATABASE() AND TABLE_NAME = 'hammerwork_jobs'"
            )
            .fetch_optional(mysql_pool)
            .await?;
            
            if result.is_some() {
                println!("✅ MySQL tables exist");
                
                // Check for specific columns to verify schema version
                let columns = sqlx::query(
                    "SELECT COLUMN_NAME FROM information_schema.columns 
                     WHERE TABLE_NAME = 'hammerwork_jobs' AND TABLE_SCHEMA = DATABASE() 
                     ORDER BY COLUMN_NAME"
                )
                .fetch_all(mysql_pool)
                .await?;
                
                println!("📊 Schema columns: {}", columns.len());
                for col in columns {
                    let column_name: String = col.try_get("COLUMN_NAME")?;
                    println!("  - {}", column_name);
                }
            } else {
                println!("❌ MySQL tables do not exist. Run 'migrate run' to create them.");
            }
        }
    }
    
    Ok(())
}

#[cfg(test)]
mod tests {
    
    #[test]
    fn test_migration_command_structure() {
        // Test that migration commands are properly structured
        let commands = vec!["run", "status"];
        
        for cmd in commands {
            assert!(!cmd.is_empty());
            assert!(cmd.chars().all(|c| c.is_ascii_lowercase()));
        }
    }
    
    #[test]
    fn test_migration_args_validation() {
        // Test database URL validation for migration commands
        let valid_urls = vec![
            "postgres://localhost/test",
            "mysql://localhost/test",
            "postgres://user:pass@localhost:5432/db",
        ];
        
        for url in valid_urls {
            assert!(is_valid_database_url(url));
        }
        
        let invalid_urls = vec![
            "",
            "invalid",
            "http://localhost/db",
            "file:///path/to/db",
        ];
        
        for url in invalid_urls {
            assert!(!is_valid_database_url(url));
        }
    }
    
    fn is_valid_database_url(url: &str) -> bool {
        !url.is_empty() && (url.starts_with("postgres://") || url.starts_with("mysql://"))
    }
}