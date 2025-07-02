-- Migration 010: Add job archival support for MySQL
-- Creates archive table and adds archival metadata fields

-- Create archive table for long-term job storage
CREATE TABLE IF NOT EXISTS hammerwork_jobs_archive (
    id CHAR(36) PRIMARY KEY,
    queue_name VARCHAR(255) NOT NULL,
    payload LONGBLOB NOT NULL,  -- Compressed payload for storage efficiency
    payload_compressed BOOLEAN NOT NULL DEFAULT true,
    original_payload_size INTEGER,
    status VARCHAR(50) NOT NULL,
    priority VARCHAR(20) NOT NULL DEFAULT 'Normal',
    attempts INTEGER NOT NULL DEFAULT 0,
    max_attempts INTEGER NOT NULL DEFAULT 3,
    
    -- Timing information
    created_at TIMESTAMP NOT NULL,
    scheduled_at TIMESTAMP NOT NULL,
    started_at TIMESTAMP NULL,
    completed_at TIMESTAMP NULL,
    failed_at TIMESTAMP NULL,
    timed_out_at TIMESTAMP NULL,
    archived_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    
    -- Error and result information
    error_message TEXT,
    result JSON,
    result_ttl TIMESTAMP NULL,
    
    -- Retry configuration
    retry_strategy VARCHAR(50),
    
    -- Timeout configuration
    timeout_seconds INTEGER,
    
    -- Priority information
    priority_weight INTEGER,
    
    -- Cron scheduling
    cron_schedule VARCHAR(255),
    next_run_at TIMESTAMP NULL,
    recurring BOOLEAN NOT NULL DEFAULT false,
    timezone VARCHAR(50),
    
    -- Batch information
    batch_id CHAR(36),
    
    -- Job dependencies
    depends_on JSON,
    dependency_status VARCHAR(20) DEFAULT 'Pending',
    
    -- Result configuration
    result_config JSON,
    
    -- Tracing information
    trace_id VARCHAR(128),
    correlation_id VARCHAR(128),
    parent_span_id VARCHAR(128),
    span_context TEXT,
    
    -- Archival metadata
    archival_reason VARCHAR(50) NOT NULL DEFAULT 'Automatic',
    archived_by VARCHAR(100),
    original_table VARCHAR(50) NOT NULL DEFAULT 'hammerwork_jobs',
    
    INDEX idx_hammerwork_jobs_archive_queue_status (queue_name, status),
    INDEX idx_hammerwork_jobs_archive_archived_at (archived_at),
    INDEX idx_hammerwork_jobs_archive_created_at (created_at),
    INDEX idx_hammerwork_jobs_archive_batch_id (batch_id),
    INDEX idx_hammerwork_jobs_archive_trace_id (trace_id),
    INDEX idx_hammerwork_jobs_archive_correlation_id (correlation_id)
);

-- Add archival metadata to main jobs table (MySQL doesn't support IF NOT EXISTS for columns)
-- Check and add columns individually using a more MySQL-compatible approach
SET @sql = IF(
    (SELECT COUNT(*) FROM information_schema.columns 
     WHERE table_schema = DATABASE() AND table_name = 'hammerwork_jobs' AND column_name = 'archived_at') = 0,
    'ALTER TABLE hammerwork_jobs ADD COLUMN archived_at TIMESTAMP NULL',
    'SELECT "archived_at column already exists"'
);
PREPARE stmt FROM @sql;
EXECUTE stmt;
DEALLOCATE PREPARE stmt;

SET @sql = IF(
    (SELECT COUNT(*) FROM information_schema.columns 
     WHERE table_schema = DATABASE() AND table_name = 'hammerwork_jobs' AND column_name = 'archival_reason') = 0,
    'ALTER TABLE hammerwork_jobs ADD COLUMN archival_reason VARCHAR(50) NULL',
    'SELECT "archival_reason column already exists"'
);
PREPARE stmt FROM @sql;
EXECUTE stmt;
DEALLOCATE PREPARE stmt;

SET @sql = IF(
    (SELECT COUNT(*) FROM information_schema.columns 
     WHERE table_schema = DATABASE() AND table_name = 'hammerwork_jobs' AND column_name = 'archival_policy_applied') = 0,
    'ALTER TABLE hammerwork_jobs ADD COLUMN archival_policy_applied VARCHAR(100) NULL',
    'SELECT "archival_policy_applied column already exists"'
);
PREPARE stmt FROM @sql;
EXECUTE stmt;
DEALLOCATE PREPARE stmt;

-- Indexes for main table archival queries (MySQL doesn't support IF NOT EXISTS for indexes)
-- Use DROP IF EXISTS followed by CREATE to ensure idempotency
DROP INDEX IF EXISTS idx_hammerwork_jobs_archival_candidates ON hammerwork_jobs;
CREATE INDEX idx_hammerwork_jobs_archival_candidates
    ON hammerwork_jobs (status, completed_at, failed_at, created_at);

-- Index for finding jobs eligible for archival based on age and status
DROP INDEX IF EXISTS idx_hammerwork_jobs_archival_completed ON hammerwork_jobs;
CREATE INDEX idx_hammerwork_jobs_archival_completed
    ON hammerwork_jobs (completed_at, status);

DROP INDEX IF EXISTS idx_hammerwork_jobs_archival_failed ON hammerwork_jobs;
CREATE INDEX idx_hammerwork_jobs_archival_failed
    ON hammerwork_jobs (failed_at, status);

DROP INDEX IF EXISTS idx_hammerwork_jobs_archival_dead ON hammerwork_jobs;
CREATE INDEX idx_hammerwork_jobs_archival_dead
    ON hammerwork_jobs (failed_at, status);