-- Migration 011: Add job payload encryption and key management for PostgreSQL
-- Adds encryption fields to jobs table and creates key management tables

-- Create encryption keys table for secure key storage and rotation
CREATE TABLE IF NOT EXISTS hammerwork_encryption_keys (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    key_id VARCHAR NOT NULL UNIQUE,
    key_version INTEGER NOT NULL DEFAULT 1,
    algorithm VARCHAR NOT NULL, -- 'AES256GCM' or 'ChaCha20Poly1305'
    key_material BYTEA NOT NULL, -- Encrypted key material (never stored in plain text)
    key_derivation_salt BYTEA, -- Salt for key derivation if using password-based keys
    key_source VARCHAR NOT NULL, -- 'Environment', 'External', 'Generated', 'Derived'
    key_purpose VARCHAR NOT NULL DEFAULT 'Encryption', -- 'Encryption', 'MAC', 'KEK' (Key Encryption Key)
    
    -- Key metadata
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    created_by VARCHAR, -- Service or user that created the key
    expires_at TIMESTAMPTZ, -- When the key expires
    rotated_at TIMESTAMPTZ, -- When the key was rotated
    retired_at TIMESTAMPTZ, -- When the key was retired (kept for decryption only)
    
    -- Key status and rotation
    status VARCHAR NOT NULL DEFAULT 'Active', -- 'Active', 'Retired', 'Revoked', 'Expired'
    rotation_interval INTERVAL, -- How often to rotate this key
    next_rotation_at TIMESTAMPTZ, -- When the next rotation is due
    
    -- Security metadata
    key_strength INTEGER NOT NULL, -- Key length in bits (256, 512, etc.)
    master_key_id UUID, -- Reference to master key if this key is encrypted
    
    -- Audit trail
    last_used_at TIMESTAMPTZ,
    usage_count BIGINT NOT NULL DEFAULT 0,
    
    CONSTRAINT valid_algorithm CHECK (algorithm IN ('AES256GCM', 'ChaCha20Poly1305')),
    CONSTRAINT valid_key_source CHECK (key_source IN ('Environment', 'External', 'Generated', 'Derived', 'Static')),
    CONSTRAINT valid_key_purpose CHECK (key_purpose IN ('Encryption', 'MAC', 'KEK')),
    CONSTRAINT valid_status CHECK (status IN ('Active', 'Retired', 'Revoked', 'Expired')),
    CONSTRAINT valid_key_strength CHECK (key_strength >= 128 AND key_strength <= 512)
);

-- Add encryption fields to main jobs table
ALTER TABLE hammerwork_jobs ADD COLUMN IF NOT EXISTS is_encrypted BOOLEAN NOT NULL DEFAULT false;

ALTER TABLE hammerwork_jobs ADD COLUMN IF NOT EXISTS encryption_key_id VARCHAR; -- References hammerwork_encryption_keys.key_id

ALTER TABLE hammerwork_jobs ADD COLUMN IF NOT EXISTS encryption_algorithm VARCHAR; -- Algorithm used for this specific job

ALTER TABLE hammerwork_jobs ADD COLUMN IF NOT EXISTS encrypted_payload BYTEA; -- Encrypted payload data when is_encrypted = true

ALTER TABLE hammerwork_jobs ADD COLUMN IF NOT EXISTS encryption_nonce BYTEA; -- Nonce/IV used for encryption

ALTER TABLE hammerwork_jobs ADD COLUMN IF NOT EXISTS encryption_tag BYTEA; -- Authentication tag for AEAD ciphers

ALTER TABLE hammerwork_jobs ADD COLUMN IF NOT EXISTS encryption_metadata JSONB; -- Metadata about encryption (compression, PII fields, etc.)

ALTER TABLE hammerwork_jobs ADD COLUMN IF NOT EXISTS payload_hash VARCHAR; -- Hash of original payload for integrity verification

ALTER TABLE hammerwork_jobs ADD COLUMN IF NOT EXISTS pii_fields TEXT[]; -- Array of field names containing PII

ALTER TABLE hammerwork_jobs ADD COLUMN IF NOT EXISTS retention_policy VARCHAR; -- Retention policy for encrypted data

ALTER TABLE hammerwork_jobs ADD COLUMN IF NOT EXISTS retention_delete_at TIMESTAMPTZ; -- When to delete encrypted data based on retention policy

ALTER TABLE hammerwork_jobs ADD COLUMN IF NOT EXISTS encrypted_at TIMESTAMPTZ; -- When the payload was encrypted

