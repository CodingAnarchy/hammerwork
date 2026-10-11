-- Migration 020: Index archived jobs by workflow (PostgreSQL)
-- A workflow's progress counts its archived member jobs too, so refreshing a workflow
-- looks its jobs up in hammerwork_jobs_archive by workflow_id.
-- (MySQL also widens hammerwork_jobs_archive.retry_strategy in this migration; the
-- PostgreSQL column is an unbounded VARCHAR already.)

CREATE INDEX IF NOT EXISTS idx_hammerwork_jobs_archive_workflow_id
    ON hammerwork_jobs_archive (workflow_id) WHERE workflow_id IS NOT NULL;
