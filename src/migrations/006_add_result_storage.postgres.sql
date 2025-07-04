-- Migration 006: Add result storage for PostgreSQL
-- Adds job result storage with TTL support

-- Add result storage fields
ALTER TABLE hammerwork_jobs 
ADD COLUMN IF NOT EXISTS result_data JSONB;

ALTER TABLE hammerwork_jobs 
ADD COLUMN IF NOT EXISTS result_stored_at TIMESTAMPTZ;

ALTER TABLE hammerwork_jobs 
ADD COLUMN IF NOT EXISTS result_expires_at TIMESTAMPTZ;

-- Create index for result cleanup operations
CREATE INDEX IF NOT EXISTS idx_hammerwork_jobs_result_expires_at
    ON hammerwork_jobs (result_expires_at) WHERE result_expires_at IS NOT NULL;