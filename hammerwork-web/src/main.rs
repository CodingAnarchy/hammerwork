//! Main binary entry point for the Hammerwork Web Dashboard.

use anyhow::{Context, Result, anyhow};
use clap::{Arg, ArgMatches, Command};
use hammerwork_web::{DashboardConfig, WebDashboard};
use std::path::PathBuf;
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

/// The command line of `hammerwork-web`.
///
/// `--bind`, `--port` and `--static-dir` have no clap defaults on purpose: a default would be
/// indistinguishable from an explicit value and would override the configuration file. The
/// defaults come from [`DashboardConfig`].
fn cli() -> Command {
    Command::new("hammerwork-web")
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
                .help("Database connection URL (default: $DATABASE_URL)"),
        )
        .arg(
            Arg::new("bind")
                .short('b')
                .long("bind")
                .value_name("ADDRESS")
                .help("Server bind address [default: 127.0.0.1]"),
        )
        .arg(
            Arg::new("port")
                .short('p')
                .long("port")
                .value_name("PORT")
                .value_parser(clap::value_parser!(u16))
                .help("Server port [default: 8080]"),
        )
        .arg(
            Arg::new("static-dir")
                .long("static-dir")
                .value_name("DIR")
                .help("Directory containing static assets [default: ./assets]"),
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
                .help("Username for authentication (requires --auth) [default: admin]"),
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
        .arg(
            Arg::new("no-auth")
                .long("no-auth")
                .help("Run without authentication: anyone who can reach the dashboard can manage jobs")
                .action(clap::ArgAction::SetTrue)
                .conflicts_with("auth"),
        )
}

/// The configuration the server runs with, and notes for the operator.
#[derive(Debug)]
struct Resolved {
    config: DashboardConfig,
    /// Things worth a warning in the log.
    warnings: Vec<String>,
}

/// Combine the configuration file (if any), the environment and the command line.
///
/// Precedence, highest first: command-line arguments given explicitly, the configuration file,
/// then built-in defaults. `DATABASE_URL` stands in for the database URL only when neither
/// `--database-url` nor `--config` is used.
fn resolve_config(matches: &ArgMatches, env_database_url: Option<&str>) -> Result<Resolved> {
    let mut warnings = Vec::new();

    let mut config = if let Some(config_file) = matches.get_one::<String>("config") {
        info!("Loading configuration from: {}", config_file);
        DashboardConfig::from_file(config_file)
            .with_context(|| format!("Cannot load configuration file {config_file}"))?
    } else {
        let mut config = DashboardConfig::new();
        if let Some(url) = env_database_url.filter(|u| !u.is_empty()) {
            config.database_url = url.to_string();
        }
        config
    };

    if let Some(db_url) = matches.get_one::<String>("database-url") {
        config.database_url = db_url.clone();
    }
    if config.database_url.is_empty() {
        return Err(anyhow!(
            "Database URL is required. Use --database-url or set DATABASE_URL environment variable."
        ));
    }

    if let Some(bind) = matches.get_one::<String>("bind") {
        config.bind_address = bind.clone();
    }
    if let Some(port) = matches.get_one::<u16>("port") {
        config.port = *port;
    }
    if let Some(static_dir) = matches.get_one::<String>("static-dir") {
        config.static_dir = PathBuf::from(static_dir);
    }
    if matches.get_flag("cors") {
        config.enable_cors = true;
    }

    // Handle authentication
    if matches.get_flag("auth") {
        config.auth.enabled = true;

        if let Some(username) = matches.get_one::<String>("username") {
            config.auth.username = username.clone();
        }

        // Handle password or password file
        let password_hash = if let Some(password_file) = matches.get_one::<String>("password-file")
        {
            std::fs::read_to_string(password_file)
                .with_context(|| format!("Cannot read password file {password_file}"))?
                .trim()
                .to_string()
        } else if let Some(password) = matches.get_one::<String>("password") {
            hash_password(password)?
        } else if !config.auth.password_hash.is_empty() {
            // The configuration file already holds a hash.
            config.auth.password_hash.clone()
        } else {
            return Err(anyhow!(
                "Authentication enabled but no password provided. Use --password or --password-file"
            ));
        };
        if password_hash.is_empty() {
            return Err(anyhow!("The password hash must not be empty"));
        }

        config.auth.password_hash = password_hash;
    } else if matches.get_one::<String>("username").is_some()
        || matches.get_one::<String>("password").is_some()
        || matches.get_one::<String>("password-file").is_some()
    {
        warnings.push(
            "--username, --password and --password-file have no effect without --auth".into(),
        );
    }

    if matches.get_flag("no-auth") {
        config.auth.enabled = false;
    }

    // The built-in default enables authentication but cannot know a password. Fail closed:
    // running open must be an explicit choice.
    if config.auth.enabled && config.auth.password_hash.is_empty() {
        return Err(anyhow!(
            "Authentication is enabled but no password is configured. Use --auth with \
             --password or --password-file, set auth.password_hash in the config file, or pass \
             --no-auth to run the dashboard without authentication"
        ));
    }
    if !config.auth.enabled {
        warnings.push(
            "Authentication is disabled: anyone who can reach the dashboard can manage jobs".into(),
        );
    }

    Ok(Resolved { config, warnings })
}

