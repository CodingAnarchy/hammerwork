# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- **🎨 Comprehensive CLI Workflow Management**
  - Complete `cargo hammerwork workflow` command suite with list, show, create, cancel, dependencies, and graph subcommands
  - Visual workflow dependency graph generation in multiple formats (text, DOT, Mermaid, JSON)
  - Professional Mermaid graph output with color-coded status indicators and dependency visualization
  - Workflow lifecycle management with failure policy configuration (fail_fast, continue_on_failure, manual)
  - Job dependency visualization with tree view and dependency status indicators
  - Example Mermaid documentation demonstrating workflow graph integration

- **🔗 Enhanced Job Dependencies & Workflow Features**
  - Complete workflow visualization system with dependency graph calculation and level-based grouping
  - Workflow metadata support with JSON configuration and validation
  - Advanced dependency tree traversal and visualization algorithms
  - Professional status color coding for completed, failed, running, and pending jobs

### Enhanced
- **📖 Documentation Improvements**
  - Updated README.md with comprehensive workflow examples and job dependency documentation
  - Enhanced ROADMAP.md marking job dependencies and workflows as completed features
  - Added workflow documentation section with pipeline examples and synchronization barriers

### Fixed
- **🧹 Code Quality Improvements**
  - Removed unused imports in CLI command test modules (config.rs, migration.rs)
  - Cleaned up compilation warnings and improved code organization
  - Enhanced CLI command module structure with proper workflow integration

### Technical Implementation
- **CLI Architecture**: Comprehensive workflow command structure with database integration
- **Visualization**: Multi-format graph output (text, DOT, Mermaid, JSON) for diverse integration needs
- **Dependencies**: Complete dependency graph algorithms with cycle detection and level calculation
- **Professional Output**: Bootstrap-inspired color schemes and professional formatting for workflow visualization

## [1.0.0] - 2025-06-27

🎉 **STABLE RELEASE** - Hammerwork has reached v1.0.0 with comprehensive feature completeness!

### Added
- **🔄 Advanced Retry Strategies** - The final Phase 1 feature completing Hammerwork's core functionality
  - `RetryStrategy` enum with five comprehensive retry patterns:
    - `Fixed(Duration)` - Consistent delay between retry attempts
    - `Linear { base, increment, max_delay }` - Linear backoff with optional ceiling
    - `Exponential { base, multiplier, max_delay, jitter }` - Exponential backoff with configurable jitter
    - `Fibonacci { base, max_delay }` - Fibonacci sequence delays for gentle growth
    - `Custom(Box<dyn Fn(u32) -> Duration>)` - Fully customizable retry logic
  - `JitterType` enum for preventing thundering herd problems:
    - `Additive` - Adds random jitter to delay
    - `Multiplicative` - Multiplies delay by random factor
  - Comprehensive job-level retry configuration with builder methods:
    - `with_retry_strategy()` - Set complete retry strategy
    - `with_exponential_backoff()` - Quick exponential backoff setup
    - `with_linear_backoff()` - Quick linear backoff setup  
    - `with_fibonacci_backoff()` - Quick Fibonacci backoff setup
  - Worker-level default retry strategies with `with_default_retry_strategy()`
  - Priority order: Job strategy → Worker default strategy → Legacy fixed delay
  - Full backward compatibility with existing fixed retry delay system
  - Comprehensive serialization support for database persistence
  - `fibonacci()` utility function for easy Fibonacci sequence generation

- **🧪 Comprehensive Test Suite Migration**
  - Migrated entire test suite from deprecated `create_tables()` to migration system
  - New `test_utils.rs` module with migration-based setup functions
  - Updated all test files: `integration_tests.rs`, `result_storage_tests.rs`, `worker_batch_tests.rs`, `batch_tests.rs`
  - Leverages proper `cargo hammerwork migrate` workflow for test database setup
  - Maintains comprehensive test coverage across PostgreSQL and MySQL

- **📖 Complete Documentation and Examples**
  - `retry_strategies.rs` example demonstrating all retry patterns
  - Worker-level and job-level retry configuration examples
  - Comprehensive ROADMAP.md updates marking Phase 1 complete
  - Detailed implementation examples for each retry strategy type
  - Best practices documentation for retry strategy selection

