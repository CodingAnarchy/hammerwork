# Integration Testing Guide

This guide covers how to run and maintain integration tests for the Hammerwork job queue library.

## Overview

Hammerwork includes comprehensive integration testing infrastructure that validates:

- ✅ Job queue functionality with real databases
- ✅ PostgreSQL and MySQL database compatibility
- ✅ Worker pool management and job processing
- ✅ Performance benchmarks and regression testing
- ✅ Containerized deployment scenarios
- ✅ End-to-end job lifecycle management

## Quick Start

### Prerequisites

- **Docker & Docker Compose**: For database containers
- **Rust 1.75+**: For building and running tests
- **Make** (optional): For convenient command execution

### Run All Tests Locally

```bash
# Start databases and run all integration tests
make integration-all

# Or using the script directly
./scripts/test-local.sh
```

### Run Specific Database Tests

```bash
# PostgreSQL only
make integration-postgres
# or
./scripts/test-postgres.sh

# MySQL only
make integration-mysql
# or
./scripts/test-mysql.sh
```

## Test Infrastructure

### Directory Structure

```
hammerwork/
├── docker-compose.yml           # Database containers
├── Makefile                     # Convenient commands
├── scripts/
│   ├── test-local.sh           # Main test runner
│   ├── test-postgres.sh        # PostgreSQL tests
│   ├── test-mysql.sh           # MySQL tests
│   ├── setup-db.sh             # Database management
│   └── cleanup.sh              # Environment cleanup
├── config/
│   ├── init-postgres.sql       # PostgreSQL initialization
│   └── init-mysql.sql          # MySQL initialization
├── integrations/
│   ├── shared/
│   │   └── test_scenarios.rs   # Common test scenarios
│   ├── postgres-integration/   # PostgreSQL-specific tests
│   └── mysql-integration/      # MySQL-specific tests
└── .github/workflows/
    └── integration.yml          # CI/CD pipeline
```

### Test Categories

1. **Unit Tests**: Fast tests without external dependencies
2. **Integration Tests**: Database-backed functionality tests
3. **Performance Tests**: Benchmarking and regression detection
4. **Docker Tests**: Containerized deployment validation

## Local Development

### Database Management

```bash
# Start database containers
make start-db

# Check database status
make status-db

# View database logs
make logs-db

# Stop databases
make stop-db

# Clean up everything
make clean-db
```

### Running Tests

```bash
# Quick development cycle
make quick              # format + check + unit tests

# Full development cycle
make full               # clean + build + lint + all tests

# Unit tests only
make test-unit

# Integration tests (no database)
make test-integration

# Performance benchmarks
make performance
```

### Test Options

The main test script supports several options:

```bash
./scripts/test-local.sh --help

Options:
  --unit-only         Run only unit tests
  --postgres-only     Run only PostgreSQL integration tests
  --mysql-only        Run only MySQL integration tests
  --performance       Run performance benchmarks
  --no-cleanup        Don't cleanup containers after tests
  --help              Show help message
```

## Test Scenarios

### Core Functionality Tests

Each integration application runs these shared test scenarios:

1. **Basic Job Lifecycle**
   - Job creation, enqueueing, dequeuing
   - Status transitions and completion
   - Database consistency verification

2. **Delayed Jobs**
   - Future scheduling functionality
   - Proper timing behavior
   - Schedule-based job availability

3. **Job Retries**
   - Failure handling and retry logic
   - Max attempts enforcement
   - Error message persistence

4. **Worker Pool Management**
   - Multiple worker coordination
   - Concurrent job processing
   - Graceful shutdown handling

5. **Concurrent Processing**
   - Race condition prevention
   - Database locking behavior
   - Transaction isolation

6. **Error Handling**
   - Edge case management
   - Graceful failure modes
   - Data consistency preservation

### Database-Specific Tests

#### PostgreSQL Tests

- **JSONB Operations**: Complex JSON payload handling
- **FOR UPDATE SKIP LOCKED**: Concurrent job dequeuing
- **Advanced Indexing**: Query performance optimization
- **Transaction Semantics**: PostgreSQL-specific behavior

#### MySQL Tests

- **JSON Operations**: MySQL JSON datatype handling
- **Transaction Isolation**: MySQL locking behavior
- **Character Encoding**: Unicode and special character support
- **Performance Characteristics**: MySQL-specific optimizations

## Performance Benchmarking

### Running Benchmarks

```bash
# Run performance tests
make benchmark

# Or with specific databases
./scripts/test-local.sh --performance
```

### Performance Metrics

Tests measure and validate:

- **Enqueue Rate**: Jobs per second insertion
- **Dequeue Rate**: Jobs per second retrieval
- **Processing Latency**: End-to-end job processing time
- **Memory Usage**: Resource utilization patterns
- **Concurrent Throughput**: Multi-worker performance

### Performance Assertions

- PostgreSQL: > 10 jobs/sec enqueue/dequeue
- MySQL: > 5 jobs/sec enqueue/dequeue
- Memory: Stable usage patterns
- Latency: < 100ms for basic operations

## Continuous Integration

