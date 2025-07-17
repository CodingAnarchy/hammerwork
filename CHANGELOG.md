# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed
- **🔐 Encryption Key Rotation Architecture Simplification**
  - **BREAKING CHANGE**: Removed `rotate_key_if_needed()` method from `EncryptionEngine`
  - Delegated all key rotation responsibility to the `KeyManager` for cleaner separation of concerns
  - Eliminated redundant rotation logic between engine and key manager
  - Key rotation now uses database-driven scheduling through `KeyManager` methods:
    - `perform_automatic_rotation()` - rotates all keys due for rotation
    - `start_rotation_service()` - background service for automatic rotation
    - `is_key_due_for_rotation()` - checks if a specific key needs rotation
    - `rotate_key()` - manually rotates a specific key
  - Updated trait bounds on `EncryptionEngine` to support KeyManager operations
  - Removed fallback rotation tests as rotation is now handled entirely by KeyManager

### Fixed
- **🔐 Master Key Storage Database Operations**
  - Implemented missing database operations in `store_master_key_securely()` method
  - Added concrete PostgreSQL and MySQL implementations for secure master key storage
  - Master keys are now properly encrypted and stored in the `hammerwork_encryption_keys` table
  - Database records include proper metadata: `key_purpose = 'KEK'`, `status = 'Active'`, `algorithm = 'AES256GCM'`
  - Encrypted master key material is stored with unique salt for key derivation security
  - Added proper error handling with descriptive error messages for database operations
  - Fixed unreachable code warnings by restructuring conditional compilation blocks
  - Master keys are identified as Key Encryption Keys (KEK) and don't have rotation intervals

## [1.13.1] - 2025-07-17

### Fixed
- **🧪 Unit Test Compilation and TOML Serialization**
  - Fixed compilation errors in `key_management_example.rs` by adding required `encryption` feature flag
  - Resolved TOML serialization failures for configuration structs with proper serde attribute handling
  - Fixed missing optional field handling in TOML deserialization by adding `skip_serializing_if` and `default` attributes
  - Corrected `exposition_addr` field serialization in `MetricsConfig` to handle `None` values properly
  - Fixed `backoff_on_error` field serialization in `ThrottleConfig` for proper TOML compatibility
  - Updated `test_duration_serialization` test to use current configuration structure
  - Fixed enum deserialization for `RetryStrategy` and `JitterType` using flat TOML structure with `type` field
  - Resolved u128 serialization issues by converting `Duration.as_millis()` to u64 for TOML compatibility
  - Removed unused UUID serialization functions to eliminate compiler warnings
  - All 263 unit tests now pass successfully with no compilation errors or warnings

- **🔧 Encryption Module Compilation Issues**
  - Fixed duplicate method definitions in database-specific KeyManager implementations
  - Resolved sqlx trait bound issues for generic database types by adding proper String decode constraints
  - Fixed lifetime syntax errors in trait bounds (changed `'_` to proper `for<'r>` syntax)
  - Removed unused imports in encryption module to eliminate compiler warnings
  - Added placeholder implementations for methods called from generic code to prevent compilation errors
  - All crate features now compile successfully with only minor dead code warnings

## [1.13.0] - 2025-07-16

### Fixed
- **⚙️ Configuration Serialization**
  - Fixed Duration serialization in TOML configuration files to use human-readable format ("30s", "5m", "1h", "1d")
  - Fixed UUID serialization in TOML by converting to string format to prevent u128 compatibility issues
  - Removed duplicate struct definitions that caused serialization conflicts
  - Enhanced duration parsing to support multiple formats: plain numbers (seconds), and suffixes (s, m, h, d)
  - Re-enabled previously ignored configuration file tests (`test_config_file_operations`)

### Enhanced
- **🔐 Encryption Key Management Statistics**
  - Implemented real database statistics queries for encryption key management system
  - Added comprehensive PostgreSQL and MySQL statistics queries for key counts by status (Active, Retired, Revoked, Expired)
  - Enhanced key age calculation using database-specific date functions (PostgreSQL `EXTRACT(EPOCH)`, MySQL `TIMESTAMPDIFF`)
  - Added expiration monitoring with 7-day early warning for keys approaching expiration
  - Implemented rotation tracking for keys due for automated rotation
  - Added integration tests for statistics queries with both PostgreSQL and MySQL backends
  - Replaced placeholder statistics implementation with production-ready database queries

- **🔄 Database-Managed Key Rotation System**
  - Implemented complete database-managed key rotation with PostgreSQL and MySQL support
  - Added automatic key rotation scheduling with configurable intervals and next rotation timestamps
  - Enhanced rotation detection queries using database-native time comparisons
  - Implemented key rotation schedule management (update, query, schedule specific times)
  - Added background rotation service for automated key lifecycle management
  - Enhanced rotation methods with proper version management and status tracking
  - Added comprehensive integration tests for rotation functionality, scheduling, and automation
  - Implemented Clone trait for KeyManager to support background service operations

## [1.12.0] - 2025-07-15

### Added
- **🏢 Enterprise Key Management Service (KMS) Integrations**
  - Complete AWS KMS integration for enterprise key management with support for key aliases, ARNs, and IAM authentication
  - Google Cloud KMS integration with full resource path support and service account authentication
  - HashiCorp Vault KMS integration using KV v2 secrets engine with token and AppRole authentication
  - New feature flags: `aws-kms`, `gcp-kms`, and `vault-kms` for selective compilation
  - Graceful fallback to deterministic key generation when external KMS services are unavailable

- **🔐 Enhanced External Key Source Support**
  - AWS KMS URI format: `aws://key-id?region=us-east-1` with support for key aliases and ARNs
  - GCP KMS URI format: `gcp://projects/PROJECT/locations/LOCATION/keyRings/RING/cryptoKeys/KEY`
  - Vault KMS URI format: `vault://secret/path/to/key` with optional address parameter
  - Flexible authentication via environment variables (AWS_*, GOOGLE_*, VAULT_*)
  - Base64 key encoding/decoding with proper error handling and validation

- **📚 Comprehensive Documentation and Examples**
  - `aws_kms_encryption_example.rs` - Complete AWS KMS setup, authentication, and best practices
  - `gcp_kms_encryption_example.rs` - Google Cloud KMS configuration and service account setup
  - `vault_kms_encryption_example.rs` - HashiCorp Vault KMS with policies, authentication, and troubleshooting
  - Updated README.md with installation instructions for all KMS providers
  - Enhanced `docs/encryption.md` with detailed KMS configuration sections

- **🧪 Extensive Test Coverage**
  - 18 new unit tests covering KMS configuration parsing and validation
  - 9 integration tests for KMS functionality and fallback behavior
  - Comprehensive test coverage for URI parsing, authentication, and error handling
  - SQL injection prevention tests for dynamic query generation

