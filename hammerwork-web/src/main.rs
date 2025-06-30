//! Main binary entry point for the Hammerwork Web Dashboard.

use anyhow::Result;
use clap::{Arg, Command};
use hammerwork_web::{DashboardConfig, WebDashboard};
use std::path::PathBuf;
use tracing::{error, info};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::from_default_env().add_directive("hammerwork_web=info".parse()?),
        )
        .init();

    let matches = Command::new("hammerwork-web")
        .version(env!("CARGO_PKG_VERSION"))
        .author("CodingAnarchy <noreply@codinganarchy.com>")
        .about("Web-based admin dashboard for Hammerwork job queues")
        .arg(
            Arg::new("config")
                .short('c')
                .long("config")
                .value_name("FILE")
                .help("Path to configuration file"),
        )
        .arg(
            Arg::new("database-url")
                .short('d')
                .long("database-url")
                .value_name("URL")
                .help("Database connection URL"),
        )
        .arg(
            Arg::new("bind")
                .short('b')
                .long("bind")
                .value_name("ADDRESS")
                .help("Server bind address")
                .default_value("127.0.0.1"),
        )
        .arg(
            Arg::new("port")
                .short('p')
                .long("port")
                .value_name("PORT")
                .help("Server port")
                .default_value("8080"),
        )
        .arg(
            Arg::new("static-dir")
                .long("static-dir")
                .value_name("DIR")
                .help("Directory containing static assets")
                .default_value("./assets"),
        )
        .arg(
            Arg::new("cors")
                .long("cors")
                .help("Enable CORS support")
                .action(clap::ArgAction::SetTrue),
        )
        .arg(
            Arg::new("auth")
                .long("auth")
                .help("Enable basic authentication")
                .action(clap::ArgAction::SetTrue),
        )
        .arg(
            Arg::new("username")
                .long("username")
                .value_name("USER")
                .help("Username for authentication (requires --auth)")
                .default_value("admin"),
        )
        .arg(
            Arg::new("password")
                .long("password")
                .value_name("PASS")
                .help("Password for authentication (requires --auth)"),
        )
        .arg(
            Arg::new("password-file")
                .long("password-file")
                .value_name("FILE")
                .help("File containing password hash (requires --auth)"),
        )
        .get_matches();

    // Load configuration
    let mut config = if let Some(config_file) = matches.get_one::<String>("config") {
        info!("Loading configuration from: {}", config_file);
        DashboardConfig::from_file(config_file)?
    } else {
        DashboardConfig::new()
    };

    // Override with command line arguments
    if let Some(db_url) = matches.get_one::<String>("database-url") {
        config.database_url = db_url.clone();
    }

    if config.database_url.is_empty() {
        error!(
            "Database URL is required. Use --database-url or set DATABASE_URL environment variable."
        );
        std::process::exit(1);
    }

    let bind_address = matches.get_one::<String>("bind").unwrap();
    let port: u16 = matches.get_one::<String>("port").unwrap().parse()?;
    config = config.with_bind_address(bind_address, port);

    if let Some(static_dir) = matches.get_one::<String>("static-dir") {
        config = config.with_static_dir(PathBuf::from(static_dir));
    }

    config.enable_cors = matches.get_flag("cors");

    // Handle authentication
    if matches.get_flag("auth") {
        config.auth.enabled = true;

        if let Some(username) = matches.get_one::<String>("username") {
            config.auth.username = username.clone();
        }

        // Handle password or password file
        let password_hash = if let Some(password_file) = matches.get_one::<String>("password-file")
        {
            std::fs::read_to_string(password_file)?.trim().to_string()
        } else if let Some(password) = matches.get_one::<String>("password") {
            // For simplicity, we'll use bcrypt if available, otherwise plain text (not recommended)
            #[cfg(feature = "auth")]
            {
                bcrypt::hash(password, bcrypt::DEFAULT_COST)?
            }
            #[cfg(not(feature = "auth"))]
            {
                let _ = password; // Silence unused variable warning
                error!("Authentication feature not enabled. Rebuild with --features auth");
                std::process::exit(1);
            }
        } else {
            error!(
                "Authentication enabled but no password provided. Use --password or --password-file"
            );
            std::process::exit(1);
        };

        config.auth.password_hash = password_hash;
    }

    info!("Starting Hammerwork Web Dashboard");
    info!("Server: http://{}", config.bind_addr());
    info!("Database: {}", mask_database_url(&config.database_url));
    info!("Static assets: {}", config.static_dir.display());

    if config.auth.enabled {
        info!("Authentication: enabled (user: {})", config.auth.username);
    } else {
        info!("Authentication: disabled");
    }

    // Create and start the web dashboard
    let dashboard = WebDashboard::new(config).await?;

    // Handle graceful shutdown
    let shutdown_signal = async {
        tokio::signal::ctrl_c()
            .await
            .expect("Failed to install CTRL+C signal handler");
        info!("Shutdown signal received");
    };

    tokio::select! {
        result = dashboard.start() => {
            if let Err(e) = result {
                error!("Dashboard error: {}", e);
                std::process::exit(1);
            }
        }
        _ = shutdown_signal => {
            info!("Shutting down gracefully...");
        }
    }

    Ok(())
}

/// Mask sensitive parts of database URL for logging
fn mask_database_url(url: &str) -> String {
    if let Some(at_pos) = url.rfind('@') {
        if let Some(scheme_pos) = url.find("://") {
            let scheme = &url[..scheme_pos + 3];
            let host_and_path = &url[at_pos..];
            format!("{}***{}", scheme, host_and_path)
        } else {
            "***".to_string()
        }
    } else {
        url.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mask_database_url() {
        assert_eq!(
            mask_database_url("postgresql://user:pass@localhost/db"),
            "postgresql://***@localhost/db"
        );
        assert_eq!(
            mask_database_url("mysql://root:secret@127.0.0.1:3306/hammerwork"),
            "mysql://***@127.0.0.1:3306/hammerwork"
        );
        assert_eq!(
            mask_database_url("postgresql://localhost/db"),
            "postgresql://localhost/db"
        );
    }
}
