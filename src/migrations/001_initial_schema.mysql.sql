-- Migration 001: Initial schema for MySQL
-- Creates the basic hammerwork_jobs table with core functionality

CREATE TABLE IF NOT EXISTS hammerwork_jobs (
    id CHAR(36) PRIMARY KEY,
    queue_name VARCHAR(255) NOT NULL,
    payload JSON NOT NULL,
    status VARCHAR(50) NOT NULL,
    attempts INTEGER NOT NULL DEFAULT 0,
    max_attempts INTEGER NOT NULL DEFAULT 3,
    created_at TIMESTAMP(6) NOT NULL,
    scheduled_at TIMESTAMP(6) NOT NULL,
    started_at TIMESTAMP(6),
    completed_at TIMESTAMP(6),
    failed_at TIMESTAMP(6),
    error_message TEXT,
    
    -- Basic indexes for job processing
    INDEX idx_queue_status (queue_name, status),
    INDEX idx_scheduled (scheduled_at),
    INDEX idx_status_failed_at (status, failed_at)
);