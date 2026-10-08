-- Migration 003: Add timeout functionality for MySQL
-- Adds timeout tracking and timed out status support
--
-- Every statement is idempotent: MySQL DDL auto-commits, so a migration that fails
-- part-way leaves its earlier statements applied and must be safe to re-run.

-- Add timeout fields
SET @sql = IF(
    (SELECT COUNT(*) FROM information_schema.columns
     WHERE table_schema = DATABASE() AND table_name = 'hammerwork_jobs' AND column_name = 'timeout_seconds') = 0,
    'ALTER TABLE hammerwork_jobs ADD COLUMN timeout_seconds INTEGER',
    'SELECT 1'
);
PREPARE stmt FROM @sql;
EXECUTE stmt;
DEALLOCATE PREPARE stmt;

SET @sql = IF(
    (SELECT COUNT(*) FROM information_schema.columns
     WHERE table_schema = DATABASE() AND table_name = 'hammerwork_jobs' AND column_name = 'timed_out_at') = 0,
    'ALTER TABLE hammerwork_jobs ADD COLUMN timed_out_at TIMESTAMP(6)',
    'SELECT 1'
);
PREPARE stmt FROM @sql;
EXECUTE stmt;
DEALLOCATE PREPARE stmt;