### Enhanced
- **🔑 Key Management Flexibility**
  - Support for multiple concurrent KMS providers within the same application
  - Improved error messages with specific guidance for KMS configuration issues
  - Enhanced key caching and connection management for better performance
  - Consistent API across all KMS providers for seamless switching

- **🔒 Security Improvements**
  - Proper secret handling with no plain-text key storage in logs or memory dumps
  - Secure key material transport with authenticated encryption
  - Audit trail support for all KMS operations and key lifecycle events
  - Environment variable validation and sanitization

## [1.11.0] - 2025-07-14

### Added
- **⏸️ Queue Pause/Resume Functionality**
  - Complete queue pause and resume system for operational control and maintenance windows
  - New `pause_queue()`, `resume_queue()`, `is_queue_paused()`, `get_queue_pause_info()`, and `get_paused_queues()` methods in DatabaseQueue trait
  - Database migration 014 adding `hammerwork_queue_pause` table for persistent pause state storage
  - Full PostgreSQL and MySQL backend implementation with optimized queries and proper indexing
  - Worker integration automatically respecting paused queues - workers skip job dequeuing when queues are paused
  - Graceful operation: jobs already in progress continue to completion while new jobs are blocked
  - Audit trail support tracking who paused/resumed queues and when for operational transparency

- **🌐 Web UI Queue Management**
  - Enhanced web dashboard with visual queue status indicators showing active/paused state
  - Interactive pause/resume buttons with dynamic UI updates based on current queue state
  - Real-time status badges with color-coded indicators: 🟢 Active, 🟡 Paused
  - Immediate user feedback with success/error notifications for all queue operations
  - Updated queue API endpoints supporting pause/resume actions via `/api/queues/{name}/actions`
  - Extended queue information API responses including `is_paused`, `paused_at`, and `paused_by` fields

- **🏗️ Database Schema and Migration**
  - New `hammerwork_queue_pause` table with queue_name (primary key), timestamps, and audit fields
  - Automatic timestamp management for PostgreSQL (triggers) and MySQL (ON UPDATE CURRENT_TIMESTAMP)
  - Proper indexing on `paused_at` for efficient query performance
  - Cross-database compatibility with database-specific SQL optimizations

### Enhanced
- **📊 API Responses**
  - Queue information now includes pause status, pause timestamp, and who initiated the pause
  - Enhanced queue statistics with operational state visibility
  - Improved error handling and user feedback for all queue management operations

- **🎨 Web Interface**
  - Updated queue table layout with new Status column for better visibility
  - Added success/warning button styles for pause/resume actions
  - Enhanced CSS styling with consistent color scheme and visual feedback
  - Improved user experience with contextual action buttons

## [1.10.0] - 2025-07-14

### Added
- **🔐 Complete Encryption Key Management System**
  - Implemented comprehensive encryption key lifecycle management with secure storage, rotation, and retirement
  - Added `KeyManager<DB>` with full PostgreSQL and MySQL support for enterprise-grade key operations
  - Support for multiple encryption algorithms: AES-256-GCM and ChaCha20-Poly1305 with configurable key strengths
  - Master key encryption (KEK) system ensuring data encryption keys are never stored in plaintext
  - Automatic key rotation with configurable intervals and next rotation scheduling
  - Key versioning system supporting up to configurable maximum versions per key ID
  - Secure key derivation using Argon2 with customizable memory cost, time cost, and parallelism parameters

- **🏗️ Database Schema and Migration Support**
  - New `hammerwork_encryption_keys` table with comprehensive metadata tracking and optimized indexes
  - New `hammerwork_key_audit_log` table for complete audit trail of all key operations
  - Database migration files for both PostgreSQL (013_add_key_audit.postgres.sql) and MySQL (013_add_key_audit.mysql.sql)
  - Proper constraint validation ensuring data integrity and encryption consistency
  - Optimized indexes for key lookup, rotation queries, expiration tracking, and audit log searches

- **🔑 Advanced Key Operations**
  - `store_key()` and `load_key()` operations with automatic encryption and version management
  - `retire_key_version()` for secure key retirement while maintaining decryption capabilities
  - `cleanup_old_key_versions()` with configurable retention policies preventing key sprawl
  - `get_keys_due_for_rotation()` for automated rotation scheduling and compliance
  - `record_key_usage()` with comprehensive usage statistics and last access tracking
  - `record_audit_event()` providing complete audit trails for compliance and security monitoring

- **🛡️ Security and Compliance Features**
  - External Key Management Service (KMS) integration support for AWS KMS, Azure Key Vault, HashiCorp Vault
  - Key source management supporting environment variables, static keys, generated keys, and external services
  - Comprehensive audit logging with operation type, success/failure tracking, and error message capture
  - Key purpose categorization: Encryption, MAC (Message Authentication Code), and KEK (Key Encryption Key)
  - Key status management: Active, Retired, Revoked, and Expired with proper lifecycle transitions
  - Configurable key expiration, rotation intervals, and automated cleanup policies

- **📊 Key Management Statistics and Monitoring**
  - `KeyManagerStats` providing comprehensive metrics: total keys, active/retired/revoked/expired counts
  - Key usage analytics: total access operations, rotations performed, average key age
  - Proactive monitoring: keys expiring soon alerts and rotation due notifications
  - Performance metrics and key management health indicators

- **🔧 Configuration and Flexibility**
  - `KeyManagerConfig` with fluent builder pattern for easy configuration management
  - Support for auto-rotation with configurable intervals and maximum key version limits
  - Audit logging enable/disable with comprehensive event tracking
  - External KMS configuration with service type, endpoint, authentication, and namespace support
  - Key derivation configuration with Argon2 parameter tuning for security vs. performance optimization

### Enhanced
- **🔒 Database Feature Parity**
  - Complete feature parity between PostgreSQL and MySQL implementations for all key management operations
  - Database-specific optimizations: PostgreSQL uses native UUID arrays and INTERVAL types
  - MySQL implementation uses JSON columns and seconds-based interval storage for compatibility
  - Proper error handling and conversion between different database type systems

- **📝 Comprehensive Testing**
  - Added 20 comprehensive unit tests covering all key management functionality
  - Error handling tests validating robust parsing and graceful failure modes
  - Database operation tests ensuring proper integration with both PostgreSQL and MySQL
  - Configuration validation tests for all builder patterns and default values
  - Serialization/deserialization tests ensuring cross-system compatibility

### Fixed
- **🛠️ Code Quality and Maintainability**
  - Exposed parsing helper functions (`parse_algorithm`, `parse_key_source`, `parse_key_purpose`, `parse_key_status`) for extensibility
  - Implemented `Display` traits for all key management enums enabling human-readable output
  - Added comprehensive error types and messages for debugging and troubleshooting
  - Proper feature flag isolation ensuring encryption functionality is optional and self-contained

