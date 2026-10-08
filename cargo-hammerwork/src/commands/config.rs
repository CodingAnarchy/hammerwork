use anyhow::Result;
use clap::Subcommand;
use std::path::Path;
use tracing::info;

use crate::config::Config;

#[derive(Subcommand)]
pub enum ConfigCommand {
    #[command(about = "Show current configuration")]
    Show,
    #[command(about = "Set a configuration value")]
    Set {
        #[arg(help = "Configuration key")]
        key: String,
        #[arg(help = "Configuration value")]
        value: String,
    },
    #[command(about = "Get a configuration value")]
    Get {
        #[arg(help = "Configuration key")]
        key: String,
    },
    #[command(about = "Reset configuration to defaults")]
    Reset {
        #[arg(long, help = "Confirm the reset operation")]
        confirm: bool,
    },
    #[command(about = "Show configuration file path")]
    Path,
}

impl ConfigCommand {
    pub async fn execute(&self, config: &mut Config) -> Result<()> {
        self.execute_at(config, &Config::config_file_path()?).await
    }

    /// Run the command against the config file at `path`.
    pub async fn execute_at(&self, config: &mut Config, path: &Path) -> Result<()> {
        match self {
            ConfigCommand::Show => {
                show_config(config).await?;
            }
            ConfigCommand::Set { key, value } => {
                set_config_value(config, key, value, path).await?;
            }
            ConfigCommand::Get { key } => {
                get_config_value(config, key).await?;
            }
            ConfigCommand::Reset { confirm } => {
                reset_config(config, *confirm, path).await?;
            }
            ConfigCommand::Path => {
                show_config_path(path).await?;
            }
        }
        Ok(())
    }
}

async fn show_config(config: &Config) -> Result<()> {
    println!("⚙️  Hammerwork Configuration");
    println!("═══════════════════════════");

    let mut table = comfy_table::Table::new();
    table.set_header(vec!["Setting", "Value", "Source"]);

    // Database URL
    let (db_url, db_source) = if let Some(url) = &config.database_url {
        (url.clone(), "Config File")
    } else if let Ok(env_url) = std::env::var("DATABASE_URL") {
        (env_url, "Environment")
    } else {
        ("Not set".to_string(), "Default")
    };
    table.add_row(vec!["database_url", &db_url, db_source]);

    // Default queue
    let (queue, queue_source) = if let Some(q) = &config.default_queue {
        (q.clone(), "Config File")
    } else if let Ok(env_queue) = std::env::var("HAMMERWORK_DEFAULT_QUEUE") {
        (env_queue, "Environment")
    } else {
        ("Not set".to_string(), "Default")
    };
    table.add_row(vec!["default_queue", &queue, queue_source]);

    // Default limit
    let limit = config.get_default_limit().to_string();
    table.add_row(vec!["default_limit", &limit, "Config File"]);

    // Log level
    let log_level = config.get_log_level();
    table.add_row(vec!["log_level", log_level, "Config File"]);

    // Connection pool size
    let pool_size = config.get_connection_pool_size().to_string();
    table.add_row(vec!["connection_pool_size", &pool_size, "Config File"]);

    println!("{}", table);

    println!("\n💡 Configuration priority: Environment Variables > Config File > Defaults");
    println!("📝 Use 'config set <key> <value>' to update configuration");

    Ok(())
}

