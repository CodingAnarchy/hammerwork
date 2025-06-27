//! Cargo subcommand for Hammerwork operations.
//!
//! This binary provides `cargo hammerwork` subcommands for managing Hammerwork
//! job queue databases and operations.
//!
//! Usage:
//!   cargo hammerwork migrate --database-url postgresql://localhost/hammerwork
//!   cargo hammerwork migrate --database-url mysql://localhost/hammerwork
//!   cargo hammerwork status --database-url postgresql://localhost/hammerwork

use clap::{Parser, Subcommand};
use hammerwork::migrations::MigrationManager;
use std::process;
use tracing_subscriber;

#[derive(Parser)]
#[command(
    name = "cargo",
    bin_name = "cargo",
    version = "0.8.0",
    about = "Hammerwork job queue management tool"
)]
struct Cli {
    #[command(subcommand)]
    command: CargoCommand,
}

#[derive(Subcommand)]
enum CargoCommand {
    #[command(
        name = "hammerwork",
        about = "Hammerwork job queue operations",
        version = "0.8.0"
    )]
    Hammerwork {
        #[command(subcommand)]
        command: HammerworkCommand,
    },
}

#[derive(Subcommand)]
enum HammerworkCommand {
    #[command(about = "Run database migrations")]
    Migrate {
        #[arg(
            long = "database-url",
            help = "Database connection URL",
            value_name = "URL"
        )]
        database_url: String,
        
        #[arg(
            long = "status",
            help = "Show migration status instead of running migrations"
        )]
        status: bool,
    },
    
    #[command(about = "Show migration status")]
    Status {
        #[arg(
            long = "database-url",
            help = "Database connection URL",
            value_name = "URL"
        )]
        database_url: String,
    },
}

#[tokio::main]
async fn main() {
    // Initialize logging
    tracing_subscriber::fmt::init();

    let cli = Cli::parse();

    match cli.command {
        CargoCommand::Hammerwork { command } => {
            let result = match command {
                HammerworkCommand::Migrate { database_url, status } => {
                    if status {
                        show_migration_status(&database_url).await
                    } else {
                        run_migrations(&database_url).await
                    }
                }
                HammerworkCommand::Status { database_url } => {
                    show_migration_status(&database_url).await
                }
            };

            if let Err(e) = result {
                eprintln!("❌ Error: {}", e);
                process::exit(1);
            }
        }
    }
}

async fn run_migrations(database_url: &str) -> Result<(), Box<dyn std::error::Error>> {
    println!("🔧 Running Hammerwork migrations...");
    println!("📍 Database URL: {}", database_url);

    #[cfg(feature = "postgres")]
    if database_url.starts_with("postgres") {
        let pool = sqlx::PgPool::connect(database_url).await?;
        let runner = Box::new(hammerwork::migrations::postgres::PostgresMigrationRunner::new(pool));
        let manager = MigrationManager::new(runner);
        manager.run_migrations().await?;
        println!("✅ PostgreSQL migrations completed successfully!");
        return Ok(());
    }

    #[cfg(feature = "mysql")]
    if database_url.starts_with("mysql") {
        let pool = sqlx::MySqlPool::connect(database_url).await?;
        let runner = Box::new(hammerwork::migrations::mysql::MySqlMigrationRunner::new(pool));
        let manager = MigrationManager::new(runner);
        manager.run_migrations().await?;
        println!("✅ MySQL migrations completed successfully!");
        return Ok(());
    }

    eprintln!("❌ Unsupported database URL or missing feature flag");
    eprintln!("💡 Ensure you're using a PostgreSQL or MySQL URL");
    eprintln!("💡 Build with: cargo build --features postgres or cargo build --features mysql");
    process::exit(1);
}

async fn show_migration_status(database_url: &str) -> Result<(), Box<dyn std::error::Error>> {
    println!("📊 Hammerwork migration status");
    println!("📍 Database URL: {}", database_url);
    println!();

    #[cfg(feature = "postgres")]
    if database_url.starts_with("postgres") {
        let pool = sqlx::PgPool::connect(database_url).await?;
        let runner = Box::new(hammerwork::migrations::postgres::PostgresMigrationRunner::new(pool));
        let manager = MigrationManager::new(runner);
        show_status(&manager).await?;
        return Ok(());
    }

    #[cfg(feature = "mysql")]
    if database_url.starts_with("mysql") {
        let pool = sqlx::MySqlPool::connect(database_url).await?;
        let runner = Box::new(hammerwork::migrations::mysql::MySqlMigrationRunner::new(pool));
        let manager = MigrationManager::new(runner);
        show_status(&manager).await?;
        return Ok(());
    }

    eprintln!("❌ Unsupported database URL or missing feature flag");
    eprintln!("💡 Build with: cargo build --features postgres or cargo build --features mysql");
    process::exit(1);
}

async fn show_status<DB: sqlx::Database>(
    manager: &MigrationManager<DB>,
) -> Result<(), Box<dyn std::error::Error>> {
    let status = manager.get_migration_status().await?;

    println!("Migration Status:");
    println!("================");
    
    let mut executed_count = 0;
    let total_count = status.len();
    
    for (migration, executed) in &status {
        let status_emoji = if *executed { "✅" } else { "⏳" };
        let status_text = if *executed { "EXECUTED" } else { "PENDING" };
        
        if *executed {
            executed_count += 1;
        }
        
        println!("{} {} {} - {}", 
                 status_emoji, 
                 status_text, 
                 migration.id, 
                 migration.description);
    }
    
    println!();
    println!("📈 Progress: {}/{} migrations executed", executed_count, total_count);
    
    if executed_count == total_count {
        println!("🎉 All migrations up to date!");
    } else {
        println!("⚠️  {} pending migrations", total_count - executed_count);
        println!("💡 Run: cargo hammerwork migrate --database-url <URL>");
    }
    
    println!();
    Ok(())
}