### Enhanced
- **Job Structure**: Extended with optional `retry_strategy` field
- **Worker Configuration**: Added `default_retry_strategy` field for worker-level defaults
- **Library Exports**: Added all retry strategy types to public API: `RetryStrategy`, `JitterType`, `fibonacci`
- **Database Compatibility**: Both PostgreSQL and MySQL implementations updated for new retry system
- **Error Handling**: Improved error messages and validation for retry configuration

### Technical Implementation
- **Backward Compatibility**: Existing jobs continue working with fixed delay system
- **Memory Efficient**: Lazy strategy evaluation with smart default handling
- **Type Safety**: Strongly typed retry strategies with comprehensive validation
- **Async Compatible**: Full async/await support throughout retry system
- **Database Agnostic**: Retry strategies work identically across PostgreSQL and MySQL
- **Extensible**: Custom retry strategies support any business logic requirements

### Migration Guide
- **No Breaking Changes**: All existing code continues to work unchanged
- **Opt-in Enhancement**: Add retry strategies to jobs for enhanced retry behavior
- **Database Migration**: Run `cargo hammerwork migrate` to prepare for v1.0.0 features
- **Test Updates**: Tests now use migration system instead of `create_tables()`

This release represents the completion of Hammerwork's Phase 1 roadmap, establishing a robust foundation for high-performance job processing with advanced retry strategies, comprehensive monitoring, and enterprise-grade features.

## [0.9.0] - 2025-06-27

### Added
- **📈 Worker Autoscaling**
  - `AutoscaleConfig` with comprehensive configuration options and sane defaults
  - Three preset configurations: `conservative()`, `aggressive()`, and `disabled()`
  - Dynamic worker pool scaling based on queue depth per worker metrics
  - Configurable min/max worker limits, scaling thresholds, and cooldown periods
  - `AutoscaleMetrics` for real-time monitoring of scaling decisions and worker utilization
  - Background autoscaling task with configurable evaluation windows and scaling steps
  - Graceful scaling with proper cooldown periods to prevent thrashing
  - Integration with existing WorkerPool infrastructure and statistics collection
  - Comprehensive test suite covering all scaling scenarios and edge cases
  - Complete example demonstrating various autoscaling configurations

### Enhanced
- **🔧 WorkerPool Improvements**
  - Added `with_autoscaling()` and `without_autoscaling()` configuration methods
  - Worker template system for creating new workers during scale-up operations
  - Improved worker lifecycle management with proper shutdown handling
  - Enhanced metrics collection and monitoring integration

## [0.8.0] - 2025-06-27

### Added
- **🔧 Cargo Subcommand for Database Migrations**
  - `cargo hammerwork migrate` command for running database migrations
  - `cargo hammerwork status` command for checking migration status
  - Progressive schema evolution with versioned migrations (001-006)
  - Database-specific optimizations (PostgreSQL JSONB vs MySQL JSON)
  - Migration tracking table for execution history and rollback safety
- **💾 Comprehensive Job Result Storage**
  - `ResultStorage` enum with `Database`, `Memory`, and `None` options for flexible result storage strategies
  - `ResultConfig` struct with TTL support, max size limits, and configurable storage backends
  - `JobResult` struct for structured result data returned by enhanced job handlers
  - Automatic result storage when jobs complete successfully with configurable expiration
  - Result retrieval, deletion, and cleanup operations integrated into the DatabaseQueue trait

- **🔧 Enhanced Job Handler System**
  - `JobHandlerWithResult` type for handlers that return result data alongside success/failure status
  - Dual handler system maintaining 100% backward compatibility with existing `JobHandler` implementations
  - `JobResult::success()` and `JobResult::with_data()` constructors for flexible result creation
  - Automatic result storage integration in worker processing loop

- **🗄️ Database Schema and Implementation Enhancements**
  - Added `result_data`, `result_stored_at`, `result_expires_at` columns to job tables
  - PostgreSQL implementation using JSONB for efficient result storage and queries
  - MySQL implementation using JSON with string-based UUID handling
  - Split monolithic queue implementation into separate PostgreSQL and MySQL modules for better maintainability
  - Optimized queries for result storage, retrieval, and expiration cleanup