## [1.9.0] - 2025-07-14

### Added
- **🔄 Complete Workflow and Dependency Management System**
  - Implemented full workflow orchestration capabilities with job dependency management
  - Added `JobGroup` workflow builder with fluent API for creating complex job pipelines
  - Support for sequential job chains using `.then()` method
  - Support for parallel job execution using `.add_parallel_jobs()` method
  - Comprehensive dependency resolution engine that automatically manages job execution order
  - Workflow validation with circular dependency detection using topological sorting
  - Three failure policies: `FailFast`, `ContinueOnFailure`, and `Manual` intervention modes

- **🗄️ Database Schema and Storage**
  - New `hammerwork_workflows` table for workflow metadata tracking
  - Extended `hammerwork_jobs` table with dependency fields: `depends_on`, `dependents`, `dependency_status`, `workflow_id`, `workflow_name`
  - Optimized database indexes for efficient dependency resolution queries
  - PostgreSQL implementation uses native UUID arrays and JSONB for dependencies
  - MySQL implementation uses JSON columns with proper constraint validation

- **🚀 Queue Interface Extensions**
  - `enqueue_workflow()` - Validates and atomically inserts entire workflows
  - `get_workflow_status()` - Retrieves workflow metadata and current execution state
  - `resolve_job_dependencies()` - Updates dependency status when jobs complete successfully
  - `get_ready_jobs()` - Efficiently finds jobs ready for execution (no unsatisfied dependencies)
  - `fail_job_dependencies()` - Cascades failure through dependency graph with configurable policies
  - `get_workflow_jobs()` - Retrieves all jobs within a specific workflow
  - `cancel_workflow()` - Cancels workflow and marks all pending jobs as failed

- **⚡ Advanced Dependency Features**
  - Automatic dependency satisfaction tracking with real-time status updates
  - Intelligent failure propagation that respects workflow failure policies
  - Support for complex dependency graphs with multiple fan-in/fan-out patterns
  - Transactional workflow operations ensuring data consistency
  - Workflow statistics tracking: total jobs, completed jobs, failed jobs

### Fixed
- **🔧 Implementation Completeness**
  - Replaced all `todo!()` placeholders in workflow code with full implementations
  - Added comprehensive error handling for workflow validation and execution
  - Implemented proper UUID conversion handling between PostgreSQL and MySQL
  - Added helper methods `insert_job_in_transaction()` for both database backends
  - Fixed compilation issues and ensured feature parity between PostgreSQL and MySQL

### Enhanced
- **📊 MySQL Encryption Deserialization**
  - Completed MySQL encryption deserialization implementation to match PostgreSQL
  - Updated all MySQL SQL queries to include encryption fields using `JOB_SELECT_FIELDS` constant
  - Added encryption helper methods: `build_encryption_config()`, `parse_retention_policy()`, `build_encrypted_payload()`
  - Proper handling of MySQL JSON types vs PostgreSQL arrays for encryption metadata
  - Full feature parity between PostgreSQL and MySQL encryption implementations

## [1.8.4] - 2025-07-14

### Added
- **🔐 Encryption Deserialization Implementation**
  - Implemented complete deserialization logic for encrypted job payloads in PostgreSQL queue
  - Added encryption fields to `JobRow` struct: `is_encrypted`, `encryption_key_id`, `encryption_algorithm`, `encrypted_payload`, `encryption_nonce`, `encryption_tag`, `encryption_metadata`, `payload_hash`, `pii_fields`, `retention_policy`, `retention_delete_at`, `encrypted_at`
  - Created helper methods `build_encryption_config()`, `parse_retention_policy()`, and `build_encrypted_payload()` for reconstructing encryption data structures from database fields
  - Updated all SQL SELECT queries to include encryption fields using new `JOB_SELECT_FIELDS` constant
  - Properly handles base64 encoding/decoding of binary encryption data
  - Full backward compatibility - works with and without encryption feature enabled

### Fixed
- **📊 Query Consistency**
  - Standardized all job selection queries to include complete field list
  - Fixed missing encryption fields in `dequeue()`, `get_job()`, `get_batch_jobs()`, `get_due_cron_jobs()`, and `get_recurring_jobs()` queries
  - Resolved borrow checker issues in `into_job()` method by extracting encryption data before consuming self

## [1.8.3] - 2025-07-12

### Fixed
- **🔐 Database Queue Compilation**
  - Fixed PostgreSQL and MySQL queue implementations to use feature-gated encryption fields
  - Added `#[cfg(feature = "encryption")]` guards around `encryption_config`, `retention_policy`, and `encrypted_payload` field assignments
  - Resolved compilation errors when using Hammerwork without the encryption feature in client applications
  - Ensures Job struct creation works correctly in all database queue operations regardless of feature flags

## [1.8.2] - 2025-07-12

### Fixed
- **🔐 Encryption Feature Compilation**
  - Fixed compilation errors when `encryption` feature is disabled
  - Added proper `#[cfg(feature = "encryption")]` feature gates throughout encryption modules
  - Resolved unused import warnings in encryption engine and key manager
  - Ensured encryption functionality is properly isolated behind feature flags

- **🧹 Code Quality**
  - Cleaned up unused imports in `encryption::engine` and `encryption::key_manager` modules
  - Fixed unused parameter warnings by removing unnecessary underscore prefixes
  - Verified encryption tests pass with proper feature flag isolation

## [1.8.1] - 2025-07-12

### Added
- **🔄 Clone Trait Implementation**
  - Implemented `Clone` trait for `JobQueue<DB>` struct for better ergonomics
  - Added manual `Clone` implementation to handle generic database types
  - `TestQueue` already had `Clone` support (no changes needed)
  - Improved developer experience when sharing queue instances across application components

### Fixed
- **🐛 Code Quality Improvements**
  - Fixed unnecessary `.clone()` calls on `JobStatus` enum (implements `Copy`)
  - Resolved clippy warnings related to `clone_on_copy`
  - Enhanced code efficiency by using copy semantics where appropriate

## [1.8.0] - 2025-07-07

### Added
- **🚀 PostgreSQL Native UUID Arrays for Dependencies**
  - Added migration 012 to optimize job dependencies using native PostgreSQL UUID arrays
  - PostgreSQL now uses `UUID[]` instead of JSONB for `depends_on` and `dependents` columns
  - Provides ~30% storage reduction and better query performance for dependency operations
  - Migration includes transaction safety, UUID validation, and data integrity checks
  - MySQL continues to use JSONB for compatibility

### Changed
- **🔧 Improved Enum Serialization**
  - `JobStatus` and `BatchStatus` enums now use proper SQLx `Encode`/`Decode` implementations
  - Removed unnecessary JSON serialization for enum storage
  - Added `JobStatus::as_str()` helper method for consistent string conversion
  - Database values now stored as plain strings instead of JSON-encoded strings

