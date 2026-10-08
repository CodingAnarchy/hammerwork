-- Migration 018: Persist per-job retry strategies (MySQL)
-- Job::with_retry_strategy was previously lost on enqueue because hammerwork_jobs
-- had no column for it. The strategy is stored in its serde JSON form.
-- MySQL DDL auto-commits, so each step checks information_schema first and the
-- migration can be re-run after a partial failure.

SET @sql = IF(
    (SELECT COUNT(*) FROM information_schema.columns
     WHERE table_schema = DATABASE() AND table_name = 'hammerwork_jobs' AND column_name = 'retry_strategy') = 0,
    'ALTER TABLE hammerwork_jobs ADD COLUMN retry_strategy JSON NULL',
    'SELECT "retry_strategy column already exists"'
);
PREPARE stmt FROM @sql;
EXECUTE stmt;
DEALLOCATE PREPARE stmt;

-- Workers check whether a batch still has unfinished jobs every time one of its
-- jobs reaches a terminal status; this index keeps that check cheap for big batches.
SET @sql = IF(
    (SELECT COUNT(*) FROM information_schema.statistics
     WHERE table_schema = DATABASE() AND table_name = 'hammerwork_jobs' AND index_name = 'idx_hammerwork_jobs_batch_status') = 0,
    'CREATE INDEX idx_hammerwork_jobs_batch_status ON hammerwork_jobs (batch_id, status)',
    'SELECT "idx_hammerwork_jobs_batch_status already exists"'
);
PREPARE stmt FROM @sql;
EXECUTE stmt;
DEALLOCATE PREPARE stmt;
