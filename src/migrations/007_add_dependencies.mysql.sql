-- Migration 007: Add job dependencies for workflow support (MySQL)
-- Adds dependency tracking fields to enable job chains and workflow orchestration

-- Add dependency tracking columns to hammerwork_jobs table
ALTER TABLE hammerwork_jobs
ADD COLUMN depends_on JSON DEFAULT (JSON_ARRAY()),
ADD COLUMN dependents JSON DEFAULT (JSON_ARRAY()),
ADD COLUMN dependency_status VARCHAR(20) DEFAULT 'none',
ADD COLUMN workflow_id CHAR(36),
ADD COLUMN workflow_name VARCHAR(255);

-- Add indexes for dependency queries (MySQL doesn't support functional indexes on JSON as broadly)
CREATE INDEX idx_hammerwork_jobs_dependency_status
    ON hammerwork_jobs (dependency_status);

CREATE INDEX idx_hammerwork_jobs_workflow
    ON hammerwork_jobs (workflow_id);

-- Index for efficient dependency resolution queries
CREATE INDEX idx_hammerwork_jobs_dependency_resolution
    ON hammerwork_jobs (queue_name, status, dependency_status, scheduled_at);

-- Create workflow metadata table for tracking job groups
CREATE TABLE IF NOT EXISTS hammerwork_workflows (
    id CHAR(36) PRIMARY KEY,
    name VARCHAR(255) NOT NULL,
    status VARCHAR(20) NOT NULL DEFAULT 'running',
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    completed_at DATETIME NULL,
    failed_at DATETIME NULL,
    total_jobs INTEGER NOT NULL DEFAULT 0,
    completed_jobs INTEGER NOT NULL DEFAULT 0,
    failed_jobs INTEGER NOT NULL DEFAULT 0,
    failure_policy VARCHAR(20) NOT NULL DEFAULT 'fail_fast',
    metadata JSON DEFAULT (JSON_OBJECT())
);

-- Indexes for workflow queries
CREATE INDEX idx_hammerwork_workflows_status
    ON hammerwork_workflows (status);

CREATE INDEX idx_hammerwork_workflows_created
    ON hammerwork_workflows (created_at);

-- Add constraints to ensure valid enum values
ALTER TABLE hammerwork_jobs
ADD CONSTRAINT chk_dependency_status 
CHECK (dependency_status IN ('none', 'waiting', 'satisfied', 'failed'));

ALTER TABLE hammerwork_workflows
ADD CONSTRAINT chk_workflow_status
CHECK (status IN ('running', 'completed', 'failed', 'cancelled'));

ALTER TABLE hammerwork_workflows
ADD CONSTRAINT chk_workflow_failure_policy
CHECK (failure_policy IN ('fail_fast', 'continue_on_failure', 'manual'));