/// The stored form of a command-line password.
fn hash_password(password: &str) -> Result<String> {
    #[cfg(feature = "auth")]
    {
        Ok(bcrypt::hash(password, bcrypt::DEFAULT_COST)?)
    }
    #[cfg(not(feature = "auth"))]
    {
        let _ = password;
        Err(anyhow!(
            "Authentication feature not enabled. Rebuild with --features auth"
        ))
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::from_default_env().add_directive("hammerwork_web=info".parse()?),
        )
        .init();

    let matches = cli().get_matches();

    let env_database_url = std::env::var("DATABASE_URL").ok();
    let Resolved { config, warnings } = match resolve_config(&matches, env_database_url.as_deref())
    {
        Ok(resolved) => resolved,
        Err(e) => {
            error!("{:#}", e);
            std::process::exit(1);
        }
    };
    for warning in &warnings {
        warn!("{}", warning);
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

    fn resolve(args: &[&str], env: Option<&str>) -> Result<Resolved> {
        let mut argv = vec!["hammerwork-web"];
        argv.extend_from_slice(args);
        let matches = cli()
            .try_get_matches_from(argv)
            .map_err(|e| anyhow!("{e}"))?;
        resolve_config(&matches, env)
    }

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
        assert_eq!(mask_database_url("weird@thing"), "***");
    }

    #[test]
    fn the_command_line_definition_is_consistent() {
        cli().debug_assert();
    }

    #[test]
    fn defaults_come_from_the_dashboard_config() {
        let resolved = resolve(&["--no-auth"], None).unwrap();
        let defaults = DashboardConfig::new();
        assert_eq!(resolved.config.bind_address, defaults.bind_address);
        assert_eq!(resolved.config.port, defaults.port);
        assert_eq!(resolved.config.static_dir, defaults.static_dir);
        assert_eq!(resolved.config.database_url, defaults.database_url);
        assert!(!resolved.config.enable_cors);
    }

    #[test]
    fn explicit_arguments_override_the_defaults() {
        let resolved = resolve(
            &[
                "--no-auth",
                "-d",
                "mysql://root@db/app",
                "-b",
                "0.0.0.0",
                "-p",
                "9191",
                "--static-dir",
                "/srv/www",
                "--cors",
            ],
            None,
        )
        .unwrap();
        let config = resolved.config;
        assert_eq!(config.database_url, "mysql://root@db/app");
        assert_eq!(config.bind_addr(), "0.0.0.0:9191");
        assert_eq!(config.static_dir, PathBuf::from("/srv/www"));
        assert!(config.enable_cors);
    }

    #[test]
    fn invalid_ports_are_rejected() {
        for port in ["abc", "70000", "-1"] {
            assert!(resolve(&["--no-auth", "-p", port], None).is_err(), "{port}");
        }
    }

    #[test]
    fn the_environment_supplies_the_database_url_unless_a_flag_or_file_does() {
        let from_env = resolve(&["--no-auth"], Some("postgres://env/db")).unwrap();
        assert_eq!(from_env.config.database_url, "postgres://env/db");

        let flag_wins = resolve(
            &["--no-auth", "-d", "mysql://flag/db"],
            Some("postgres://env/db"),
        )
        .unwrap();
        assert_eq!(flag_wins.config.database_url, "mysql://flag/db");

        // An empty variable is as good as unset.
        let empty = resolve(&["--no-auth"], Some("")).unwrap();
        assert_eq!(
            empty.config.database_url,
            DashboardConfig::new().database_url
        );
    }

    fn write_config(dir: &tempfile::TempDir, content: &str) -> String {
        let path = dir.path().join("dashboard.toml");
        std::fs::write(&path, content).unwrap();
        path.to_str().unwrap().to_string()
    }

    #[test]
    fn the_config_file_is_not_overridden_by_unset_arguments() {
        let dir = tempfile::tempdir().unwrap();
        let mut file_config = DashboardConfig::new()
            .with_bind_address("10.1.2.3", 9999)
            .with_database_url("mysql://file/db")
            .with_static_dir(PathBuf::from("/from/file"))
            .with_cors(true);
        file_config.auth.enabled = false;
        let path = dir.path().join("dashboard.toml");
        file_config.save_to_file(path.to_str().unwrap()).unwrap();
        let path = path.to_str().unwrap();

        // The environment does not replace what the file says.
        let resolved = resolve(&["-c", path], Some("postgres://env/db")).unwrap();
        let config = resolved.config;
        assert_eq!(config.bind_addr(), "10.1.2.3:9999");
        assert_eq!(config.database_url, "mysql://file/db");
        assert_eq!(config.static_dir, PathBuf::from("/from/file"));
        assert!(config.enable_cors, "the file's CORS setting survives");

        // Explicit arguments win over the file.
        let resolved = resolve(&["-c", path, "-p", "1234", "-b", "127.0.0.9"], None).unwrap();
        assert_eq!(resolved.config.bind_addr(), "127.0.0.9:1234");
        assert_eq!(resolved.config.database_url, "mysql://file/db");
    }

    #[test]
    fn bad_config_files_are_reported() {
        let dir = tempfile::tempdir().unwrap();
        let err = resolve(&["-c", "/no/such/file.toml"], None).unwrap_err();
        assert!(format!("{err:#}").contains("/no/such/file.toml"), "{err:#}");

        let broken = write_config(&dir, "port = \"not a number\"");
        assert!(resolve(&["-c", &broken], None).is_err());

        let empty_url = write_config(
            &dir,
            &toml::to_string(&DashboardConfig {
                database_url: String::new(),
                ..DashboardConfig::new()
            })
            .unwrap(),
        );
        let err = resolve(&["-c", &empty_url], None).unwrap_err();
        assert!(
            err.to_string().contains("Database URL is required"),
            "{err}"
        );
    }

    #[test]
    fn authentication_needs_a_password_or_an_explicit_opt_out() {
        // By default there is no password: refuse to start rather than run open.
        let err = resolve(&[], None).unwrap_err();
        assert!(err.to_string().contains("--no-auth"), "{err}");

        // --no-auth runs open, with a warning.
        let open = resolve(&["--no-auth"], None).unwrap();
        assert!(!open.config.auth.enabled);
        assert!(
            open.warnings
                .iter()
                .any(|w| w.contains("Authentication is disabled"))
        );
        let err = resolve(&["--no-auth", "--auth"], None).unwrap_err();
        assert!(err.to_string().contains("cannot be used with"), "{err}");

        // --auth without any password is an error.
        let err = resolve(&["--auth"], None).unwrap_err();
        assert!(err.to_string().contains("no password provided"), "{err}");

        // A password file holds the hash.
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("hash.txt");
        std::fs::write(&file, "  $2b$12$abcdefghijklmnopqrstuv  \n").unwrap();
        let resolved = resolve(
            &[
                "--auth",
                "--username",
                "ops",
                "--password-file",
                file.to_str().unwrap(),
            ],
            None,
        )
        .unwrap();
        assert!(resolved.config.auth.enabled);
        assert_eq!(resolved.config.auth.username, "ops");
        assert_eq!(
            resolved.config.auth.password_hash,
            "$2b$12$abcdefghijklmnopqrstuv"
        );
        assert!(resolved.warnings.is_empty());

        let err = resolve(&["--auth", "--password-file", "/no/such/hash"], None).unwrap_err();
        assert!(format!("{err:#}").contains("/no/such/hash"));
        let blank = dir.path().join("blank.txt");
        std::fs::write(&blank, "\n").unwrap();
        let err = resolve(
            &["--auth", "--password-file", blank.to_str().unwrap()],
            None,
        )
        .unwrap_err();
        assert!(err.to_string().contains("must not be empty"), "{err}");

        // Credentials without --auth are ignored, and the user is told.
        let ignored = resolve(&["--no-auth", "--username", "x", "--password", "y"], None).unwrap();
        assert!(!ignored.config.auth.enabled);
        assert!(
            ignored
                .warnings
                .iter()
                .any(|w| w.contains("no effect without --auth"))
        );
    }

    #[test]
    fn a_hash_in_the_config_file_is_used_by_auth() {
        let dir = tempfile::tempdir().unwrap();
        let mut file_config = DashboardConfig::new();
        file_config.auth.enabled = true;
        file_config.auth.password_hash = "$2b$12$fromfile".to_string();
        let path = dir.path().join("dashboard.toml");
        file_config.save_to_file(path.to_str().unwrap()).unwrap();
        let path = path.to_str().unwrap();

        // Enabled in the file with a hash: stays enabled without --auth.
        let resolved = resolve(&["-c", path], None).unwrap();
        assert!(resolved.config.auth.enabled);
        assert_eq!(resolved.config.auth.password_hash, "$2b$12$fromfile");
        // And --auth reuses it instead of demanding another password.
        let resolved = resolve(&["-c", path, "--auth"], None).unwrap();
        assert_eq!(resolved.config.auth.password_hash, "$2b$12$fromfile");
    }

    #[cfg(feature = "auth")]
    #[test]
    fn a_command_line_password_is_stored_as_a_bcrypt_hash() {
        let resolved = resolve(&["--auth", "--password", "hunter2"], None).unwrap();
        let hash = &resolved.config.auth.password_hash;
        assert!(hash.starts_with("$2"));
        assert!(bcrypt::verify("hunter2", hash).unwrap());
    }

    #[cfg(not(feature = "auth"))]
    #[test]
    fn a_command_line_password_needs_the_auth_feature() {
        let err = resolve(&["--auth", "--password", "hunter2"], None).unwrap_err();
        assert!(err.to_string().contains("--features auth"), "{err}");
    }
}