- **⚡ Advanced Result Management API**
  - `store_job_result()` - Store result data with optional TTL expiration
  - `get_job_result()` - Retrieve stored results with automatic expiration checking
  - `delete_job_result()` - Manual result cleanup
  - `cleanup_expired_results()` - Batch cleanup of expired results returning count
  - TTL support with automatic expiration based on configurable time-to-live settings

- **👷 Worker Integration and Compatibility**
  - `Worker::new_with_result_handler()` constructor for enhanced result-storing workers
  - Automatic result storage when jobs complete successfully (respects job configuration)
  - Legacy handler compatibility - existing workers continue to work unchanged
  - `JobHandlerType` enum for internal handler type management and routing

- **🧪 Comprehensive Testing Suite**
  - 8 new result storage tests covering all functionality across PostgreSQL and MySQL
  - Worker integration tests using WorkerPool approach for realistic testing scenarios
  - Result expiration and TTL testing with time-based validation
  - Legacy handler compatibility testing ensuring backward compatibility
  - Configuration testing for all result storage modes and settings

- **📖 Documentation and Examples**  
  - Complete `result_storage_example.rs` demonstrating all result storage features
  - Database-specific implementations to handle complex generic constraints
  - Basic storage, enhanced workers, result expiration, and legacy compatibility examples
  - Visual feedback using emoji indicators for clear demonstration output
  - Comprehensive documentation for result storage configuration and usage

### Enhanced
- **Job Structure**: Added result configuration fields with builder methods
  - `with_result_storage()` - Configure storage backend (Database, Memory, None)
  - `with_result_ttl()` - Set time-to-live for result expiration
  - `with_result_config()` - Apply complete result configuration
- **Database Queue**: Extended trait with result storage operations
- **Library Exports**: Added new result storage types to public API
- **Architecture**: Modularized queue implementations for better code organization

### Removed (Breaking Changes)
- **BREAKING**: Removed `create_tables()` method from DatabaseQueue trait
- **BREAKING**: Removed `run_migrations()` method from DatabaseQueue trait  
- **BREAKING**: Removed standalone `migrate` binary
- **BREAKING**: All examples now require running `cargo hammerwork migrate` before use
- Database setup is now exclusively handled by the cargo subcommand for better separation of concerns

### Enhanced
- **Simplified API**: Cleaner DatabaseQueue trait focused on job operations only
- **Standard Workflow**: Database migrations follow Rust ecosystem conventions
- **Production Ready**: Migrations run during deployment, not application startup
- **Better Testing**: Database setup is external to application code

### Technical Implementation
- **Single Source of Truth**: Only one way to run migrations (`cargo hammerwork migrate`)
- **Idempotent Operations**: Safe to run migrations multiple times
- **Progressive Schema**: 6 versioned migrations covering Hammerwork's evolution (v0.1.0 to v0.8.0)
- **Database Optimizations**: PostgreSQL JSONB vs MySQL JSON with appropriate indexing
- **Migration Tracking**: Comprehensive execution history and rollback safety

## [0.7.1] - 2025-06-27

### Fixed
- **🔧 MySQL Compilation Fixes**
  - Fixed missing `batch_id` field in MySQL `DeadJobRow::into_job()` method
  - Added missing `use sqlx::Row;` import for MySQL `.get()` method functionality
  - Fixed type annotation for MySQL bulk insert operations
  - Removed unused imports to clean up compilation warnings

### Changed  
- **⚡ Updated to Rust Edition 2024**
  - Bumped Rust edition from 2021 to 2024 for latest language features
  - Set minimum supported Rust version (MSRV) to 1.86
  - All features now compile successfully with MySQL and PostgreSQL

### Technical
- Resolved MySQL-specific compilation errors in `src/queue.rs`
- Enhanced import handling for database-specific Row traits
- Improved code quality with clippy fixes and warning cleanup
- All 124+ tests pass with both database backends

## [0.7.0] - 2025-06-27

### Added
- **📦 Comprehensive Job Batching & Bulk Operations**
  - `JobBatch` struct for creating and managing job batches with configurable batch sizes
  - Three partial failure modes: `ContinueOnError`, `FailFast`, and `CollectErrors`
  - Batch validation ensuring all jobs in a batch belong to the same queue
  - Automatic chunking for large batches to respect database limits (10,000 jobs max)
  - Batch metadata support for tracking and categorization
  
