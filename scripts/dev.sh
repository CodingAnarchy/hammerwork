#!/bin/bash

# Hammerwork Development Helper Script
# Common development tasks and workflows

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

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

# Build everything
build() {
    print_status "Building all crates..."
    cargo build --all-features
    print_success "Build completed"
}

# Run all tests
test() {
    print_status "Running all tests..."
    
    # Unit tests
    print_status "Running unit tests..."
    cargo test --all-features
    
    # CLI unit tests
    print_status "Running CLI unit tests..."
    cargo test -p cargo-hammerwork --lib
    
    # CLI integration tests (non-database)
    print_status "Running CLI integration tests..."
    cargo test -p cargo-hammerwork --test integration_tests
    cargo test -p cargo-hammerwork --test sql_query_tests unit_tests error_handling_tests
    
    print_success "All tests completed"
}

# Run tests with database integration
test_with_db() {
    print_status "Running tests with database integration..."
    
    # Ensure databases are running
    ./scripts/setup-test-databases.sh status
    
    # Run basic tests first
    test
    
    # Run database integration tests
    print_status "Running database integration tests..."
    ./scripts/setup-test-databases.sh test
    
    print_success "All tests with database integration completed"
}

# Format code
fmt() {
    print_status "Formatting code..."
    cargo fmt
    print_success "Code formatting completed"
}

# Run clippy
lint() {
    print_status "Running clippy..."
    cargo clippy --all-features -- -D warnings
    print_success "Linting completed"
}

# Check everything (format, lint, test)
check() {
    print_status "Running full check (format, lint, test)..."
    fmt
    lint
    test
    print_success "Full check completed"
}

# Run examples
examples() {
    print_status "Running examples..."
    
    print_status "PostgreSQL example..."
    cargo run --example postgres_example --features postgres
    
    print_status "MySQL example..."
    cargo run --example mysql_example --features mysql
    
    print_status "Cron example..."
    cargo run --example cron_example --features postgres
    
    print_status "Priority example..."
    cargo run --example priority_example --features postgres
    
    print_success "Examples completed"
}

# Generate documentation
docs() {
    print_status "Generating documentation..."
    cargo doc --all-features --no-deps
    print_success "Documentation generated"
    print_status "Open target/doc/hammerwork/index.html to view"
}

# Package check
package() {
    print_status "Checking packaging..."
    cargo package --allow-dirty
    print_success "Package check completed"
}

# Release preparation
release() {
    print_status "Preparing for release..."
    
    # Full check
    check
    
    # Test with databases
    test_with_db
    
    # Check examples
    examples
    
    # Generate docs
    docs
    
    # Package check
    package
    
    print_success "Release preparation completed"
    print_status "Ready for release! Don't forget to:"
    echo "  1. Update CHANGELOG.md"
    echo "  2. Update version in Cargo.toml"
    echo "  3. Commit changes"
    echo "  4. Tag release: git tag vX.Y.Z"
    echo "  5. Push: git push origin main --tags"
    echo "  6. Publish: cargo publish"
}

# CLI development workflow
cli() {
    print_status "CLI development workflow..."
    
    # Build CLI
    print_status "Building CLI..."
    cargo build -p cargo-hammerwork
    
    # Test CLI
    print_status "Testing CLI..."
    cargo test -p cargo-hammerwork
    
    # Test CLI commands
    print_status "Testing CLI commands..."
    cargo run -p cargo-hammerwork -- --help
    cargo run -p cargo-hammerwork -- migration --help
    cargo run -p cargo-hammerwork -- config --help
    
    print_success "CLI development workflow completed"
}

# Database setup and testing
db() {
    case "${2:-both}" in
        setup)
            ./scripts/setup-test-databases.sh both
            ;;
        test)
            ./scripts/setup-test-databases.sh test
            ;;
        status)
            ./scripts/setup-test-databases.sh status
            ;;
        stop)
            ./scripts/setup-test-databases.sh stop
            ;;
        remove)
            ./scripts/setup-test-databases.sh remove
            ;;
        *)
            ./scripts/setup-test-databases.sh both
            ;;
    esac
}

# Show help
help() {
    echo "Hammerwork Development Helper"
    echo
    echo "Usage: $0 [COMMAND]"
    echo
    echo "Commands:"
    echo "  build        Build all crates"
    echo "  test         Run all tests (unit + integration, no database)"
    echo "  test-db      Run all tests including database integration"
    echo "  fmt          Format code with cargo fmt"
    echo "  lint         Run clippy linter"
    echo "  check        Run format + lint + test"
    echo "  examples     Run all examples"
    echo "  docs         Generate documentation"
    echo "  package      Check packaging"
    echo "  release      Full release preparation"
    echo "  cli          CLI development workflow"
    echo "  db [setup|test|status|stop|remove]  Database operations"
    echo "  help         Show this help message"
    echo
    echo "Examples:"
    echo "  $0 check     # Full development check"
    echo "  $0 test-db   # Test with database integration"
    echo "  $0 db setup  # Set up test databases"
    echo "  $0 cli       # CLI development workflow"
}

# Main script logic
case "${1:-help}" in
    build)
        build
        ;;
    test)
        test
        ;;
    test-db)
        test_with_db
        ;;
    fmt)
        fmt
        ;;
    lint)
        lint
        ;;
    check)
        check
        ;;
    examples)
        examples
        ;;
    docs)
        docs
        ;;
    package)
        package
        ;;
    release)
        release
        ;;
    cli)
        cli
        ;;
    db)
        db "$@"
        ;;
    help|--help|-h)
        help
        ;;
    *)
        print_error "Unknown command: $1"
        echo
        help
        exit 1
        ;;
esac