# Changelog - cargo-hammerwork

All notable changes to the cargo-hammerwork CLI tool will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [1.5.0] - 2025-07-02

### Added
- **🔗 Webhook Management Commands**
  - `webhook list` - List all configured webhooks with status, delivery stats, and configuration details
  - `webhook test` - Test webhook endpoints with sample job lifecycle events
  - `webhook enable` / `webhook disable` - Control webhook activation and deactivation
  - `webhook stats` - Detailed webhook delivery statistics with success rates and error analysis
  - `webhook add` - Add new webhook configurations with authentication and filtering options
  - `webhook remove` - Remove webhook configurations by ID or name
  - `webhook show` - Display detailed configuration and statistics for specific webhooks

- **🌊 Event Streaming Management Commands**
  - `streaming list` - List all configured event streams with backend information and status
  - `streaming test` - Test stream connectivity and event delivery to external systems
  - `streaming stats` - Stream-specific delivery statistics and performance metrics
  - `streaming add` - Add new streaming configurations for Kafka, Kinesis, or Pub/Sub
  - `streaming remove` - Remove streaming configurations by ID or name
  - `streaming show` - Display detailed stream configuration and delivery statistics

- **📊 Enhanced Monitoring Integration**
  - Event system integration in existing monitor commands
  - Real-time webhook delivery monitoring with `monitor --webhooks` flag
  - Stream delivery monitoring with `monitor --streams` flag
  - Event filtering options in monitoring commands for specific event types
  - Webhook and streaming health checks in `monitor health` command

- **🔧 Configuration Management Enhancements**
  - `config show` command extended to display webhook and streaming configurations
  - `config validate` command includes webhook URL validation and stream connectivity checks
  - `config export` / `config import` support for webhook and streaming configurations
  - Environment variable support for webhook secrets and stream credentials

### Enhanced
- **📈 Statistics and Reporting**
  - Queue statistics commands now include event publishing metrics
  - Job lifecycle commands show related webhook deliveries and stream events
  - Enhanced error reporting with webhook delivery failures and stream connectivity issues
  - Performance metrics include event system overhead and delivery latencies

- **🛡️ Security and Validation**
  - HMAC signature validation testing in webhook test commands
  - Authentication method validation for webhook configurations
  - Stream credential validation and connectivity testing
  - Enhanced input validation for webhook URLs and stream endpoints

- **🎨 Output Formatting**
  - Rich table formatting for webhook and streaming configuration displays
  - Color-coded status indicators for webhook delivery health and stream connectivity
  - JSON output support for webhook and streaming statistics for programmatic access
  - Enhanced error messages with actionable troubleshooting information

### Technical Implementation
- **Database Integration**: All webhook and streaming commands use the same database connection patterns as existing CLI commands
- **Security**: HMAC signature generation and validation using the same cryptographic libraries as the core library
- **Error Handling**: Comprehensive error handling with specific error types for webhook and streaming operations
- **Testing**: Complete test coverage for all new commands with mock HTTP servers and stream endpoints

### Breaking Changes
- None - all webhook and streaming functionality is additive to existing CLI commands

## [1.4.0] - 2025-07-01

### Added
- **🚀 Dynamic Job Spawning CLI Commands**
  - Complete `spawn` command suite with list, tree, stats, lineage, pending, and monitor subcommands
  - Visual spawn hierarchy generation in text, JSON, and Mermaid formats
  - Real-time spawn operation monitoring with auto-refresh capabilities
  - Spawn lineage tracking and dependency visualization
  - Cross-database support for both PostgreSQL and MySQL with optimized queries

### Enhanced
- **🔧 Database Query Optimization**
  - Spawn-specific database queries with proper parameterization
  - Enhanced error handling and validation for spawn operations
  - Performance improvements for large spawn hierarchies

## [1.3.0] - 2025-07-01

### Added
- **🗄️ Job Archiving CLI Commands**
  - `archive` command for archiving completed, failed, and dead jobs
  - `restore` command for restoring archived jobs back to pending status
  - `list-archived` command for browsing archived jobs with pagination
  - `purge-archived` command for permanent deletion of old archived jobs
  - Archive statistics and compression ratio reporting

### Enhanced
- **📊 Archive Management**
  - Archive policy configuration and validation
  - Compression level settings and integrity verification
  - Batch archiving operations for efficient processing

## [1.2.0] - 2025-06-29

### Added
- **🎨 Comprehensive Workflow Management CLI**
  - Complete `workflow` command suite with list, show, create, cancel, dependencies, and graph subcommands
  - Visual workflow dependency graph generation in multiple formats (text, DOT, Mermaid, JSON)
  - Professional Mermaid graph output with color-coded status indicators
  - Workflow lifecycle management with configurable failure policies

- **🛠️ Complete CLI Architecture**
  - Full implementation of all cargo-hammerwork commands across all feature areas
  - Modular command structure with dedicated modules for each functionality
  - Professional table formatting and comprehensive error handling

### Enhanced
- **📖 Documentation and Examples**
  - Complete command documentation for all features
  - Updated README with comprehensive command reference
  - Enhanced help text and usage examples

## [1.1.0] - 2025-06-29

### Added
- **🔧 Database Migration Management**
  - `migrate` command for running database schema migrations
  - `status` command for checking migration status and history
  - Progressive schema evolution with versioned migrations
  - Database-specific optimizations and migration tracking

### Enhanced
- **💾 Job Result Storage CLI**
  - Enhanced job commands to display and manage job results
  - Result storage statistics in queue and job statistics commands
  - TTL and expiration management for stored results

## [1.0.0] - 2025-06-27

### Added
- **📊 Core CLI Infrastructure**
  - Complete job management commands (list, show, enqueue, retry, cancel, delete)
  - Worker control commands (start, stop, status, restart)
  - Queue operations (list, stats, clear, pause, resume)
  - Monitoring dashboard and health checks
  - Batch operations management
  - Cron scheduling commands
  - Database maintenance and backup/restore functionality

### Technical Implementation
- **Cross-Database Support**: Full PostgreSQL and MySQL compatibility
- **Professional Output**: Rich table formatting and status indicators
- **Error Handling**: Comprehensive validation and error reporting
- **Configuration**: TOML-based configuration with environment variable support

---

## Contributing
Please see the main [CONTRIBUTING.md](../CONTRIBUTING.md) for details on our development process.

## License
This project is licensed under the MIT License - see the [LICENSE-MIT](../LICENSE-MIT) file for details.