#!/bin/bash

# Hammerwork Local Integration Test Runner
# This script runs comprehensive integration tests locally

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Configuration
PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
COMPOSE_FILE="$PROJECT_ROOT/docker-compose.yml"
POSTGRES_DB_URL="postgres://postgres:password@localhost:5432/hammerwork_test"
MYSQL_DB_URL="mysql://hammerwork:password@localhost:3306/hammerwork_test"

echo -e "${BLUE}🚀 Hammerwork Local Integration Test Runner${NC}"
echo -e "${BLUE}===========================================${NC}"

# Function to print colored output
print_status() {
    echo -e "${GREEN}✅ $1${NC}"
}

print_error() {
    echo -e "${RED}❌ $1${NC}"
}

print_warning() {
    echo -e "${YELLOW}⚠️  $1${NC}"
}

print_info() {
    echo -e "${BLUE}ℹ️  $1${NC}"
}

# Function to wait for database to be ready
wait_for_db() {
    local db_type="$1"
    local max_attempts=30
    local attempt=1

    print_info "Waiting for $db_type to be ready..."

    while [ $attempt -le $max_attempts ]; do
        if [ "$db_type" = "postgres" ]; then
            if docker exec hammerwork-postgres pg_isready -U postgres -d hammerwork_test >/dev/null 2>&1; then
                print_status "$db_type is ready"
                return 0
            fi
        elif [ "$db_type" = "mysql" ]; then
            if docker exec hammerwork-mysql mysqladmin ping -h localhost -u root -prootpassword >/dev/null 2>&1; then
                print_status "$db_type is ready"
                return 0
            fi
        fi

        echo -n "."
        sleep 2
        ((attempt++))
    done

    print_error "$db_type failed to become ready after $max_attempts attempts"
    return 1
}

# Function to run cargo tests
run_cargo_tests() {
    print_info "Running Rust unit and integration tests"
    
    cd "$PROJECT_ROOT"
    
    # Run unit tests
    print_info "Running unit tests..."
    if cargo test --lib; then
        print_status "Unit tests passed"
    else
        print_error "Unit tests failed"
        return 1
    fi
    
    # Run integration tests without database features
    print_info "Running integration tests (without database)..."
    if cargo test --test integration_tests; then
        print_status "Integration tests passed"
    else
        print_error "Integration tests failed"
        return 1
    fi
}

# Function to run database integration tests
run_db_integration_tests() {
    local db_type="$1"
    local db_url="$2"
    
    print_info "Running $db_type integration tests..."
    
    cd "$PROJECT_ROOT"
    
    export DATABASE_URL="$db_url"
    export RUST_LOG="info"
    
    if [ "$db_type" = "postgres" ]; then
        if cargo run --bin postgres-integration --features postgres; then
            print_status "PostgreSQL integration tests passed"
        else
            print_error "PostgreSQL integration tests failed"
            return 1
        fi
    elif [ "$db_type" = "mysql" ]; then
        if cargo run --bin mysql-integration --features mysql; then
            print_status "MySQL integration tests passed"
        else
            print_error "MySQL integration tests failed"
            return 1
        fi
    fi
}

# Function to run performance tests
run_performance_tests() {
    print_info "Running performance benchmarks..."
    
    cd "$PROJECT_ROOT"
    
    # PostgreSQL performance test
    export DATABASE_URL="$POSTGRES_DB_URL"
    print_info "Testing PostgreSQL performance..."
    if timeout 120 cargo run --bin postgres-integration --features postgres --release; then
        print_status "PostgreSQL performance test completed"
    else
        print_warning "PostgreSQL performance test timed out or failed"
    fi
    
    # MySQL performance test
    export DATABASE_URL="$MYSQL_DB_URL"
    print_info "Testing MySQL performance..."
    if timeout 120 cargo run --bin mysql-integration --features mysql --release; then
        print_status "MySQL performance test completed"
    else
        print_warning "MySQL performance test timed out or failed"
    fi
}

# Function to cleanup
cleanup() {
    print_info "Cleaning up test environment..."
    cd "$PROJECT_ROOT"
    
    # Stop and remove containers
    docker-compose -f "$COMPOSE_FILE" down --volumes --remove-orphans >/dev/null 2>&1 || true
    
    # Remove any dangling images
    docker image prune -f >/dev/null 2>&1 || true
    
    print_status "Cleanup completed"
}

