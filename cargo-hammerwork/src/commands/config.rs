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

/// The rows of `config show`: setting, value, source. Secrets are never shown: the
/// database URL's password is replaced by `***`, and encryption keys are only ever
/// referenced (`env://VAR`, a KMS URI), never stored in a configuration.
fn config_rows(
    config: &Config,
    env: impl Fn(&str) -> Option<String>,
) -> Vec<(&'static str, String, &'static str)> {
    let from = |value: &Option<String>, var: &str| match (value, env(var)) {
        (Some(value), _) => (value.clone(), "Config File"),
        (None, Some(value)) => (value, "Environment"),
        (None, None) => ("Not set".to_string(), "Default"),
    };
    let (db_url, db_source) = from(&config.database_url, "DATABASE_URL");
    let db_url = crate::utils::validation::redact_url(&db_url);
    let (queue, queue_source) = from(&config.default_queue, "HAMMERWORK_DEFAULT_QUEUE");
    let (encryption_config, encryption_source) =
        from(&config.encryption_config, "HAMMERWORK_ENCRYPTION_CONFIG");
    let encryption = match config.encryption_settings() {
        Ok(settings) if settings.enabled => format!(
            "enabled ({:?}, key {} from {}, encrypted queues: {})",
            settings.algorithm,
            settings.key_id.as_deref().unwrap_or("default"),
            settings.key_source,
            if settings.encrypted_queues.is_empty() {
                "none".to_string()
            } else {
                settings.encrypted_queues.join(", ")
            }
        ),
        Ok(_) => "disabled".to_string(),
        Err(e) => format!("invalid: {}", e),
    };
    vec![
        ("database_url", db_url, db_source),
        ("default_queue", queue, queue_source),
        (
            "default_limit",
            config.get_default_limit().to_string(),
            "Config File",
        ),
        (
            "log_level",
            config.get_log_level().to_string(),
            "Config File",
        ),
        (
            "connection_pool_size",
            config.get_connection_pool_size().to_string(),
            "Config File",
        ),
        (
            "connect_timeout_secs",
            config.get_connect_timeout_secs().to_string(),
            "Config File",
        ),
        ("encryption_config", encryption_config, encryption_source),
        (
            "encryption",
            encryption,
            "encryption_config + HAMMERWORK_ENCRYPTION_*",
        ),
    ]
}

async fn show_config(config: &Config) -> Result<()> {
    println!("⚙️  Hammerwork Configuration");
    println!("═══════════════════════════");

    let mut table = comfy_table::Table::new();
    table.set_header(vec!["Setting", "Value", "Source"]);
    for (setting, value, source) in config_rows(config, |var| std::env::var(var).ok()) {
        table.add_row(vec![setting, value.as_str(), source]);
    }

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
        "connect_timeout_secs" => {
            let secs: u64 = value
                .parse()
                .map_err(|_| anyhow::anyhow!("connect_timeout_secs must be a positive integer"))?;
            if secs == 0 || secs > 600 {
                return Err(anyhow::anyhow!(
                    "connect_timeout_secs must be between 1 and 600"
                ));
            }
            config.connect_timeout_secs = Some(secs);
            info!("✅ Set connect_timeout_secs to: {}", secs);
        }
        "encryption_config" => {
            let candidate = Config {
                encryption_config: Some(value.to_string()),
                ..config.clone()
            };
            candidate.encryption_settings()?;
            config.encryption_config = Some(value.to_string());
            info!("✅ Set encryption_config to: {}", value);
        }
        _ => {
            return Err(anyhow::anyhow!(
                "Unknown configuration key: {}. Valid keys: {}",
                key,
                VALID_KEYS
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
        "connect_timeout_secs" => config.get_connect_timeout_secs().to_string(),
        "encryption_config" => config
            .encryption_config
            .as_deref()
            .unwrap_or("Not set")
            .to_string(),
        _ => {
            return Err(anyhow::anyhow!(
                "Unknown configuration key: {}. Valid keys: {}",
                key,
                VALID_KEYS
            ));
        }
    };

    println!("{}", value);
    Ok(())
}

const VALID_KEYS: &str = "database_url, default_queue, default_limit, log_level, \
                          connection_pool_size, connect_timeout_secs, encryption_config";

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
        let mut config = Config {
            database_url: Some("mysql://h/db".into()),
            ..Config::default()
        };
        for key in [
            "database_url",
            "default_queue",
            "default_limit",
            "log_level",
            "connection_pool_size",
            "connect_timeout_secs",
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

    #[test]
    fn show_never_prints_the_database_password() {
        let config = Config {
            database_url: Some("postgres://app:hunter2-db@db.internal/jobs".into()),
            ..Config::default()
        };
        let rows = config_rows(&config, |_| None);
        let shown = format!("{rows:?}");
        assert!(!shown.contains("hunter2-db"), "{shown}");
        assert!(
            shown.contains("postgres://app:***@db.internal/jobs"),
            "{shown}"
        );
        assert!(!format!("{config:?}").contains("hunter2-db"));

        // Also when the URL comes from the environment
        let rows = config_rows(&Config::default(), |var| {
            (var == "DATABASE_URL").then(|| "mysql://root:hunter2-env@db/jobs".to_string())
        });
        let (_, url, source) = &rows[0];
        assert_eq!(
            (url.as_str(), *source),
            ("mysql://root:***@db/jobs", "Environment")
        );
    }

    #[test]
    fn show_summarises_the_encryption_settings() {
        let _guard = crate::utils::test_support::serial_blocking();
        let dir = tempfile::tempdir().unwrap();
        let app = dir.path().join("hammerwork.toml");
        std::fs::write(
            &app,
            "[encryption]\nenabled = true\nkey_source = \"env://APP_KEY\"\n\
             encrypted_queues = [\"payments\"]\n",
        )
        .unwrap();
        let config = Config {
            encryption_config: Some(app.to_string_lossy().into_owned()),
            ..Config::default()
        };
        let rows = config_rows(&config, |_| None);
        let encryption = &rows.iter().find(|r| r.0 == "encryption").unwrap().1;
        assert!(encryption.starts_with("enabled"), "{encryption}");
        assert!(encryption.contains("env://APP_KEY"), "{encryption}");
        assert!(encryption.contains("payments"), "{encryption}");
        let rows = config_rows(&Config::default(), |_| None);
        assert_eq!(
            rows.iter().find(|r| r.0 == "encryption").unwrap().1,
            "disabled"
        );
    }

    #[tokio::test]
    async fn encryption_config_can_be_set_and_read() {
        let _guard = crate::utils::test_support::serial().await;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let app = dir.path().join("hammerwork.toml");
        std::fs::write(&app, "[encryption]\nenabled = false\n").unwrap();
        let mut config = Config::default();
        run(
            &["set", "encryption_config", app.to_str().unwrap()],
            &mut config,
            &path,
        )
        .await
        .unwrap();
        assert_eq!(
            Config::load_from_path(&path)
                .unwrap()
                .encryption_config
                .as_deref(),
            app.to_str()
        );
        run(&["get", "encryption_config"], &mut config, &path)
            .await
            .unwrap();
        // A file that cannot be read is rejected and not saved
        let err = run(
            &["set", "encryption_config", "/no/such/hammerwork.toml"],
            &mut config,
            &path,
        )
        .await
        .unwrap_err();
        assert!(
            err.to_string().contains("Invalid encryption settings"),
            "{err}"
        );
        assert_eq!(config.encryption_config.as_deref(), app.to_str());
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
