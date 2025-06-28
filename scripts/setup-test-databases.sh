#!/bin/bash

# Hammerwork Test Database Setup Script
# This script sets up PostgreSQL and MySQL Docker containers for testing

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Database configuration
POSTGRES_CONTAINER="hammerwork-postgres"
MYSQL_CONTAINER="hammerwork-mysql"
POSTGRES_PORT="5433"
MYSQL_PORT="3307"
POSTGRES_PASSWORD="hammerwork"
MYSQL_PASSWORD="hammerwork"
DB_NAME="hammerwork"

print_status() {
    echo -e "${BLUE}[INFO]${NC} $1"
}

print_success() {
    echo -e "${GREEN}[SUCCESS]${NC} $1"
}

print_warning() {
    echo -e "${YELLOW}[WARNING]${NC} $1"
}

print_error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

check_docker() {
    if ! command -v docker &> /dev/null; then
        print_error "Docker is not installed or not in PATH"
        exit 1
    fi
    
    if ! docker info &> /dev/null; then
        print_error "Docker daemon is not running"
        exit 1
    fi
    
    print_success "Docker is available"
}

setup_postgres() {
    print_status "Setting up PostgreSQL container..."
    
    # Check if container already exists
    if docker ps -a --format '{{.Names}}' | grep -q "^${POSTGRES_CONTAINER}$"; then
        print_warning "PostgreSQL container already exists"
        
        # Check if it's running
        if docker ps --format '{{.Names}}' | grep -q "^${POSTGRES_CONTAINER}$"; then
            print_status "PostgreSQL container is already running"
        else
            print_status "Starting existing PostgreSQL container..."
            docker start $POSTGRES_CONTAINER
        fi
    else
        print_status "Creating new PostgreSQL container..."
        docker run --name $POSTGRES_CONTAINER \
            -e POSTGRES_PASSWORD=$POSTGRES_PASSWORD \
            -e POSTGRES_DB=$DB_NAME \
            -p $POSTGRES_PORT:5432 \
            -d postgres:15
    fi
    
    # Wait for PostgreSQL to be ready
    print_status "Waiting for PostgreSQL to be ready..."
    for i in {1..30}; do
        if docker exec $POSTGRES_CONTAINER pg_isready -U postgres > /dev/null 2>&1; then
            break
        fi
        sleep 1
        if [ $i -eq 30 ]; then
            print_error "PostgreSQL failed to start within 30 seconds"
            exit 1
        fi
    done
    
    # Run migrations
    print_status "Running PostgreSQL migrations..."
    export DATABASE_URL="postgres://postgres:${POSTGRES_PASSWORD}@localhost:${POSTGRES_PORT}/${DB_NAME}"
    cargo run -p cargo-hammerwork -- migration run --database-url "$DATABASE_URL"
    
    print_success "PostgreSQL setup complete"
    print_status "Connection string: postgres://postgres:${POSTGRES_PASSWORD}@localhost:${POSTGRES_PORT}/${DB_NAME}"
}

setup_mysql() {
    print_status "Setting up MySQL container..."
    
    # Check if container already exists
    if docker ps -a --format '{{.Names}}' | grep -q "^${MYSQL_CONTAINER}$"; then
        print_warning "MySQL container already exists"
        
        # Check if it's running
        if docker ps --format '{{.Names}}' | grep -q "^${MYSQL_CONTAINER}$"; then
            print_status "MySQL container is already running"
        else
            print_status "Starting existing MySQL container..."
            docker start $MYSQL_CONTAINER
        fi
    else
        print_status "Creating new MySQL container..."
        docker run --name $MYSQL_CONTAINER \
            -e MYSQL_ROOT_PASSWORD=$MYSQL_PASSWORD \
            -e MYSQL_DATABASE=$DB_NAME \
            -p $MYSQL_PORT:3306 \
            -d mysql:8.0
    fi
    
    # Wait for MySQL to be ready
    print_status "Waiting for MySQL to be ready..."
    for i in {1..60}; do
        if docker exec $MYSQL_CONTAINER mysqladmin ping -h localhost -u root -p$MYSQL_PASSWORD > /dev/null 2>&1; then
            break
        fi
        sleep 1
        if [ $i -eq 60 ]; then
            print_error "MySQL failed to start within 60 seconds"
            exit 1
        fi
    done
    
    # Run migrations
    print_status "Running MySQL migrations..."
    export MYSQL_DATABASE_URL="mysql://root:${MYSQL_PASSWORD}@localhost:${MYSQL_PORT}/${DB_NAME}"
    cargo run -p cargo-hammerwork -- migration run --database-url "$MYSQL_DATABASE_URL"
    
    print_success "MySQL setup complete"
    print_status "Connection string: mysql://root:${MYSQL_PASSWORD}@localhost:${MYSQL_PORT}/${DB_NAME}"
}

