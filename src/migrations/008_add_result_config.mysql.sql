-- Migration 008: Add result configuration storage for MySQL
-- Adds job result configuration fields to persist result storage settings
--
-- Every statement is idempotent: MySQL DDL auto-commits, so a migration that fails
-- part-way leaves its earlier statements applied and must be safe to re-run.

-- Add result configuration fields
SET @sql = IF(
    (SELECT COUNT(*) FROM information_schema.columns
     WHERE table_schema = DATABASE() AND table_name = 'hammerwork_jobs' AND column_name = 'result_storage_type') = 0,
    'ALTER TABLE hammerwork_jobs ADD COLUMN result_storage_type VARCHAR(20) DEFAULT ''none''',
    'SELECT 1'
);
PREPARE stmt FROM @sql;
EXECUTE stmt;
DEALLOCATE PREPARE stmt;

SET @sql = IF(
    (SELECT COUNT(*) FROM information_schema.columns
     WHERE table_schema = DATABASE() AND table_name = 'hammerwork_jobs' AND column_name = 'result_ttl_seconds') = 0,
    'ALTER TABLE hammerwork_jobs ADD COLUMN result_ttl_seconds BIGINT',
    'SELECT 1'
);
PREPARE stmt FROM @sql;
EXECUTE stmt;
DEALLOCATE PREPARE stmt;

SET @sql = IF(
    (SELECT COUNT(*) FROM information_schema.columns
     WHERE table_schema = DATABASE() AND table_name = 'hammerwork_jobs' AND column_name = 'result_max_size_bytes') = 0,
    'ALTER TABLE hammerwork_jobs ADD COLUMN result_max_size_bytes BIGINT',
    'SELECT 1'
);
PREPARE stmt FROM @sql;
EXECUTE stmt;
DEALLOCATE PREPARE stmt;
