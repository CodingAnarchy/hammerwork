-- Migration 016: Keep every version of an encryption key (MySQL)
--
-- Migration 011 declared hammerwork_encryption_keys.key_id UNIQUE, so rotating a key had to
-- overwrite the previous version and data encrypted with it could no longer be decrypted.
-- Key versions are now unique per (key_id, key_version).

ALTER TABLE hammerwork_encryption_keys
    ADD UNIQUE INDEX idx_hammerwork_encryption_keys_key_version (key_id, key_version);

ALTER TABLE hammerwork_encryption_keys
    DROP INDEX key_id;

-- Allow the 'Update' key operation in the audit log (KeyOperation::Update)
ALTER TABLE hammerwork_key_audit_log
    DROP CHECK check_operation;

ALTER TABLE hammerwork_key_audit_log
    ADD CONSTRAINT check_operation
    CHECK (operation IN ('Create', 'Access', 'Rotate', 'Retire', 'Revoke', 'Delete', 'Update'));
