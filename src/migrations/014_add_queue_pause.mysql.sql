-- Add queue pause functionality
-- Migration 014: Add queue pause state tracking
--
-- Every statement is idempotent: MySQL DDL auto-commits, so a migration that fails
-- part-way leaves its earlier statements applied and must be safe to re-run.

-- Create table for tracking queue pause states
CREATE TABLE IF NOT EXISTS hammerwork_queue_pause (
    queue_name VARCHAR(255) PRIMARY KEY,
    paused_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    paused_by VARCHAR(255),
    reason TEXT,
    created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP
);

-- Create index for faster lookups
SET @sql = IF(
    (SELECT COUNT(*) FROM information_schema.statistics
     WHERE table_schema = DATABASE() AND table_name = 'hammerwork_queue_pause' AND index_name = 'idx_hammerwork_queue_pause_paused_at') = 0,
    'CREATE INDEX idx_hammerwork_queue_pause_paused_at ON hammerwork_queue_pause (paused_at)',
    'SELECT 1'
);
PREPARE stmt FROM @sql;
EXECUTE stmt;
DEALLOCATE PREPARE stmt;
