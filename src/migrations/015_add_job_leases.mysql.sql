-- Migration 015: Add job leases for stale job recovery (MySQL)
-- Workers record a heartbeat and extend a lease while a job is Running.
-- A reaper (DatabaseQueue::requeue_stale_jobs) reclaims Running jobs whose
-- lease has expired, e.g. because the worker crashed or was killed.
--
-- Every statement is idempotent: MySQL DDL auto-commits, so a migration that fails
-- part-way leaves its earlier statements applied and must be safe to re-run.

SET @sql = IF(
    (SELECT COUNT(*) FROM information_schema.columns
     WHERE table_schema = DATABASE() AND table_name = 'hammerwork_jobs' AND column_name = 'last_heartbeat_at') = 0,
    'ALTER TABLE hammerwork_jobs ADD COLUMN last_heartbeat_at TIMESTAMP(6) NULL DEFAULT NULL',
    'SELECT 1'
);
PREPARE stmt FROM @sql;
EXECUTE stmt;
DEALLOCATE PREPARE stmt;

SET @sql = IF(
    (SELECT COUNT(*) FROM information_schema.columns
     WHERE table_schema = DATABASE() AND table_name = 'hammerwork_jobs' AND column_name = 'lease_expires_at') = 0,
    'ALTER TABLE hammerwork_jobs ADD COLUMN lease_expires_at TIMESTAMP(6) NULL DEFAULT NULL',
    'SELECT 1'
);
PREPARE stmt FROM @sql;
EXECUTE stmt;
DEALLOCATE PREPARE stmt;

-- Index so the reaper only scans Running jobs
SET @sql = IF(
    (SELECT COUNT(*) FROM information_schema.statistics
     WHERE table_schema = DATABASE() AND table_name = 'hammerwork_jobs' AND index_name = 'idx_hammerwork_jobs_status_lease') = 0,
    'CREATE INDEX idx_hammerwork_jobs_status_lease ON hammerwork_jobs (status, lease_expires_at)',
    'SELECT 1'
);
PREPARE stmt FROM @sql;
EXECUTE stmt;
DEALLOCATE PREPARE stmt;