### Fixed
- **🐛 Enum Storage Format**
  - Fixed `JobStatus` being stored as `"\"Pending\""` instead of `"Pending"`
  - Fixed `BatchStatus` deserialization to use direct SQLx types
  - Improved backward compatibility handling for both quoted and unquoted formats

## [1.7.4] - 2025-07-07

### Fixed
- **🐛 Job Status Encoding** 
  - Fixed job status values being stored with extra quotes in database
  - Replaced `serde_json::to_string()` with proper SQLx type implementations for `JobStatus` enum
  - Job status values now stored as clean strings (`"Pending"`, `"Running"`, etc.) instead of JSON strings (`"\"Pending\""`, `"\"Running\""`, etc.)
  - Added backward compatibility support to handle both quoted and unquoted status formats during database reads
  - CLI commands now work correctly with job status filtering and querying
  - Added comprehensive tests for backward compatibility and encoding logic

## [1.7.3] - 2025-07-04

### Changed
- **⬆️ Dependency Updates**
  - Updated `prometheus` from version 0.13 to 0.14
  - Improved workspace dependency management for `prometheus` crate
  - Fixed metrics API compatibility for prometheus 0.14 label value handling

## [1.7.2] - 2025-07-04

### Fixed
- **🐛 PostgreSQL Migration Runner**
  - Fixed PostgreSQL migration runner to properly split SQL statements on semicolon
  - Resolved "cannot insert multiple commands into a prepared statement" error completely
  - Improved SQL statement parsing to handle all statement formats correctly

## [1.7.1] - 2025-07-04

### Fixed
- **🐛 PostgreSQL Migration Compatibility**
  - Fixed PostgreSQL migration 011_add_encryption to separate multiple ALTER TABLE statements
  - Resolved "cannot insert multiple commands into a prepared statement" error
  - Each ADD COLUMN statement now executed individually for PostgreSQL compatibility

## [1.7.0] - 2025-07-03

### Added
- **🔐 Job Encryption & PII Protection**
  - Complete encryption system for protecting sensitive job payloads and personally identifiable information (PII)
  - Support for multiple encryption algorithms: AES-256-GCM and ChaCha20-Poly1305 with authenticated encryption
  - Field-level encryption targeting specific PII fields like credit cards, SSNs, and other sensitive data
  - Automatic PII field detection using pattern matching for common sensitive data types
  - Thread-safe `EncryptionEngine` with performance statistics and operation tracking
  - Configurable retention policies: `DeleteAfter`, `DeleteAt`, `KeepIndefinitely`, `DeleteImmediately`, and `UseDefault`

- **🗝️ Advanced Key Management System**
  - Enterprise-grade key management with `KeyManager` supporting PostgreSQL and MySQL
  - Master key encryption (KEK - Key Encryption Keys) for securing data encryption keys
  - Key rotation and lifecycle management with automatic rotation scheduling
  - Key versioning system with configurable maximum versions (default: 10 versions)
  - Key audit trails tracking all key operations: Create, Access, Rotate, Retire, Revoke, Delete, Update
  - External KMS integration support (AWS, GCP, Azure, HashiCorp Vault) with authentication configuration
  - Key derivation from passwords using Argon2 with configurable memory, time, and parallelism parameters

- **🛡️ Security Features**
  - Keys never stored in plain text - all keys encrypted with master key in database
  - Secure key caching with 1-hour TTL and thread-safe access patterns
  - Key status management: Active, Retired, Revoked, Expired with proper lifecycle enforcement
  - Comprehensive key statistics: total keys, active/retired/revoked counts, usage tracking, rotation metrics
  - Key expiration handling with automatic status updates and access prevention
  - Salt-based key derivation for password-based keys with configurable parameters

- **📊 Database Schema Enhancements**
  - New `hammerwork_encryption_keys` table for secure key storage with encrypted key material
  - Added encryption fields to `hammerwork_jobs` table: `is_encrypted`, `encrypted_payload`, `pii_fields`, `retention_policy`
  - Extended `hammerwork_job_archive` table with encryption support for archived job data
  - Comprehensive indexes for efficient key lookup and rotation queries
  - Migration 011_add_encryption for both PostgreSQL and MySQL with proper column types and constraints

### Enhanced
- **🔧 Job System Integration**
  - Extended `Job` struct with encryption fields: `encryption_config`, `pii_fields`, `retention_policy`, `is_encrypted`, `encrypted_payload`
  - Builder pattern methods: `with_encryption()`, `with_pii_fields()`, `with_retention_policy()` for fluent job creation
  - Seamless integration with existing job processing - no changes required to job handlers
  - Automatic encryption/decryption during job enqueue/dequeue operations when configured
  - Compression before encryption for large payloads to optimize storage and performance

- **⚡ Performance Optimizations**
  - Zero overhead for non-encrypted jobs - encryption only activated when explicitly configured
  - Efficient field-level encryption that only processes specified PII fields
  - Key caching reduces database queries for frequently accessed keys
  - Batch key operations for improved performance in high-throughput scenarios
  - Memory-efficient encryption with streaming for large payloads

- **🎯 Configuration Flexibility**
  - Feature flag `encryption` for optional compilation - only includes dependencies when needed
  - Multiple key sources: Environment variables, static keys, generated keys, external KMS
  - Configurable encryption algorithms with algorithm-specific key sizes and security parameters
  - Flexible retention policies supporting various compliance requirements (GDPR, HIPAA, PCI-DSS)
  - Runtime configuration loading from environment variables and configuration files

### Security Considerations
- **🔒 Cryptographic Standards**
  - Uses industry-standard encryption algorithms with authenticated encryption (AEAD)
  - AES-256-GCM provides 256-bit security with Galois/Counter Mode for performance and security
  - ChaCha20-Poly1305 offers modern stream cipher with Poly1305 MAC for mobile and embedded systems
  - Secure random number generation using OS entropy sources (`OsRng`)
  - Proper nonce handling with unique nonces for each encryption operation

- **🛡️ Key Security**
  - Master keys loaded from secure sources with proper access control
  - Key rotation prevents long-term key exposure with configurable rotation intervals
  - Key versioning maintains backward compatibility while enabling forward security
  - Audit logging provides complete key operation history for compliance and security monitoring
  - Key expiration and revocation capabilities for incident response and key compromise scenarios

### Examples & Documentation
- **📖 Comprehensive Examples**
  - `encryption_example.rs` demonstrating all encryption features with realistic PII scenarios
  - `key_management_example.rs` showing enterprise key management patterns and best practices
  - Doctests throughout encryption modules with proper feature guards and usage patterns
  - Integration examples showing encryption with job processing, archiving, and worker operations