show_status() {
    print_status "Database container status:"
    echo
    
    # PostgreSQL status
    if docker ps --format '{{.Names}}' | grep -q "^${POSTGRES_CONTAINER}$"; then
        print_success "PostgreSQL: Running on port $POSTGRES_PORT"
        echo "  Connection: postgres://postgres:${POSTGRES_PASSWORD}@localhost:${POSTGRES_PORT}/${DB_NAME}"
    elif docker ps -a --format '{{.Names}}' | grep -q "^${POSTGRES_CONTAINER}$"; then
        print_warning "PostgreSQL: Stopped"
        echo "  Use './scripts/setup-test-databases.sh postgres' to start"
    else
        print_warning "PostgreSQL: Not created"
        echo "  Use './scripts/setup-test-databases.sh postgres' to create"
    fi
    
    echo
    
    # MySQL status
    if docker ps --format '{{.Names}}' | grep -q "^${MYSQL_CONTAINER}$"; then
        print_success "MySQL: Running on port $MYSQL_PORT"
        echo "  Connection: mysql://root:${MYSQL_PASSWORD}@localhost:${MYSQL_PORT}/${DB_NAME}"
    elif docker ps -a --format '{{.Names}}' | grep -q "^${MYSQL_CONTAINER}$"; then
        print_warning "MySQL: Stopped"
        echo "  Use './scripts/setup-test-databases.sh mysql' to start"
    else
        print_warning "MySQL: Not created"
        echo "  Use './scripts/setup-test-databases.sh mysql' to create"
    fi
}

stop_databases() {
    print_status "Stopping database containers..."
    
    if docker ps --format '{{.Names}}' | grep -q "^${POSTGRES_CONTAINER}$"; then
        docker stop $POSTGRES_CONTAINER
        print_success "PostgreSQL container stopped"
    fi
    
    if docker ps --format '{{.Names}}' | grep -q "^${MYSQL_CONTAINER}$"; then
        docker stop $MYSQL_CONTAINER
        print_success "MySQL container stopped"
    fi
}

remove_databases() {
    print_status "Removing database containers..."
    
    # Stop first if running
    stop_databases
    
    if docker ps -a --format '{{.Names}}' | grep -q "^${POSTGRES_CONTAINER}$"; then
        docker rm $POSTGRES_CONTAINER
        print_success "PostgreSQL container removed"
    fi
    
    if docker ps -a --format '{{.Names}}' | grep -q "^${MYSQL_CONTAINER}$"; then
        docker rm $MYSQL_CONTAINER
        print_success "MySQL container removed"
    fi
}

run_tests() {
    print_status "Running integration tests..."
    
    # Check if databases are running
    if ! docker ps --format '{{.Names}}' | grep -q "^${POSTGRES_CONTAINER}$"; then
        print_error "PostgreSQL container is not running. Run './scripts/setup-test-databases.sh postgres' first"
        exit 1
    fi
    
    # Set environment variables for tests
    export DATABASE_URL="postgres://postgres:${POSTGRES_PASSWORD}@localhost:${POSTGRES_PORT}/${DB_NAME}"
    export MYSQL_DATABASE_URL="mysql://root:${MYSQL_PASSWORD}@localhost:${MYSQL_PORT}/${DB_NAME}"
    
    print_status "Running PostgreSQL integration tests..."
    cargo test -p cargo-hammerwork --test sql_query_tests postgres_tests -- --ignored
    
    if docker ps --format '{{.Names}}' | grep -q "^${MYSQL_CONTAINER}$"; then
        print_status "Running MySQL integration tests..."
        cargo test -p cargo-hammerwork --test sql_query_tests mysql_tests -- --ignored
    else
        print_warning "MySQL container not running, skipping MySQL tests"
    fi
    
    print_success "Integration tests completed"
}

show_help() {
    echo "Hammerwork Test Database Setup"
    echo
    echo "Usage: $0 [COMMAND]"
    echo
    echo "Commands:"
    echo "  postgres     Set up PostgreSQL container"
    echo "  mysql        Set up MySQL container"
    echo "  both         Set up both PostgreSQL and MySQL containers"
    echo "  status       Show status of database containers"
    echo "  stop         Stop all database containers"
    echo "  remove       Remove all database containers"
    echo "  test         Run integration tests against databases"
    echo "  help         Show this help message"
    echo
    echo "Examples:"
    echo "  $0 both      # Set up both databases"
    echo "  $0 status    # Check container status"
    echo "  $0 test      # Run integration tests"
}

# Main script logic
case "${1:-both}" in
    postgres)
        check_docker
        setup_postgres
        ;;
    mysql)
        check_docker
        setup_mysql
        ;;
    both)
        check_docker
        setup_postgres
        setup_mysql
        ;;
    status)
        show_status
        ;;
    stop)
        stop_databases
        ;;
    remove)
        remove_databases
        ;;
    test)
        check_docker
        run_tests
        ;;
    help|--help|-h)
        show_help
        ;;
    *)
        print_error "Unknown command: $1"
        echo
        show_help
        exit 1
        ;;
esac