-- Migration 005: Add batch processing for PostgreSQL
-- Creates batch tracking table and adds batch_id to jobs

-- Create batch metadata table
CREATE TABLE IF NOT EXISTS hammerwork_batches (
    id UUID PRIMARY KEY,
    batch_name VARCHAR NOT NULL,
    total_jobs INTEGER NOT NULL,
    completed_jobs INTEGER NOT NULL DEFAULT 0,
    failed_jobs INTEGER NOT NULL DEFAULT 0,
    pending_jobs INTEGER NOT NULL DEFAULT 0,
    status VARCHAR NOT NULL,
    failure_mode VARCHAR NOT NULL,
    created_at TIMESTAMPTZ NOT NULL,
    completed_at TIMESTAMPTZ,
    error_summary TEXT,
    metadata JSONB
);

-- Add batch_id to jobs table
ALTER TABLE hammerwork_jobs 
ADD COLUMN IF NOT EXISTS batch_id UUID;

-- Create indexes for batch operations
CREATE INDEX IF NOT EXISTS idx_hammerwork_jobs_batch_id
    ON hammerwork_jobs (batch_id) WHERE batch_id IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_hammerwork_batches_status
    ON hammerwork_batches (status);

CREATE INDEX IF NOT EXISTS idx_hammerwork_batches_created_at
    ON hammerwork_batches (created_at);