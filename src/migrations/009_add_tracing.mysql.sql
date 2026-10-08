-- Migration 009: Add job tracing and correlation support for MySQL
-- Adds distributed tracing fields for job correlation and observability
--
-- Every statement is idempotent: MySQL DDL auto-commits, so a migration that fails
-- part-way leaves its earlier statements applied and must be safe to re-run.

-- Add tracing fields to support distributed tracing and correlation
SET @sql = IF(
    (SELECT COUNT(*) FROM information_schema.columns
     WHERE table_schema = DATABASE() AND table_name = 'hammerwork_jobs' AND column_name = 'trace_id') = 0,
    'ALTER TABLE hammerwork_jobs ADD COLUMN trace_id VARCHAR(128) NULL',
    'SELECT 1'
);
PREPARE stmt FROM @sql;
EXECUTE stmt;
DEALLOCATE PREPARE stmt;

SET @sql = IF(
    (SELECT COUNT(*) FROM information_schema.columns
     WHERE table_schema = DATABASE() AND table_name = 'hammerwork_jobs' AND column_name = 'correlation_id') = 0,
    'ALTER TABLE hammerwork_jobs ADD COLUMN correlation_id VARCHAR(128) NULL',
    'SELECT 1'
);
PREPARE stmt FROM @sql;
EXECUTE stmt;
DEALLOCATE PREPARE stmt;

SET @sql = IF(
    (SELECT COUNT(*) FROM information_schema.columns
     WHERE table_schema = DATABASE() AND table_name = 'hammerwork_jobs' AND column_name = 'parent_span_id') = 0,
    'ALTER TABLE hammerwork_jobs ADD COLUMN parent_span_id VARCHAR(128) NULL',
    'SELECT 1'
);
PREPARE stmt FROM @sql;
EXECUTE stmt;
DEALLOCATE PREPARE stmt;

SET @sql = IF(
    (SELECT COUNT(*) FROM information_schema.columns
     WHERE table_schema = DATABASE() AND table_name = 'hammerwork_jobs' AND column_name = 'span_context') = 0,
    'ALTER TABLE hammerwork_jobs ADD COLUMN span_context TEXT NULL',
    'SELECT 1'
);
PREPARE stmt FROM @sql;
EXECUTE stmt;
DEALLOCATE PREPARE stmt;

-- Index for trace ID lookups (finding all jobs in a trace)
SET @sql = IF(
    (SELECT COUNT(*) FROM information_schema.statistics
     WHERE table_schema = DATABASE() AND table_name = 'hammerwork_jobs' AND index_name = 'idx_hammerwork_jobs_trace_id') = 0,
    'CREATE INDEX idx_hammerwork_jobs_trace_id ON hammerwork_jobs (trace_id)',
    'SELECT 1'
);
PREPARE stmt FROM @sql;
EXECUTE stmt;
DEALLOCATE PREPARE stmt;

-- Index for correlation ID lookups (finding correlated business operations)
SET @sql = IF(
    (SELECT COUNT(*) FROM information_schema.statistics
     WHERE table_schema = DATABASE() AND table_name = 'hammerwork_jobs' AND index_name = 'idx_hammerwork_jobs_correlation_id') = 0,
    'CREATE INDEX idx_hammerwork_jobs_correlation_id ON hammerwork_jobs (correlation_id)',
    'SELECT 1'
);
PREPARE stmt FROM @sql;
EXECUTE stmt;
DEALLOCATE PREPARE stmt;

-- Composite index for trace and correlation queries
SET @sql = IF(
    (SELECT COUNT(*) FROM information_schema.statistics
     WHERE table_schema = DATABASE() AND table_name = 'hammerwork_jobs' AND index_name = 'idx_hammerwork_jobs_trace_correlation') = 0,
    'CREATE INDEX idx_hammerwork_jobs_trace_correlation ON hammerwork_jobs (trace_id, correlation_id)',
    'SELECT 1'
);
PREPARE stmt FROM @sql;
EXECUTE stmt;
DEALLOCATE PREPARE stmt;

-- Index for parent span lookups (hierarchical tracing)
SET @sql = IF(
    (SELECT COUNT(*) FROM information_schema.statistics
     WHERE table_schema = DATABASE() AND table_name = 'hammerwork_jobs' AND index_name = 'idx_hammerwork_jobs_parent_span_id') = 0,
    'CREATE INDEX idx_hammerwork_jobs_parent_span_id ON hammerwork_jobs (parent_span_id)',
    'SELECT 1'
);
PREPARE stmt FROM @sql;
EXECUTE stmt;
DEALLOCATE PREPARE stmt;
