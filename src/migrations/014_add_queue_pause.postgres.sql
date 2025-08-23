-- Add queue pause functionality
-- Migration 014: Add queue pause state tracking

-- Create table for tracking queue pause states
CREATE TABLE IF NOT EXISTS hammerwork_queue_pause (
    queue_name VARCHAR(255) PRIMARY KEY,
    paused_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW(),
    paused_by VARCHAR(255),
    reason TEXT,
    created_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW()
);

-- Create index for faster lookups
CREATE INDEX IF NOT EXISTS idx_hammerwork_queue_pause_paused_at ON hammerwork_queue_pause(paused_at);

-- Add function to automatically update the updated_at timestamp
CREATE OR REPLACE FUNCTION update_hammerwork_queue_pause_updated_at()
RETURNS TRIGGER AS $$
BEGIN
    NEW.updated_at = NOW();
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

-- Create trigger to automatically update updated_at
DROP TRIGGER IF EXISTS trigger_update_hammerwork_queue_pause_updated_at ON hammerwork_queue_pause;
CREATE TRIGGER trigger_update_hammerwork_queue_pause_updated_at
    BEFORE UPDATE ON hammerwork_queue_pause
    FOR EACH ROW
    EXECUTE FUNCTION update_hammerwork_queue_pause_updated_at();