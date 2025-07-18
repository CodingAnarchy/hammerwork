# Changelog

All notable changes to the `hammerwork-web` crate will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- **🔔 WebSocket Subscription Management**
  - Implemented per-connection subscription tracking for targeted event delivery
  - Added subscription state management with HashSet for efficient event filtering
  - New `broadcast_to_subscribed()` method for sending messages only to interested clients
  - Automatic cleanup of subscriptions when clients disconnect
  - Event type categorization: "queue_updates", "job_updates", "system_alerts", "archive_events"

- **📊 Enhanced Statistics and Monitoring**
  - Real-time uptime tracking using SystemState's `started_at` timestamp
  - Improved job listing with comprehensive data from all job sources (ready, dead, recurring)
  - Enhanced search functionality querying across all job types and statuses
  - Better pagination with accurate total count estimation for archived jobs

### Fixed
- **🔧 Database Query Implementations**
  - Implemented `get_last_job_time()` using actual job queries instead of placeholder
  - Implemented `get_oldest_pending_job()` with proper pending job filtering
  - Implemented `get_recent_errors()` by querying dead jobs with error messages
  - Added missing `get_priority_stats()` method in TestQueue for trait compliance

- **📡 WebSocket Broadcast Processing**
  - Fixed broadcast message delivery by implementing proper state access in `start_broadcast_listener`
  - Integrated broadcast listener into server startup sequence
  - Messages now properly route to subscribed clients based on event types
  - Replaced placeholder TODO with fully functional broadcast implementation

### Enhanced
- **🔍 Archive Operations**
  - Improved dry run estimation using actual job count queries
  - Enhanced statistics collection with per-queue stats when no filter applied
  - Added mock recent operations for better UI representation
  - Better error messages for operations requiring additional DatabaseQueue methods

- **⚙️ Configuration Detection**
  - Metrics feature detection now uses compile-time `cfg!` macro
  - Added helper functions for custom metrics counting and scrape time tracking
  - Improved error handling with more descriptive messages

## [1.11.0] - 2025-07-14

### Added
- **⏸️ Queue Pause/Resume Management**
  - Interactive queue pause and resume controls in the web dashboard
  - Visual queue status indicators with color-coded badges: 🟢 Active, 🟡 Paused
  - Dynamic action buttons that change based on queue state (Pause ↔ Resume)
  - Real-time status updates when queues are paused or resumed
  - Queue status column added to the main queues table for operational visibility

- **🎨 Enhanced User Interface**
  - New status badge styling for queue states with proper visual distinction
  - Success/warning button styles for pause/resume operations
  - Improved queue table layout with better information density
  - Immediate user feedback with success/error notifications
  - Responsive design ensuring controls work across different screen sizes

- **🌐 API Integration**
  - Enhanced queue API responses including pause state information
  - Updated queue action endpoints supporting pause/resume operations
  - Extended queue information with `is_paused`, `paused_at`, and `paused_by` fields
  - Improved error handling and user feedback for all queue management operations

### Enhanced
- **📊 Queue Management**
  - Queue table now displays comprehensive status information
  - Better visual distinction between active and paused queues
  - Enhanced user experience with contextual action buttons
  - Improved accessibility with clear status indicators

## [1.7.3] - 2025-07-05

### Fixed
- Fixed compilation error on Linux systems in memory usage detection for system metrics
  - Changed pattern matching from `Ok(kb)` to `Some(kb)` for Option type
  - Added `.ok()` conversion after `parse::<u64>()` to properly handle Result to Option conversion

## [1.5.0] - 2025-07-02

### Added
- **🔗 Webhook Management Interface**: Complete web-based webhook configuration and monitoring
  - Webhook configuration dashboard with CRUD operations for webhook endpoints
  - Authentication method selection (Bearer, Basic, API Key, Custom Headers)
  - HMAC signature configuration with secret management
  - Event filtering interface for targeting specific job lifecycle events
  - Real-time webhook delivery statistics and success rate monitoring
  - Webhook testing interface with sample event delivery
  - Delivery history tracking with error analysis and retry attempts

