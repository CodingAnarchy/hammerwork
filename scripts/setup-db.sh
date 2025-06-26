#!/bin/bash

# Database setup script for Hammerwork integration testing

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
COMPOSE_FILE="$PROJECT_ROOT/docker-compose.yml"

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

echo -e "${BLUE}🗄️  Hammerwork Database Setup${NC}"
echo -e "${BLUE}=============================${NC}"

# Function to wait for database to be ready
wait_for_db() {
    local db_type="$1"
    local max_attempts=60
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

        if [ $((attempt % 10)) -eq 0 ]; then
            print_info "Still waiting for $db_type... (attempt $attempt/$max_attempts)"
        else
            echo -n "."
        fi
        sleep 2
        ((attempt++))
    done

    print_error "$db_type failed to become ready after $max_attempts attempts"
    return 1
}

# Function to check database status
check_db_status() {
    local db_type="$1"
    
    print_info "Checking $db_type status..."
    
    if [ "$db_type" = "postgres" ]; then
        if docker exec hammerwork-postgres psql -U postgres -d hammerwork_test -c "SELECT COUNT(*) FROM hammerwork_jobs;" >/dev/null 2>&1; then
            local count=$(docker exec hammerwork-postgres psql -U postgres -d hammerwork_test -t -c "SELECT COUNT(*) FROM hammerwork_jobs;" 2>/dev/null | tr -d ' ')
            print_status "PostgreSQL table exists with $count jobs"
        else
            print_warning "PostgreSQL table does not exist or is not accessible"
        fi
    elif [ "$db_type" = "mysql" ]; then
        if docker exec hammerwork-mysql mysql -u hammerwork -ppassword hammerwork_test -e "SELECT COUNT(*) FROM hammerwork_jobs;" >/dev/null 2>&1; then
            local count=$(docker exec hammerwork-mysql mysql -u hammerwork -ppassword hammerwork_test -se "SELECT COUNT(*) FROM hammerwork_jobs;" 2>/dev/null)
            print_status "MySQL table exists with $count jobs"
        else
            print_warning "MySQL table does not exist or is not accessible"
        fi
    fi
}

# Function to show help
show_help() {
    echo "Usage: $0 [OPTIONS]"
    echo ""
    echo "Options:"
    echo "  --start         Start database containers"
    echo "  --stop          Stop database containers"
    echo "  --restart       Restart database containers"
    echo "  --status        Check database status"
    echo "  --logs          Show database logs"
    echo "  --clean         Clean up all containers and volumes"
    echo "  --postgres-only Only operate on PostgreSQL"
    echo "  --mysql-only    Only operate on MySQL"
    echo "  --help          Show this help message"
    echo ""
    echo "Examples:"
    echo "  $0 --start      # Start both databases"
    echo "  $0 --status     # Check status of both databases"
    echo "  $0 --logs       # Show logs from both databases"
}

# Parse command line arguments
ACTION=""
DB_FILTER="both"

while [[ $# -gt 0 ]]; do
    case $1 in
        --start)
            ACTION="start"
            shift
            ;;
        --stop)
            ACTION="stop"
            shift
            ;;
        --restart)
            ACTION="restart"
            shift
            ;;
        --status)
            ACTION="status"
            shift
            ;;
        --logs)
            ACTION="logs"
            shift
            ;;
        --clean)
            ACTION="clean"
            shift
            ;;
        --postgres-only)
            DB_FILTER="postgres"
            shift
            ;;
        --mysql-only)
            DB_FILTER="mysql"
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

# Default action if none specified
if [ -z "$ACTION" ]; then
    ACTION="start"
fi

# Main execution based on action
case $ACTION in
    start)
        print_info "Starting database containers..."
        cd "$PROJECT_ROOT"
        
        if [ "$DB_FILTER" = "postgres" ]; then
            docker-compose -f "$COMPOSE_FILE" up -d postgres
            wait_for_db "postgres"
            check_db_status "postgres"
        elif [ "$DB_FILTER" = "mysql" ]; then
            docker-compose -f "$COMPOSE_FILE" up -d mysql
            wait_for_db "mysql"
            check_db_status "mysql"
        else
            docker-compose -f "$COMPOSE_FILE" up -d postgres mysql
            wait_for_db "postgres"
            wait_for_db "mysql"
            check_db_status "postgres"
            check_db_status "mysql"
        fi
        
        print_status "Database setup completed"
        ;;
    
    stop)
        print_info "Stopping database containers..."
        cd "$PROJECT_ROOT"
        
        if [ "$DB_FILTER" = "postgres" ]; then
            docker-compose -f "$COMPOSE_FILE" stop postgres
        elif [ "$DB_FILTER" = "mysql" ]; then
            docker-compose -f "$COMPOSE_FILE" stop mysql
        else
            docker-compose -f "$COMPOSE_FILE" stop postgres mysql
        fi
        
        print_status "Databases stopped"
        ;;
    
    restart)
        print_info "Restarting database containers..."
        cd "$PROJECT_ROOT"
        
        if [ "$DB_FILTER" = "postgres" ]; then
            docker-compose -f "$COMPOSE_FILE" restart postgres
            wait_for_db "postgres"
        elif [ "$DB_FILTER" = "mysql" ]; then
            docker-compose -f "$COMPOSE_FILE" restart mysql
            wait_for_db "mysql"
        else
            docker-compose -f "$COMPOSE_FILE" restart postgres mysql
            wait_for_db "postgres"
            wait_for_db "mysql"
        fi
        
        print_status "Databases restarted"
        ;;
    
    status)
        print_info "Checking database status..."
        
        if [ "$DB_FILTER" = "postgres" ] || [ "$DB_FILTER" = "both" ]; then
            if docker ps | grep hammerwork-postgres >/dev/null 2>&1; then
                print_status "PostgreSQL container is running"
                check_db_status "postgres"
            else
                print_warning "PostgreSQL container is not running"
            fi
        fi
        
        if [ "$DB_FILTER" = "mysql" ] || [ "$DB_FILTER" = "both" ]; then
            if docker ps | grep hammerwork-mysql >/dev/null 2>&1; then
                print_status "MySQL container is running"
                check_db_status "mysql"
            else
                print_warning "MySQL container is not running"
            fi
        fi
        ;;
    
    logs)
        print_info "Showing database logs..."
        cd "$PROJECT_ROOT"
        
        if [ "$DB_FILTER" = "postgres" ]; then
            docker-compose -f "$COMPOSE_FILE" logs -f postgres
        elif [ "$DB_FILTER" = "mysql" ]; then
            docker-compose -f "$COMPOSE_FILE" logs -f mysql
        else
            docker-compose -f "$COMPOSE_FILE" logs -f postgres mysql
        fi
        ;;
    
    clean)
        print_warning "This will remove all containers, volumes, and data!"
        read -p "Are you sure? (y/N) " -n 1 -r
        echo
        if [[ $REPLY =~ ^[Yy]$ ]]; then
            print_info "Cleaning up..."
            cd "$PROJECT_ROOT"
            docker-compose -f "$COMPOSE_FILE" down --volumes --remove-orphans
            docker image prune -f
            print_status "Cleanup completed"
        else
            print_info "Cleanup cancelled"
        fi
        ;;
    
    *)
        print_error "Unknown action: $ACTION"
        show_help
        exit 1
        ;;
esac