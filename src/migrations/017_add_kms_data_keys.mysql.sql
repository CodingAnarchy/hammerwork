-- Migration 017: Persist KMS-wrapped data keys (MySQL)
--
-- `aws://` and `gcp://` key sources use envelope encryption: on first use a data key is
-- generated with the KMS and only its KMS-encrypted form (AWS CiphertextBlob, GCP
-- ciphertext) is stored here. Later loads call KMS Decrypt on the stored blob, so every
-- process and every restart gets the same key. Plaintext key material is never stored.
--
-- Keys are versioned per (key_name, kms_provider, kms_key_id): rotation inserts a new
-- Active version and retires the previous one, which stays available for decryption.

CREATE TABLE IF NOT EXISTS hammerwork_kms_data_keys (
    id CHAR(36) PRIMARY KEY,
    key_name VARCHAR(191) NOT NULL,      -- What the key is for, e.g. 'key-manager/master'
    kms_provider VARCHAR(16) NOT NULL,   -- 'aws' or 'gcp'
    kms_key_id VARCHAR(512) NOT NULL,    -- AWS key id/ARN/alias or GCP CryptoKey resource name
    key_version INT NOT NULL,
    key_size INT NOT NULL,               -- Plaintext key length in bytes
    wrapped_key BLOB NOT NULL,           -- KMS-encrypted data key
    status VARCHAR(20) NOT NULL DEFAULT 'Active',
    created_at TIMESTAMP(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    retired_at TIMESTAMP(6) NULL,

    CONSTRAINT check_kms_provider CHECK (kms_provider IN ('aws', 'gcp')),
    CONSTRAINT check_kms_data_key_status CHECK (status IN ('Active', 'Retired')),
    UNIQUE INDEX idx_hammerwork_kms_data_keys_version
        (key_name, kms_provider, kms_key_id, key_version)
);
