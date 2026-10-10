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
            Cli::try_parse_from(["cargo-hammerwork", "batch", "enqueue"]).is_err(),
            "queue stays required"
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
}