async fn set_config_value(config: &mut Config, key: &str, value: &str, path: &Path) -> Result<()> {
    match key {
        "database_url" => {
            crate::utils::validation::validate_database_url(value)?;
            config.database_url = Some(value.to_string());
            info!("✅ Set database_url");
        }
        "default_queue" => {
            config.default_queue = Some(value.to_string());
            info!("✅ Set default_queue to: {}", value);
        }
        "default_limit" => {
            let limit: u32 = value
                .parse()
                .map_err(|_| anyhow::anyhow!("default_limit must be a positive integer"))?;
            config.default_limit = Some(limit);
            info!("✅ Set default_limit to: {}", limit);
        }
        "log_level" => {
            if !["trace", "debug", "info", "warn", "error"].contains(&value.to_lowercase().as_str())
            {
                return Err(anyhow::anyhow!(
                    "log_level must be one of: trace, debug, info, warn, error"
                ));
            }
            config.log_level = Some(value.to_lowercase());
            info!("✅ Set log_level to: {}", value);
        }
        "connection_pool_size" => {
            let size: u32 = value
                .parse()
                .map_err(|_| anyhow::anyhow!("connection_pool_size must be a positive integer"))?;
            if size == 0 || size > 100 {
                return Err(anyhow::anyhow!(
                    "connection_pool_size must be between 1 and 100"
                ));
            }
            config.connection_pool_size = Some(size);
            info!("✅ Set connection_pool_size to: {}", size);
        }
        _ => {
            return Err(anyhow::anyhow!(
                "Unknown configuration key: {}. Valid keys: database_url, default_queue, default_limit, log_level, connection_pool_size",
                key
            ));
        }
    }

    // Save the updated configuration
    config.save_to(path)?;
    println!("💾 Configuration saved");

    Ok(())
}

async fn get_config_value(config: &Config, key: &str) -> Result<()> {
    let value = match key {
        "database_url" => config.get_database_url().unwrap_or("Not set").to_string(),
        "default_queue" => config.get_default_queue().unwrap_or("Not set").to_string(),
        "default_limit" => config.get_default_limit().to_string(),
        "log_level" => config.get_log_level().to_string(),
        "connection_pool_size" => config.get_connection_pool_size().to_string(),
        _ => {
            return Err(anyhow::anyhow!(
                "Unknown configuration key: {}. Valid keys: database_url, default_queue, default_limit, log_level, connection_pool_size",
                key
            ));
        }
    };

    println!("{}", value);
    Ok(())
}

async fn reset_config(config: &mut Config, confirm: bool, path: &Path) -> Result<()> {
    if !confirm {
        println!("⚠️  This will reset all configuration to defaults. Use --confirm to proceed.");
        return Ok(());
    }

    *config = Config::default();
    config.save_to(path)?;

    println!("🔄 Configuration reset to defaults");
    info!("Configuration has been reset to defaults");

    Ok(())
}

