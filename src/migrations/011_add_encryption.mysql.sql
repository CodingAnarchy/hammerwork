-- Migration 011: Add job payload encryption and key management for MySQL
-- Adds encryption fields to jobs table and creates key management tables

-- Create encryption keys table for secure key storage and rotation
CREATE TABLE IF NOT EXISTS hammerwork_encryption_keys (
    id CHAR(36) PRIMARY KEY DEFAULT (UUID()),
    key_id VARCHAR(255) NOT NULL UNIQUE,
    key_version INT NOT NULL DEFAULT 1,
    algorithm VARCHAR(50) NOT NULL, -- 'AES256GCM' or 'ChaCha20Poly1305'
    key_material BLOB NOT NULL, -- Encrypted key material (never stored in plain text)
    key_derivation_salt BLOB, -- Salt for key derivation if using password-based keys
    key_source VARCHAR(50) NOT NULL, -- 'Environment', 'External', 'Generated', 'Derived'
    key_purpose VARCHAR(50) NOT NULL DEFAULT 'Encryption', -- 'Encryption', 'MAC', 'KEK' (Key Encryption Key)
    
    -- Key metadata
    created_at TIMESTAMP(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    created_by VARCHAR(255), -- Service or user that created the key
    expires_at TIMESTAMP(6), -- When the key expires
    rotated_at TIMESTAMP(6), -- When the key was rotated
    retired_at TIMESTAMP(6), -- When the key was retired (kept for decryption only)
    
    -- Key status and rotation
    status VARCHAR(50) NOT NULL DEFAULT 'Active', -- 'Active', 'Retired', 'Revoked', 'Expired'
    rotation_interval_seconds BIGINT, -- Rotation interval in seconds
    next_rotation_at TIMESTAMP(6), -- When the next rotation is due
    
    -- Security metadata
    key_strength INT NOT NULL, -- Key length in bits (256, 512, etc.)
    master_key_id CHAR(36), -- Reference to master key if this key is encrypted
    
    -- Audit trail
    last_used_at TIMESTAMP(6),
    usage_count BIGINT NOT NULL DEFAULT 0,
    
    CONSTRAINT check_algorithm CHECK (algorithm IN ('AES256GCM', 'ChaCha20Poly1305')),
    CONSTRAINT check_key_source CHECK (key_source IN ('Environment', 'External', 'Generated', 'Derived', 'Static')),
    CONSTRAINT check_key_purpose CHECK (key_purpose IN ('Encryption', 'MAC', 'KEK')),
    CONSTRAINT check_status CHECK (status IN ('Active', 'Retired', 'Revoked', 'Expired')),
    CONSTRAINT check_key_strength CHECK (key_strength >= 128 AND key_strength <= 512)
);

-- Add encryption fields to main jobs table
ALTER TABLE hammerwork_jobs ADD COLUMN IF NOT EXISTS is_encrypted BOOLEAN NOT NULL DEFAULT false;
ALTER TABLE hammerwork_jobs ADD COLUMN IF NOT EXISTS encryption_key_id VARCHAR(255);
ALTER TABLE hammerwork_jobs ADD COLUMN IF NOT EXISTS encryption_algorithm VARCHAR(50);
ALTER TABLE hammerwork_jobs ADD COLUMN IF NOT EXISTS encrypted_payload LONGBLOB;
ALTER TABLE hammerwork_jobs ADD COLUMN IF NOT EXISTS encryption_nonce BLOB;
ALTER TABLE hammerwork_jobs ADD COLUMN IF NOT EXISTS encryption_tag BLOB;
ALTER TABLE hammerwork_jobs ADD COLUMN IF NOT EXISTS encryption_metadata JSON;
ALTER TABLE hammerwork_jobs ADD COLUMN IF NOT EXISTS payload_hash VARCHAR(255);
ALTER TABLE hammerwork_jobs ADD COLUMN IF NOT EXISTS pii_fields JSON;
ALTER TABLE hammerwork_jobs ADD COLUMN IF NOT EXISTS retention_policy VARCHAR(50);
ALTER TABLE hammerwork_jobs ADD COLUMN IF NOT EXISTS retention_delete_at TIMESTAMP(6);
ALTER TABLE hammerwork_jobs ADD COLUMN IF NOT EXISTS encrypted_at TIMESTAMP(6);

-- Add encryption fields to archive table
ALTER TABLE hammerwork_jobs_archive ADD COLUMN IF NOT EXISTS is_encrypted BOOLEAN NOT NULL DEFAULT false;
ALTER TABLE hammerwork_jobs_archive ADD COLUMN IF NOT EXISTS encryption_key_id VARCHAR(255);
ALTER TABLE hammerwork_jobs_archive ADD COLUMN IF NOT EXISTS encryption_algorithm VARCHAR(50);
ALTER TABLE hammerwork_jobs_archive ADD COLUMN IF NOT EXISTS encrypted_payload LONGBLOB;
ALTER TABLE hammerwork_jobs_archive ADD COLUMN IF NOT EXISTS encryption_nonce BLOB;
ALTER TABLE hammerwork_jobs_archive ADD COLUMN IF NOT EXISTS encryption_tag BLOB;
ALTER TABLE hammerwork_jobs_archive ADD COLUMN IF NOT EXISTS encryption_metadata JSON;
ALTER TABLE hammerwork_jobs_archive ADD COLUMN IF NOT EXISTS payload_hash VARCHAR(255);
ALTER TABLE hammerwork_jobs_archive ADD COLUMN IF NOT EXISTS pii_fields JSON;
ALTER TABLE hammerwork_jobs_archive ADD COLUMN IF NOT EXISTS retention_policy VARCHAR(50);
ALTER TABLE hammerwork_jobs_archive ADD COLUMN IF NOT EXISTS retention_delete_at TIMESTAMP(6);
ALTER TABLE hammerwork_jobs_archive ADD COLUMN IF NOT EXISTS encrypted_at TIMESTAMP(6);

-- Create indexes for encryption keys table
CREATE INDEX idx_hammerwork_encryption_keys_key_id
    ON hammerwork_encryption_keys (key_id);

CREATE INDEX idx_hammerwork_encryption_keys_status
    ON hammerwork_encryption_keys (status);

CREATE INDEX idx_hammerwork_encryption_keys_rotation
    ON hammerwork_encryption_keys (next_rotation_at, status);

CREATE INDEX idx_hammerwork_encryption_keys_expires
    ON hammerwork_encryption_keys (expires_at, status);

CREATE INDEX idx_hammerwork_encryption_keys_algorithm
    ON hammerwork_encryption_keys (algorithm, status);

-- Create indexes for encrypted jobs
CREATE INDEX idx_hammerwork_jobs_encrypted
    ON hammerwork_jobs (is_encrypted, encryption_key_id);

CREATE INDEX idx_hammerwork_jobs_retention_cleanup
    ON hammerwork_jobs (retention_delete_at, is_encrypted);

CREATE INDEX idx_hammerwork_jobs_encrypted_at
    ON hammerwork_jobs (encrypted_at);

-- Create indexes for encrypted archive jobs
CREATE INDEX idx_hammerwork_jobs_archive_encrypted
    ON hammerwork_jobs_archive (is_encrypted, encryption_key_id);

CREATE INDEX idx_hammerwork_jobs_archive_retention_cleanup
    ON hammerwork_jobs_archive (retention_delete_at, is_encrypted);

-- Add check constraints for encryption consistency (MySQL 8.0+)
-- Note: For older MySQL versions, these would need to be enforced in application logic
ALTER TABLE hammerwork_jobs
ADD CONSTRAINT check_encryption_consistency 
CHECK (
    (is_encrypted = false AND encrypted_payload IS NULL AND encryption_nonce IS NULL AND encryption_tag IS NULL) OR
    (is_encrypted = true AND encrypted_payload IS NOT NULL AND encryption_nonce IS NOT NULL AND encryption_tag IS NOT NULL AND encryption_key_id IS NOT NULL)
);

ALTER TABLE hammerwork_jobs_archive
ADD CONSTRAINT check_archive_encryption_consistency 
CHECK (
    (is_encrypted = false AND encrypted_payload IS NULL AND encryption_nonce IS NULL AND encryption_tag IS NULL) OR
    (is_encrypted = true AND encrypted_payload IS NOT NULL AND encryption_nonce IS NOT NULL AND encryption_tag IS NOT NULL AND encryption_key_id IS NOT NULL)
);

-- Add check constraints for valid algorithms
ALTER TABLE hammerwork_jobs
ADD CONSTRAINT check_encryption_algorithm 
CHECK (encryption_algorithm IS NULL OR encryption_algorithm IN ('AES256GCM', 'ChaCha20Poly1305'));

ALTER TABLE hammerwork_jobs_archive
ADD CONSTRAINT check_archive_encryption_algorithm 
CHECK (encryption_algorithm IS NULL OR encryption_algorithm IN ('AES256GCM', 'ChaCha20Poly1305'));

-- Add check constraints for retention policies
ALTER TABLE hammerwork_jobs
ADD CONSTRAINT check_retention_policy 
CHECK (retention_policy IS NULL OR retention_policy IN ('DeleteAfter', 'DeleteAt', 'KeepIndefinitely', 'DeleteImmediately', 'UseDefault'));

ALTER TABLE hammerwork_jobs_archive
ADD CONSTRAINT check_archive_retention_policy 
CHECK (retention_policy IS NULL OR retention_policy IN ('DeleteAfter', 'DeleteAt', 'KeepIndefinitely', 'DeleteImmediately', 'UseDefault'));