- **🚀 High-Performance Bulk Database Operations**
  - PostgreSQL implementation using `UNNEST` for optimal bulk insertions
  - MySQL implementation using multi-row `VALUES` with automatic 100-job chunking
  - Atomic batch operations with transaction support
  - New database tables: `hammerwork_batches` for batch metadata and tracking
  - Batch status tracking with job counts (pending, completed, failed, total)
  
- **👷 Enhanced Worker Batch Processing**
  - `with_batch_processing_enabled()` for optimized batch job handling
  - `BatchProcessingStats` for comprehensive batch processing metrics
  - Automatic batch completion detection and status updates
  - Batch-aware job processing with enhanced monitoring
  - Success rate tracking with configurable thresholds (>95% for successful batches)
  
- **📊 Batch Operations API**
  - `enqueue_batch()` - Bulk enqueue jobs with optimized database operations
  - `get_batch_status()` - Real-time batch progress and statistics
  - `get_batch_jobs()` - Retrieve all jobs belonging to a batch
  - `delete_batch()` - Clean up completed batches
  - `BatchResult` with success/failure rates and job error tracking
  
- **🧪 Comprehensive Testing & Examples**
  - 18 new batch-specific tests covering all functionality
  - `batch_example.rs` demonstrating bulk job operations
  - `worker_batch_example.rs` showcasing worker batch processing features
  - Integration tests for both PostgreSQL and MySQL batch operations
  - Edge case testing for large batches and failure scenarios
  
- **📖 Documentation**
  - Complete batch operations documentation at `docs/batch-operations.md`
  - Best practices for batch size selection and failure handling
  - Performance considerations and database-specific optimizations
  - Troubleshooting guide for common batch processing issues

### Enhanced
- Extended `DatabaseQueue` trait with batch operation methods
- Worker event recording now tracks batch-specific metrics
- Job struct enhanced with optional `batch_id` field
- Statistics integration for batch job monitoring

### Technical Implementation
- **Memory Efficient**: Configurable batch sizes to manage memory usage
- **Network Optimized**: Reduces database round trips from N to 1-10 for N jobs
- **Type Safe**: Strongly typed batch operations with comprehensive validation
- **Backward Compatible**: All existing functionality preserved, batching is opt-in

## [0.6.0] - 2025-06-26

### Added
- **📊 Prometheus Metrics Integration** (enabled by default)
  - `PrometheusMetricsCollector` with comprehensive job queue metrics
  - Built-in metrics: job counts, duration histograms, failure rates, queue depth, worker utilization
  - Custom gauge and histogram metrics support
  - HTTP exposition server for Prometheus scraping using warp
  - Real-time metrics collection integrated into worker event recording

- **🚨 Advanced Alerting System** (enabled by default)
  - `AlertingConfig` with configurable thresholds for error rates, queue depth, worker starvation, and processing times
  - Multiple notification targets: Webhook, Slack (with rich formatting), and email alerts
  - Cooldown periods to prevent alert storms
  - Background monitoring task for real-time threshold checking
  - Custom alert types and severity levels (Info, Warning, Critical)

- **⚙️ Optional Feature Flags** 
  - `metrics` feature flag for Prometheus integration (enabled by default)
  - `alerting` feature flag for notification system (enabled by default)
  - Backward compatible: existing users automatically get new features
  - Opt-out available with `default-features = false` for minimal installations

- **🔍 Background Monitoring**
  - Automatic background task for metrics collection and alerting
  - Queue depth monitoring every 30 seconds
  - Worker starvation detection and alerts
  - Statistics-based threshold monitoring

### Changed
- Default features now include `metrics` and `alerting` for enhanced monitoring capabilities
- Enhanced worker integration with optional metrics and alerting components
- Updated documentation with feature flag usage examples

### Enhanced
- Worker event recording now feeds both statistics collectors and metrics collectors
- Thread-safe alerting with proper async handling and Send compatibility
- Comprehensive test suite with 110 tests covering all features

## [0.5.0] - 2025-06-26

