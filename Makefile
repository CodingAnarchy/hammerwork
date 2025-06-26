# Hammerwork Job Queue Makefile
# Provides convenient targets for development and testing

.PHONY: help build test test-unit test-integration test-postgres test-mysql
.PHONY: setup-db start-db stop-db restart-db clean-db logs-db
.PHONY: integration-postgres integration-mysql integration-all
.PHONY: performance benchmark clean cleanup docker-build
.PHONY: lint format check ci

# Default target
help:
	@echo "Hammerwork Job Queue - Available Targets"
	@echo "========================================"
	@echo ""
	@echo "Development:"
	@echo "  build              Build the project"
	@echo "  test               Run all tests"
	@echo "  test-unit          Run unit tests only"
	@echo "  test-integration   Run integration tests without databases"
	@echo "  lint               Run clippy linting"
	@echo "  format             Format code with rustfmt"
	@echo "  check              Run cargo check"
	@echo ""
	@echo "Database Management:"
	@echo "  setup-db           Start database containers"
	@echo "  start-db           Start database containers"
	@echo "  stop-db            Stop database containers" 
	@echo "  restart-db         Restart database containers"
	@echo "  clean-db           Clean database containers and volumes"
	@echo "  logs-db            Show database logs"
	@echo ""
	@echo "Integration Testing:"
	@echo "  integration-all    Run all integration tests"
	@echo "  integration-postgres  Run PostgreSQL integration tests"
	@echo "  integration-mysql     Run MySQL integration tests"
	@echo "  test-postgres      Run PostgreSQL tests only"
	@echo "  test-mysql         Run MySQL tests only"
	@echo ""
	@echo "Performance & Benchmarks:"
	@echo "  performance        Run performance benchmarks"
	@echo "  benchmark          Alias for performance"
	@echo ""
	@echo "Docker:"
	@echo "  docker-build       Build integration Docker images"
	@echo "  docker-run-postgres   Run PostgreSQL integration container"
	@echo "  docker-run-mysql      Run MySQL integration container"
	@echo ""
	@echo "Cleanup:"
	@echo "  clean              Clean Rust build artifacts"
	@echo "  cleanup            Full cleanup (containers, volumes, images)"
	@echo ""
	@echo "CI/CD:"
	@echo "  ci                 Run all CI checks"
	@echo ""

# Build targets
build:
	@echo "🔨 Building Hammerwork..."
	cargo build

build-release:
	@echo "🔨 Building Hammerwork (release)..."
	cargo build --release

build-all:
	@echo "🔨 Building all workspace members..."
	cargo build --workspace

# Test targets
test: test-unit test-integration
	@echo "✅ All tests completed"

test-unit:
	@echo "🧪 Running unit tests..."
	cargo test --lib

test-integration:
	@echo "🧪 Running integration tests (without databases)..."
	cargo test --test integration_tests

test-postgres:
	@echo "🐘 Running PostgreSQL tests..."
	./scripts/test-postgres.sh

test-mysql:
	@echo "🐬 Running MySQL tests..."
	./scripts/test-mysql.sh

# Database management
setup-db: start-db

start-db:
	@echo "🗄️  Starting databases..."
	./scripts/setup-db.sh --start

stop-db:
	@echo "🛑 Stopping databases..."
	./scripts/setup-db.sh --stop

restart-db:
	@echo "🔄 Restarting databases..."
	./scripts/setup-db.sh --restart

clean-db:
	@echo "🧹 Cleaning databases..."
	./scripts/setup-db.sh --clean

logs-db:
	@echo "📋 Showing database logs..."
	./scripts/setup-db.sh --logs

status-db:
	@echo "📊 Checking database status..."
	./scripts/setup-db.sh --status

# Integration testing
integration-all:
	@echo "🚀 Running all integration tests..."
	./scripts/test-local.sh

integration-postgres:
	@echo "🐘 Running PostgreSQL integration..."
	./scripts/test-local.sh --postgres-only

integration-mysql:
	@echo "🐬 Running MySQL integration..."
	./scripts/test-local.sh --mysql-only

# Performance testing
performance: benchmark

benchmark:
	@echo "🏎️  Running performance benchmarks..."
	./scripts/test-local.sh --performance

# Docker targets
docker-build:
	@echo "🐳 Building Docker images..."
	docker-compose build postgres-integration mysql-integration

docker-run-postgres:
	@echo "🐳 Running PostgreSQL integration container..."
	docker-compose --profile integration up --build postgres-integration

docker-run-mysql:
	@echo "🐳 Running MySQL integration container..."
	docker-compose --profile integration up --build mysql-integration

# Code quality
lint:
	@echo "📝 Running clippy..."
	cargo clippy --workspace --all-targets --all-features -- -D warnings

format:
	@echo "🎨 Formatting code..."
	cargo fmt --all

format-check:
	@echo "🎨 Checking code formatting..."
	cargo fmt --all -- --check

check:
	@echo "🔍 Running cargo check..."
	cargo check --workspace --all-targets --all-features

# Feature-specific builds and tests
postgres-build:
	@echo "🐘 Building with PostgreSQL features..."
	cargo build --features postgres

mysql-build:
	@echo "🐬 Building with MySQL features..."
	cargo build --features mysql

postgres-test:
	@echo "🐘 Testing PostgreSQL features..."
	cargo test --features postgres

mysql-test:
	@echo "🐬 Testing MySQL features..."
	cargo test --features mysql

# Examples
run-postgres-example:
	@echo "🐘 Running PostgreSQL example..."
	cargo run --example postgres_example --features postgres

run-mysql-example:
	@echo "🐬 Running MySQL example..."
	cargo run --example mysql_example --features mysql

# Cleanup targets
clean:
	@echo "🧹 Cleaning build artifacts..."
	cargo clean

cleanup:
	@echo "🧹 Full cleanup..."
	./scripts/cleanup.sh

cleanup-force:
	@echo "🧹 Force cleanup..."
	./scripts/cleanup.sh --force

# CI/CD targets
ci: format-check lint check test-unit test-integration
	@echo "✅ All CI checks passed"

ci-postgres: postgres-build postgres-test integration-postgres
	@echo "✅ PostgreSQL CI checks passed"

ci-mysql: mysql-build mysql-test integration-mysql
	@echo "✅ MySQL CI checks passed"

# Development helpers
dev-setup: start-db
	@echo "🛠️  Development environment ready"
	@echo "  - Databases started"
	@echo "  - Run 'make test' to verify setup"

dev-reset: cleanup dev-setup
	@echo "🔄 Development environment reset"

# Release preparation
pre-release: format lint check test integration-all benchmark
	@echo "🚢 Pre-release checks completed"

# Documentation
docs:
	@echo "📚 Building documentation..."
	cargo doc --workspace --all-features

docs-open:
	@echo "📚 Opening documentation..."
	cargo doc --workspace --all-features --open

# Dependency management
update-deps:
	@echo "📦 Updating dependencies..."
	cargo update

audit:
	@echo "🔍 Running security audit..."
	cargo audit

# Quick development cycle
quick: format check test-unit
	@echo "⚡ Quick development cycle completed"

full: clean build lint test integration-all
	@echo "🎯 Full development cycle completed"