- **🔧 CLI Integration**
  - Extended cargo-hammerwork CLI with encryption key management commands
  - Database migration support for encryption schema with `cargo hammerwork migration run`
  - Key generation, rotation, and audit trail inspection through CLI interface
  - Configuration validation and security best practice recommendations

### Technical Implementation
- **🏗️ Architecture**
  - Modular design with `encryption::engine` and `encryption::key_manager` separation
  - Generic key management supporting multiple database backends
  - Event-driven key operations with proper error handling and rollback capabilities
  - Thread-safe design using `Arc<Mutex<>>` patterns for concurrent access

- **🧪 Testing Coverage**
  - Comprehensive unit tests for all encryption and key management operations
  - Integration tests with real database backends (PostgreSQL and MySQL)
  - Security tests validating encryption strength, key protection, and audit trail integrity
  - Performance benchmarks ensuring encryption overhead remains acceptable
  - Edge case testing including key rotation failures, expired keys, and revoked key handling

## [1.6.0] - 2025-07-02

### Added
- **📡 Real-time Archive WebSocket Events**
  - Complete implementation of real-time archive operation events for enhanced web dashboard integration
  - `ArchiveEvent` enum with 6 event types: `JobArchived`, `JobRestored`, `BulkArchiveStarted`, `BulkArchiveProgress`, `BulkArchiveCompleted`, and `JobsPurged`
  - WebSocket integration in `hammerwork-web` with `publish_archive_event()` method for broadcasting archive events
  - Real-time progress tracking for bulk archive operations with unique operation IDs
  - Dashboard JavaScript handlers for live archive operation updates and notifications
  - CSS styling for archive progress bars and operation status notifications

- **🔧 Public Pool Field Access**
  - Made `JobQueue.pool` field public (was previously `pub(crate)`) for improved API ergonomics
  - Enables direct pool access for advanced use cases and better integration with external components
  - Added `get_pool()` method with comprehensive documentation and usage examples
  - Supports patterns like `JobArchiver::new(queue.pool.clone())` for sharing database connections

- **🧪 Comprehensive Test Coverage**
  - Added extensive test suite for archive WebSocket events including serialization, progress tracking, and error handling
  - Created `comprehensive_archive_tests.rs` with 300+ lines of tests covering edge cases and event publishing
  - Added `jobarchiver_pool_tests.rs` with tests for public pool field access patterns and multiple archiver scenarios
  - Comprehensive doctests for `JobArchiver::new()` demonstrating public pool usage patterns
  - Integration tests for archive events with real-time progress callbacks and operation tracking
  - Performance benchmarks for archive operations with 100+ job batches

### Enhanced
- **📈 Archive Operation Tracking**
  - Enhanced `JobArchiver` with progress tracking methods: `archive_jobs_with_progress()` and `archive_jobs_with_events()`
  - Real-time progress callbacks during bulk archive operations with `(current, total)` parameters
  - Operation ID generation for tracking concurrent archive operations
  - Event-driven architecture supporting custom event handlers and WebSocket integration
  - Improved `estimate_archival_jobs()` method using actual archival policies instead of simplifications

- **🎨 Dashboard User Experience**
  - Live archive operation notifications with job IDs, queue names, and archival reasons
  - Real-time progress bars showing completion percentage during bulk operations
  - Archive operation history with timestamps and statistics
  - Automatic data refresh when archive events are received
  - Enhanced visual feedback for archive, restore, and purge operations

### Fixed
- **🐛 Code Quality Improvements**
  - Removed all unnecessary `assert!(true)` calls from test files (7 instances across multiple files)
  - Fixed clippy warnings including field reassignment patterns and dead code warnings
  - Resolved compilation errors in integration tests and examples with proper import management
  - Fixed pointer dereference issues in archive event handling (`*estimated_jobs` → `estimated_jobs > &0`)
  - Added missing imports for `json!` macro, database traits, and standard library types across test files

- **🔧 Import Organization**
  - Standardized import organization across all test files with alphabetical ordering
  - Added missing `DatabaseQueue` trait imports for proper method access in integration tests
  - Fixed import paths for worker types, job handlers, and result storage components
  - Resolved examples compilation with proper UUID, Duration, and async imports

### Technical Improvements
- **🏗️ Architecture Enhancements**
  - Event-driven archive system with operation IDs for tracking concurrent operations
  - Bridge pattern implementation for WebSocket event publishing without modifying core archive operations
  - Comprehensive error handling for archive events with proper async patterns
  - Type-safe event serialization with serde support for JSON WebSocket transmission

- **⚡ Performance & Testing**
  - Archive operation benchmarks with timing measurements and performance assertions
  - Concurrent archive testing with multiple `JobArchiver` instances sharing database pools
  - Edge case testing including zero-job operations, invalid queue names, and error scenarios
  - Memory-efficient event tracking using `Arc<Mutex<Vec<Event>>>` patterns for concurrent access

## [1.5.2] - 2025-07-02

### Fixed
- **🔧 Migration System Improvements**
  - Added missing migration 010_add_archival to the migration registration system
  - Migration 010 was present as SQL files but not registered in the migration framework
  - Updated `docs/migrations.md` to accurately reflect all 10 available migrations
  - Fixed CLI command documentation to use correct `cargo hammerwork migration` syntax
  - Corrected all usage examples throughout migration documentation

### Enhanced
- **📖 Migration Documentation Accuracy**
  - Updated migration descriptions to match actual implementation:
    - 001_initial_schema - Create initial hammerwork_jobs table
    - 002_add_priority - Add priority field and indexes for job prioritization
    - 003_add_timeouts - Add timeout_seconds and timed_out_at fields
    - 004_add_cron - Add cron scheduling fields and indexes
    - 005_add_batches - Add batch processing table and job batch_id field
    - 006_add_result_storage - Add result storage fields for job execution results
    - 007_add_dependencies - Add job dependencies and workflow support
    - 008_add_result_config - Add result configuration storage fields
    - 009_add_tracing - Add distributed tracing and correlation fields
    - 010_add_archival - Add job archival support and archive table
  - Fixed CLI command examples to use `cargo hammerwork migration run` and `cargo hammerwork migration status`
  - Updated Docker and Kubernetes deployment examples with correct migration commands
  - Added `--drop` flag documentation for development scenarios

## [1.5.1] - 2025-07-02

### Added
- **📚 Complete Documentation Coverage**
  - Added missing documentation files linked from README:
    - `docs/tracing.md` - Comprehensive distributed tracing and correlation guide
    - `docs/workflows.md` - Job dependencies and workflow orchestration documentation
    - `docs/archiving.md` - Job archiving, retention, and compliance management guide
  - All documentation includes practical examples, configuration options, and best practices

