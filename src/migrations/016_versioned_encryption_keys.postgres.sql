-- Migration 016: Keep every version of an encryption key (PostgreSQL)
--
-- Migration 011 declared hammerwork_encryption_keys.key_id UNIQUE, so rotating a key had to
-- overwrite the previous version and data encrypted with it could no longer be decrypted.
-- Key versions are now unique per (key_id, key_version).

ALTER TABLE hammerwork_encryption_keys
    DROP CONSTRAINT IF EXISTS hammerwork_encryption_keys_key_id_key;

CREATE UNIQUE INDEX IF NOT EXISTS idx_hammerwork_encryption_keys_key_version
    ON hammerwork_encryption_keys (key_id, key_version);

-- Allow the 'Update' key operation in the audit log (KeyOperation::Update)
ALTER TABLE hammerwork_key_audit_log
    DROP CONSTRAINT IF EXISTS valid_operation;

ALTER TABLE hammerwork_key_audit_log
    ADD CONSTRAINT valid_operation
    CHECK (operation IN ('Create', 'Access', 'Rotate', 'Retire', 'Revoke', 'Delete', 'Update'));
