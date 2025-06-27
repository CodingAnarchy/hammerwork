-- Migration 006: Add result storage for MySQL
-- Adds job result storage with TTL support

-- Add result storage fields
ALTER TABLE hammerwork_jobs 
ADD COLUMN result_data JSON,
ADD COLUMN result_stored_at TIMESTAMP(6),
ADD COLUMN result_expires_at TIMESTAMP(6);

-- Create index for result cleanup operations
CREATE INDEX idx_result_expires_at ON hammerwork_jobs (result_expires_at);