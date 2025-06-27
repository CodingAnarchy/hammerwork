-- Migration 002: Add priority system for PostgreSQL
-- Adds priority field and optimized indexes for job prioritization

-- Add priority column with default Normal priority (2)
ALTER TABLE hammerwork_jobs 
ADD COLUMN IF NOT EXISTS priority INTEGER NOT NULL DEFAULT 2;

-- Drop old queue_status index and replace with priority-aware index
DROP INDEX IF EXISTS idx_hammerwork_jobs_queue_status;

-- Create optimized index for priority-aware job polling
CREATE INDEX IF NOT EXISTS idx_hammerwork_jobs_queue_status_priority_scheduled
    ON hammerwork_jobs (queue_name, status, priority DESC, scheduled_at ASC)
    WHERE status IN ('Pending', 'Retrying');