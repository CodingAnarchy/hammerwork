# cargo-hammerwork

A comprehensive cargo subcommand for managing Hammerwork job queues with advanced tooling and monitoring capabilities.

## Installation

Install from the workspace:

```bash
# Build and install locally
cargo install --path ./cargo-hammerwork

# Or build for development
cargo build -p cargo-hammerwork
```

## Overview

cargo-hammerwork provides a modern, modular CLI for managing Hammerwork-based applications with support for:

- 🗄️  **Database Migration Management** - Setup and maintain database schemas
- ⚙️  **Configuration Management** - Centralized config with file and environment support  
- 🧩 **Modular Architecture** - Clean separation of concerns with dedicated command modules
- 🐘 **Multi-Database Support** - PostgreSQL and MySQL compatibility
- 📊 **Advanced Monitoring** - Real-time dashboards and health checks
- 👷 **Worker Management** - Control job processing workers and pools
- 🎯 **Queue Operations** - Comprehensive queue management and statistics
- 📋 **Job Management** - Full job lifecycle control and management
- 📦 **Batch Operations** - Bulk job processing and management
- ⏰ **Cron Management** - Recurring job scheduling and management
- 🔧 **Database Maintenance** - Cleanup, optimization, and health checks
- 🔄 **Workflow Management** - Job dependencies and complex pipelines
- 💾 **Backup & Restore** - Database backup and recovery operations

## Quick Start

### 1. Database Setup

```bash
# Run migrations to set up the database schema
cargo hammerwork migration run --database-url postgres://localhost/mydb

# Check migration status
cargo hammerwork migration status --database-url postgres://localhost/mydb
```

### 2. Configuration

```bash
# Set your default database URL
cargo hammerwork config set database_url postgres://localhost/mydb

# View current configuration
cargo hammerwork config show

# Set other defaults
cargo hammerwork config set default_queue emails
cargo hammerwork config set log_level debug
```

## Command Structure

The CLI is organized into logical command groups:

### Migration Commands

```bash
# Database migration operations
cargo hammerwork migration run [--database-url URL] [--drop]
cargo hammerwork migration status [--database-url URL]
```

### Configuration Commands

```bash
# Configuration management
cargo hammerwork config show                    # View all settings
cargo hammerwork config set <key> <value>       # Set a configuration value  
cargo hammerwork config get <key>               # Get a specific value
cargo hammerwork config reset --confirm         # Reset to defaults
cargo hammerwork config path                    # Show config file location
```

### Job Management Commands

```bash
# Job lifecycle management
cargo hammerwork job list [--queue QUEUE] [--status STATUS] [--limit N]
cargo hammerwork job show <JOB_ID>              # Detailed job information
cargo hammerwork job enqueue --queue QUEUE --payload JSON [OPTIONS]
cargo hammerwork job retry <JOB_ID>             # Retry a failed job
cargo hammerwork job cancel <JOB_ID>            # Cancel a pending job
cargo hammerwork job delete <JOB_ID>            # Remove a job
```

### Worker Management Commands

```bash
# Worker control and monitoring
cargo hammerwork worker start [--queue QUEUE] [--workers N]
cargo hammerwork worker stop [--queue QUEUE]    # Graceful shutdown
cargo hammerwork worker status [--queue QUEUE]  # Worker pool status
cargo hammerwork worker restart [--queue QUEUE] # Restart workers
```

### Queue Management Commands

```bash
# Queue operations and statistics
cargo hammerwork queue list                     # List all queues
cargo hammerwork queue stats [--queue QUEUE]    # Queue statistics
cargo hammerwork queue clear --queue QUEUE      # Clear all jobs
cargo hammerwork queue pause --queue QUEUE      # Pause processing
cargo hammerwork queue resume --queue QUEUE     # Resume processing
```

### Monitoring Commands

```bash
# Real-time monitoring and health checks
cargo hammerwork monitor dashboard              # Live dashboard
cargo hammerwork monitor health [--format json] # System health check
cargo hammerwork monitor metrics [--period 1h]  # Performance metrics
cargo hammerwork monitor logs [--tail]          # Log streaming
```

### Batch Operation Commands

```bash
# Bulk job operations
cargo hammerwork batch create --jobs FILE       # Create job batch
cargo hammerwork batch status <BATCH_ID>        # Batch progress
cargo hammerwork batch retry <BATCH_ID>         # Retry failed jobs
cargo hammerwork batch cancel <BATCH_ID>        # Cancel batch
```

