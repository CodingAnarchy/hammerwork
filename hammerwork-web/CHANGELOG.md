# Changelog

All notable changes to the `hammerwork-web` crate will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- Comprehensive doctests for all major modules to improve docs.rs documentation
- Base64 dependency for authentication credential parsing
- Complete API documentation with practical examples for:
  - Job management endpoints (`api::jobs`)
  - Queue management endpoints (`api::queues`) 
  - Statistics and monitoring endpoints (`api::stats`)
  - System administration endpoints (`api::system`)
- Configuration examples with builder pattern usage
- Authentication system documentation with security examples
- WebSocket implementation examples for real-time communication
- Server setup and management examples

### Fixed
- Type system conflicts when both PostgreSQL and MySQL features are enabled
- Base64 credential parsing in authentication module
- Missing Serialize trait implementation for CreateJobRequest
- Conditional compilation issues with database feature flags

### Changed
- Enhanced module documentation with comprehensive usage examples
- Improved authentication module with better error handling
- Updated server module to handle multiple database backends correctly

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