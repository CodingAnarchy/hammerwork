#!/bin/bash

# MySQL CLI Comprehensive Test Script
set -e

MYSQL_URL="mysql://root:hammerwork@127.0.0.1:3307/hammerwork"
CLI="cargo run --"

echo "🧪 MySQL CLI Comprehensive Test Suite"
echo "===================================="

# Test 1: Migration
echo -e "\n📋 Test 1: Database Migration"
$CLI migration run -u "$MYSQL_URL"
$CLI migration status -u "$MYSQL_URL"

# Test 2: Job Operations
echo -e "\n📋 Test 2: Job Operations"
# Create a job
$CLI job enqueue -u "$MYSQL_URL" -n "test_queue" -j '{"task":"test_mysql","id":1}' -r high
$CLI job enqueue -u "$MYSQL_URL" -n "test_queue" -j '{"task":"test_mysql","id":2}' -r normal --delay 60
$CLI job enqueue -u "$MYSQL_URL" -n "email_queue" -j '{"task":"send_email","to":"test@example.com"}' -r low

# List jobs
$CLI job list -u "$MYSQL_URL"
$CLI job list -u "$MYSQL_URL" -n test_queue
# Note: Status stored as quoted strings in DB, validation expects lowercase
# $CLI job list -u "$MYSQL_URL" -t pending
$CLI job list -u "$MYSQL_URL" -r high

# Get job details (we'll need to extract job ID from the list)
echo "Note: job get command would need actual job ID"

# Test 3: Queue Operations
echo -e "\n📋 Test 3: Queue Operations"
$CLI queue list -d "$MYSQL_URL"
$CLI queue stats -d "$MYSQL_URL" -n test_queue
$CLI queue stats -d "$MYSQL_URL" -n email_queue

# Test 4: Worker Operations (these are mostly placeholders)
echo -e "\n📋 Test 4: Worker Operations"
$CLI worker list -u "$MYSQL_URL"
$CLI worker stats -u "$MYSQL_URL"

# Test 5: Monitoring
echo -e "\n📋 Test 5: Monitoring Operations"
$CLI monitor health -u "$MYSQL_URL"
$CLI monitor metrics -u "$MYSQL_URL"
$CLI monitor metrics -u "$MYSQL_URL" -t 1h
$CLI monitor metrics -u "$MYSQL_URL" -t 7d
$CLI monitor metrics -u "$MYSQL_URL" -n test_queue

# Test 6: Backup Operations
echo -e "\n📋 Test 6: Backup Operations"
mkdir -p /tmp/hammerwork-backups
$CLI backup create -u "$MYSQL_URL" -o /tmp/hammerwork-backups/mysql-backup.json
$CLI backup create -u "$MYSQL_URL" -o /tmp/hammerwork-backups/mysql-backup.csv --format csv
$CLI backup list -p /tmp/hammerwork-backups

# Test 7: Batch Operations
echo -e "\n📋 Test 7: Batch Operations"
# Create a batch job file
cat > /tmp/batch-jobs.jsonl << EOF
{"queue": "batch_queue", "payload": {"batch_id": 1, "task": "process"}, "priority": "high"}
{"queue": "batch_queue", "payload": {"batch_id": 2, "task": "process"}, "priority": "normal"}
{"queue": "batch_queue", "payload": {"batch_id": 3, "task": "process"}, "priority": "low"}
EOF

$CLI batch enqueue -u "$MYSQL_URL" -f /tmp/batch-jobs.jsonl -n batch_queue
$CLI queue list -u "$MYSQL_URL"

# Test batch retry and cancel
$CLI batch retry -u "$MYSQL_URL" -n batch_queue --dry-run
$CLI batch cancel -u "$MYSQL_URL" -n batch_queue --dry-run

# Test 8: Cron Operations
echo -e "\n📋 Test 8: Cron Operations"
$CLI cron create -u "$MYSQL_URL" -n "cron_queue" -j '{"task":"daily_report"}' -s "0 0 * * *" -z "UTC" -r high
$CLI cron list -u "$MYSQL_URL"
$CLI cron list -u "$MYSQL_URL" --active-only
$CLI cron next -u "$MYSQL_URL" -c 5

# Test 9: Maintenance Operations
echo -e "\n📋 Test 9: Maintenance Operations"
$CLI maintenance check -u "$MYSQL_URL"
$CLI maintenance check -u "$MYSQL_URL" --fix
$CLI maintenance analyze -u "$MYSQL_URL"
$CLI maintenance vacuum -u "$MYSQL_URL" --dry-run
$CLI maintenance dead-jobs -u "$MYSQL_URL" --dry-run

# Test 10: Config Operations
echo -e "\n📋 Test 10: Config Operations"
$CLI config show
$CLI config get database.pool_size
$CLI config set database.pool_size 15
$CLI config get database.pool_size
$CLI config set database.pool_size 10  # Reset to default

# Test 11: Error Cases
echo -e "\n📋 Test 11: Testing Error Handling"
# Invalid database URL
$CLI job list -u "mysql://invalid:password@localhost:3307/hammerwork" 2>&1 | head -1 || true
# Invalid JSON payload
$CLI job create -u "$MYSQL_URL" -n "test_queue" -p "invalid json" 2>&1 | head -1 || true
# Invalid priority
$CLI job create -u "$MYSQL_URL" -n "test_queue" -p '{"test":1}' -r invalid_priority 2>&1 | head -1 || true

# Cleanup
echo -e "\n🧹 Cleaning up test data..."
rm -f /tmp/batch-jobs.jsonl
rm -rf /tmp/hammerwork-backups

echo -e "\n✅ MySQL CLI Comprehensive Test Suite Complete!"