- **🌊 Event Streaming Dashboard**: Comprehensive streaming configuration and monitoring
  - Stream configuration interface for Kafka, AWS Kinesis, and Google Pub/Sub
  - Backend-specific configuration panels with connection testing
  - Partitioning strategy selection (Job ID, Queue Name, Priority, Custom)
  - Serialization format configuration (JSON, Avro, Protobuf, MessagePack)
  - Stream health monitoring with connectivity status indicators
  - Real-time stream delivery metrics and throughput statistics
  - Buffer configuration interface with performance tuning options

- **📊 Event System Integration**: Real-time event monitoring across the dashboard
  - Live event feed displaying job lifecycle events as they occur
  - Event filtering controls for monitoring specific queues, priorities, or event types
  - Webhook delivery status integration in job detail views
  - Stream delivery tracking with partition and serialization information
  - Event correlation between jobs and external system deliveries

- **⚙️ Enhanced Configuration Management**: Extended configuration interface
  - Webhook and streaming configuration sections in settings panel
  - Environment variable management for secrets and credentials
  - Configuration validation with connectivity testing
  - Import/export functionality for webhook and stream configurations
  - Configuration templates for common webhook and streaming patterns

- **🎨 Advanced UI Components**: New dashboard components for event system management
  - Webhook configuration modals with step-by-step setup wizards
  - Stream configuration panels with backend-specific field validation
  - Real-time delivery status indicators with color-coded health states
  - Event timeline visualization showing job lifecycle progression
  - Statistics cards for webhook and streaming performance metrics
  - Interactive charts for delivery success rates and error patterns

### Enhanced
- **🔄 Real-time Updates**: Extended WebSocket integration for event system updates
  - Live webhook delivery notifications with success/failure indicators
  - Real-time stream delivery metrics updates
  - Event system health status updates
  - Configuration change notifications across dashboard instances

- **📈 Monitoring and Analytics**: Enhanced monitoring capabilities
  - Webhook delivery analytics with time-series charts
  - Stream throughput monitoring with partition-level details
  - Event system performance metrics integration
  - Error rate tracking and alerting for delivery failures
  - Historical data retention for trend analysis

- **🛡️ Security Enhancements**: Improved security features for external integrations
  - Secure credential storage for webhook authentication
  - HMAC signature validation testing interface
  - Stream credential encryption and management
  - Authentication token rotation support
  - Audit logging for configuration changes

- **🎯 User Experience**: Improved dashboard usability and workflow
  - Guided setup wizards for webhook and streaming configuration
  - In-context help and documentation links
  - Configuration validation with real-time feedback
  - Bulk operations for managing multiple webhooks and streams
  - Quick action buttons for common operations (test, enable/disable)

### Technical Implementation
- **API Extensions**: New REST endpoints for webhook and streaming management
  - `/api/webhooks/*` - Complete webhook CRUD and management operations
  - `/api/streams/*` - Stream configuration and monitoring endpoints
  - `/api/events/*` - Event system status and real-time event streaming
  - Enhanced WebSocket channels for real-time event delivery updates

- **Frontend Architecture**: Extended dashboard with modular event system components
  - Webhook management module with configuration and monitoring views
  - Streaming dashboard module with multi-backend support
  - Event timeline component with filtering and correlation features
  - Reusable configuration forms with validation and testing capabilities

- **Database Integration**: Event system data integration with existing dashboard queries
  - Webhook delivery history storage and retrieval
  - Stream delivery statistics aggregation
  - Event correlation data for job-to-delivery tracking
  - Configuration persistence with versioning support

### Breaking Changes
- None - all webhook and streaming functionality is additive to existing dashboard features

## [1.4.0] - 2025-07-01

### Added
- **🚀 Dynamic Job Spawning Web Interface**: Complete web-based spawn operation management
  - Interactive spawn tree visualization with hierarchical job relationships
  - Real-time spawn operation monitoring with auto-refresh capabilities
  - Spawn statistics dashboard with parent-child relationship tracking
  - Visual spawn lineage browser for tracking job dependencies
  - Spawn configuration interface for setting up dynamic job creation

### Enhanced
- **📊 Spawn Analytics**: Advanced spawn operation analytics and reporting
  - Spawn success rate monitoring with failure pattern analysis
  - Performance metrics for spawn operation execution times
  - Resource utilization tracking for spawned job processing
  - Historical spawn data with trend analysis

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