-- Migration 005: Add batch processing for MySQL
-- Creates batch tracking table and adds batch_id to jobs

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
ALTER TABLE hammerwork_jobs 
ADD COLUMN batch_id CHAR(36);

-- Create index for batch operations
CREATE INDEX idx_batch_id ON hammerwork_jobs (batch_id);