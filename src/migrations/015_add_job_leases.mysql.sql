-- Migration 015: Add job leases for stale job recovery (MySQL)
-- Workers record a heartbeat and extend a lease while a job is Running.
-- A reaper (DatabaseQueue::requeue_stale_jobs) reclaims Running jobs whose
-- lease has expired, e.g. because the worker crashed or was killed.

ALTER TABLE hammerwork_jobs
ADD COLUMN last_heartbeat_at TIMESTAMP(6) NULL DEFAULT NULL,
ADD COLUMN lease_expires_at TIMESTAMP(6) NULL DEFAULT NULL;

-- Index so the reaper only scans Running jobs
CREATE INDEX idx_hammerwork_jobs_status_lease ON hammerwork_jobs (status, lease_expires_at);
