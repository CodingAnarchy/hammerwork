-- Migration 003: Add timeout functionality for PostgreSQL
-- Adds timeout tracking and timed out status support

-- Add timeout fields
ALTER TABLE hammerwork_jobs 
ADD COLUMN IF NOT EXISTS timeout_seconds INTEGER;

ALTER TABLE hammerwork_jobs 
ADD COLUMN IF NOT EXISTS timed_out_at TIMESTAMPTZ;