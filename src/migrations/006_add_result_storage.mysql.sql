-- Migration 006: Add result storage for MySQL
-- Adds job result storage with TTL support
--
-- Every statement is idempotent: MySQL DDL auto-commits, so a migration that fails
-- part-way leaves its earlier statements applied and must be safe to re-run.

-- Add result storage fields
SET @sql = IF(
    (SELECT COUNT(*) FROM information_schema.columns
     WHERE table_schema = DATABASE() AND table_name = 'hammerwork_jobs' AND column_name = 'result_data') = 0,
    'ALTER TABLE hammerwork_jobs ADD COLUMN result_data JSON',
    'SELECT 1'
);
PREPARE stmt FROM @sql;
EXECUTE stmt;
DEALLOCATE PREPARE stmt;

SET @sql = IF(
    (SELECT COUNT(*) FROM information_schema.columns
     WHERE table_schema = DATABASE() AND table_name = 'hammerwork_jobs' AND column_name = 'result_stored_at') = 0,
    'ALTER TABLE hammerwork_jobs ADD COLUMN result_stored_at TIMESTAMP(6)',
    'SELECT 1'
);
PREPARE stmt FROM @sql;
EXECUTE stmt;
DEALLOCATE PREPARE stmt;

SET @sql = IF(
    (SELECT COUNT(*) FROM information_schema.columns
     WHERE table_schema = DATABASE() AND table_name = 'hammerwork_jobs' AND column_name = 'result_expires_at') = 0,
    'ALTER TABLE hammerwork_jobs ADD COLUMN result_expires_at TIMESTAMP(6)',
    'SELECT 1'
);
PREPARE stmt FROM @sql;
EXECUTE stmt;
DEALLOCATE PREPARE stmt;

-- Create index for result cleanup operations
SET @sql = IF(
    (SELECT COUNT(*) FROM information_schema.statistics
     WHERE table_schema = DATABASE() AND table_name = 'hammerwork_jobs' AND index_name = 'idx_result_expires_at') = 0,
    'CREATE INDEX idx_result_expires_at ON hammerwork_jobs (result_expires_at)',
    'SELECT 1'
);
PREPARE stmt FROM @sql;
EXECUTE stmt;
DEALLOCATE PREPARE stmt;
