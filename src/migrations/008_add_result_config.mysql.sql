-- Migration 008: Add result configuration storage for MySQL
-- Adds job result configuration fields to persist result storage settings

-- Add result configuration fields
ALTER TABLE hammerwork_jobs 
ADD COLUMN result_storage_type VARCHAR(20) DEFAULT 'none',
ADD COLUMN result_ttl_seconds BIGINT,
ADD COLUMN result_max_size_bytes BIGINT;