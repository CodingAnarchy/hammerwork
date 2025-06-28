use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::env;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub database_url: Option<String>,
    pub default_queue: Option<String>,
    pub default_limit: Option<u32>,
    pub log_level: Option<String>,
    pub connection_pool_size: Option<u32>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            database_url: None,
            default_queue: None,
            default_limit: Some(50),
            log_level: Some("info".to_string()),
            connection_pool_size: Some(5),
        }
    }
}

impl Config {
    pub fn load() -> Result<Self> {
        let mut config = Self::default();
        
        // Try to load from config file
        if let Ok(config_content) = Self::load_from_file() {
            config = config_content;
        }
        
        // Override with environment variables
        if let Ok(db_url) = env::var("DATABASE_URL") {
            config.database_url = Some(db_url);
        }
        
        if let Ok(queue) = env::var("HAMMERWORK_DEFAULT_QUEUE") {
            config.default_queue = Some(queue);
        }
        
        if let Ok(limit) = env::var("HAMMERWORK_DEFAULT_LIMIT") {
            if let Ok(limit_num) = limit.parse() {
                config.default_limit = Some(limit_num);
            }
        }
        
        if let Ok(log_level) = env::var("HAMMERWORK_LOG_LEVEL") {
            config.log_level = Some(log_level);
        }
        
        if let Ok(pool_size) = env::var("HAMMERWORK_POOL_SIZE") {
            if let Ok(size_num) = pool_size.parse() {
                config.connection_pool_size = Some(size_num);
            }
        }
        
        Ok(config)
    }
    
    fn load_from_file() -> Result<Self> {
        let config_path = Self::config_file_path()?;
        let content = fs::read_to_string(config_path)?;
        let config: Config = toml::from_str(&content)?;
        Ok(config)
    }
    
    pub fn save(&self) -> Result<()> {
        let config_path = Self::config_file_path()?;
        
        // Create parent directory if it doesn't exist
        if let Some(parent) = config_path.parent() {
            fs::create_dir_all(parent)?;
        }
        
        let content = toml::to_string_pretty(self)?;
        fs::write(config_path, content)?;
        Ok(())
    }
    
    fn config_file_path() -> Result<PathBuf> {
        let mut path = dirs::config_dir()
            .or_else(dirs::home_dir)
            .ok_or_else(|| anyhow::anyhow!("Cannot find config directory"))?;
        
        path.push("hammerwork");
        path.push("config.toml");
        Ok(path)
    }
    
    pub fn get_database_url(&self) -> Option<&str> {
        self.database_url.as_deref()
    }
    
    pub fn get_default_queue(&self) -> Option<&str> {
        self.default_queue.as_deref()
    }
    
    pub fn get_default_limit(&self) -> u32 {
        self.default_limit.unwrap_or(50)
    }
    
    pub fn get_log_level(&self) -> &str {
        self.log_level.as_deref().unwrap_or("info")
    }
    
    pub fn get_connection_pool_size(&self) -> u32 {
        self.connection_pool_size.unwrap_or(5)
    }
}