### Cron Management Commands

```bash
# Recurring job scheduling
cargo hammerwork cron list                      # List cron jobs
cargo hammerwork cron add --schedule "0 */6 * * *" --queue QUEUE --payload JSON
cargo hammerwork cron remove <JOB_ID>           # Remove cron job
cargo hammerwork cron enable <JOB_ID>           # Enable scheduling
cargo hammerwork cron disable <JOB_ID>          # Disable scheduling
```

### Maintenance Commands

```bash
# Database maintenance and optimization
cargo hammerwork maintenance cleanup            # Remove old completed jobs
cargo hammerwork maintenance vacuum             # Optimize database
cargo hammerwork maintenance analyze            # Update table statistics
cargo hammerwork maintenance health             # Database health check
```

### Workflow Commands

```bash
# Job dependencies and complex pipelines
cargo hammerwork workflow create --file WORKFLOW.json
cargo hammerwork workflow list                  # List active workflows
cargo hammerwork workflow status <WORKFLOW_ID>  # Workflow progress
cargo hammerwork workflow cancel <WORKFLOW_ID>  # Cancel workflow
```

### Backup Commands

```bash
# Database backup and recovery
cargo hammerwork backup create --output FILE    # Create backup
cargo hammerwork backup restore --input FILE    # Restore from backup
cargo hammerwork backup list                    # List available backups
cargo hammerwork backup verify --input FILE     # Verify backup integrity
```

## Architecture & Design

### Modular Structure

```
cargo-hammerwork/
├── src/
│   ├── commands/           # Command implementations
│   │   ├── migration.rs    # Database migration operations
│   │   ├── config.rs       # Configuration management
│   │   ├── job.rs         # Job management (framework)
│   │   ├── queue.rs       # Queue operations (framework)
│   │   ├── worker.rs      # Worker control (framework)
│   │   └── monitor.rs     # Monitoring & observability (framework)
│   ├── config/            # Configuration system
│   │   └── mod.rs         # Config loading and management
│   ├── utils/             # Shared utilities
│   │   ├── database.rs    # Database connection handling
│   │   ├── display.rs     # Table formatting and display
│   │   └── validation.rs  # Input validation
│   └── main.rs           # CLI entry point
```

### Command Feature Highlights

#### Job Management
- List jobs with advanced filtering (queue, status, priority, time-based)
- Job enqueueing with priority, delays, timeouts, and retry configuration
- Bulk operations (retry, cancel, purge) with safety confirmations
- Detailed job inspection with full lifecycle tracking

#### Queue Management  
- Queue listing with comprehensive statistics
- Queue operations (clear, pause, resume)
- Health monitoring with configurable thresholds
- Detailed vs. summary statistics views

#### Worker Management
- Worker lifecycle control (start, stop, status)
- Configurable worker pools with priority handling
- Real-time worker monitoring and metrics
- Graceful shutdown and resource management

#### Monitoring & Observability
- Real-time dashboard with auto-refresh
- System health checks with JSON/table output
- Performance metrics with configurable time periods
- Log tailing and filtering capabilities

#### Advanced Features
- **Batch Operations**: Bulk job processing with progress tracking
- **Cron Scheduling**: Time-based recurring job management
- **Database Maintenance**: Cleanup, optimization, and health monitoring
- **Workflow Management**: Complex job dependency orchestration
- **Backup & Restore**: Complete database backup and recovery

### Configuration System

Supports multiple configuration sources with proper precedence:

1. **Environment Variables** (highest priority)
2. **Configuration File** (`~/.config/hammerwork/config.toml`)
3. **Default Values** (lowest priority)

Example configuration file:

```toml
database_url = "postgres://localhost/hammerwork"
default_queue = "emails"
default_limit = 50
log_level = "info"
connection_pool_size = 5
```

### Database Support

- **PostgreSQL**: Full support with optimized queries and indexes
- **MySQL**: Complete compatibility with database-specific optimizations
- **Connection Pooling**: Configurable pool sizes for optimal performance
- **Migration Safety**: Atomic operations with rollback capabilities

## Advanced Usage

### Environment Integration

