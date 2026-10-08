-- Migration 002: Add priority system for MySQL
-- Adds priority field and optimized indexes for job prioritization
--
-- Every statement is idempotent: MySQL DDL auto-commits, so a migration that fails
-- part-way leaves its earlier statements applied and must be safe to re-run.

-- Add priority column with default Normal priority (2)
SET @sql = IF(
    (SELECT COUNT(*) FROM information_schema.columns
     WHERE table_schema = DATABASE() AND table_name = 'hammerwork_jobs' AND column_name = 'priority') = 0,
    'ALTER TABLE hammerwork_jobs ADD COLUMN priority INTEGER NOT NULL DEFAULT 2',
    'SELECT 1'
);
PREPARE stmt FROM @sql;
EXECUTE stmt;
DEALLOCATE PREPARE stmt;

-- Drop old index and add priority-aware index
SET @sql = IF(
    (SELECT COUNT(*) FROM information_schema.statistics
     WHERE table_schema = DATABASE() AND table_name = 'hammerwork_jobs' AND index_name = 'idx_queue_status') > 0,
    'DROP INDEX idx_queue_status ON hammerwork_jobs',
    'SELECT 1'
);
PREPARE stmt FROM @sql;
EXECUTE stmt;
DEALLOCATE PREPARE stmt;

-- Create optimized index for priority-aware job polling
SET @sql = IF(
    (SELECT COUNT(*) FROM information_schema.statistics
     WHERE table_schema = DATABASE() AND table_name = 'hammerwork_jobs' AND index_name = 'idx_queue_status_priority_scheduled') = 0,
    'CREATE INDEX idx_queue_status_priority_scheduled ON hammerwork_jobs (queue_name, status, priority DESC, scheduled_at ASC)',
    'SELECT 1'
);
PREPARE stmt FROM @sql;
EXECUTE stmt;
DEALLOCATE PREPARE stmt;