### Added
- **🚀 Comprehensive Rate Limiting & Throttling System**
  - `RateLimit` struct with flexible time windows: `per_second()`, `per_minute()`, `per_hour()`
  - Configurable burst limits with `with_burst_limit()` for handling traffic spikes
  - Token bucket algorithm implementation for efficient and precise rate limiting
  - `RateLimiter` with both blocking (`acquire()`) and non-blocking (`try_acquire()`) token acquisition

- **⚖️ Advanced Worker Rate Limiting**
  - Worker-level rate limiting with `with_rate_limit()` configuration method
  - `with_throttle_config()` for advanced throttling with error backoff
  - Automatic rate limit enforcement in job processing loop
  - Configurable backoff periods when rate limits are exceeded or errors occur

- **🎛️ Queue-Level Throttling Configuration**
  - `ThrottleConfig` for queue-specific throttling policies
  - Maximum concurrent job limits per queue
  - Rate limiting with automatic conversion from throttle configs
  - Error backoff configuration for resilient job processing
  - In-memory throttling configuration storage with async access

- **🔧 Production-Ready Features**
  - Token availability monitoring for operational visibility
  - Rate limiter cloning for shared rate limits across workers
  - Integration with existing timeout and statistics systems
  - Graceful handling of rate limit exhaustion with intelligent waiting

- **🧪 Comprehensive Testing Suite**
  - 17 rate limiting specific unit tests covering all functionality
  - Token bucket algorithm validation and edge case testing
  - Rate limiter integration tests with async behavior verification
  - Worker integration tests ensuring seamless rate limiting integration
  - Performance and timing validation for production reliability

- **📖 Enhanced Documentation and Examples**
  - Updated PostgreSQL example demonstrating worker and queue-level rate limiting
  - Rate limiting configuration examples with realistic scenarios
  - Throttling configuration with burst support and error handling
  - Statistics integration showing rate limiting effects in monitoring

### Enhanced
- Extended `DatabaseQueue` trait with throttling configuration methods
- Enhanced worker error handling with configurable backoff periods
- Updated package exports to include all rate limiting types
- Improved example documentation with rate limiting best practices

### Technical Implementation
- **Token Bucket Algorithm**: Efficient rate limiting with configurable capacity and refill rates
- **Async-First Design**: Non-blocking rate limiting with `tokio::time::timeout` integration
- **Memory Efficient**: Shared rate limiters with `Arc<Mutex<TokenBucket>>` for concurrent access
- **Error Resilience**: Graceful degradation when rate limiters encounter errors
- **Backward Compatibility**: All existing functionality preserved, rate limiting is opt-in

### Breaking Changes
- None - all changes are backward compatible with existing deployments

## [0.4.0] - 2025-06-26

### Added
- **🎯 Comprehensive Job Prioritization System**
  - Five priority levels: `Background`, `Low`, `Normal` (default), `High`, `Critical`
  - Enhanced `Job` struct with priority field and builder methods: `as_critical()`, `as_high_priority()`, `as_low_priority()`, `as_background()`, `with_priority()`
  - Utility methods: `is_critical()`, `is_high_priority()`, `is_normal_priority()`, `is_low_priority()`, `is_background()`, `priority_value()`
  - String parsing support with multiple aliases (e.g., "crit", "c" for Critical)
  - Integer conversion methods for database storage: `as_i32()`, `from_i32()`

- **⚖️ Advanced Priority-Aware Job Scheduling**
  - Weighted priority scheduling with configurable weights per priority level
  - Strict priority scheduling (highest priority jobs always first)
  - Fairness factor to prevent low-priority job starvation
  - Hash-based job selection for Send compatibility in async contexts
  - `PriorityWeights` configuration with builder pattern

- **🗄️ Database Schema and Query Enhancements**
  - Added `priority` column to both PostgreSQL and MySQL schemas with default value `2` (Normal)
  - Optimized database indexes: `idx_hammerwork_jobs_queue_status_priority_scheduled` for efficient priority-based querying
  - Priority-aware `dequeue()` and `dequeue_with_priority_weights()` methods
  - Backward compatibility with existing job records (default to Normal priority)