### Enhanced
- **📖 Updated Quick Start Guide**
  - Completely redesigned `docs/quick-start.md` to match current v1.5.0 API
  - Updated import statements and module structure to reflect actual implementation
  - Added comprehensive examples for both PostgreSQL and MySQL
  - Included statistics collection, rate limiting, and monitoring examples
  - Added proper error handling patterns with `HammerworkError::Worker`
  - Demonstrated worker configuration with timeouts, retry policies, and rate limits
  - Added production considerations and environment setup guidance
  - Updated all code examples to use current builder patterns and configuration methods

- **🔧 Documentation Structure Improvements**
  - Enhanced navigation with proper cross-references between documentation files
  - Added prerequisite sections emphasizing database migration requirements
  - Improved code examples with realistic job processing scenarios
  - Added troubleshooting sections and performance considerations
  - Standardized documentation format across all files

### Fixed
- **🐛 Documentation Accuracy**
  - Corrected outdated API usage patterns in quick start examples
  - Fixed import paths to match current module organization
  - Updated job handler type signatures to match actual implementation
  - Corrected database setup instructions to use `cargo hammerwork migrate`
  - Fixed worker pool and statistics collector integration examples

## [1.5.0] - 2025-07-02

### Added
- **🔗 Comprehensive Event System & Webhook Integration**
  - Complete job lifecycle event system with real-time event publishing and subscription
  - `EventManager` for centralized event publishing with broadcast channels and filtering
  - `JobLifecycleEvent` struct with detailed job metadata, timestamps, and error tracking
  - Flexible `EventFilter` system for filtering by event type, queue, priority, processing time, and metadata
  - `EventSubscription` handles for receiving filtered events with async channels
  - Thread-safe event publishing integrated into job processing pipeline

- **📡 Production-Ready Webhook System**
  - `WebhookManager` for managing webhook configurations and delivery
  - Multiple authentication methods: Bearer tokens, Basic auth, API keys, custom headers
  - HMAC-SHA256 signature generation and verification for webhook security
  - Configurable retry policies with exponential backoff and jitter
  - Event filtering to deliver only relevant events to each webhook endpoint
  - Delivery tracking with comprehensive statistics and failure analysis
  - Rate limiting with configurable concurrent delivery limits

- **🌊 Advanced Event Streaming Integration**
  - `StreamManager` for delivering events to external message systems
  - Multi-backend support: Apache Kafka, AWS Kinesis, Google Cloud Pub/Sub
  - Flexible partitioning strategies: by job ID, queue name, priority, event type, or custom fields
  - Multiple serialization formats: JSON, Avro with schema registry, Protocol Buffers, MessagePack
  - Configurable buffering and batching for high-throughput scenarios
  - Stream-specific retry policies and health monitoring
  - Per-stream statistics tracking and global metrics

- **⚙️ Enhanced Configuration System**
  - Extended `HammerworkConfig` with webhook and streaming configurations
  - TOML-based configuration with environment variable overrides
  - `WebhookConfig` and `StreamConfig` for individual endpoint configuration
  - Global settings for webhook and streaming behavior
  - Development and production configuration presets
  - Configuration validation and error handling

- **🛠️ CLI Webhook & Streaming Management**
  - Complete CLI integration in `cargo-hammerwork` for webhook management:
    - `webhook list` - List configured webhooks with status and statistics
    - `webhook test` - Test webhook deliveries with sample events
    - `webhook enable/disable` - Control webhook activation
    - `webhook stats` - Detailed webhook delivery statistics
  - Streaming management commands:
    - `stream list` - List configured streams with backend information
    - `stream test` - Test stream connectivity and delivery
    - `stream stats` - Stream-specific delivery statistics
  - Enhanced monitoring commands with event system integration

### Enhanced
- **📈 Event-Driven Monitoring & Alerting**
  - Webhook and streaming integration with existing metrics and alerting systems
  - Real-time event delivery for external monitoring systems
  - Event-based triggers for alerting and notification systems
  - Integration with Prometheus metrics for webhook and streaming statistics

- **🧪 Comprehensive Testing & Documentation**
  - Complete test suite for event system, webhooks, and streaming (200+ new tests)
  - Extensive documentation with doctests and practical examples
  - Security testing for HMAC signature validation and authentication
  - Integration tests with mock HTTP servers and message systems
  - Performance testing for high-throughput event delivery scenarios

- **📖 Enhanced Documentation**
  - Comprehensive module-level documentation with architecture overviews
  - Detailed examples for webhook authentication, HMAC signatures, and retry policies
  - Streaming configuration examples for Kafka, Kinesis, and Pub/Sub
  - Event filtering and partitioning strategy examples
  - Updated main library documentation with event system integration examples

### Technical Implementation
- **Event Architecture**: Publish/subscribe pattern with broadcast channels and async event delivery
- **Webhook Security**: HMAC-SHA256 signatures with configurable secrets and verification
- **Stream Processing**: Async batch processing with configurable buffer sizes and flush intervals
- **Configuration Management**: Hierarchical configuration with feature flags and environment variables
- **Database Integration**: Event metadata stored in job payloads for correlation and debugging
- **Error Handling**: Comprehensive error types with detailed error messages and retry logic
- **Performance**: Optimized for high-throughput scenarios with concurrent processing limits

### Breaking Changes
- None - all webhook and streaming functionality is additive and backward compatible

## [1.4.0] - 2025-07-01

### Added
- **🚀 Dynamic Job Spawning System**
  - Complete trait-based spawn system with `SpawnHandler`, `SpawnManager`, and `SpawnConfig`
  - Jobs can dynamically create child jobs during execution with configurable spawning rules
  - Fan-out processing patterns: single jobs spawning multiple workers for parallel processing
  - Parent-child job relationships with dependency tracking and lineage management
  - Spawn operation monitoring with statistics and performance tracking
  - Configurable spawn limits, inheritance rules, and batch processing capabilities

- **🔧 Comprehensive CLI Spawn Management**
  - Six new CLI commands in `cargo-hammerwork` for complete spawn operation management:
    - `spawn list` - List active spawn operations with filtering and queue-specific views
    - `spawn tree` - Visualize spawn hierarchies in text, JSON, or Mermaid formats
    - `spawn stats` - Detailed spawn statistics with queue breakdowns and time-based analysis
    - `spawn lineage` - Track ancestor and descendant chains for any job
    - `spawn pending` - Monitor jobs awaiting spawn execution with configuration details
    - `spawn monitor` - Real-time monitoring of spawn operations with auto-refresh
  - Full MySQL and PostgreSQL support with database-specific optimized queries
  - Multiple output formats: human-readable text, structured JSON, Mermaid diagrams

- **🌐 Web API Spawn Endpoints**
  - RESTful API endpoints for spawn operations management via `hammerwork-web`
  - Spawn tree visualization API with hierarchical data structures
  - Spawn statistics API for monitoring and dashboard integration
  - Parent-child relationship tracking with metadata support

