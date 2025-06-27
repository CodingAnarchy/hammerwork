-- Migration 003: Add timeout functionality for MySQL
-- Adds timeout tracking and timed out status support

-- Add timeout fields
ALTER TABLE hammerwork_jobs 
ADD COLUMN timeout_seconds INTEGER,
ADD COLUMN timed_out_at TIMESTAMP(6);