# Function to show help
show_help() {
    echo "Usage: $0 [OPTIONS]"
    echo ""
    echo "Options:"
    echo "  --unit-only         Run only unit tests"
    echo "  --postgres-only     Run only PostgreSQL integration tests"
    echo "  --mysql-only        Run only MySQL integration tests"
    echo "  --performance       Run performance benchmarks"
    echo "  --no-cleanup        Don't cleanup containers after tests"
    echo "  --help              Show this help message"
    echo ""
    echo "Examples:"
    echo "  $0                  # Run all tests"
    echo "  $0 --postgres-only  # Run only PostgreSQL tests"
    echo "  $0 --performance    # Run performance benchmarks"
}

# Parse command line arguments
RUN_UNIT=true
RUN_POSTGRES=true
RUN_MYSQL=true
RUN_PERFORMANCE=false
CLEANUP_AFTER=true

while [[ $# -gt 0 ]]; do
    case $1 in
        --unit-only)
            RUN_POSTGRES=false
            RUN_MYSQL=false
            shift
            ;;
        --postgres-only)
            RUN_UNIT=false
            RUN_MYSQL=false
            shift
            ;;
        --mysql-only)
            RUN_UNIT=false
            RUN_POSTGRES=false
            shift
            ;;
        --performance)
            RUN_PERFORMANCE=true
            shift
            ;;
        --no-cleanup)
            CLEANUP_AFTER=false
            shift
            ;;
        --help)
            show_help
            exit 0
            ;;
        *)
            print_error "Unknown option: $1"
            show_help
            exit 1
            ;;
    esac
done

# Trap for cleanup
trap cleanup EXIT

# Main execution
main() {
    print_info "Starting Hammerwork integration tests in: $PROJECT_ROOT"
    
    # Run unit tests if requested
    if [ "$RUN_UNIT" = true ]; then
        if ! run_cargo_tests; then
            print_error "Unit tests failed"
            exit 1
        fi
    fi
    
    # Start database containers if needed
    if [ "$RUN_POSTGRES" = true ] || [ "$RUN_MYSQL" = true ]; then
        print_info "Starting database containers..."
        cd "$PROJECT_ROOT"
        
        # Stop any existing containers
        docker-compose -f "$COMPOSE_FILE" down --volumes >/dev/null 2>&1 || true
        
        # Start databases
        if ! docker-compose -f "$COMPOSE_FILE" up -d postgres mysql; then
            print_error "Failed to start database containers"
            exit 1
        fi
        
        print_status "Database containers started"
    fi
    
    # Wait for databases and run tests
    if [ "$RUN_POSTGRES" = true ]; then
        if wait_for_db "postgres"; then
            if ! run_db_integration_tests "postgres" "$POSTGRES_DB_URL"; then
                print_error "PostgreSQL integration tests failed"
                exit 1
            fi
        else
            print_error "PostgreSQL failed to start"
            exit 1
        fi
    fi
    
    if [ "$RUN_MYSQL" = true ]; then
        if wait_for_db "mysql"; then
            if ! run_db_integration_tests "mysql" "$MYSQL_DB_URL"; then
                print_error "MySQL integration tests failed"
                exit 1
            fi
        else
            print_error "MySQL failed to start"
            exit 1
        fi
    fi
    
    # Run performance tests if requested
    if [ "$RUN_PERFORMANCE" = true ]; then
        if [ "$RUN_POSTGRES" = true ] || [ "$RUN_MYSQL" = true ]; then
            run_performance_tests
        else
            print_warning "Performance tests require database containers"
        fi
    fi
    
    print_status "All tests completed successfully! 🎉"
}

# Check if Docker is available
if ! command -v docker >/dev/null 2>&1; then
    print_error "Docker is required but not installed"
    exit 1
fi

if ! command -v docker-compose >/dev/null 2>&1; then
    print_error "Docker Compose is required but not installed"
    exit 1
fi

# Check if Cargo is available
if ! command -v cargo >/dev/null 2>&1; then
    print_error "Rust/Cargo is required but not installed"
    exit 1
fi

# Run main function
main