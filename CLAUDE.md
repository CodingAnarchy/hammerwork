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

# Run examples
cargo run --example postgres_example --features postgres
cargo run --example mysql_example --features mysql
cargo run --example cron_example --features postgres
cargo run --example priority_example --features postgres

# Check code formatting
cargo fmt --check

# Run clippy for linting
cargo clippy -- -D warnings
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

# 4. Verify examples compile and work
cargo check --examples --all-features

# 5. Build release version to catch optimization issues
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