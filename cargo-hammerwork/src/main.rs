//! # Cargo Hammerwork CLI
//!
//! A comprehensive command-line interface for managing Hammerwork job queues.
//!
//! This crate provides the `cargo hammerwork` CLI tool for database migrations,
//! job management, worker lease visibility, queue operations, and monitoring of Hammerwork
//! job processing systems.
//!
//! Rust examples in the doc comments of the internal modules are marked `ignore`: this is a
//! binary crate, so rustdoc does not compile or run them. The behaviour they describe is
//! covered by unit tests next to the code.
//!
//! ## Installation
//!
//! ```bash
//! cargo install cargo-hammerwork --features postgres
//! # or
//! cargo install cargo-hammerwork --features mysql
//! ```
//!
//! ## Basic Usage
//!
//! The CLI can be invoked either as a cargo subcommand or directly:
//!
//! ```bash
//! # As cargo subcommand
//! cargo hammerwork migration status --database-url postgresql://localhost/mydb
//!
//! # Direct invocation
//! cargo-hammerwork migration status --database-url postgresql://localhost/mydb
//! ```
//!
//! ## Common Commands
//!
//! ### Database Migrations
//!
//! ```bash
//! # Run all pending migrations
//! cargo hammerwork migration run --database-url postgresql://localhost/mydb
//!
//! # Check migration status
//! cargo hammerwork migration status --database-url postgresql://localhost/mydb
//!
//! # Rollback last migration
//! cargo hammerwork migration rollback --database-url postgresql://localhost/mydb
//! ```
//!
//! ### Job Management
//!
//! ```bash
//! # List jobs in a queue
//! cargo hammerwork job list --database-url postgresql://localhost/mydb --queue default
//!
//! # Enqueue a new job
//! cargo hammerwork job enqueue --database-url postgresql://localhost/mydb \
//!   --queue email --payload '{"to": "user@example.com"}'
//!
//! # Retry failed jobs
//! cargo hammerwork job retry --database-url postgresql://localhost/mydb --all-failed
//!
//! # Cancel a specific job
//! cargo hammerwork job cancel --database-url postgresql://localhost/mydb <job-id>
//! ```
//!
//! ### Queue Operations
//!
//! ```bash
//! # List all queues with statistics
//! cargo hammerwork queue list --database-url postgresql://localhost/mydb
//!
//! # Clear dead jobs from a queue
//! cargo hammerwork queue clear --database-url postgresql://localhost/mydb \
//!   --queue default --dead
//!
//! # Show queue statistics
//! cargo hammerwork queue stats --database-url postgresql://localhost/mydb \
//!   --queue default
//! ```
//!
//! ### Worker Management
//!
//! ```bash
//! # Running jobs and the worker leases (heartbeats) behind them, per queue
//! cargo hammerwork worker status --database-url postgresql://localhost/mydb
//!
//! # Also list every running job
//! cargo hammerwork worker status --database-url postgresql://localhost/mydb --jobs
//! ```
//!
//! ### Monitoring
//!
//! ```bash
//! # Show real-time dashboard
//! cargo hammerwork monitor dashboard --database-url postgresql://localhost/mydb
//!
//! # Display queue metrics
//! cargo hammerwork monitor metrics --database-url postgresql://localhost/mydb
//!
//! # Check system health
//! cargo hammerwork monitor health --database-url postgresql://localhost/mydb
//! ```
//!
//! ## Configuration
//!
//! The CLI supports configuration files to avoid repetitive arguments:
//!
//! ```bash
//! # Create default config
//! cargo hammerwork config init
//!
//! # Show current config
//! cargo hammerwork config show
//!
//! # Set database URL in config
//! cargo hammerwork config set database_url postgresql://localhost/mydb
//! ```
//!
//! Configuration files are stored at:
//! - Linux/Mac: `~/.config/hammerwork/config.toml`
//! - Windows: `%APPDATA%\hammerwork\config.toml`
//!
//! ## Environment Variables
//!
//! The CLI respects these environment variables:
//!
//! - `DATABASE_URL` - Default database connection URL
//! - `HAMMERWORK_CONFIG` - Path to config file
//! - `HAMMERWORK_LOG` - Log level (error, warn, info, debug, trace)
//!
//! ## Examples
//!
//! ### Complete Workflow Example
//!
//! ```bash
//! # 1. Initialize configuration
//! cargo hammerwork config init
//! cargo hammerwork config set database_url postgresql://localhost/hammerwork
//!
//! # 2. Run migrations to set up database
//! cargo hammerwork migration run
//!
//! # 3. Enqueue some jobs
//! cargo hammerwork job enqueue --queue email \
//!   --payload '{"to": "user@example.com", "subject": "Welcome!"}'
//!
//! cargo hammerwork job enqueue --queue email \
//!   --payload '{"to": "admin@example.com", "subject": "New user"}' \
//!   --priority high
//!
//! # 4. Check queue status
//! cargo hammerwork queue list
//!
//! # 5. Process jobs with a worker in your application (the CLI does not run workers)
//!
//! # 6. Monitor progress
//! cargo hammerwork monitor dashboard
//! ```
//!
//! ### Batch Operations Example
//!
//! ```bash
//! # Enqueue multiple jobs at once
//! cargo hammerwork batch enqueue --queue data-processing \
//!   --payloads '[
//!     {"file": "data1.csv"},
//!     {"file": "data2.csv"},
//!     {"file": "data3.csv"}
//!   ]'
//!
//! # Cancel all jobs in a batch
//! cargo hammerwork batch cancel <batch-id>
//!
//! # Show batch status
//! cargo hammerwork batch status <batch-id>
//! ```
//!
//! ### Cron Scheduling Example
//!
//! ```bash
//! # Create a recurring job
//! cargo hammerwork cron create --queue reports \
//!   --schedule "0 9 * * MON-FRI" \
//!   --timezone "America/New_York" \
//!   --payload '{"type": "daily_report"}'
//!
//! # List cron jobs
//! cargo hammerwork cron list
//!
//! # Disable a cron job
//! cargo hammerwork cron disable <job-id>
//! ```
//!
//! ## Feature Flags
//!
//! - `postgres` - PostgreSQL support (recommended)
//! - `mysql` - MySQL support
//! - `completions` - Shell completion generation
//!
//! ## Error Handling
//!
//! The CLI provides detailed error messages and non-zero exit codes on failure:
//!
//! - Exit code 0: Success
//! - Exit code 1: General error
//! - Exit code 2: Configuration error
//! - Exit code 3: Database connection error
//! - Exit code 4: Invalid arguments
//!
//! ## Verbose and Quiet Modes
//!
//! Control output verbosity with global flags:
//!
//! ```bash
//! # Verbose mode - show debug information
//! cargo hammerwork -v migration run
//!
//! # Quiet mode - suppress all but errors
//! cargo hammerwork -q job list
//! ```

