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
- 📊 **Advanced Monitoring** - Real-time dashboards and health checks (framework included)
- 👷 **Worker Management** - Control job processing workers (framework included)
- 🎯 **Queue Operations** - Comprehensive queue management (framework included)
- 📋 **Job Management** - Full job lifecycle control (framework included)

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
cargo hammerwork config set <key> <value>     # Set a configuration value  
cargo hammerwork config get <key>             # Get a specific value
cargo hammerwork config reset --confirm       # Reset to defaults
cargo hammerwork config path                  # Show config file location
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

### Framework Extensions (Implemented but Not Exposed)

The codebase includes comprehensive implementations for advanced features that provide a solid foundation for extending the CLI:

#### Job Management Framework
- List jobs with advanced filtering (queue, status, priority, time-based)
- Job enqueueing with priority, delays, timeouts, and retry configuration
- Bulk operations (retry, cancel, purge) with safety confirmations
- Detailed job inspection with full lifecycle tracking

#### Queue Management Framework  
- Queue listing with comprehensive statistics
- Queue operations (clear, pause, resume)
- Health monitoring with configurable thresholds
- Detailed vs. summary statistics views

#### Worker Management Framework
- Worker lifecycle control (start, stop, status)
- Configurable worker pools with priority handling
- Real-time worker monitoring and metrics
- Graceful shutdown and resource management

#### Monitoring & Observability Framework
- Real-time dashboard with auto-refresh
- System health checks with JSON/table output
- Performance metrics with configurable time periods
- Log tailing and filtering capabilities

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

- **Job Scheduling**: Advanced cron-based job scheduling
- **Metrics Export**: Prometheus/OpenTelemetry integration  
- **Web Dashboard**: Browser-based monitoring interface
- **Cluster Management**: Multi-node coordination features
- **Plugin System**: Extensible plugin architecture

## License

Same as the parent Hammerwork project: MIT OR Apache-2.0