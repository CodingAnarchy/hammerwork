-- Migration 005: Add batch processing for MySQL
-- Creates batch tracking table and adds batch_id to jobs
--
-- Every statement is idempotent: MySQL DDL auto-commits, so a migration that fails
-- part-way leaves its earlier statements applied and must be safe to re-run.

-- Create batch metadata table
CREATE TABLE IF NOT EXISTS hammerwork_batches (
    id CHAR(36) PRIMARY KEY,
    batch_name VARCHAR(255) NOT NULL,
    total_jobs INTEGER NOT NULL,
    completed_jobs INTEGER NOT NULL DEFAULT 0,
    failed_jobs INTEGER NOT NULL DEFAULT 0,
    pending_jobs INTEGER NOT NULL DEFAULT 0,
    status VARCHAR(50) NOT NULL,
    failure_mode VARCHAR(50) NOT NULL,
    created_at TIMESTAMP(6) NOT NULL,
    completed_at TIMESTAMP(6),
    error_summary TEXT,
    metadata JSON,
    
    -- Indexes for batch operations
    INDEX idx_status (status),
    INDEX idx_created_at (created_at)
);

-- Add batch_id to jobs table
SET @sql = IF(
    (SELECT COUNT(*) FROM information_schema.columns
     WHERE table_schema = DATABASE() AND table_name = 'hammerwork_jobs' AND column_name = 'batch_id') = 0,
    'ALTER TABLE hammerwork_jobs ADD COLUMN batch_id CHAR(36)',
    'SELECT 1'
);
PREPARE stmt FROM @sql;
EXECUTE stmt;
DEALLOCATE PREPARE stmt;

-- Create index for batch operations
SET @sql = IF(
    (SELECT COUNT(*) FROM information_schema.statistics
     WHERE table_schema = DATABASE() AND table_name = 'hammerwork_jobs' AND index_name = 'idx_batch_id') = 0,
    'CREATE INDEX idx_batch_id ON hammerwork_jobs (batch_id)',
    'SELECT 1'
);
PREPARE stmt FROM @sql;
EXECUTE stmt;
DEALLOCATE PREPARE stmt;
