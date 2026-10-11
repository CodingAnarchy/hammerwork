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
                show_config(config, path).await?;
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

/// Where a setting's effective value comes from: an environment variable that is set
/// (and, for numbers, parses, as `Config::load` ignores the others) wins, then the config
/// file, else the built-in default.
fn source_of(in_env: bool, in_file: bool) -> &'static str {
    if in_env {
        "Environment"
    } else if in_file {
        "Config File"
    } else {
        "Default"
    }
}

/// The rows of `config show`: setting, value, source. `config` is the effective
/// configuration (file plus environment) and `file` the config file alone, which tells
/// the two sources apart. Secrets are never shown: the database URL's password is
/// replaced by `***`, and encryption keys are only ever referenced (`env://VAR`, a KMS
/// URI), never stored in a configuration.
fn config_rows(
    config: &Config,
    file: &Config,
    env: impl Fn(&str) -> Option<String>,
) -> Vec<(&'static str, String, &'static str)> {
    let text = |var: &str| env(var).is_some();
    let number = |var: &str| env(var).is_some_and(|v| v.parse::<u64>().is_ok());
    let from = |value: &Option<String>, in_file: &Option<String>, var: &str| {
        let in_env = env(var).is_some_and(|v| !v.is_empty());
        (
            value
                .clone()
                .or_else(|| env(var).filter(|v| !v.is_empty()))
                .unwrap_or_else(|| "Not set".to_string()),
            source_of(in_env, in_file.is_some()),
        )
    };
    let (db_url, db_source) = from(&config.database_url, &file.database_url, "DATABASE_URL");
    let db_url = crate::utils::validation::redact_url(&db_url);
    let (queue, queue_source) = from(
        &config.default_queue,
        &file.default_queue,
        "HAMMERWORK_DEFAULT_QUEUE",
    );
    let (encryption_config, encryption_source) = from(
        &config.encryption_config,
        &file.encryption_config,
        "HAMMERWORK_ENCRYPTION_CONFIG",
    );
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
            source_of(
                number("HAMMERWORK_DEFAULT_LIMIT"),
                file.default_limit.is_some(),
            ),
        ),
        (
            "log_level",
            config.get_log_level().to_string(),
            source_of(text("HAMMERWORK_LOG_LEVEL"), file.log_level.is_some()),
        ),
        (
            "connection_pool_size",
            config.get_connection_pool_size().to_string(),
            source_of(
                number("HAMMERWORK_POOL_SIZE"),
                file.connection_pool_size.is_some(),
            ),
        ),
        (
            "connect_timeout_secs",
            config.get_connect_timeout_secs().to_string(),
            source_of(
                number("HAMMERWORK_CONNECT_TIMEOUT"),
                file.connect_timeout_secs.is_some(),
            ),
        ),
        ("encryption_config", encryption_config, encryption_source),
        (
            "encryption",
            encryption,
            "encryption_config + HAMMERWORK_ENCRYPTION_*",
        ),
    ]
}

async fn show_config(config: &Config, path: &Path) -> Result<()> {
    // The file alone, to tell file values from environment ones; a broken file shows
    // as all defaults (`main` already warned about it).
    let file = Config::file_values(path);
    println!("⚙️  Hammerwork Configuration");
    println!("═══════════════════════════");

    let mut table = comfy_table::Table::new();
    table.set_header(vec!["Setting", "Value", "Source"]);
    for (setting, value, source) in config_rows(config, &file, |var| std::env::var(var).ok()) {
        table.add_row(vec![setting, value.as_str(), source]);
    }

    println!("{}", table);

    println!("\n💡 Configuration priority: Environment Variables > Config File > Defaults");
    println!("📝 Use 'config set <key> <value>' to update configuration");

    Ok(())
}

/// Validate `value` for `key` and store it in `config`.
fn apply_key(config: &mut Config, key: &str, value: &str) -> Result<()> {
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

    Ok(())
}

