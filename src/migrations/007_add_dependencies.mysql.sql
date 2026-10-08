-- Migration 007: Add job dependencies for workflow support (MySQL)
-- Adds dependency tracking fields to enable job chains and workflow orchestration
--
-- Every statement is idempotent: MySQL DDL auto-commits, so a migration that fails
-- part-way leaves its earlier statements applied and must be safe to re-run.

-- Add dependency tracking columns to hammerwork_jobs table
SET @sql = IF(
    (SELECT COUNT(*) FROM information_schema.columns
     WHERE table_schema = DATABASE() AND table_name = 'hammerwork_jobs' AND column_name = 'depends_on') = 0,
    'ALTER TABLE hammerwork_jobs ADD COLUMN depends_on JSON DEFAULT (JSON_ARRAY())',
    'SELECT 1'
);
PREPARE stmt FROM @sql;
EXECUTE stmt;
DEALLOCATE PREPARE stmt;

SET @sql = IF(
    (SELECT COUNT(*) FROM information_schema.columns
     WHERE table_schema = DATABASE() AND table_name = 'hammerwork_jobs' AND column_name = 'dependents') = 0,
    'ALTER TABLE hammerwork_jobs ADD COLUMN dependents JSON DEFAULT (JSON_ARRAY())',
    'SELECT 1'
);
PREPARE stmt FROM @sql;
EXECUTE stmt;
DEALLOCATE PREPARE stmt;

SET @sql = IF(
    (SELECT COUNT(*) FROM information_schema.columns
     WHERE table_schema = DATABASE() AND table_name = 'hammerwork_jobs' AND column_name = 'dependency_status') = 0,
    'ALTER TABLE hammerwork_jobs ADD COLUMN dependency_status VARCHAR(20) DEFAULT ''none''',
    'SELECT 1'
);
PREPARE stmt FROM @sql;
EXECUTE stmt;
DEALLOCATE PREPARE stmt;

SET @sql = IF(
    (SELECT COUNT(*) FROM information_schema.columns
     WHERE table_schema = DATABASE() AND table_name = 'hammerwork_jobs' AND column_name = 'workflow_id') = 0,
    'ALTER TABLE hammerwork_jobs ADD COLUMN workflow_id CHAR(36)',
    'SELECT 1'
);
PREPARE stmt FROM @sql;
EXECUTE stmt;
DEALLOCATE PREPARE stmt;

SET @sql = IF(
    (SELECT COUNT(*) FROM information_schema.columns
     WHERE table_schema = DATABASE() AND table_name = 'hammerwork_jobs' AND column_name = 'workflow_name') = 0,
    'ALTER TABLE hammerwork_jobs ADD COLUMN workflow_name VARCHAR(255)',
    'SELECT 1'
);
PREPARE stmt FROM @sql;
EXECUTE stmt;
DEALLOCATE PREPARE stmt;

