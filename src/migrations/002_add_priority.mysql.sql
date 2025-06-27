-- Migration 002: Add priority system for MySQL
-- Adds priority field and optimized indexes for job prioritization

-- Add priority column with default Normal priority (2)
ALTER TABLE hammerwork_jobs 
ADD COLUMN priority INTEGER NOT NULL DEFAULT 2;

-- Drop old index and add priority-aware index
DROP INDEX idx_queue_status ON hammerwork_jobs;

-- Create optimized index for priority-aware job polling
CREATE INDEX idx_queue_status_priority_scheduled 
    ON hammerwork_jobs (queue_name, status, priority DESC, scheduled_at ASC);