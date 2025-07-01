-- Migration 010: Add job archival support for PostgreSQL
-- Creates archive table and adds archival metadata fields

-- Create archive table for long-term job storage
CREATE TABLE IF NOT EXISTS hammerwork_jobs_archive (
    id UUID PRIMARY KEY,
    queue_name VARCHAR NOT NULL,
    payload BYTEA NOT NULL,  -- Compressed payload for storage efficiency
    payload_compressed BOOLEAN NOT NULL DEFAULT true,
    original_payload_size INTEGER,
    status VARCHAR NOT NULL,
    priority VARCHAR NOT NULL DEFAULT 'Normal',
    attempts INTEGER NOT NULL DEFAULT 0,
    max_attempts INTEGER NOT NULL DEFAULT 3,
    
    -- Timing information
    created_at TIMESTAMPTZ NOT NULL,
    scheduled_at TIMESTAMPTZ NOT NULL,
    started_at TIMESTAMPTZ,
    completed_at TIMESTAMPTZ,
    failed_at TIMESTAMPTZ,
    timed_out_at TIMESTAMPTZ,
    archived_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    
    -- Error and result information
    error_message TEXT,
    result JSONB,
    result_ttl TIMESTAMPTZ,
    
    -- Retry configuration
    retry_strategy VARCHAR,
    
    -- Timeout configuration
    timeout_seconds INTEGER,
    
    -- Priority information
    priority_weight INTEGER,
    
    -- Cron scheduling
    cron_schedule VARCHAR,
    next_run_at TIMESTAMPTZ,
    recurring BOOLEAN NOT NULL DEFAULT false,
    timezone VARCHAR,
    
    -- Batch information
    batch_id UUID,
    
    -- Job dependencies
    depends_on UUID[],
    dependency_status VARCHAR DEFAULT 'Pending',
    
    -- Result configuration
    result_config JSONB,
    
    -- Tracing information
    trace_id VARCHAR(128),
    correlation_id VARCHAR(128),
    parent_span_id VARCHAR(128),
    span_context TEXT,
    
    -- Archival metadata
    archival_reason VARCHAR NOT NULL DEFAULT 'Automatic',
    archived_by VARCHAR,
    original_table VARCHAR NOT NULL DEFAULT 'hammerwork_jobs'
);

-- Add archival metadata to main jobs table
ALTER TABLE hammerwork_jobs 
ADD COLUMN IF NOT EXISTS archived_at TIMESTAMPTZ,
ADD COLUMN IF NOT EXISTS archival_reason VARCHAR,
ADD COLUMN IF NOT EXISTS archival_policy_applied VARCHAR;

-- Indexes for archive table - optimized for archival queries
CREATE INDEX IF NOT EXISTS idx_hammerwork_jobs_archive_queue_status
    ON hammerwork_jobs_archive (queue_name, status);

CREATE INDEX IF NOT EXISTS idx_hammerwork_jobs_archive_archived_at
    ON hammerwork_jobs_archive (archived_at);

CREATE INDEX IF NOT EXISTS idx_hammerwork_jobs_archive_created_at
    ON hammerwork_jobs_archive (created_at);

CREATE INDEX IF NOT EXISTS idx_hammerwork_jobs_archive_batch_id
    ON hammerwork_jobs_archive (batch_id) WHERE batch_id IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_hammerwork_jobs_archive_trace_id
    ON hammerwork_jobs_archive (trace_id) WHERE trace_id IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_hammerwork_jobs_archive_correlation_id
    ON hammerwork_jobs_archive (correlation_id) WHERE correlation_id IS NOT NULL;

-- Indexes for main table archival queries
CREATE INDEX IF NOT EXISTS idx_hammerwork_jobs_archival_candidates
    ON hammerwork_jobs (status, completed_at, failed_at, created_at) 
    WHERE archived_at IS NULL;

-- Index for finding jobs eligible for archival based on age and status
CREATE INDEX IF NOT EXISTS idx_hammerwork_jobs_archival_completed
    ON hammerwork_jobs (completed_at) 
    WHERE status = 'Completed' AND archived_at IS NULL;

CREATE INDEX IF NOT EXISTS idx_hammerwork_jobs_archival_failed
    ON hammerwork_jobs (failed_at) 
    WHERE status = 'Failed' AND archived_at IS NULL;

CREATE INDEX IF NOT EXISTS idx_hammerwork_jobs_archival_dead
    ON hammerwork_jobs (failed_at) 
    WHERE status = 'Dead' AND archived_at IS NULL;