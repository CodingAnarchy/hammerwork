# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Hammerwork is a high-performance, database-driven job queue library for Rust with support for both PostgreSQL and MySQL. It provides async/await job processing with configurable retry logic, delayed jobs, job prioritization with weighted scheduling, and worker pools.

## Development Notes

- SQLx compile-time query checking requires database connections
- PostgreSQL uses `FOR UPDATE SKIP LOCKED` for efficient job polling
- MySQL implementation uses transaction-based locking (less optimal)
- Priority-aware queries use `ORDER BY priority DESC, scheduled_at ASC`
- Weighted selection uses hash-based algorithms for Send compatibility
- Tracing is integrated for observability
- Examples demonstrate database configurations, cron scheduling, and prioritization
- Use edition 2024 as the preference

## Common Development Commands

### Building and Testing
```bash
# Build the project
cargo build

# Run tests
cargo test

# Run tests with specific features
cargo test --features postgres
cargo test --features mysql

# Test CLI crate specifically
cargo test -p cargo-hammerwork

# Run examples
cargo run --example postgres_example --features postgres
cargo run --example mysql_example --features mysql
cargo run --example cron_example --features postgres
cargo run --example priority_example --features postgres

# Test CLI functionality
cargo run -p cargo-hammerwork -- --help
cargo run -p cargo-hammerwork -- migration --help
cargo run -p cargo-hammerwork -- config --help

# Check code formatting
cargo fmt --check

# Run clippy for linting
cargo clippy -- -D warnings

# Set up test databases for integration testing
./scripts/setup-test-databases.sh both

# Run integration tests with databases
./scripts/setup-test-databases.sh test

# Development helper shortcuts
./scripts/dev.sh check    # Run format + lint + test
./scripts/dev.sh test-db  # Run all tests including database integration
./scripts/dev.sh cli      # CLI development workflow
```

### Feature Flags
The project uses feature flags for database support:
- `postgres`: Enable PostgreSQL support
- `mysql`: Enable MySQL support
- `default`: No database features enabled by default

## Architecture

The codebase follows a modular architecture with clear separation of concerns:

### Core Components

1. **Job (`src/job.rs`)**
   - Defines the `Job` struct with UUID, payload, status, priority, retry logic
   - Job statuses: Pending, Running, Completed, Failed, Retrying, Dead, TimedOut
   - Job priorities: Background, Low, Normal, High, Critical
   - Supports delayed execution, configurable retry attempts, and priority levels

2. **Queue (`src/queue.rs`)**
   - `DatabaseQueue` trait defines the interface for database operations
   - `JobQueue<DB>` generic struct wraps database connection pools
   - Database-specific implementations in `postgres` and `mysql` modules
   - Uses database transactions for atomic job operations
   - Priority-aware job selection with weighted algorithms

3. **Worker (`src/worker.rs`)**
   - `Worker<DB>` processes jobs from specific queues
   - Configurable polling intervals, retry delays, max retries, and priority weights
   - Support for strict priority or weighted priority scheduling
   - `WorkerPool<DB>` manages multiple workers with graceful shutdown
   - Uses async channels for shutdown coordination

4. **Priority System (`src/priority.rs`)**
   - `JobPriority` enum with 5 levels: Background, Low, Normal, High, Critical
   - `PriorityWeights` for configurable weighted job selection
   - `PriorityStats` for monitoring priority distribution and performance
   - Prevents job starvation through fairness mechanisms

5. **Error Handling (`src/error.rs`)**
   - `HammerworkError` enum using thiserror for structured error handling
   - Wraps SQLx, serialization, and custom errors

### Database Schema

Both PostgreSQL and MySQL implementations use a single table `hammerwork_jobs` with:
- Job metadata (id, queue_name, status, priority, attempts)
- Timing information (created_at, scheduled_at, started_at, completed_at, failed_at, timed_out_at)
- Payload data (JSON/JSONB) and error messages
- Cron scheduling fields (cron_schedule, next_run_at, recurring, timezone)
- Optimized indexes for priority-aware queue polling

### Key Design Patterns

- **Generic over Database**: Uses `sqlx::Database` trait to support multiple databases
- **Feature-gated implementations**: Database-specific code behind feature flags
- **Async-first**: Built on Tokio with async/await throughout
- **Transaction safety**: Uses database transactions for job state changes
- **Type-safe job handling**: Job handlers return `Result<()>` for error handling
- **Priority-aware scheduling**: Weighted and strict priority algorithms prevent starvation
- **Comprehensive monitoring**: Statistics track priority distribution and performance

## Rust Library Development Best Practices

### Pre-Commit Quality Checks (REQUIRED)

**ALWAYS run these commands before committing and pushing any changes:**

```bash
# 1. Format code according to Rust standards
cargo fmt

# 2. Run all linting checks with strict warnings
cargo clippy --all-features -- -D warnings

# 3. Run complete test suite with all features
cargo test --all-features

# 4. Test CLI crate specifically
cargo test -p cargo-hammerwork

# 5. Verify examples compile and work
cargo check --examples --all-features

# 6. Test CLI functionality
cargo check -p cargo-hammerwork

# 7. Build release version to catch optimization issues
cargo build --release --all-features
```

### Code Quality Standards

1. **Code Formatting**
   - Use `cargo fmt` for consistent formatting
   - Follow Rust naming conventions (snake_case, PascalCase, SCREAMING_SNAKE_CASE)
   - Keep line length reasonable (under 100 characters when possible)

2. **Documentation**
   - Document all public APIs with `///` doc comments
   - Include examples in doc comments where helpful
   - Update CHANGELOG.md for all user-facing changes
   - Maintain CLAUDE.md for development guidance

