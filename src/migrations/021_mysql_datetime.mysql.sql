-- Migration 021: Store MySQL times as DATETIME(6) instead of TIMESTAMP (MySQL)
-- TIMESTAMP ends at 2038-01-19 03:14:07 UTC; DATETIME(6) runs to 9999-12-31, so jobs can
-- be scheduled (and retried, leased, archived) beyond 2038. The library reads and writes
-- UTC everywhere (UTC_TIMESTAMP(6), UTC values bound by sqlx), so the stored values
-- are the same instants either way.
--
-- TIMESTAMP stores UTC and converts to and from the session time zone, DATETIME stores
-- the literal value. The conversion therefore runs with the session time zone set to UTC
-- (the session's own setting is restored at the end), so every existing value is kept.
--
-- MySQL rebuilds each table for this change (ALGORITHM=COPY), blocking writes to it, so
-- there is one ALTER per table, and the big ones are hammerwork_jobs and
-- hammerwork_jobs_archive. Run it off-peak; see docs/upgrading-to-3.0.md.
--
-- hammerwork_workflows was DATETIME (no fractional seconds) already; it becomes DATETIME(6)
-- too, so every MySQL time keeps microseconds.
--
-- Nullability and defaults are kept; CURRENT_TIMESTAMP defaults become
-- CURRENT_TIMESTAMP(6) (DATETIME(6) requires the precision to match). The only ON UPDATE
-- clause is hammerwork_queue_pause.updated_at, which keeps it. Each ALTER runs only
-- while its table still has a TIMESTAMP column (MySQL DDL auto-commits, so the
-- migration can be re-run after a partial failure).

SET @hammerwork_previous_time_zone = @@SESSION.time_zone;
SET SESSION time_zone = '+00:00';

SET @sql = IF(
    (SELECT COUNT(*) FROM information_schema.columns
     WHERE table_schema = DATABASE() AND table_name = 'hammerwork_jobs'
       AND data_type = 'timestamp') > 0,
    'ALTER TABLE hammerwork_jobs
        MODIFY COLUMN `created_at` DATETIME(6) NOT NULL,
        MODIFY COLUMN `scheduled_at` DATETIME(6) NOT NULL,
        MODIFY COLUMN `started_at` DATETIME(6) NULL,
        MODIFY COLUMN `completed_at` DATETIME(6) NULL,
        MODIFY COLUMN `failed_at` DATETIME(6) NULL,
        MODIFY COLUMN `timed_out_at` DATETIME(6) NULL,
        MODIFY COLUMN `next_run_at` DATETIME(6) NULL,
        MODIFY COLUMN `result_stored_at` DATETIME(6) NULL,
        MODIFY COLUMN `result_expires_at` DATETIME(6) NULL,
        MODIFY COLUMN `retention_delete_at` DATETIME(6) NULL,
        MODIFY COLUMN `encrypted_at` DATETIME(6) NULL,
        MODIFY COLUMN `archived_at` DATETIME(6) NULL,
        MODIFY COLUMN `last_heartbeat_at` DATETIME(6) NULL,
        MODIFY COLUMN `lease_expires_at` DATETIME(6) NULL',
    'SELECT 1'
);
PREPARE stmt FROM @sql;
EXECUTE stmt;
DEALLOCATE PREPARE stmt;

SET @sql = IF(
    (SELECT COUNT(*) FROM information_schema.columns
     WHERE table_schema = DATABASE() AND table_name = 'hammerwork_jobs_archive'
       AND data_type = 'timestamp') > 0,
    'ALTER TABLE hammerwork_jobs_archive
        MODIFY COLUMN `created_at` DATETIME(6) NOT NULL,
        MODIFY COLUMN `scheduled_at` DATETIME(6) NOT NULL,
        MODIFY COLUMN `started_at` DATETIME(6) NULL,
        MODIFY COLUMN `completed_at` DATETIME(6) NULL,
        MODIFY COLUMN `failed_at` DATETIME(6) NULL,
        MODIFY COLUMN `timed_out_at` DATETIME(6) NULL,
        MODIFY COLUMN `archived_at` DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
        MODIFY COLUMN `result_ttl` DATETIME(6) NULL,
        MODIFY COLUMN `next_run_at` DATETIME(6) NULL,
        MODIFY COLUMN `retention_delete_at` DATETIME(6) NULL,
        MODIFY COLUMN `encrypted_at` DATETIME(6) NULL',
    'SELECT 1'
);
PREPARE stmt FROM @sql;
EXECUTE stmt;
DEALLOCATE PREPARE stmt;

SET @sql = IF(
    (SELECT COUNT(*) FROM information_schema.columns
     WHERE table_schema = DATABASE() AND table_name = 'hammerwork_batches'
       AND data_type = 'timestamp') > 0,
    'ALTER TABLE hammerwork_batches
        MODIFY COLUMN `created_at` DATETIME(6) NOT NULL,
        MODIFY COLUMN `completed_at` DATETIME(6) NULL',
    'SELECT 1'
);
PREPARE stmt FROM @sql;
EXECUTE stmt;
DEALLOCATE PREPARE stmt;

SET @sql = IF(
    (SELECT COUNT(*) FROM information_schema.columns
     WHERE table_schema = DATABASE() AND table_name = 'hammerwork_encryption_keys'
       AND data_type = 'timestamp') > 0,
    'ALTER TABLE hammerwork_encryption_keys
        MODIFY COLUMN `created_at` DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
        MODIFY COLUMN `expires_at` DATETIME(6) NULL,
        MODIFY COLUMN `rotated_at` DATETIME(6) NULL,
        MODIFY COLUMN `retired_at` DATETIME(6) NULL,
        MODIFY COLUMN `next_rotation_at` DATETIME(6) NULL,
        MODIFY COLUMN `last_used_at` DATETIME(6) NULL',
    'SELECT 1'
);
PREPARE stmt FROM @sql;
EXECUTE stmt;
DEALLOCATE PREPARE stmt;

SET @sql = IF(
    (SELECT COUNT(*) FROM information_schema.columns
     WHERE table_schema = DATABASE() AND table_name = 'hammerwork_key_audit_log'
       AND data_type = 'timestamp') > 0,
    'ALTER TABLE hammerwork_key_audit_log
        MODIFY COLUMN `timestamp` DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6)',
    'SELECT 1'
);
PREPARE stmt FROM @sql;
EXECUTE stmt;
DEALLOCATE PREPARE stmt;

