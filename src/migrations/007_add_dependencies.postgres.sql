-- Migration 007: Add job dependencies for workflow support (PostgreSQL)
-- Adds dependency tracking fields to enable job chains and workflow orchestration

-- Add dependency tracking columns to hammerwork_jobs table
ALTER TABLE hammerwork_jobs
ADD COLUMN IF NOT EXISTS depends_on JSONB DEFAULT '[]'::jsonb;

ALTER TABLE hammerwork_jobs
ADD COLUMN IF NOT EXISTS dependents JSONB DEFAULT '[]'::jsonb;

ALTER TABLE hammerwork_jobs
ADD COLUMN IF NOT EXISTS dependency_status VARCHAR DEFAULT 'none';

ALTER TABLE hammerwork_jobs
ADD COLUMN IF NOT EXISTS workflow_id UUID;

ALTER TABLE hammerwork_jobs
ADD COLUMN IF NOT EXISTS workflow_name VARCHAR;

-- Add comments for clarity
COMMENT ON COLUMN hammerwork_jobs.depends_on IS 'Array of job IDs this job depends on';
COMMENT ON COLUMN hammerwork_jobs.dependents IS 'Cached array of job IDs that depend on this job';
COMMENT ON COLUMN hammerwork_jobs.dependency_status IS 'Status of dependency resolution: none, waiting, satisfied, failed';
COMMENT ON COLUMN hammerwork_jobs.workflow_id IS 'ID of the workflow/JobGroup this job belongs to';
COMMENT ON COLUMN hammerwork_jobs.workflow_name IS 'Name of the workflow for easier identification';

-- Indexes for dependency queries
CREATE INDEX IF NOT EXISTS idx_hammerwork_jobs_depends_on
    ON hammerwork_jobs USING GIN (depends_on);

CREATE INDEX IF NOT EXISTS idx_hammerwork_jobs_dependents
    ON hammerwork_jobs USING GIN (dependents);

CREATE INDEX IF NOT EXISTS idx_hammerwork_jobs_dependency_status
    ON hammerwork_jobs (dependency_status) WHERE dependency_status != 'none';

CREATE INDEX IF NOT EXISTS idx_hammerwork_jobs_workflow
    ON hammerwork_jobs (workflow_id) WHERE workflow_id IS NOT NULL;

-- Index for efficient dependency resolution queries
CREATE INDEX IF NOT EXISTS idx_hammerwork_jobs_dependency_resolution
    ON hammerwork_jobs (queue_name, status, dependency_status, scheduled_at)
    WHERE status = 'Pending' AND dependency_status IN ('none', 'satisfied');

-- Create workflow metadata table for tracking job groups
CREATE TABLE IF NOT EXISTS hammerwork_workflows (
    id UUID PRIMARY KEY,
    name VARCHAR NOT NULL,
    status VARCHAR NOT NULL DEFAULT 'running',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    completed_at TIMESTAMPTZ,
    failed_at TIMESTAMPTZ,
    total_jobs INTEGER NOT NULL DEFAULT 0,
    completed_jobs INTEGER NOT NULL DEFAULT 0,
    failed_jobs INTEGER NOT NULL DEFAULT 0,
    failure_policy VARCHAR NOT NULL DEFAULT 'fail_fast',
    metadata JSONB DEFAULT '{}'::jsonb
);

-- Indexes for workflow queries
CREATE INDEX IF NOT EXISTS idx_hammerwork_workflows_status
    ON hammerwork_workflows (status);

CREATE INDEX IF NOT EXISTS idx_hammerwork_workflows_created
    ON hammerwork_workflows (created_at);

-- Add constraint to ensure dependency_status values are valid
ALTER TABLE hammerwork_jobs
ADD CONSTRAINT chk_dependency_status 
CHECK (dependency_status IN ('none', 'waiting', 'satisfied', 'failed'));

-- Add constraint to ensure workflow status values are valid
ALTER TABLE hammerwork_workflows
ADD CONSTRAINT chk_workflow_status
CHECK (status IN ('running', 'completed', 'failed', 'cancelled'));

-- Add constraint to ensure failure_policy values are valid
ALTER TABLE hammerwork_workflows
ADD CONSTRAINT chk_workflow_failure_policy
CHECK (failure_policy IN ('fail_fast', 'continue_on_failure', 'manual'));