/// `config set`: change one key of the config file and save it. The file is loaded on its
/// own, without the environment, so `DATABASE_URL` and the `HAMMERWORK_*` variables are
/// never copied into it; a file that exists but cannot be parsed is an error and is left
/// untouched rather than replaced by the defaults. The in-memory `config` gets the value
/// too.
async fn set_config_value(config: &mut Config, key: &str, value: &str, path: &Path) -> Result<()> {
    let mut file = Config::load_from_path(path).map_err(|e| {
        anyhow::anyhow!("{e}\nRefusing to overwrite it; fix or remove the file first")
    })?;
    apply_key(&mut file, key, value)?;
    file.save_to(path)?;
    apply_key(config, key, value)?;
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

    // Only the file is reset (and an unreadable one replaced, which is the point); the
    // environment is never written to it.
    Config::default().save_to(path)?;
    *config = Config::default();

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
        let rows = config_rows(&config, &config, |_| None);
        let shown = format!("{rows:?}");
        assert!(!shown.contains("hunter2-db"), "{shown}");
        assert!(
            shown.contains("postgres://app:***@db.internal/jobs"),
            "{shown}"
        );
        assert!(!format!("{config:?}").contains("hunter2-db"));

        // Also when the URL comes from the environment
        let rows = config_rows(&Config::default(), &Config::default(), |var| {
            (var == "DATABASE_URL").then(|| "mysql://root:hunter2-env@db/jobs".to_string())
        });
        let (_, url, source) = &rows[0];
        assert_eq!(
            (url.as_str(), *source),
            ("mysql://root:***@db/jobs", "Environment")
        );
    }

    #[test]
    fn show_tells_environment_file_and_default_sources_apart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        assert_eq!(Config::file_values(&path).default_limit, None, "no file");
        std::fs::write(
            &path,
            "default_queue = \"from_file\"\ndefault_limit = 10\nlog_level = \"debug\"\n",
        )
        .unwrap();
        let file = Config::file_values(&path);
        assert_eq!(file.connection_pool_size, None, "only the keys in the file");
        let env = |var: &str| match var {
            "DATABASE_URL" => Some("postgres://u:pw@h/db".to_string()),
            "HAMMERWORK_DEFAULT_LIMIT" => Some("99".to_string()),
            "HAMMERWORK_POOL_SIZE" => Some("not-a-number".to_string()),
            _ => None,
        };
        // the effective config is the file with the environment applied
        let effective = Config {
            database_url: Some("postgres://u:pw@h/db".into()),
            default_limit: Some(99),
            ..file.clone()
        };
        let rows = config_rows(&effective, &file, env);
        let source = |name: &str| rows.iter().find(|r| r.0 == name).unwrap().2;
        assert_eq!(source("database_url"), "Environment");
        assert_eq!(source("default_limit"), "Environment", "env beats the file");
        assert_eq!(source("default_queue"), "Config File");
        assert_eq!(source("log_level"), "Config File");
        assert_eq!(
            source("connection_pool_size"),
            "Default",
            "an unparseable variable is ignored"
        );
        assert_eq!(source("connect_timeout_secs"), "Default");
        assert_eq!(source("encryption_config"), "Default");
    }

    #[tokio::test]
    async fn set_and_reset_never_write_the_environment_into_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "default_limit = 12\n").unwrap();
        // what `main` hands to the command: file plus environment (a fake one, so the
        // real process environment is left alone)
        let path_text = path.to_string_lossy().into_owned();
        let mut config = Config::load_with(|k| match k {
            "HAMMERWORK_CONFIG" => Some(path_text.clone()),
            "DATABASE_URL" => Some("postgres://app:hunter2-env@db/jobs".to_string()),
            "HAMMERWORK_DEFAULT_QUEUE" => Some("env_queue".to_string()),
            _ => None,
        })
        .unwrap();
        assert_eq!(
            config.get_database_url().map(|u| u.contains("hunter2-env")),
            Some(true)
        );
        assert_eq!(config.get_default_queue(), Some("env_queue"));

        run(&["set", "log_level", "warn"], &mut config, &path)
            .await
            .unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            !text.contains("hunter2-env") && !text.contains("env_queue"),
            "{text}"
        );
        let saved = Config::load_from_path(&path).unwrap();
        assert_eq!(saved.get_log_level(), "warn");
        assert_eq!(saved.get_default_limit(), 12, "other file values are kept");
        assert_eq!(saved.database_url, None);

        run(&["reset", "--confirm"], &mut config, &path)
            .await
            .unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            !text.contains("hunter2-env") && !text.contains("env_queue"),
            "{text}"
        );

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[tokio::test]
    async fn set_refuses_to_overwrite_an_unparseable_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "default_limit = = broken").unwrap();
        let mut config = Config::default();
        let err = run(&["set", "default_queue", "q"], &mut config, &path)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("Invalid config file"), "{err}");
        assert!(err.contains("Refusing to overwrite"), "{err}");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "default_limit = = broken",
            "the broken file is untouched"
        );
        assert_eq!(config.get_default_queue(), None);
        // reset is the way out: it replaces the file with the defaults
        run(&["reset", "--confirm"], &mut config, &path)
            .await
            .unwrap();
        assert!(Config::load_from_path(&path).is_ok());
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
        let rows = config_rows(&config, &config, |_| None);
        let encryption = &rows.iter().find(|r| r.0 == "encryption").unwrap().1;
        assert!(encryption.starts_with("enabled"), "{encryption}");
        assert!(encryption.contains("env://APP_KEY"), "{encryption}");
        assert!(encryption.contains("payments"), "{encryption}");
        let rows = config_rows(&Config::default(), &Config::default(), |_| None);
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
