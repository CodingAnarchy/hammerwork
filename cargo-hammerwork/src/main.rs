use anyhow::Result;
use clap::{Parser, Subcommand};
use std::env;
use tracing::{error, info};
use tracing_subscriber::EnvFilter;

use cargo_hammerwork::commands::*;
use cargo_hammerwork::config::Config;

#[derive(Parser)]
#[command(name = "cargo-hammerwork")]
#[command(bin_name = "cargo-hammerwork")]
#[command(about = "A comprehensive CLI tool for managing Hammerwork job queues")]
#[command(version, propagate_version = true)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
    
    #[arg(short, long, global = true, help = "Enable verbose logging")]
    verbose: bool,
    
    #[arg(short, long, global = true, help = "Suppress output except errors")]
    quiet: bool,
}

#[derive(Subcommand)]
enum Commands {
    #[command(about = "Database migration operations")]
    Migration {
        #[command(subcommand)]
        command: MigrationCommand,
    },
    
    #[command(about = "Configuration management")]
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
    
    #[command(about = "Job management operations")]
    Job {
        #[command(subcommand)]
        command: JobCommand,
    },
    
    #[command(about = "Worker management operations")]
    Worker {
        #[command(subcommand)]
        command: WorkerCommand,
    },
    
    #[command(about = "Queue management operations")]
    Queue {
        #[command(subcommand)]
        command: QueueCommand,
    },
    
    #[command(about = "Monitoring and observability")]
    Monitor {
        #[command(subcommand)]
        command: MonitorCommand,
    },
    
    #[command(about = "Backup and restore operations")]
    Backup {
        #[command(subcommand)]
        command: BackupCommand,
    },
    
    #[command(about = "Batch operations for multiple jobs")]
    Batch {
        #[command(subcommand)]
        command: BatchCommand,
    },
    
    #[command(about = "Cron job scheduling and management")]
    Cron {
        #[command(subcommand)]
        command: CronCommand,
    },
    
    #[command(about = "Database maintenance operations")]
    Maintenance {
        #[command(subcommand)]
        command: MaintenanceCommand,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    // Parse arguments early to handle cargo subcommand invocation
    let args = env::args_os().collect::<Vec<_>>();
    let is_cargo_subcommand = args.get(1).map(|s| s == "hammerwork").unwrap_or(false);
    
    let cli = if is_cargo_subcommand {
        Cli::parse_from(args.into_iter().skip(1))
    } else {
        Cli::parse()
    };

    // Initialize logging based on verbosity flags
    setup_logging(&cli)?;

    // Load configuration
    let mut config = Config::load().unwrap_or_else(|e| {
        if !cli.quiet {
            eprintln!("⚠️  Warning: Could not load config ({}), using defaults", e);
        }
        Config::default()
    });

    // Execute command
    let result = execute_command(&cli.command, &mut config).await;

    // Handle results
    match result {
        Ok(()) => {
            if cli.verbose {
                info!("✅ Command completed successfully");
            }
        }
        Err(e) => {
            error!("❌ Command failed: {}", e);
            std::process::exit(1);
        }
    }

    Ok(())
}

fn setup_logging(cli: &Cli) -> Result<()> {
    let log_level = if cli.quiet {
        "error"
    } else if cli.verbose {
        "debug"
    } else {
        "info"
    };

    let env_filter = EnvFilter::from_default_env()
        .add_directive(format!("cargo_hammerwork={}", log_level).parse()?)
        .add_directive(format!("hammerwork={}", log_level).parse()?);

    tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_target(false)
        .with_level(true)
        .init();

    Ok(())
}

async fn execute_command(command: &Commands, config: &mut Config) -> Result<()> {
    match command {
        Commands::Migration { command } => {
            command.execute(config).await?;
        }
        Commands::Config { command } => {
            command.execute(config).await?;
        }
        Commands::Job { command } => {
            command.execute(config).await?;
        }
        Commands::Worker { command } => {
            command.execute(config).await?;
        }
        Commands::Queue { command } => {
            command.execute(config).await?;
        }
        Commands::Monitor { command } => {
            command.execute(config).await?;
        }
        Commands::Backup { command } => {
            command.execute(config).await?;
        }
        Commands::Batch { command } => {
            command.execute(config).await?;
        }
        Commands::Cron { command } => {
            command.execute(config).await?;
        }
        Commands::Maintenance { command } => {
            command.execute(config).await?;
        }
    }

    Ok(())
}