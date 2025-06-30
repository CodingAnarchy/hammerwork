//! # Cargo Hammerwork CLI
//!
//! A comprehensive command-line interface for managing Hammerwork job queues.
//!
//! This crate provides the `cargo hammerwork` CLI tool for database migrations,
//! job management, worker control, queue operations, and monitoring of Hammerwork
//! job processing systems.
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
//! # Start a worker (requires configuration file)
//! cargo hammerwork worker start --database-url postgresql://localhost/mydb \
//!   --queue default --handler ./my_handler
//!
//! # Show worker status
//! cargo hammerwork worker status --database-url postgresql://localhost/mydb
//!
//! # Stop all workers gracefully
//! cargo hammerwork worker stop --database-url postgresql://localhost/mydb --all
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
//! # 5. Process jobs (in another terminal)
//! cargo hammerwork worker start --queue email
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

pub mod commands;
pub mod config;
pub mod utils;

pub use commands::*;
pub use config::*;
pub use utils::*;