3. **Testing Strategy**
   - Write unit tests for all core functionality
   - Include integration tests for database operations
   - Test with both PostgreSQL and MySQL features
   - Cover edge cases and error conditions
   - Use `#[ignore]` for tests requiring database connections
   - **CLI Testing**: Ensure cargo-hammerwork crate has comprehensive tests for all command modules
   - **Test Organization**: Place unit tests in the same file as the code being tested using `#[cfg(test)]` modules at the bottom of each file. Only use separate test files for integration tests that span multiple modules.

4. **Error Handling**
   - Use `thiserror` for structured error types
   - Provide meaningful error messages
   - Avoid unwrap() in library code
   - Propagate errors using `?` operator

5. **Dependencies & Features**
   - Keep dependencies minimal and well-justified
   - Use feature flags for optional functionality
   - Ensure backward compatibility when possible
   - Document MSRV (Minimum Supported Rust Version)

6. **Performance Considerations**
   - Profile database queries for efficiency
   - Use appropriate indexes in schema
   - Minimize allocations in hot paths
   - Test with realistic data volumes

### Release Process

1. **Version Bumping**
   - Follow Semantic Versioning (semver)
   - Update version in Cargo.toml
   - Update CHANGELOG.md with release notes
   - Tag releases with `git tag vX.Y.Z`

2. **Testing Before Release**
   - Run full test suite: `cargo test --all-features`
   - Test examples: `cargo run --example <name> --features <db>`
   - Verify documentation: `cargo doc --all-features`
   - Check packaging: `cargo package`

3. **Publishing**
   - Commit all changes with descriptive messages
   - Push to origin: `git push origin main && git push origin --tags`
   - Publish to crates.io: `cargo publish`

### Test Database Setup

The project includes scripts to easily set up PostgreSQL and MySQL test databases using Docker:

```bash
# Set up both PostgreSQL and MySQL test databases
./scripts/setup-test-databases.sh both

# Set up only PostgreSQL
./scripts/setup-test-databases.sh postgres

# Set up only MySQL
./scripts/setup-test-databases.sh mysql

# Check status of test databases
./scripts/setup-test-databases.sh status

# Run integration tests
./scripts/setup-test-databases.sh test

# Stop test databases
./scripts/setup-test-databases.sh stop

# Remove test databases
./scripts/setup-test-databases.sh remove
```

Database connection details:
- **PostgreSQL**: `postgres://postgres:hammerwork@localhost:5433/hammerwork`
- **MySQL**: `mysql://root:hammerwork@localhost:3307/hammerwork`

The CLI migrations are automatically run when setting up the databases.

### Database Development Guidelines

1. **Schema Changes**
   - Always provide migration paths
   - Test with both PostgreSQL and MySQL
   - Consider backward compatibility
   - Document schema changes in CHANGELOG

2. **Query Optimization**
   - Use proper indexes for job polling
   - Leverage database-specific features appropriately
   - Test query performance under load
   - Use `EXPLAIN` to analyze query plans

3. **Feature Parity**
   - Maintain functional equivalence between database backends
   - Handle database-specific limitations gracefully
   - Test all features with both databases

## CLI Development Guidelines

### SQL Query Strategy

The `cargo-hammerwork` crate uses **dynamic SQL queries** with `sqlx::query()` rather than compile-time `sqlx::query!()` macros to support:
- Complex filtering with optional parameters
- Database-agnostic queries (PostgreSQL and MySQL)
- Runtime query building for CLI flexibility

**Security**: All queries use parameterized statements to prevent SQL injection. Comprehensive tests in `tests/sql_query_tests.rs` validate query correctness.

### Feature Integration with CLI Tooling

**IMPORTANT**: When adding new features to the core Hammerwork library, ALWAYS consider and implement corresponding CLI tooling in the `cargo-hammerwork` crate:

1. **New Core Features**
   - Add appropriate CLI commands in `cargo-hammerwork/src/commands/`
   - Expose new functionality through the CLI interface
   - Update CLI help text and documentation
   - Add CLI tests for the new functionality

2. **CLI Command Categories**
   - **Migration**: Database schema management
   - **Job**: Job lifecycle management (enqueue, list, retry, cancel, inspect)
   - **Queue**: Queue operations (list, clear, pause, resume, stats)
   - **Worker**: Worker control (start, stop, status, pool management)
   - **Monitor**: Real-time monitoring and health checks
   - **Config**: Configuration management

3. **CLI Testing Requirements**
   - **Unit tests for command modules**: Each command module must have inline `#[cfg(test)]` modules
   - **SQL Query Validation**: Comprehensive tests in `tests/sql_query_tests.rs` validate all dynamic SQL queries
   - **Integration tests**: Database-backed tests with `#[ignore]` attribute for optional execution
   - **CLI argument parsing tests**: Validate clap command structure and argument validation
   - **Configuration loading tests**: Test config file loading, environment variable precedence
   - **Error handling tests**: Include SQL injection prevention and input validation tests
   - **Query Security**: All queries use parameterized statements (`sqlx::query(...).bind(...)`) to prevent SQL injection

4. **CLI Documentation**
   - Update CLI README when adding commands
   - Include usage examples in help text
   - Document configuration options
   - Maintain command reference documentation

### Git Workflow

1. **Commit Messages**
   - Use conventional commit format when appropriate
   - Include clear, descriptive titles
   - Reference issues/PRs when relevant
   - Add co-authorship for AI-assisted development

2. **Branch Management**
   - Use descriptive branch names
   - Keep commits focused and atomic
   - Squash commits before merging when appropriate

3. **Code Reviews**
   - Review for correctness, performance, and maintainability
   - Ensure tests pass on all supported platforms
   - Verify documentation is updated
   - **CLI Review**: Verify that new core features have appropriate CLI tooling