-- Migration 020: Widen archived retry strategies, index archived jobs by workflow,
-- keep microseconds in archive timestamps (MySQL)
-- hammerwork_jobs_archive.retry_strategy was VARCHAR(50), but a serialized retry
-- strategy is usually longer, so strict MySQL rejected the archive insert (error 1406)
-- and every later archival pass failed on the same oldest jobs. It becomes TEXT, like
-- PostgreSQL's unbounded VARCHAR.
-- A workflow's progress counts its archived member jobs too, so workflow refreshes look
-- jobs up in the archive by workflow_id.
-- The archive's timestamps were second-precision TIMESTAMP columns, unlike the
-- TIMESTAMP(6) columns of hammerwork_jobs, so archiving rounded a job's times.
-- MySQL DDL auto-commits, so each step checks information_schema first and the
-- migration can be re-run after a partial failure.

SET @sql = IF(
    (SELECT COUNT(*) FROM information_schema.columns
     WHERE table_schema = DATABASE() AND table_name = 'hammerwork_jobs_archive'
       AND column_name = 'retry_strategy' AND data_type = 'varchar') > 0,
    'ALTER TABLE hammerwork_jobs_archive MODIFY COLUMN retry_strategy TEXT NULL',
    'SELECT 1'
);
PREPARE stmt FROM @sql;
EXECUTE stmt;
DEALLOCATE PREPARE stmt;

SET @sql = IF(
    (SELECT COUNT(*) FROM information_schema.statistics
     WHERE table_schema = DATABASE() AND table_name = 'hammerwork_jobs_archive'
       AND index_name = 'idx_hammerwork_jobs_archive_workflow_id') = 0,
    'CREATE INDEX idx_hammerwork_jobs_archive_workflow_id ON hammerwork_jobs_archive (workflow_id)',
    'SELECT 1'
);
PREPARE stmt FROM @sql;
EXECUTE stmt;
DEALLOCATE PREPARE stmt;

SET @sql = IF(
    (SELECT COUNT(*) FROM information_schema.columns
     WHERE table_schema = DATABASE() AND table_name = 'hammerwork_jobs_archive'
       AND column_name = 'archived_at' AND datetime_precision = 0) > 0,
    'ALTER TABLE hammerwork_jobs_archive
        MODIFY COLUMN created_at TIMESTAMP(6) NOT NULL,
        MODIFY COLUMN scheduled_at TIMESTAMP(6) NOT NULL,
        MODIFY COLUMN started_at TIMESTAMP(6) NULL,
        MODIFY COLUMN completed_at TIMESTAMP(6) NULL,
        MODIFY COLUMN failed_at TIMESTAMP(6) NULL,
        MODIFY COLUMN timed_out_at TIMESTAMP(6) NULL,
        MODIFY COLUMN archived_at TIMESTAMP(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
        MODIFY COLUMN result_ttl TIMESTAMP(6) NULL,
        MODIFY COLUMN next_run_at TIMESTAMP(6) NULL',
    'SELECT 1'
);
PREPARE stmt FROM @sql;
EXECUTE stmt;
DEALLOCATE PREPARE stmt;