-- Add encryption fields to archive table
ALTER TABLE hammerwork_jobs_archive ADD COLUMN IF NOT EXISTS is_encrypted BOOLEAN NOT NULL DEFAULT false;

ALTER TABLE hammerwork_jobs_archive ADD COLUMN IF NOT EXISTS encryption_key_id VARCHAR;

ALTER TABLE hammerwork_jobs_archive ADD COLUMN IF NOT EXISTS encryption_algorithm VARCHAR;

ALTER TABLE hammerwork_jobs_archive ADD COLUMN IF NOT EXISTS encrypted_payload BYTEA;

ALTER TABLE hammerwork_jobs_archive ADD COLUMN IF NOT EXISTS encryption_nonce BYTEA;

ALTER TABLE hammerwork_jobs_archive ADD COLUMN IF NOT EXISTS encryption_tag BYTEA;

ALTER TABLE hammerwork_jobs_archive ADD COLUMN IF NOT EXISTS encryption_metadata JSONB;

ALTER TABLE hammerwork_jobs_archive ADD COLUMN IF NOT EXISTS payload_hash VARCHAR;

ALTER TABLE hammerwork_jobs_archive ADD COLUMN IF NOT EXISTS pii_fields TEXT[];

ALTER TABLE hammerwork_jobs_archive ADD COLUMN IF NOT EXISTS retention_policy VARCHAR;

ALTER TABLE hammerwork_jobs_archive ADD COLUMN IF NOT EXISTS retention_delete_at TIMESTAMPTZ;

ALTER TABLE hammerwork_jobs_archive ADD COLUMN IF NOT EXISTS encrypted_at TIMESTAMPTZ;

-- Create indexes for encryption keys table
CREATE INDEX IF NOT EXISTS idx_hammerwork_encryption_keys_key_id
    ON hammerwork_encryption_keys (key_id);

CREATE INDEX IF NOT EXISTS idx_hammerwork_encryption_keys_status
    ON hammerwork_encryption_keys (status) WHERE status = 'Active';

CREATE INDEX IF NOT EXISTS idx_hammerwork_encryption_keys_rotation
    ON hammerwork_encryption_keys (next_rotation_at) 
    WHERE next_rotation_at IS NOT NULL AND status = 'Active';

CREATE INDEX IF NOT EXISTS idx_hammerwork_encryption_keys_expires
    ON hammerwork_encryption_keys (expires_at) 
    WHERE expires_at IS NOT NULL AND status = 'Active';

CREATE INDEX IF NOT EXISTS idx_hammerwork_encryption_keys_algorithm
    ON hammerwork_encryption_keys (algorithm, status);

-- Create indexes for encrypted jobs
CREATE INDEX IF NOT EXISTS idx_hammerwork_jobs_encrypted
    ON hammerwork_jobs (is_encrypted, encryption_key_id) WHERE is_encrypted = true;

CREATE INDEX IF NOT EXISTS idx_hammerwork_jobs_retention_cleanup
    ON hammerwork_jobs (retention_delete_at) 
    WHERE retention_delete_at IS NOT NULL AND is_encrypted = true;

CREATE INDEX IF NOT EXISTS idx_hammerwork_jobs_pii
    ON hammerwork_jobs USING GIN (pii_fields) 
    WHERE pii_fields IS NOT NULL AND array_length(pii_fields, 1) > 0;

CREATE INDEX IF NOT EXISTS idx_hammerwork_jobs_encrypted_at
    ON hammerwork_jobs (encrypted_at) WHERE encrypted_at IS NOT NULL;

-- Create indexes for encrypted archive jobs
CREATE INDEX IF NOT EXISTS idx_hammerwork_jobs_archive_encrypted
    ON hammerwork_jobs_archive (is_encrypted, encryption_key_id) WHERE is_encrypted = true;

CREATE INDEX IF NOT EXISTS idx_hammerwork_jobs_archive_retention_cleanup
    ON hammerwork_jobs_archive (retention_delete_at) 
    WHERE retention_delete_at IS NOT NULL AND is_encrypted = true;

CREATE INDEX IF NOT EXISTS idx_hammerwork_jobs_archive_pii
    ON hammerwork_jobs_archive USING GIN (pii_fields) 
    WHERE pii_fields IS NOT NULL AND array_length(pii_fields, 1) > 0;

-- Add constraints for encryption consistency
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