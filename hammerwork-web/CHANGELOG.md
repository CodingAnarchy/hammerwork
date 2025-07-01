# Changelog

All notable changes to the `hammerwork-web` crate will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [1.3.0] - 2025-07-01

### Added
- **🗄️ Job Archive Management**: Complete web interface for job archiving and retention
  - Archive API endpoints for archiving, restoring, and purging jobs
  - Interactive archive section in dashboard with statistics cards
  - Archive jobs table with pagination and filtering capabilities
  - Archive modal for configuring archival policies and executing operations
  - Archive statistics modal with detailed metrics and recent operations
  - Job restoration functionality with audit trails
- **📊 Archive Statistics**: Real-time archive metrics and monitoring
  - Total archived jobs counter
  - Storage saved through compression tracking
  - Last archive operation timestamp
  - Compression ratio statistics
  - Recent operations history
- **🎨 Enhanced UI Components**: Archive-specific styling and interactions
  - Color-coded archival reason badges (Automatic, Manual, Compliance, Maintenance)
  - Compression status indicators with visual feedback
  - Responsive grid layouts for archive statistics
  - Enhanced modal dialogs for complex archive operations
- **⚙️ Archive Policy Configuration**: Web-based policy management
  - Configurable retention periods for different job statuses
  - Compression settings with integrity verification options
  - Dry run capability for testing archival operations
  - Batch size configuration for large-scale operations
- **🔄 Seamless Integration**: Archive functionality integrated into existing dashboard
  - Automatic data refresh when archive operations complete
  - Consistent authentication and error handling
  - Queue population from existing dashboard data
  - Real-time filtering and pagination controls

### Enhanced
- **API Documentation**: Added comprehensive documentation for archive endpoints
- **JavaScript Architecture**: Extended dashboard class with archive management methods
- **CSS Framework**: Added archive-specific styling with consistent design patterns
- **Error Handling**: Enhanced user notifications for archive operations
- **Workspace Dependencies**: Migrated to workspace-based dependency management

### Fixed
- Type system conflicts when both PostgreSQL and MySQL features are enabled
- Base64 credential parsing in authentication module
- Missing Serialize trait implementation for CreateJobRequest
- Conditional compilation issues with database feature flags

### Changed
- Enhanced module documentation with comprehensive usage examples
- Improved authentication module with better error handling
- Updated server module to handle multiple database backends correctly
- Migrated to workspace version and metadata inheritance

## [1.2.0] - 2024-06-30

### Added
- Initial release of hammerwork-web dashboard
- REST API endpoints for job and queue management
- Real-time WebSocket support for dashboard updates
- Authentication middleware with basic auth and rate limiting
- Modern HTML/CSS/JS frontend for job queue monitoring
- Configuration management with TOML support
- Database support for both PostgreSQL and MySQL
- Comprehensive system administration features

### Features
- **Web Dashboard**: Modern real-time web interface for monitoring queues, managing jobs, and system administration
- **REST API**: Complete API for job and queue management operations
- **WebSocket Support**: Real-time updates for dashboard components
- **Authentication**: Secure access with configurable authentication
- **Multi-Database**: Support for both PostgreSQL and MySQL backends
- **Configuration**: Flexible TOML-based configuration system