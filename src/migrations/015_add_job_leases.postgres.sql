-- Migration 015: Add job leases for stale job recovery (PostgreSQL)
-- Workers record a heartbeat and extend a lease while a job is Running.
-- A reaper (DatabaseQueue::requeue_stale_jobs) reclaims Running jobs whose
-- lease has expired, e.g. because the worker crashed or was killed.

ALTER TABLE hammerwork_jobs
ADD COLUMN IF NOT EXISTS last_heartbeat_at TIMESTAMPTZ;

ALTER TABLE hammerwork_jobs
ADD COLUMN IF NOT EXISTS lease_expires_at TIMESTAMPTZ;

-- Partial index so the reaper only scans Running jobs
CREATE INDEX IF NOT EXISTS idx_hammerwork_jobs_running_lease
    ON hammerwork_jobs (lease_expires_at, started_at)
    WHERE status = 'Running';