### Enhanced
- **⚡ Database Compatibility & Performance**
  - PostgreSQL implementation using `@>` operator and JSONB queries for optimal performance
  - MySQL implementation using `JSON_CONTAINS()` and `JSON_EXTRACT()` functions
  - Cross-database spawn operation queries with proper parameterization for security
  - Efficient dependency tracking using database-native JSON operations

- **🧪 Comprehensive Testing Suite**
  - 15 new unit tests for CLI spawn commands covering all functionality paths
  - 7 SQL query integration tests validating both PostgreSQL and MySQL implementations
  - Spawn-specific example (`spawn_cli_example.rs`) demonstrating real-world usage patterns
  - Comprehensive edge case testing for spawn tree structures and complex hierarchies

### Technical Implementation
- New `spawn` module with complete trait-based architecture for extensible spawn handlers
- Enhanced `Worker` integration with automatic spawn execution during job completion
- `JobSpawnExt` trait for adding spawn capabilities to existing job structures
- Spawn operation tracking with operation IDs, timestamps, and success/failure metrics
- Database schema support for spawn configurations stored in job payload metadata

### Breaking Changes
- None - all spawn functionality is additive and backward compatible

## [1.3.0] - 2025-07-01

### Added
- **🗄️ Job Archiving & Retention System**
  - Policy-driven job archival with configurable retention periods per job status
  - Payload compression using gzip for efficient long-term storage
  - Archive table (`hammerwork_jobs_archive`) with compressed payloads
  - Restore archived jobs back to pending status when needed
  - Purge old archived jobs for compliance requirements (GDPR, data retention)
  - Comprehensive archival statistics and monitoring
  - CLI commands for archive management in `cargo-hammerwork`

### Enhanced
- **⚡ Archive Management Features**
  - Automatic archival based on job age and status (Completed, Failed, Dead, TimedOut)
  - Configurable compression levels (0-9) with integrity verification
  - Batch processing for efficient large-scale archival operations
  - Archive metadata tracking (reason, timestamp, who initiated)
  - List and search archived jobs with pagination support
  - Database schema migration (010) for archive table setup

### Technical Implementation
- New `archive` module with `ArchivalPolicy`, `ArchivalConfig`, and `JobArchiver` types
- Extended `DatabaseQueue` trait with archival methods for all database backends
- PostgreSQL and MySQL implementations with optimized archive queries
- Comprehensive doctests and unit tests for all archival functionality
- Archive CLI commands: `archive`, `restore`, `list-archived`, `purge-archived`

## [1.2.2] - 2025-07-01

### Added
- **🧪 Comprehensive TestQueue Implementation**
  - Complete in-memory test implementation of DatabaseQueue trait for unit testing
  - MockClock for deterministic time-based testing of delayed jobs and cron schedules
  - Full support for all queue operations: enqueue, dequeue, batch operations, workflows, cron jobs
  - Priority-aware job selection with weighted and strict priority algorithms
  - Job dependency management and workflow execution testing
  - Comprehensive statistics and monitoring capabilities for test scenarios

### Enhanced
- **⚡ TestQueue Feature Completeness**
  - Delayed job execution with time control using MockClock
  - Batch operations with PartialFailureMode support (ContinueOnError, FailFast)
  - Workflow dependency resolution and cancellation policies
  - Cron job scheduling with timezone support (6-field cron expressions)
  - Dead job management with purging and retry capabilities
  - Job result storage with expiration and cleanup
  - Throttling configuration management
  - Queue statistics with accurate failed/dead job counting

### Fixed
- **🐛 TestQueue Core Functionality**
  - Fixed job scheduling timestamp logic to use MockClock consistently across all enqueue methods
  - Fixed retry logic off-by-one error where jobs required 4 failures instead of 3 to become dead
  - Fixed workflow fail-fast policy implementation to automatically fail related jobs
  - Fixed workflow status preservation for cancelled workflows
  - Fixed dead job purging time comparison logic (changed from `<` to `<=`)
  - Fixed cron job scheduling to use 6-field format with seconds
  - Fixed queue statistics to count Dead jobs as failed for reporting purposes

### Technical Implementation
- All 19 TestQueue integration tests now passing
- Proper mock clock integration across enqueue, batch, and workflow operations  
- Fail-fast logic implementation for both workflows and batches
- Comprehensive error handling and status management
- Support for complex job dependency graphs and workflow execution
- Full compatibility with DatabaseQueue trait for drop-in testing

## [1.2.1] - 2025-06-30

### Fixed
- **🐛 MySQL Query Field Completeness**
  - Fixed `Database(ColumnNotFound("trace_id"))` errors in MySQL dequeue operations
  - Updated MySQL `dequeue()` and `dequeue_with_priority_weights()` queries to include all tracing fields: `trace_id`, `correlation_id`, `parent_span_id`, `span_context`
  - Ensures JobRow struct mapping works correctly with all database schema fields added in migration 009_add_tracing.mysql.sql
  - Fixed two failing tests: `test_mysql_dequeue_includes_all_fields` and `test_mysql_dequeue_with_priority_weights_includes_all_fields`

### Enhanced
- **🧪 Test Infrastructure Improvements** 
  - Improved test isolation using unique queue names to prevent test interference
  - Fixed race conditions in result storage tests by implementing proper job completion polling
  - Enhanced test database setup to use migration-based approach ensuring schema consistency
  - Fixed 6 failing doctests in worker.rs by correcting async/await usage in documentation examples

### Technical Implementation
- MySQL dequeue queries now SELECT all 34 fields required by JobRow struct mapping
- Complete field list includes: id, queue_name, payload, status, priority, attempts, max_attempts, timeout_seconds, created_at, scheduled_at, started_at, completed_at, failed_at, timed_out_at, error_message, cron_schedule, next_run_at, recurring, timezone, batch_id, result_data, result_stored_at, result_expires_at, result_storage_type, result_ttl_seconds, result_max_size_bytes, depends_on, dependents, dependency_status, workflow_id, workflow_name, trace_id, correlation_id, parent_span_id, span_context
- All tests now passing: 228 unit tests, 135 doctests, 0 failures

## [1.2.0] - 2025-06-29

### Added
- **🔍 Job Tracing & Correlation** - Comprehensive distributed tracing system for production observability
  - **Core Tracing Fields**: Added `trace_id`, `correlation_id`, `parent_span_id`, and `span_context` fields to Job struct
  - **Database Migrations**: New migration 009_add_tracing for PostgreSQL and MySQL with optimized indexes for trace/correlation ID lookups
  - **Job Builder Methods**: Added `.with_trace_id()`, `.with_correlation_id()`, `.with_parent_span_id()`, and `.with_span_context()` for easy job tracing configuration
  - **TraceId and CorrelationId Types**: New strongly-typed identifiers with generation, conversion, and validation methods
  - **OpenTelemetry Integration**: Feature-gated OpenTelemetry support with OTLP export to Jaeger, Zipkin, DataDog, etc.
  - **TracingConfig**: Complete OpenTelemetry configuration with service metadata, resource attributes, and endpoint configuration
  - **Automatic Span Creation**: `create_job_span()` function creates spans with rich job metadata and trace context propagation
  - **Span Context Management**: `set_job_trace_context()` for extracting and storing trace context from OpenTelemetry spans