### GitHub Actions Workflow

The CI pipeline (`.github/workflows/integration.yml`) runs:

1. **Unit Tests**: Fast validation without dependencies
2. **PostgreSQL Integration**: Full database testing
3. **MySQL Integration**: MySQL-specific validation
4. **Docker Tests**: Containerized scenario testing
5. **Security Audit**: Vulnerability scanning
6. **Performance Tests**: Benchmark execution (scheduled)

### Triggers

- **Push/PR**: On master/main/develop branches
- **Scheduled**: Daily performance benchmarks
- **Manual**: `[benchmark]` in commit message

### Test Matrix

| Job | Database | Purpose |
|-----|----------|---------|
| unit-tests | None | Fast validation |
| postgres-integration | PostgreSQL 16 | Feature testing |
| mysql-integration | MySQL 8.0 | Feature testing |
| docker-tests | Both | Container validation |
| performance-tests | Both | Benchmark regression |

## Docker Testing

### Building Images

```bash
# Build integration images
make docker-build

# Run specific integrations
make docker-run-postgres
make docker-run-mysql
```

### Container Configuration

Integration containers are configured with:
- Minimal runtime dependencies
- Non-root user execution
- Health checks for monitoring
- Proper logging configuration

## Troubleshooting

### Common Issues

#### Database Connection Failures

```bash
# Check database status
make status-db

# View database logs
make logs-db

# Restart databases
make restart-db
```

#### Test Timeouts

```bash
# Increase timeout in scripts
export TEST_TIMEOUT=600  # 10 minutes

# Run with verbose logging
export RUST_LOG=debug
./scripts/test-local.sh
```

#### Container Issues

```bash
# Full cleanup and restart
make cleanup
make dev-setup

# Check Docker resources
docker system df
docker system prune
```

### Debugging Tests

1. **Enable Debug Logging**:
   ```bash
   export RUST_LOG=hammerwork=debug
   ./scripts/test-local.sh
   ```

2. **Run Individual Test Scenarios**:
   ```bash
   # Modify integration apps to run specific tests
   cargo run --bin postgres-integration --features postgres
   ```

3. **Inspect Database State**:
   ```bash
   # PostgreSQL
   docker exec -it hammerwork-postgres psql -U postgres -d hammerwork_test
   
   # MySQL
   docker exec -it hammerwork-mysql mysql -u hammerwork -ppassword hammerwork_test
   ```

## Adding New Tests

### Creating New Test Scenarios

1. **Add to Shared Scenarios** (`integrations/shared/test_scenarios.rs`):
   ```rust
   impl<DB> TestScenarios<DB> {
       pub async fn test_new_feature(&self) -> Result<()> {
           // Your test implementation
       }
   }
   ```

2. **Update Integration Applications**:
   - Add feature-specific tests to `postgres-integration/src/main.rs`
   - Add feature-specific tests to `mysql-integration/src/main.rs`

3. **Update CI Pipeline**:
   - Add new test validation to `.github/workflows/integration.yml`

### Database Schema Changes

1. **Update Initialization Scripts**:
   - Modify `config/init-postgres.sql`
   - Modify `config/init-mysql.sql`

2. **Update Database Queue Implementations**:
   - Modify PostgreSQL implementation in `src/queue.rs`
   - Modify MySQL implementation in `src/queue.rs`

3. **Add Migration Tests**:
   - Test schema compatibility
   - Validate data migration procedures

## Best Practices

### Test Development

- ✅ Write tests for both happy path and edge cases
- ✅ Include cleanup logic to prevent test pollution
- ✅ Use descriptive test names and logging
- ✅ Test database-specific behaviors separately
- ✅ Validate performance regression prevention

### Database Testing

- ✅ Use fresh database instances for each test run
- ✅ Test transaction rollback scenarios
- ✅ Validate data consistency across operations
- ✅ Test concurrent access patterns
- ✅ Include schema migration validation

### CI/CD Integration

- ✅ Keep test execution time reasonable (< 10 minutes)
- ✅ Use appropriate timeouts for reliability
- ✅ Cache dependencies for faster builds
- ✅ Provide clear failure reporting
- ✅ Include performance regression detection

## Environment Variables

### Required for Testing

```bash
# Database connections
DATABASE_URL=postgres://postgres:password@localhost:5432/hammerwork_test
# or
DATABASE_URL=mysql://hammerwork:password@localhost:3306/hammerwork_test

# Logging configuration
RUST_LOG=info                    # General logging
RUST_BACKTRACE=1                 # Error backtraces

# Test configuration
TEST_TIMEOUT=300                 # Test timeout in seconds
INTEGRATION_TEST_MODE=true       # Enable integration test mode
```

### Optional Configuration

```bash
# Performance testing
PERFORMANCE_TEST_ITERATIONS=100  # Number of test jobs
BENCHMARK_MODE=true              # Enable benchmark mode

# CI-specific
CI=true                          # CI environment flag
GITHUB_ACTIONS=true              # GitHub Actions specific
```

This integration testing infrastructure ensures Hammerwork maintains high quality and reliability across different database backends and deployment scenarios.