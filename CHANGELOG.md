# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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
- [0.2.2](https://github.com/CodingAnarchy/hammerwork/releases/tag/v0.2.2)
- [0.2.1](https://github.com/CodingAnarchy/hammerwork/releases/tag/v0.2.1)
- [0.2.0](https://github.com/CodingAnarchy/hammerwork/releases/tag/v0.2.0)
- [0.1.0](https://github.com/CodingAnarchy/hammerwork/releases/tag/v0.1.0)

## Contributing
Please see [CONTRIBUTING.md](CONTRIBUTING.md) for details on our code of conduct and the process for submitting pull requests.

## License
This project is licensed under the MIT License - see the [LICENSE-MIT](LICENSE-MIT) file for details.