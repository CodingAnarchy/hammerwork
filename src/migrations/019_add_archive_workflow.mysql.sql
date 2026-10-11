-- Migration 019: Keep workflow membership in the archive (MySQL)
-- hammerwork_jobs_archive had no columns for a job's workflow, so a job restored from
-- the archive lost its workflow_id and workflow_name. Archival now copies them and
-- restore_archived_job puts them back.
-- MySQL DDL auto-commits, so each step checks information_schema first and the
-- migration can be re-run after a partial failure.

SET @sql = IF(
    (SELECT COUNT(*) FROM information_schema.columns
     WHERE table_schema = DATABASE() AND table_name = 'hammerwork_jobs_archive' AND column_name = 'workflow_id') = 0,
    'ALTER TABLE hammerwork_jobs_archive ADD COLUMN workflow_id CHAR(36) NULL',
    'SELECT "workflow_id column already exists"'
);
PREPARE stmt FROM @sql;
EXECUTE stmt;
DEALLOCATE PREPARE stmt;

SET @sql = IF(
    (SELECT COUNT(*) FROM information_schema.columns
     WHERE table_schema = DATABASE() AND table_name = 'hammerwork_jobs_archive' AND column_name = 'workflow_name') = 0,
    'ALTER TABLE hammerwork_jobs_archive ADD COLUMN workflow_name VARCHAR(255) NULL',
    'SELECT "workflow_name column already exists"'
);
PREPARE stmt FROM @sql;
EXECUTE stmt;
DEALLOCATE PREPARE stmt;