- **👷 Worker Priority Configuration**
  - `with_priority_weights()` - Configure custom priority weights
  - `with_strict_priority()` - Enable strict priority mode
  - `with_weighted_priority()` - Enable weighted priority scheduling (default)
  - Worker pools support mixed priority configurations across workers
  - Integration with existing timeout and statistics systems

- **📊 Priority Statistics and Monitoring**
  - `PriorityStats` tracking job counts, processing times, and throughput per priority
  - Priority distribution percentage calculations
  - Starvation detection with configurable thresholds
  - Integration with `InMemoryStatsCollector` and `JobEvent` system
  - Most active priority identification and trend analysis

- **🧪 Comprehensive Testing Suite**
  - 12 new priority-specific unit tests covering all functionality
  - Priority ordering, serialization, and string parsing tests
  - Worker configuration and edge case testing
  - Statistics integration and starvation detection tests
  - Example demonstrating weighted scheduling, strict priority, and statistics

### Enhanced
- Updated package description to highlight job prioritization capabilities
- Added `priority_example.rs` demonstrating all priority features
- Enhanced statistics collection to track priority-specific metrics
- Worker pools now support heterogeneous priority configurations

### Fixed
- All stats module tests now include required priority field
- Applied clippy suggestions for cleaner derive macro usage
- Consistent code formatting across all files

## [0.3.0] - 2025-06-26

### Added
- **🕐 Comprehensive Cron Job Scheduling**
  - Full cron expression support with 6-field format (seconds, minutes, hours, day, month, weekday)
  - `CronSchedule` struct with timezone-aware scheduling using `chrono-tz`
  - Built-in presets for common schedules: `every_minute()`, `every_hour()`, `daily_at_midnight()`, `weekdays_at_9am()`, `mondays_at_noon()`
  - Cron expression validation and error handling with detailed error messages
  - Support for all standard timezones for global scheduling requirements

- **📋 Enhanced Job Structure for Recurring Jobs**
  - New fields: `cron_schedule`, `next_run_at`, `recurring`, `timezone`
  - Builder methods: `with_cron()`, `with_cron_schedule()`, `as_recurring()`, `with_timezone()`
  - Utility methods: `is_recurring()`, `has_cron_schedule()`, `calculate_next_run()`, `prepare_for_next_run()`
  - Smart next execution calculation based on cron expressions and timezones
  - Seamless integration with existing job timeout and retry mechanisms

- **🗄️ Database Schema Enhancements**
  - Added `cron_schedule`, `next_run_at`, `recurring`, `timezone` columns to both PostgreSQL and MySQL
  - Optimized indexes for recurring job queries: `idx_recurring_next_run`, `idx_cron_schedule`
  - Backward compatibility maintained with existing job records
  - Enhanced database queries to handle cron-specific fields efficiently

- **🔄 Intelligent Worker Integration**
  - Automatic rescheduling of completed recurring jobs based on cron expressions
  - Smart next-run calculation preserving timezone information
  - Integration with existing statistics and monitoring systems
  - Graceful handling of cron calculation errors with fallback to job completion
  - No impact on existing one-time job processing performance

- **📊 Comprehensive Management API**
  - `enqueue_cron_job()` - Create and schedule recurring jobs
  - `get_due_cron_jobs()` - Retrieve jobs ready for execution with optional queue filtering
  - `get_recurring_jobs()` - List all recurring jobs for a specific queue
  - `reschedule_cron_job()` - Manual rescheduling with automatic job state reset
  - `disable_recurring_job()` / `enable_recurring_job()` - Job lifecycle management
  - Full support in both PostgreSQL and MySQL implementations

- **🧪 Comprehensive Testing Suite**
  - 10 new cron-specific unit tests covering all functionality
  - 9 additional job integration tests for cron features
  - Timezone handling and edge case testing
  - Cron expression validation and serialization testing
  - Complete test coverage for recurring job lifecycle management

- **📖 Documentation and Examples**
  - Complete `cron_example.rs` demonstrating all cron functionality
  - Examples of daily, weekly, monthly, and custom interval scheduling
  - Timezone-aware scheduling examples (America/New_York)
  - Cron job management and lifecycle examples
  - Performance and monitoring integration examples

