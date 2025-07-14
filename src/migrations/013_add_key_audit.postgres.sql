-- Migration 013: Add key audit logging table for PostgreSQL

-- Create audit log table for key operations
CREATE TABLE IF NOT EXISTS hammerwork_key_audit_log (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    key_id VARCHAR NOT NULL,
    operation VARCHAR NOT NULL, -- 'Create', 'Access', 'Rotate', 'Retire', 'Revoke', 'Delete'
    success BOOLEAN NOT NULL,
    error_message TEXT,
    timestamp TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    
    -- Additional audit fields
    user_id VARCHAR, -- User who performed the operation
    client_ip INET, -- IP address of the client
    user_agent TEXT, -- User agent string
    session_id VARCHAR, -- Session identifier
    
    CONSTRAINT valid_operation CHECK (operation IN ('Create', 'Access', 'Rotate', 'Retire', 'Revoke', 'Delete'))
);

-- Create indexes for efficient audit log queries
CREATE INDEX idx_hammerwork_key_audit_log_key_id
    ON hammerwork_key_audit_log (key_id);

CREATE INDEX idx_hammerwork_key_audit_log_timestamp
    ON hammerwork_key_audit_log (timestamp);

CREATE INDEX idx_hammerwork_key_audit_log_operation
    ON hammerwork_key_audit_log (operation);

CREATE INDEX idx_hammerwork_key_audit_log_success
    ON hammerwork_key_audit_log (success);

-- Create composite index for common queries
CREATE INDEX idx_hammerwork_key_audit_log_key_time
    ON hammerwork_key_audit_log (key_id, timestamp);