SET @sql = IF(
    (SELECT COUNT(*) FROM information_schema.columns
     WHERE table_schema = DATABASE() AND table_name = 'hammerwork_kms_data_keys'
       AND data_type = 'timestamp') > 0,
    'ALTER TABLE hammerwork_kms_data_keys
        MODIFY COLUMN `created_at` DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
        MODIFY COLUMN `retired_at` DATETIME(6) NULL',
    'SELECT 1'
);
PREPARE stmt FROM @sql;
EXECUTE stmt;
DEALLOCATE PREPARE stmt;

SET @sql = IF(
    (SELECT COUNT(*) FROM information_schema.columns
     WHERE table_schema = DATABASE() AND table_name = 'hammerwork_queue_pause'
       AND data_type = 'timestamp') > 0,
    'ALTER TABLE hammerwork_queue_pause
        MODIFY COLUMN `paused_at` DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
        MODIFY COLUMN `created_at` DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
        MODIFY COLUMN `updated_at` DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6)',
    'SELECT 1'
);
PREPARE stmt FROM @sql;
EXECUTE stmt;
DEALLOCATE PREPARE stmt;

SET @sql = IF(
    (SELECT COUNT(*) FROM information_schema.columns
     WHERE table_schema = DATABASE() AND table_name = 'hammerwork_migrations'
       AND data_type = 'timestamp') > 0,
    'ALTER TABLE hammerwork_migrations
        MODIFY COLUMN `executed_at` DATETIME(6) NOT NULL',
    'SELECT 1'
);
PREPARE stmt FROM @sql;
EXECUTE stmt;
DEALLOCATE PREPARE stmt;

SET @sql = IF(
    (SELECT COUNT(*) FROM information_schema.columns
     WHERE table_schema = DATABASE() AND table_name = 'hammerwork_workflows'
       AND column_name = 'created_at' AND data_type = 'datetime' AND datetime_precision = 0) > 0,
    'ALTER TABLE hammerwork_workflows
        MODIFY COLUMN `created_at` DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
        MODIFY COLUMN `completed_at` DATETIME(6) NULL,
        MODIFY COLUMN `failed_at` DATETIME(6) NULL',
    'SELECT 1'
);
PREPARE stmt FROM @sql;
EXECUTE stmt;
DEALLOCATE PREPARE stmt;

SET SESSION time_zone = @hammerwork_previous_time_zone;
