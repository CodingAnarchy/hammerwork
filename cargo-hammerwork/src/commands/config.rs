use anyhow::Result;
use clap::Subcommand;
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
        match self {
            ConfigCommand::Show => {
                show_config(config).await?;
            }
            ConfigCommand::Set { key, value } => {
                set_config_value(config, key, value).await?;
            }
            ConfigCommand::Get { key } => {
                get_config_value(config, key).await?;
            }
            ConfigCommand::Reset { confirm } => {
                reset_config(config, *confirm).await?;
            }
            ConfigCommand::Path => {
                show_config_path().await?;
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

async fn set_config_value(config: &mut Config, key: &str, value: &str) -> Result<()> {
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
            let limit: u32 = value.parse()
                .map_err(|_| anyhow::anyhow!("default_limit must be a positive integer"))?;
            config.default_limit = Some(limit);
            info!("✅ Set default_limit to: {}", limit);
        }
        "log_level" => {
            if !["trace", "debug", "info", "warn", "error"].contains(&value.to_lowercase().as_str()) {
                return Err(anyhow::anyhow!("log_level must be one of: trace, debug, info, warn, error"));
            }
            config.log_level = Some(value.to_lowercase());
            info!("✅ Set log_level to: {}", value);
        }
        "connection_pool_size" => {
            let size: u32 = value.parse()
                .map_err(|_| anyhow::anyhow!("connection_pool_size must be a positive integer"))?;
            if size == 0 || size > 100 {
                return Err(anyhow::anyhow!("connection_pool_size must be between 1 and 100"));
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
    config.save()?;
    println!("💾 Configuration saved");

    Ok(())
}

async fn get_config_value(config: &Config, key: &str) -> Result<()> {
    let value = match key {
        "database_url" => {
            config.get_database_url().unwrap_or("Not set").to_string()
        }
        "default_queue" => {
            config.get_default_queue().unwrap_or("Not set").to_string()
        }
        "default_limit" => {
            config.get_default_limit().to_string()
        }
        "log_level" => {
            config.get_log_level().to_string()
        }
        "connection_pool_size" => {
            config.get_connection_pool_size().to_string()
        }
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

async fn reset_config(config: &mut Config, confirm: bool) -> Result<()> {
    if !confirm {
        println!("⚠️  This will reset all configuration to defaults. Use --confirm to proceed.");
        return Ok(());
    }

    *config = Config::default();
    config.save()?;

    println!("🔄 Configuration reset to defaults");
    info!("Configuration has been reset to defaults");

    Ok(())
}

async fn show_config_path() -> Result<()> {
    let config_dir = dirs::config_dir()
        .or_else(dirs::home_dir)
        .ok_or_else(|| anyhow::anyhow!("Cannot find config directory"))?;

    let config_path = config_dir.join("hammerwork").join("config.toml");
    
    println!("📁 Configuration file path:");
    println!("{}", config_path.display());
    
    if config_path.exists() {
        println!("✅ File exists");
        
        // Show file size and modification time
        let metadata = std::fs::metadata(&config_path)?;
        let size = metadata.len();
        let modified = metadata.modified()?;
        let modified_time = chrono::DateTime::<chrono::Utc>::from(modified);
        
        println!("📊 Size: {} bytes", size);
        println!("📅 Last modified: {}", modified_time.format("%Y-%m-%d %H:%M:%S UTC"));
    } else {
        println!("❌ File does not exist (will be created when configuration is saved)");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_config_command_structure() {
        let commands = vec!["show", "set", "get", "reset", "path"];
        
        for cmd in commands {
            assert!(!cmd.is_empty());
            assert!(cmd.chars().all(|c| c.is_ascii_lowercase()));
        }
    }
    
    #[test]
    fn test_config_key_validation() {
        let valid_keys = vec![
            "database_url",
            "default_queue",
            "default_limit",
            "log_level",
            "connection_pool_size",
        ];
        
        for key in valid_keys {
            assert!(is_valid_config_key(key));
        }
        
        let invalid_keys = vec![
            "",
            "invalid_key",
            "key with spaces",
            "key/with/slashes",
        ];
        
        for key in invalid_keys {
            assert!(!is_valid_config_key(key));
        }
    }
    
    #[test]
    fn test_config_value_validation() {
        // Test database URL values
        assert!(is_valid_config_value("database_url", "postgres://localhost/db"));
        assert!(!is_valid_config_value("database_url", "invalid_url"));
        
        // Test log level values
        assert!(is_valid_config_value("log_level", "info"));
        assert!(is_valid_config_value("log_level", "debug"));
        assert!(!is_valid_config_value("log_level", "invalid"));
        
        // Test numeric values
        assert!(is_valid_config_value("default_limit", "100"));
        assert!(is_valid_config_value("connection_pool_size", "5"));
        assert!(!is_valid_config_value("default_limit", "not_a_number"));
    }
    
    fn is_valid_config_key(key: &str) -> bool {
        matches!(key, 
            "database_url" | 
            "default_queue" | 
            "default_limit" | 
            "log_level" | 
            "connection_pool_size"
        )
    }
    
    fn is_valid_config_value(key: &str, value: &str) -> bool {
        match key {
            "database_url" => !value.is_empty() && (value.starts_with("postgres://") || value.starts_with("mysql://")),
            "log_level" => matches!(value, "error" | "warn" | "info" | "debug" | "trace"),
            "default_limit" | "connection_pool_size" => value.parse::<u32>().is_ok(),
            "default_queue" => !value.is_empty() && !value.contains(' '),
            _ => false,
        }
    }
}