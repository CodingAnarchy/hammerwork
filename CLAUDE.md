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