mod commands;
mod config;
#[cfg(test)]
mod integration_tests;
mod utils;

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::env;
use tracing::{error, info};
use tracing_subscriber::EnvFilter;

use commands::*;
use config::Config;

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

    #[command(about = "Encryption key operations (key audit log)")]
    Encryption {
        #[command(subcommand)]
        command: EncryptionCommand,
    },

    #[command(about = "Database maintenance operations")]
    Maintenance {
        #[command(subcommand)]
        command: MaintenanceCommand,
    },

    #[command(about = "Workflow and job dependency management")]
    Workflow {
        #[command(subcommand)]
        command: WorkflowCommand,
    },

    #[command(about = "Job archival and retention management")]
    Archive {
        #[command(subcommand)]
        command: ArchiveCommand,
    },

    #[command(about = "Job spawning operations and spawn tree management")]
    Spawn {
        #[command(subcommand)]
        command: SpawnCommand,
    },

    #[command(about = "Webhook management for job lifecycle events")]
    Webhook {
        #[command(subcommand)]
        command: WebhookCommand,
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

    // Load configuration. A config file that cannot be read or parsed falls back to the
    // defaults, still overridden by the environment variables.
    let (mut config, load_error) = Config::load_or_env();
    if let Some(e) = load_error
        && !cli.quiet
    {
        eprintln!("⚠️  Warning: Could not load config ({}), using defaults", e);
    }

    // Initialize logging from the verbosity flags, else the configured log level
    setup_logging(&cli, &config)?;

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

/// The log level: `--quiet` and `--verbose` win, then the configured `log_level`
/// (`HAMMERWORK_LOG_LEVEL` or the config file), then `info`. An unknown configured level
/// is ignored.
fn effective_log_level(quiet: bool, verbose: bool, config: &Config) -> &'static str {
    const LEVELS: [&str; 5] = ["error", "warn", "info", "debug", "trace"];
    if quiet {
        "error"
    } else if verbose {
        "debug"
    } else {
        let configured = config.get_log_level();
        LEVELS
            .into_iter()
            .find(|level| level.eq_ignore_ascii_case(configured.trim()))
            .unwrap_or("info")
    }
}

