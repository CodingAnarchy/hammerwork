#!/bin/bash

# Cleanup script for Hammerwork development environment

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

echo -e "${BLUE}🧹 Hammerwork Environment Cleanup${NC}"
echo -e "${BLUE}==================================${NC}"

# Function to show help
show_help() {
    echo "Usage: $0 [OPTIONS]"
    echo ""
    echo "Options:"
    echo "  --containers    Remove containers only"
    echo "  --volumes       Remove volumes only"
    echo "  --images        Remove images only"
    echo "  --all           Remove everything (default)"
    echo "  --force         Don't ask for confirmation"
    echo "  --help          Show this help message"
    echo ""
    echo "Examples:"
    echo "  $0              # Clean everything with confirmation"
    echo "  $0 --force      # Clean everything without confirmation"
    echo "  $0 --containers # Remove only containers"
}

# Parse command line arguments
CLEAN_CONTAINERS=false
CLEAN_VOLUMES=false
CLEAN_IMAGES=false
FORCE=false

# If no arguments, clean everything
if [ $# -eq 0 ]; then
    CLEAN_CONTAINERS=true
    CLEAN_VOLUMES=true
    CLEAN_IMAGES=true
fi

while [[ $# -gt 0 ]]; do
    case $1 in
        --containers)
            CLEAN_CONTAINERS=true
            shift
            ;;
        --volumes)
            CLEAN_VOLUMES=true
            shift
            ;;
        --images)
            CLEAN_IMAGES=true
            shift
            ;;
        --all)
            CLEAN_CONTAINERS=true
            CLEAN_VOLUMES=true
            CLEAN_IMAGES=true
            shift
            ;;
        --force)
            FORCE=true
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

# Function to get confirmation
confirm_action() {
    local message="$1"
    
    if [ "$FORCE" = true ]; then
        return 0
    fi
    
    echo -e "${YELLOW}$message${NC}"
    read -p "Continue? (y/N) " -n 1 -r
    echo
    if [[ $REPLY =~ ^[Yy]$ ]]; then
        return 0
    else
        return 1
    fi
}

# Function to cleanup containers
cleanup_containers() {
    print_info "Cleaning up containers..."
    
    cd "$PROJECT_ROOT"
    
    # Stop and remove containers
    if docker-compose -f "$COMPOSE_FILE" ps | grep -q "Up\|running"; then
        print_info "Stopping running containers..."
        docker-compose -f "$COMPOSE_FILE" down --remove-orphans
        print_status "Containers stopped and removed"
    else
        print_info "No running containers to stop"
    fi
    
    # Remove any stopped containers
    if docker ps -a --filter "name=hammerwork" --format "table {{.Names}}" | grep -q hammerwork; then
        print_info "Removing stopped containers..."
        docker ps -a --filter "name=hammerwork" -q | xargs -r docker rm
        print_status "Stopped containers removed"
    else
        print_info "No stopped containers to remove"
    fi
}

# Function to cleanup volumes
cleanup_volumes() {
    print_info "Cleaning up volumes..."
    
    cd "$PROJECT_ROOT"
    
    # Remove volumes
    if docker volume ls | grep -q hammerwork; then
        docker-compose -f "$COMPOSE_FILE" down --volumes
        print_status "Volumes removed"
    else
        print_info "No volumes to remove"
    fi
    
    # Remove any dangling volumes
    local dangling_volumes=$(docker volume ls -f dangling=true -q)
    if [ -n "$dangling_volumes" ]; then
        print_info "Removing dangling volumes..."
        echo "$dangling_volumes" | xargs -r docker volume rm
        print_status "Dangling volumes removed"
    else
        print_info "No dangling volumes to remove"
    fi
}

# Function to cleanup images
cleanup_images() {
    print_info "Cleaning up images..."
    
    # Remove Hammerwork-related images
    local hammerwork_images=$(docker images --filter "reference=*hammerwork*" -q)
    if [ -n "$hammerwork_images" ]; then
        print_info "Removing Hammerwork images..."
        echo "$hammerwork_images" | xargs -r docker rmi -f
        print_status "Hammerwork images removed"
    else
        print_info "No Hammerwork images to remove"
    fi
    
    # Remove dangling images
    local dangling_images=$(docker images -f dangling=true -q)
    if [ -n "$dangling_images" ]; then
        print_info "Removing dangling images..."
        echo "$dangling_images" | xargs -r docker rmi
        print_status "Dangling images removed"
    else
        print_info "No dangling images to remove"
    fi
}

# Function to cleanup Rust build artifacts
cleanup_rust() {
    print_info "Cleaning up Rust build artifacts..."
    
    cd "$PROJECT_ROOT"
    
    if [ -d "target" ]; then
        if confirm_action "Remove Rust target directory? This will require rebuilding."; then
            rm -rf target
            print_status "Rust target directory removed"
        else
            print_info "Skipping Rust target directory cleanup"
        fi
    else
        print_info "No Rust target directory to remove"
    fi
}

# Function to show disk space
show_disk_usage() {
    print_info "Docker disk usage:"
    docker system df
    echo
}

# Main execution
main() {
    print_info "Starting cleanup process..."
    
    # Show current disk usage
    show_disk_usage
    
    # Confirm major operations
    if [ "$CLEAN_CONTAINERS" = true ] && [ "$CLEAN_VOLUMES" = true ] && [ "$CLEAN_IMAGES" = true ]; then
        if ! confirm_action "This will remove ALL Hammerwork containers, volumes, and images!"; then
            print_info "Cleanup cancelled"
            exit 0
        fi
    fi
    
    # Execute cleanup operations
    if [ "$CLEAN_CONTAINERS" = true ]; then
        cleanup_containers
    fi
    
    if [ "$CLEAN_VOLUMES" = true ]; then
        cleanup_volumes
    fi
    
    if [ "$CLEAN_IMAGES" = true ]; then
        cleanup_images
    fi
    
    # Offer to clean Rust artifacts
    if [ "$CLEAN_CONTAINERS" = true ]; then
        cleanup_rust
    fi
    
    # Run Docker system prune
    if [ "$CLEAN_IMAGES" = true ]; then
        if confirm_action "Run docker system prune to free additional space?"; then
            docker system prune -f
            print_status "Docker system pruned"
        fi
    fi
    
    print_status "Cleanup completed! 🧹"
    
    # Show final disk usage
    echo
    print_info "Final Docker disk usage:"
    docker system df
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

# Run main function
main