-- Add queue pause functionality
-- Migration 014: Add queue pause state tracking

-- Create table for tracking queue pause states
CREATE TABLE IF NOT EXISTS hammerwork_queue_pause (
    queue_name VARCHAR(255) PRIMARY KEY,
    paused_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    paused_by VARCHAR(255),
    reason TEXT,
    created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP
);

-- Create index for faster lookups
CREATE INDEX idx_hammerwork_queue_pause_paused_at ON hammerwork_queue_pause(paused_at);