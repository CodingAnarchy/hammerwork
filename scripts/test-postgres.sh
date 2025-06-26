#!/bin/bash

# PostgreSQL-specific integration test runner

set -e

# Colors for output
GREEN='\033[0;32m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

echo -e "${BLUE}🐘 Running PostgreSQL Integration Tests${NC}"
echo -e "${BLUE}======================================${NC}"

# Run the main test script with PostgreSQL only
exec "$PROJECT_ROOT/scripts/test-local.sh" --postgres-only "$@"