```bash
# Set environment variables
export DATABASE_URL=postgres://localhost/hammerwork
export HAMMERWORK_DEFAULT_QUEUE=processing
export HAMMERWORK_LOG_LEVEL=debug

# Commands will automatically use environment settings
cargo hammerwork migration run
```

### Cargo Subcommand Usage

```bash
# Works as a standard cargo subcommand
cargo hammerwork migration run --database-url postgres://localhost/mydb

# Or direct invocation
./target/debug/cargo-hammerwork migration run --database-url postgres://localhost/mydb
```

### Global Options

```bash
# Enable verbose logging
cargo hammerwork -v migration run

# Suppress output (errors only)
cargo hammerwork -q config show
```

## Development & Extension

The modular architecture makes it easy to extend functionality:

1. **Add New Commands**: Create modules in `src/commands/`
2. **Extend Utilities**: Add shared functionality in `src/utils/`
3. **Database Support**: Extend `DatabasePool` for new database types
4. **Configuration**: Add new config keys in `Config` struct

### Testing

```bash
# Run unit tests
cargo test -p cargo-hammerwork

# Set up test databases (requires Docker)
../scripts/setup-test-databases.sh both

# Run integration tests with databases
../scripts/setup-test-databases.sh test

# Check CLI structure
cargo run -p cargo-hammerwork -- --help

# Test specific commands
cargo run -p cargo-hammerwork -- migration status --database-url postgres://postgres:hammerwork@localhost:5433/hammerwork
cargo run -p cargo-hammerwork -- migration status --database-url mysql://root:hammerwork@localhost:3307/hammerwork
```

### Test Database Management

The project includes convenient scripts for managing test databases:

```bash
# From the project root directory:

# Set up test databases
./scripts/setup-test-databases.sh both      # Both PostgreSQL and MySQL
./scripts/setup-test-databases.sh postgres  # PostgreSQL only
./scripts/setup-test-databases.sh mysql     # MySQL only

# Check database status
./scripts/setup-test-databases.sh status

# Run integration tests
./scripts/setup-test-databases.sh test

# Stop databases
./scripts/setup-test-databases.sh stop

# Remove databases
./scripts/setup-test-databases.sh remove
```

Test database connection strings:
- PostgreSQL: `postgres://postgres:hammerwork@localhost:5433/hammerwork`
- MySQL: `mysql://root:hammerwork@localhost:3307/hammerwork`

### Development Workflow

A development helper script is available for common tasks:

```bash
# From the project root directory:

# Run full check (format + lint + test)
./scripts/dev.sh check

# Run tests with database integration
./scripts/dev.sh test-db

# CLI development workflow
./scripts/dev.sh cli

# Build everything
./scripts/dev.sh build

# Format code
./scripts/dev.sh fmt

# Run clippy
./scripts/dev.sh lint

# Generate docs
./scripts/dev.sh docs

# See all available commands
./scripts/dev.sh help
```

### Code Quality

The codebase follows Rust best practices:

- **Error Handling**: Comprehensive error types with context
- **Documentation**: Inline docs and examples
- **Modularity**: Clean separation of concerns
- **Type Safety**: Leverages Rust's type system for reliability
- **Async/Await**: Modern async patterns throughout

## Integration with Hammerwork

This CLI is designed to work seamlessly with Hammerwork applications:

- **Database Schema**: Creates and maintains compatible table structures
- **Job Format**: Handles Hammerwork job formats and priorities
- **Worker Compatibility**: Designed to work with Hammerwork workers
- **Migration Safety**: Respects existing Hammerwork installations

## Troubleshooting

### Common Issues

1. **Database Connection Errors**: Verify your DATABASE_URL and database accessibility
2. **Permission Errors**: Ensure database user has necessary privileges
3. **Configuration Issues**: Check config file location with `config path`

### Debugging

```bash
# Enable debug logging
cargo hammerwork -v migration run

# Check configuration
cargo hammerwork config show

# Verify database connectivity
cargo hammerwork migration status
```

## Future Roadmap

Planned enhancements include:

- **Web Dashboard**: Browser-based monitoring interface
- **Cluster Management**: Multi-node coordination features
- **Plugin System**: Extensible plugin architecture
- **Advanced Analytics**: Historical performance analysis and trending
- **External Integrations**: Webhook notifications and third-party service integration

## License

Same as the parent Hammerwork project: MIT OR Apache-2.0