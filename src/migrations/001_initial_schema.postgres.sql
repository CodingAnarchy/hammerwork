-- Migration 001: Initial schema for PostgreSQL
-- Creates the basic hammerwork_jobs table with core functionality

CREATE TABLE IF NOT EXISTS hammerwork_jobs (
    id UUID PRIMARY KEY,
    queue_name VARCHAR NOT NULL,
    payload JSONB NOT NULL,
    status VARCHAR NOT NULL,
    attempts INTEGER NOT NULL DEFAULT 0,
    max_attempts INTEGER NOT NULL DEFAULT 3,
    created_at TIMESTAMPTZ NOT NULL,
    scheduled_at TIMESTAMPTZ NOT NULL,
    started_at TIMESTAMPTZ,
    completed_at TIMESTAMPTZ,
    failed_at TIMESTAMPTZ,
    error_message TEXT
);

-- Basic indexes for job processing
CREATE INDEX IF NOT EXISTS idx_hammerwork_jobs_queue_status
    ON hammerwork_jobs (queue_name, status);

CREATE INDEX IF NOT EXISTS idx_hammerwork_jobs_scheduled
    ON hammerwork_jobs (scheduled_at) WHERE status = 'Pending';

CREATE INDEX IF NOT EXISTS idx_hammerwork_jobs_status_failed_at
    ON hammerwork_jobs (status, failed_at) WHERE failed_at IS NOT NULL;