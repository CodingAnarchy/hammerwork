-- Migration 013: Add key audit logging table for MySQL
--
-- Every statement is idempotent: MySQL DDL auto-commits, so a migration that fails
-- part-way leaves its earlier statements applied and must be safe to re-run.

-- Create audit log table for key operations
CREATE TABLE IF NOT EXISTS hammerwork_key_audit_log (
    id CHAR(36) PRIMARY KEY DEFAULT (UUID()),
    key_id VARCHAR(255) NOT NULL,
    operation VARCHAR(50) NOT NULL, -- 'Create', 'Access', 'Rotate', 'Retire', 'Revoke', 'Delete'
    success BOOLEAN NOT NULL,
    error_message TEXT,
    timestamp TIMESTAMP(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    
    -- Additional audit fields
    user_id VARCHAR(255), -- User who performed the operation
    client_ip VARCHAR(45), -- IP address of the client (IPv4 or IPv6)
    user_agent TEXT, -- User agent string
    session_id VARCHAR(255), -- Session identifier
    
    CONSTRAINT check_operation CHECK (operation IN ('Create', 'Access', 'Rotate', 'Retire', 'Revoke', 'Delete'))
);

-- Create indexes for efficient audit log queries
SET @sql = IF(
    (SELECT COUNT(*) FROM information_schema.statistics
     WHERE table_schema = DATABASE() AND table_name = 'hammerwork_key_audit_log' AND index_name = 'idx_hammerwork_key_audit_log_key_id') = 0,
    'CREATE INDEX idx_hammerwork_key_audit_log_key_id ON hammerwork_key_audit_log (key_id)',
    'SELECT 1'
);
PREPARE stmt FROM @sql;
EXECUTE stmt;
DEALLOCATE PREPARE stmt;

SET @sql = IF(
    (SELECT COUNT(*) FROM information_schema.statistics
     WHERE table_schema = DATABASE() AND table_name = 'hammerwork_key_audit_log' AND index_name = 'idx_hammerwork_key_audit_log_timestamp') = 0,
    'CREATE INDEX idx_hammerwork_key_audit_log_timestamp ON hammerwork_key_audit_log (timestamp)',
    'SELECT 1'
);
PREPARE stmt FROM @sql;
EXECUTE stmt;
DEALLOCATE PREPARE stmt;

SET @sql = IF(
    (SELECT COUNT(*) FROM information_schema.statistics
     WHERE table_schema = DATABASE() AND table_name = 'hammerwork_key_audit_log' AND index_name = 'idx_hammerwork_key_audit_log_operation') = 0,
    'CREATE INDEX idx_hammerwork_key_audit_log_operation ON hammerwork_key_audit_log (operation)',
    'SELECT 1'
);
PREPARE stmt FROM @sql;
EXECUTE stmt;
DEALLOCATE PREPARE stmt;

SET @sql = IF(
    (SELECT COUNT(*) FROM information_schema.statistics
     WHERE table_schema = DATABASE() AND table_name = 'hammerwork_key_audit_log' AND index_name = 'idx_hammerwork_key_audit_log_success') = 0,
    'CREATE INDEX idx_hammerwork_key_audit_log_success ON hammerwork_key_audit_log (success)',
    'SELECT 1'
);
PREPARE stmt FROM @sql;
EXECUTE stmt;
DEALLOCATE PREPARE stmt;

-- Create composite index for common queries
SET @sql = IF(
    (SELECT COUNT(*) FROM information_schema.statistics
     WHERE table_schema = DATABASE() AND table_name = 'hammerwork_key_audit_log' AND index_name = 'idx_hammerwork_key_audit_log_key_time') = 0,
    'CREATE INDEX idx_hammerwork_key_audit_log_key_time ON hammerwork_key_audit_log (key_id, timestamp)',
    'SELECT 1'
);
PREPARE stmt FROM @sql;
EXECUTE stmt;
DEALLOCATE PREPARE stmt;