### Technical Implementation
- **Dependencies**: Added `cron` (0.12) and `chrono-tz` (0.8) for robust scheduling
- **Performance**: Optimized database queries with specialized indexes for recurring jobs
- **Memory**: Efficient cron schedule caching with lazy initialization
- **Error Handling**: Comprehensive error types for cron validation and timezone handling
- **Compatibility**: Full backward compatibility with existing jobs and database schemas

### Breaking Changes
- None - all changes are backward compatible with existing deployments

## [0.2.2] - 2025-06-26

### Added
- **Comprehensive Job Timeout Functionality**
  - `TimedOut` job status for jobs that exceed their timeout duration
  - Per-job timeout configuration with `Job::with_timeout()` builder method
  - Worker-level default timeouts with `Worker::with_default_timeout()`
  - Timeout detection using `tokio::time::timeout` for efficient async timeout handling
  - `timeout` and `timed_out_at` fields added to `Job` struct for complete timeout tracking
  - Automatic timeout event recording in statistics with `JobEventType::TimedOut`

- **Enhanced Database Support for Timeouts**
  - `timeout_seconds` and `timed_out_at` columns added to database schema
  - `mark_job_timed_out()` method added to `DatabaseQueue` trait
  - Complete timeout support in both PostgreSQL and MySQL implementations
  - Database queries updated to handle timeout fields in job lifecycle operations
  - Timeout counts integrated into queue statistics with `timed_out_count` field

- **Timeout Statistics and Monitoring**
  - `timed_out` field added to `JobStatistics` for timeout event tracking
  - Timeout events included in error rate calculations for comprehensive metrics
  - `timed_out_count` added to `QueueStats` for per-queue timeout monitoring
  - Enhanced statistics display in examples showing timeout metrics
  - Timeout event processing in `InMemoryStatsCollector`

- **Comprehensive Testing**
  - 14 new comprehensive tests covering timeout functionality
  - Job timeout detection logic testing with edge cases
  - Worker timeout configuration and precedence testing
  - Timeout statistics integration testing
  - Database operation interface testing for timeout methods
  - Job lifecycle testing with timeout scenarios

- **Enhanced Examples**
  - Updated PostgreSQL example with timeout configuration demonstrations
  - Updated MySQL example with various timeout scenarios (10s, 60s, 600s)
  - Job timeout precedence examples (job-specific vs worker defaults)
  - Priority-based timeout configuration examples (VIP vs standard jobs)
  - Comprehensive timeout statistics display in both examples

### Technical Implementation
- Timeout precedence: job-specific timeout takes priority over worker default timeout
- Graceful timeout handling: jobs are marked as `TimedOut` without affecting other jobs
- Async timeout detection: uses `tokio::time::timeout` for efficient resource management
- Database consistency: timeout information persisted and retrievable across job lifecycle
- Statistics integration: timeout events fully integrated into existing statistics framework

## [0.2.1] - 2025-06-25

### Removed
- Removed the `full` feature flag that enabled both PostgreSQL and MySQL simultaneously
  - Users typically choose one database backend per application
  - Simplifies the feature set and reduces unnecessary dependencies
  - Available features are now: `postgres`, `mysql`

## [0.2.0] - 2025-06-25

### Added
- **Comprehensive Statistics Tracking**
  - `JobStatistics` struct with detailed metrics (throughput, processing times, error rates)
  - `StatisticsCollector` trait for pluggable statistics backends
  - `InMemoryStatsCollector` with time-windowed data collection and configurable cleanup
  - `QueueStats` for queue-specific insights
  - `DeadJobSummary` for dead job analysis with error patterns
  - `JobEvent` system for tracking job processing lifecycle events
  - Integration with `Worker` and `WorkerPool` for automatic statistics collection

- **Dead Job Management**
  - Enhanced `Job` struct with `failed_at` field and `Dead` status
  - Dead job utility methods: `is_dead()`, `has_exhausted_retries()`, `age()`, `processing_duration()`
  - Database operations for dead job management:
    - `mark_job_dead()` - Mark jobs as permanently failed
    - `get_dead_jobs()` - Retrieve dead jobs with pagination
    - `get_dead_jobs_by_queue()` - Queue-specific dead job retrieval
    - `retry_dead_job()` - Reset dead jobs for retry
    - `purge_dead_jobs()` - Clean up old dead jobs
    - `get_dead_job_summary()` - Summary statistics with error patterns

