-- Migration 019: Keep workflow membership in the archive (PostgreSQL)
-- hammerwork_jobs_archive had no columns for a job's workflow, so a job restored from
-- the archive lost its workflow_id and workflow_name. Archival now copies them and
-- restore_archived_job puts them back.

ALTER TABLE hammerwork_jobs_archive
ADD COLUMN IF NOT EXISTS workflow_id UUID;

ALTER TABLE hammerwork_jobs_archive
ADD COLUMN IF NOT EXISTS workflow_name VARCHAR;