-- Add indexes for dependency queries (MySQL doesn't support functional indexes on JSON as broadly)
SET @sql = IF(
    (SELECT COUNT(*) FROM information_schema.statistics
     WHERE table_schema = DATABASE() AND table_name = 'hammerwork_jobs' AND index_name = 'idx_hammerwork_jobs_dependency_status') = 0,
    'CREATE INDEX idx_hammerwork_jobs_dependency_status ON hammerwork_jobs (dependency_status)',
    'SELECT 1'
);
PREPARE stmt FROM @sql;
EXECUTE stmt;
DEALLOCATE PREPARE stmt;

SET @sql = IF(
    (SELECT COUNT(*) FROM information_schema.statistics
     WHERE table_schema = DATABASE() AND table_name = 'hammerwork_jobs' AND index_name = 'idx_hammerwork_jobs_workflow') = 0,
    'CREATE INDEX idx_hammerwork_jobs_workflow ON hammerwork_jobs (workflow_id)',
    'SELECT 1'
);
PREPARE stmt FROM @sql;
EXECUTE stmt;
DEALLOCATE PREPARE stmt;

-- Index for efficient dependency resolution queries
SET @sql = IF(
    (SELECT COUNT(*) FROM information_schema.statistics
     WHERE table_schema = DATABASE() AND table_name = 'hammerwork_jobs' AND index_name = 'idx_hammerwork_jobs_dependency_resolution') = 0,
    'CREATE INDEX idx_hammerwork_jobs_dependency_resolution ON hammerwork_jobs (queue_name, status, dependency_status, scheduled_at)',
    'SELECT 1'
);
PREPARE stmt FROM @sql;
EXECUTE stmt;
DEALLOCATE PREPARE stmt;

-- Create workflow metadata table for tracking job groups
CREATE TABLE IF NOT EXISTS hammerwork_workflows (
    id CHAR(36) PRIMARY KEY,
    name VARCHAR(255) NOT NULL,
    status VARCHAR(20) NOT NULL DEFAULT 'running',
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    completed_at DATETIME NULL,
    failed_at DATETIME NULL,
    total_jobs INTEGER NOT NULL DEFAULT 0,
    completed_jobs INTEGER NOT NULL DEFAULT 0,
    failed_jobs INTEGER NOT NULL DEFAULT 0,
    failure_policy VARCHAR(20) NOT NULL DEFAULT 'fail_fast',
    metadata JSON DEFAULT (JSON_OBJECT())
);

-- Indexes for workflow queries
SET @sql = IF(
    (SELECT COUNT(*) FROM information_schema.statistics
     WHERE table_schema = DATABASE() AND table_name = 'hammerwork_workflows' AND index_name = 'idx_hammerwork_workflows_status') = 0,
    'CREATE INDEX idx_hammerwork_workflows_status ON hammerwork_workflows (status)',
    'SELECT 1'
);
PREPARE stmt FROM @sql;
EXECUTE stmt;
DEALLOCATE PREPARE stmt;

SET @sql = IF(
    (SELECT COUNT(*) FROM information_schema.statistics
     WHERE table_schema = DATABASE() AND table_name = 'hammerwork_workflows' AND index_name = 'idx_hammerwork_workflows_created') = 0,
    'CREATE INDEX idx_hammerwork_workflows_created ON hammerwork_workflows (created_at)',
    'SELECT 1'
);
PREPARE stmt FROM @sql;
EXECUTE stmt;
DEALLOCATE PREPARE stmt;

-- Add constraints to ensure valid enum values
SET @sql = IF(
    (SELECT COUNT(*) FROM information_schema.table_constraints
     WHERE constraint_schema = DATABASE() AND table_name = 'hammerwork_jobs' AND constraint_name = 'chk_dependency_status'
       AND constraint_type = 'CHECK') = 0,
    'ALTER TABLE hammerwork_jobs ADD CONSTRAINT chk_dependency_status CHECK (dependency_status IN (''none'', ''waiting'', ''satisfied'', ''failed''))',
    'SELECT 1'
);
PREPARE stmt FROM @sql;
EXECUTE stmt;
DEALLOCATE PREPARE stmt;

SET @sql = IF(
    (SELECT COUNT(*) FROM information_schema.table_constraints
     WHERE constraint_schema = DATABASE() AND table_name = 'hammerwork_workflows' AND constraint_name = 'chk_workflow_status'
       AND constraint_type = 'CHECK') = 0,
    'ALTER TABLE hammerwork_workflows ADD CONSTRAINT chk_workflow_status CHECK (status IN (''running'', ''completed'', ''failed'', ''cancelled''))',
    'SELECT 1'
);
PREPARE stmt FROM @sql;
EXECUTE stmt;
DEALLOCATE PREPARE stmt;

SET @sql = IF(
    (SELECT COUNT(*) FROM information_schema.table_constraints
     WHERE constraint_schema = DATABASE() AND table_name = 'hammerwork_workflows' AND constraint_name = 'chk_workflow_failure_policy'
       AND constraint_type = 'CHECK') = 0,
    'ALTER TABLE hammerwork_workflows ADD CONSTRAINT chk_workflow_failure_policy CHECK (failure_policy IN (''fail_fast'', ''continue_on_failure'', ''manual''))',
    'SELECT 1'
);
PREPARE stmt FROM @sql;
EXECUTE stmt;
DEALLOCATE PREPARE stmt;