- **Database Enhancements**
  - Extended `DatabaseQueue` trait with 15 new methods for statistics and dead job management
  - Enhanced database schemas with `failed_at` column and performance indexes
  - Complete feature parity between PostgreSQL and MySQL implementations
  - Optimized queries with proper indexing for performance

- **Testing & Examples**
  - 32 comprehensive unit tests with full coverage of new features
  - Updated examples demonstrating statistics collection and dead job management
  - Enhanced integration tests with 4 new test scenarios:
    - Dead job management lifecycle
    - Statistics collection functionality
    - Database queue statistics
    - Error frequency analysis

### Changed
- Updated package description to highlight new statistics and dead job management features
- Enhanced API with backward-compatible design

## [0.1.0] - 2025-06-25

### Added
- **Core Job Queue Infrastructure**
  - `Job` struct with UUID, payload, status, retry logic, and scheduling
  - Job statuses: `Pending`, `Running`, `Completed`, `Failed`, `Retrying`
  - Support for delayed job execution with configurable retry attempts
  - `JobQueue<DB>` generic struct with database connection pooling

- **Worker System**
  - `Worker<DB>` for processing jobs from specific queues
  - Configurable polling intervals, retry delays, and maximum retries
  - `WorkerPool<DB>` for managing multiple workers with graceful shutdown
  - Async channels for coordinated shutdown signaling

- **Database Support**
  - `DatabaseQueue` trait defining interface for database operations
  - Full PostgreSQL implementation with `FOR UPDATE SKIP LOCKED` for efficient job polling
  - Full MySQL implementation with transaction-based locking
  - Database-agnostic design using SQLx for compile-time query verification

- **Error Handling & Observability**
  - `HammerworkError` enum with structured error handling using `thiserror`
  - Comprehensive error wrapping for SQLx, serialization, and custom errors
  - Integrated tracing support for observability

- **Database Schema**
  - Optimized `hammerwork_jobs` table design for both PostgreSQL and MySQL
  - Proper indexing for efficient queue polling and job management
  - JSON/JSONB payload support with metadata tracking

- **Testing & Integration**
  - Comprehensive Docker-based integration testing infrastructure
  - Support for both PostgreSQL and MySQL in CI/CD pipelines
  - Shared test scenarios for database-agnostic testing
  - Complete test coverage with realistic job processing scenarios

- **Configuration & Features**
  - Feature flags: `postgres`, `mysql` for selective database support
  - Workspace configuration for consistent dependency management
  - Production-ready configuration with performance optimizations

### Technical Details
- Built on Tokio with async/await throughout the codebase
- Uses database transactions for atomic job state changes
- Type-safe job handling with `Result<()>` error propagation
- SQLx for compile-time query checking and database abstraction
- Generic over database types using `sqlx::Database` trait

---

## Release Links
- [0.7.0](https://github.com/CodingAnarchy/hammerwork/releases/tag/v0.7.0)
- [0.6.0](https://github.com/CodingAnarchy/hammerwork/releases/tag/v0.6.0)
- [0.5.0](https://github.com/CodingAnarchy/hammerwork/releases/tag/v0.5.0)
- [0.4.0](https://github.com/CodingAnarchy/hammerwork/releases/tag/v0.4.0)
- [0.3.0](https://github.com/CodingAnarchy/hammerwork/releases/tag/v0.3.0)
- [0.2.2](https://github.com/CodingAnarchy/hammerwork/releases/tag/v0.2.2)
- [0.2.1](https://github.com/CodingAnarchy/hammerwork/releases/tag/v0.2.1)
- [0.2.0](https://github.com/CodingAnarchy/hammerwork/releases/tag/v0.2.0)
- [0.1.0](https://github.com/CodingAnarchy/hammerwork/releases/tag/v0.1.0)

## Contributing
Please see [CONTRIBUTING.md](CONTRIBUTING.md) for details on our code of conduct and the process for submitting pull requests.

## License
This project is licensed under the MIT License - see the [LICENSE-MIT](LICENSE-MIT) file for details.