- **🎯 Worker Event Hooks** - Lifecycle event system for custom tracing and monitoring integration
  - **JobHookEvent**: Event data structure with job metadata, timestamps, duration, and error information
  - **JobEventHooks**: Configurable lifecycle callbacks for job start, completion, failure, timeout, and retry events
  - **Builder Pattern**: Convenient `.on_job_start()`, `.on_job_complete()`, `.on_job_fail()`, `.on_job_timeout()`, `.on_job_retry()` methods
  - **Worker Integration**: Event hooks integrated into job processing pipeline with automatic event firing
  - **Automatic Span Management**: OpenTelemetry spans automatically created and updated throughout job lifecycle

- **⚡ Production-Ready Tracing Infrastructure**
  - **Feature Gated**: All tracing functionality behind optional `tracing` feature flag for minimal overhead
  - **Backward Compatible**: Existing jobs and workers continue working unchanged
  - **Database Optimized**: Indexed trace and correlation ID columns for efficient querying
  - **OpenTelemetry Standards**: Full OTLP support with configurable exporters and sampling
  - **Span Attributes**: Rich span metadata including job ID, queue name, priority, status, and custom business data
  - **Error Tracking**: Automatic span status updates for success, failure, and timeout scenarios

- **🧪 Comprehensive Testing** - 177 total tests including 22 new tracing-specific tests
  - **Unit Tests**: Complete coverage of TraceId, CorrelationId, TracingConfig, and span creation functionality
  - **Integration Tests**: Event hook testing with realistic job processing scenarios
  - **Feature Testing**: Validation of tracing feature flag behavior and optional inclusion
  - **Span Testing**: OpenTelemetry span creation and attribute validation

### Enhanced
- **📖 Documentation Updates**
  - **README.md**: Added Job Tracing & Correlation feature to main features list with comprehensive example
  - **Installation Guide**: Updated to show tracing feature installation options
  - **Tracing Example**: Complete OpenTelemetry setup example with worker event hooks and correlation tracking
  - **Feature Flags**: Updated to include `tracing` (optional) feature flag documentation
  - **Database Schema**: Updated schema documentation to mention distributed tracing fields

- **🗺️ ROADMAP.md**: Removed completed "Job Tracing & Correlation" feature from Phase 1 priorities

### Technical Implementation
- **OpenTelemetry Dependencies**: Added feature-gated dependencies: `opentelemetry`, `opentelemetry_sdk`, `opentelemetry-otlp`, `tracing-opentelemetry`
- **Async Integration**: Full async/await support with tokio runtime integration
- **Memory Efficient**: Trace context stored as optional strings with minimal memory overhead
- **Type Safety**: Strongly typed trace and correlation IDs with comprehensive validation
- **Database Agnostic**: Tracing works identically across PostgreSQL and MySQL backends
- **Export Support**: OTLP export to all major observability platforms (Jaeger, Zipkin, DataDog, New Relic, etc.)

### Usage Example
```rust
// Initialize tracing
let config = TracingConfig::new()
    .with_service_name("job-processor")
    .with_otlp_endpoint("http://jaeger:4317");
init_tracing(config).await?;

// Create traced jobs  
let job = Job::new("email_queue".to_string(), json!({"to": "user@example.com"}))
    .with_trace_id("trace-123")
    .with_correlation_id("order-456");

// Worker with event hooks
let worker = Worker::new(queue, "email_queue".to_string(), handler)
    .on_job_start(|event| { /* custom tracing logic */ })
    .on_job_complete(|event| { /* success tracking */ });
```

This release provides comprehensive distributed tracing capabilities essential for debugging and monitoring job processing in production distributed systems.

## [1.1.0] - 2025-06-29

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
  - `depends_on()` and `depends_on_jobs()` builder methods for job dependencies
  - `with_workflow()` method to associate jobs with workflows

- **🛠️ Comprehensive CLI Architecture**
  - Full implementation of all cargo-hammerwork commands:
    - Job management (list, show, enqueue, retry, cancel, delete)
    - Worker control (start, stop, status, restart)
    - Queue operations (list, stats, clear, pause, resume)
    - Monitoring (dashboard, health, metrics, logs)
    - Batch operations (create, status, retry, cancel)
    - Cron scheduling (list, add, remove, enable, disable)
    - Database maintenance (cleanup, vacuum, analyze, health)
    - Backup & restore (create, restore, list, verify)
  - Modular command structure with dedicated modules for each feature area
  - Professional table formatting and display utilities
  - Comprehensive error handling and validation

### Fixed
- **🐛 Workflow Dependencies Storage**
  - Fixed PostgreSQL and MySQL `enqueue` methods to properly store workflow fields (`depends_on`, `dependents`, `dependency_status`, `workflow_id`, `workflow_name`)
  - Corrected `dependency_status` serialization to use `.as_str()` instead of JSON serialization to match database constraints
  - Jobs with dependencies are now correctly stored and retrieved from the database

- **📚 Documentation Tests**
  - Fixed all 26 failing doc tests by correcting Duration API usage (`from_minutes` → `from_secs`)
  - Added missing async contexts to doc test examples
  - Added missing `DatabaseQueue` trait imports where needed
  - Removed references to deprecated `create_tables()` method

- **🧪 Test Isolation**
  - Improved test isolation by using unique queue names with UUIDs to prevent intermittent failures
  - Fixed test race conditions in workflow dependency tests

- **🧹 Code Quality Improvements**
  - Removed unused imports in migration modules (postgres.rs, mysql.rs)
  - Fixed unused variable warnings in examples
  - Cleaned up compilation warnings across the codebase

### Enhanced
- **📖 Documentation Updates**
  - Updated main README with comprehensive workflow examples and job dependency documentation
  - Enhanced ROADMAP.md marking job dependencies and workflows as completed features
  - Added workflow documentation section with pipeline examples and synchronization barriers
  - Updated cargo-hammerwork README with complete command documentation for all features
  - Added complete command documentation for job, worker, queue, monitor, batch, cron, maintenance, workflow, and backup commands
  - Updated feature lists to accurately reflect all implemented functionality

### Technical Implementation
- **CLI Architecture**: Comprehensive command structure with database integration for all operations
- **Visualization**: Multi-format graph output (text, DOT, Mermaid, JSON) for diverse integration needs
- **Dependencies**: Complete dependency graph algorithms with cycle detection and level calculation
- **Professional Output**: Bootstrap-inspired color schemes and professional formatting for all CLI output

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