async fn show_config_path(config_path: &Path) -> Result<()> {
    println!("📁 Configuration file path:");
    println!("{}", config_path.display());

    if config_path.exists() {
        println!("✅ File exists");

        // Show file size and modification time
        let metadata = std::fs::metadata(config_path)?;
        let size = metadata.len();
        let modified = metadata.modified()?;
        let modified_time = chrono::DateTime::<chrono::Utc>::from(modified);

        println!("📊 Size: {} bytes", size);
        println!(
            "📅 Last modified: {}",
            modified_time.format("%Y-%m-%d %H:%M:%S UTC")
        );
    } else {
        println!("❌ File does not exist (will be created when configuration is saved)");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct TestCli {
        #[command(subcommand)]
        command: ConfigCommand,
    }

    fn parse(args: &[&str]) -> ConfigCommand {
        let mut argv = vec!["test"];
        argv.extend_from_slice(args);
        TestCli::try_parse_from(argv).unwrap().command
    }

    async fn run(args: &[&str], config: &mut Config, path: &Path) -> Result<()> {
        parse(args).execute_at(config, path).await
    }

    #[test]
    fn parses_every_subcommand_and_flag() {
        assert!(matches!(parse(&["show"]), ConfigCommand::Show));
        assert!(matches!(parse(&["path"]), ConfigCommand::Path));
        match parse(&["set", "default_limit", "9"]) {
            ConfigCommand::Set { key, value } => {
                assert_eq!((key.as_str(), value.as_str()), ("default_limit", "9"));
            }
            _ => panic!("expected Set"),
        }
        match parse(&["get", "log_level"]) {
            ConfigCommand::Get { key } => assert_eq!(key, "log_level"),
            _ => panic!("expected Get"),
        }
        assert!(matches!(
            parse(&["reset"]),
            ConfigCommand::Reset { confirm: false }
        ));
        assert!(matches!(
            parse(&["reset", "--confirm"]),
            ConfigCommand::Reset { confirm: true }
        ));
        // set needs both arguments
        assert!(TestCli::try_parse_from(["test", "set", "only_key"]).is_err());
        assert!(TestCli::try_parse_from(["test", "frobnicate"]).is_err());
    }

    #[tokio::test]
    async fn set_persists_each_valid_key_to_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let mut config = Config::default();

        run(
            &["set", "database_url", "postgres://u@h/db"],
            &mut config,
            &path,
        )
        .await
        .unwrap();
        run(&["set", "default_queue", "emails"], &mut config, &path)
            .await
            .unwrap();
        run(&["set", "default_limit", "25"], &mut config, &path)
            .await
            .unwrap();
        run(&["set", "log_level", "DEBUG"], &mut config, &path)
            .await
            .unwrap();
        run(&["set", "connection_pool_size", "8"], &mut config, &path)
            .await
            .unwrap();

        let saved = Config::load_from_path(&path).unwrap();
        assert_eq!(saved.get_database_url(), Some("postgres://u@h/db"));
        assert_eq!(saved.get_default_queue(), Some("emails"));
        assert_eq!(saved.get_default_limit(), 25);
        assert_eq!(saved.get_log_level(), "debug", "log level is normalised");
        assert_eq!(saved.get_connection_pool_size(), 8);
        assert_eq!(config.get_default_limit(), 25, "in-memory config updated");
    }

    #[tokio::test]
    async fn set_rejects_invalid_values_without_touching_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let mut config = Config::default();

        for (key, value, expected) in [
            ("database_url", "http://nope", "Invalid database URL"),
            ("default_limit", "many", "positive integer"),
            ("log_level", "loud", "log_level must be one of"),
            ("connection_pool_size", "0", "between 1 and 100"),
            ("connection_pool_size", "101", "between 1 and 100"),
            ("connection_pool_size", "x", "positive integer"),
            ("no_such_key", "1", "Unknown configuration key: no_such_key"),
        ] {
            let err = run(&["set", key, value], &mut config, &path)
                .await
                .unwrap_err()
                .to_string();
            assert!(err.contains(expected), "{key}={value}: {err}");
        }
        assert!(!path.exists(), "nothing was saved");
        assert_eq!(config.get_default_limit(), 50);
        assert_eq!(config.get_database_url(), None);
    }

    #[tokio::test]
    async fn get_reads_known_keys_and_rejects_unknown_ones() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let mut config = Config::default();
        config.database_url = Some("mysql://h/db".into());
        for key in [
            "database_url",
            "default_queue",
            "default_limit",
            "log_level",
            "connection_pool_size",
        ] {
            run(&["get", key], &mut config, &path).await.unwrap();
        }
        let err = run(&["get", "bogus"], &mut config, &path)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("Unknown configuration key: bogus"), "{err}");
    }

    #[tokio::test]
    async fn reset_requires_confirmation_then_restores_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let mut config = Config::default();
        run(&["set", "default_limit", "77"], &mut config, &path)
            .await
            .unwrap();

        run(&["reset"], &mut config, &path).await.unwrap();
        assert_eq!(
            config.get_default_limit(),
            77,
            "no change without --confirm"
        );
        assert_eq!(
            Config::load_from_path(&path).unwrap().get_default_limit(),
            77
        );

        run(&["reset", "--confirm"], &mut config, &path)
            .await
            .unwrap();
        assert_eq!(config.get_default_limit(), 50);
        assert_eq!(
            Config::load_from_path(&path).unwrap().get_default_limit(),
            50
        );
    }

    #[tokio::test]
    async fn show_and_path_work_with_and_without_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let mut config = Config::default();
        run(&["show"], &mut config, &path).await.unwrap();
        run(&["path"], &mut config, &path).await.unwrap(); // file missing
        run(&["set", "default_queue", "q"], &mut config, &path)
            .await
            .unwrap();
        run(&["path"], &mut config, &path).await.unwrap(); // file exists
        run(&["show"], &mut config, &path).await.unwrap();
    }
}