fn setup_logging(cli: &Cli, config: &Config) -> Result<()> {
    let log_level = effective_log_level(cli.quiet, cli.verbose, config);

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
        Commands::Encryption { command } => {
            command.execute(config).await?;
        }
        Commands::Maintenance { command } => {
            command.execute(config).await?;
        }
        Commands::Workflow { command } => {
            command.execute(config.clone()).await?;
        }
        Commands::Archive { command } => {
            command.execute(config).await?;
        }
        Commands::Spawn { command } => {
            command.execute(config.clone()).await?;
        }
        Commands::Webhook { command } => {
            handle_webhook_command(command.clone(), config).await?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    /// clap's own consistency check: panics on duplicate short/long flags (including
    /// clashes with the global `--verbose`/`--quiet`), which otherwise only show up
    /// at runtime in debug builds when the offending subcommand is parsed.
    #[test]
    fn cli_definition_is_consistent() {
        Cli::command().debug_assert();
    }

    #[test]
    fn short_q_is_quiet_and_queue_uses_capital_q() {
        let cli = Cli::try_parse_from(["cargo-hammerwork", "-q", "config", "show"]).unwrap();
        assert!(cli.quiet);

        for args in [
            &["spawn", "stats", "-Q", "emails"][..],
            &["spawn", "pending", "-Q", "emails"][..],
            &["archive", "list", "-Q", "emails"][..],
            &["job", "purge", "-Q", "emails", "--completed"][..],
        ] {
            let mut argv = vec!["cargo-hammerwork"];
            argv.extend_from_slice(args);
            Cli::try_parse_from(&argv).unwrap_or_else(|e| panic!("{argv:?} should parse: {e}"));
        }

        // Long flags are unchanged and still combine with the global quiet flag.
        let cli = Cli::try_parse_from(["cargo-hammerwork", "spawn", "stats", "--queue", "e", "-q"])
            .unwrap();
        assert!(cli.quiet);
    }

    #[test]
    fn global_flags_parse_after_subcommand() {
        let cli = Cli::try_parse_from([
            "cargo-hammerwork",
            "spawn",
            "stats",
            "--queue",
            "emails",
            "-v",
        ])
        .unwrap();
        assert!(cli.verbose);
        assert!(!cli.quiet);
    }

    /// Value of `id` on the innermost subcommand matched by `args`.
    fn leaf_value(args: &[&str], id: &str) -> Option<String> {
        let mut argv = vec!["cargo-hammerwork"];
        argv.extend_from_slice(args);
        let matches = Cli::command()
            .try_get_matches_from(&argv)
            .unwrap_or_else(|e| panic!("{argv:?} should parse: {e}"));
        let mut m = &matches;
        while let Some((_, sub)) = m.subcommand() {
            m = sub;
        }
        m.get_one::<String>(id).cloned()
    }

    #[test]
    fn database_url_accepts_u_long_and_the_old_queue_d() {
        for sub in ["list", "paused"] {
            for flag in ["-u", "--database-url", "-d"] {
                assert_eq!(
                    leaf_value(&["queue", sub, flag, "postgres://x/y"], "database_url").as_deref(),
                    Some("postgres://x/y"),
                    "queue {sub} {flag}"
                );
            }
        }
        for flag in ["-u", "-d"] {
            for sub in ["stats", "health"] {
                assert!(leaf_value(&["queue", sub, flag, "u"], "database_url").is_some());
            }
            assert!(
                leaf_value(&["queue", "pause", flag, "u", "-n", "q"], "database_url").is_some()
            );
        }
        // -d stays hidden from help
        let help = Cli::command()
            .find_subcommand_mut("queue")
            .unwrap()
            .find_subcommand_mut("list")
            .unwrap()
            .render_long_help()
            .to_string();
        assert!(help.contains("-u, --database-url"), "{help}");
        assert!(!help.contains(" -d,") && !help.contains("-d "), "{help}");
    }

    #[test]
    fn queue_flag_is_n_with_q_and_long_aliases() {
        let cases: &[(&[&str], &str)] = &[
            (&["queue", "stats"], "queue"),
            (&["queue", "pause"], "queue"),
            (&["job", "list"], "queue"),
            (&["job", "purge", "--completed"], "queue"),
            (&["job", "retry"], "queue"),
            (&["batch", "retry"], "queue"),
            (&["batch", "export", "-o", "f"], "queue"),
            (&["spawn", "list"], "queue"),
            (&["spawn", "stats"], "queue"),
            (&["spawn", "pending"], "queue"),
            (&["spawn", "monitor"], "queue"),
            (&["archive", "run"], "queue_name"),
            (&["archive", "list"], "queue_name"),
            (&["archive", "stats"], "queue_name"),
            (&["monitor", "dashboard"], "queue"),
            (&["monitor", "metrics"], "queue"),
            (&["worker", "status"], "queue"),
            (&["backup", "create", "-o", "f"], "queue"),
            (&["cron", "list"], "queue"),
        ];
        for (base, id) in cases {
            for flag in ["-n", "-Q", "--queue"] {
                let mut args = base.to_vec();
                args.extend(["--database-url", "u", flag, "emails"]);
                // some of these subcommands have no database_url; retry without it
                let mut argv = vec!["cargo-hammerwork"];
                argv.extend_from_slice(&args);
                let args = if Cli::try_parse_from(&argv).is_ok() {
                    args
                } else {
                    let mut a = base.to_vec();
                    a.extend([flag, "emails"]);
                    a
                };
                assert_eq!(leaf_value(&args, id).as_deref(), Some("emails"), "{args:?}");
            }
        }
        // archive keeps its old --queue-name spelling
        assert_eq!(
            leaf_value(&["archive", "run", "--queue-name", "q"], "queue_name").as_deref(),
            Some("q")
        );
        assert_eq!(
            leaf_value(&["archive", "list", "--queue-name", "q"], "queue_name").as_deref(),
            Some("q")
        );
    }

    #[test]
    fn job_retry_and_cancel_take_the_id_positionally_or_via_flag() {
        for cmd in ["retry", "cancel"] {
            assert_eq!(
                leaf_value(&["job", cmd, "abc"], "id").as_deref(),
                Some("abc")
            );
            assert_eq!(
                leaf_value(&["job", cmd, "--job-id", "abc"], "job_id").as_deref(),
                Some("abc")
            );
            assert_eq!(leaf_value(&["job", cmd, "--job-id", "abc"], "id"), None);
            // both at once is a usage error
            assert!(
                Cli::try_parse_from(["cargo-hammerwork", "job", cmd, "abc", "--job-id", "def"])
                    .is_err()
            );
        }
        // positional id combines with the other flags
        assert!(
            Cli::try_parse_from(["cargo-hammerwork", "job", "retry", "abc", "-u", "u"]).is_ok()
        );
    }

    #[test]
    fn job_retry_help_mentions_timed_out_jobs() {
        let help = Cli::command()
            .find_subcommand_mut("job")
            .unwrap()
            .find_subcommand_mut("retry")
            .unwrap()
            .render_long_help()
            .to_string();
        assert!(
            help.to_lowercase().contains("timed-out") || help.contains("TimedOut"),
            "{help}"
        );
    }

    #[test]
    fn batch_flags_and_help_are_accurate() {
        let mut batch = Cli::command();
        let enqueue = batch
            .find_subcommand_mut("batch")
            .unwrap()
            .find_subcommand_mut("enqueue")
            .unwrap();
        let help = enqueue.render_long_help().to_string();
        assert!(!help.contains("Default queue name"), "{help}");
        assert!(help.contains("required"), "{help}");
        for args in [
            &["batch", "enqueue", "-n", "q", "--batch-size", "7"][..],
            &["batch", "enqueue", "-n", "q", "--progress-every", "7"][..],
        ] {
            let mut argv = vec!["cargo-hammerwork"];
            argv.extend_from_slice(args);
            let matches = Cli::command().try_get_matches_from(&argv).unwrap();
            let (_, b) = matches.subcommand().unwrap();
            let (_, e) = b.subcommand().unwrap();
            assert_eq!(e.get_one::<u32>("batch_size"), Some(&7));
        }
        assert!(
            Cli::try_parse_from(["cargo-hammerwork", "batch", "enqueue"]).is_ok(),
            "the queue may come from default_queue"
        );
        let retry_help = Cli::command()
            .find_subcommand_mut("batch")
            .unwrap()
            .find_subcommand_mut("retry")
            .unwrap()
            .render_long_help()
            .to_string();
        assert!(
            !retry_help.contains("Maximum attempts filter"),
            "{retry_help}"
        );
    }

    #[test]
    fn archive_compress_can_be_disabled_three_ways() {
        let compress = |args: &[&str]| -> (bool, bool) {
            let mut argv = vec!["cargo-hammerwork", "archive", "run"];
            argv.extend_from_slice(args);
            let matches = Cli::command().try_get_matches_from(&argv).unwrap();
            let (_, a) = matches.subcommand().unwrap();
            let (_, r) = a.subcommand().unwrap();
            (
                r.get_flag("no_compress"),
                *r.get_one::<bool>("compress").unwrap(),
            )
        };
        assert_eq!(compress(&[]), (false, true));
        assert_eq!(compress(&["--compress"]), (false, true));
        assert_eq!(compress(&["--compress", "true"]), (false, true));
        assert_eq!(compress(&["--compress", "false"]), (false, false));
        assert_eq!(compress(&["--no-compress"]), (true, true));
        assert!(
            Cli::try_parse_from([
                "cargo-hammerwork",
                "archive",
                "run",
                "--compress",
                "--no-compress"
            ])
            .is_err()
        );
    }

    #[test]
    fn log_level_comes_from_the_flags_then_the_config() {
        let mut config = Config {
            log_level: Some("WARN".to_string()),
            ..Config::default()
        };
        assert_eq!(effective_log_level(false, false, &config), "warn");
        assert_eq!(effective_log_level(true, false, &config), "error");
        assert_eq!(effective_log_level(false, true, &config), "debug");
        config.log_level = Some("loud".to_string());
        assert_eq!(effective_log_level(false, false, &config), "info");
        config.log_level = None;
        assert_eq!(effective_log_level(false, false, &config), "info");
    }
}
