-- Migration 016: Keep every version of an encryption key (MySQL)
--
-- Migration 011 declared hammerwork_encryption_keys.key_id UNIQUE, so rotating a key had to
-- overwrite the previous version and data encrypted with it could no longer be decrypted.
-- Key versions are now unique per (key_id, key_version).
--
-- Every statement is idempotent: MySQL DDL auto-commits, so a migration that fails
-- part-way leaves its earlier statements applied and must be safe to re-run.

SET @sql = IF(
    (SELECT COUNT(*) FROM information_schema.statistics
     WHERE table_schema = DATABASE() AND table_name = 'hammerwork_encryption_keys' AND index_name = 'idx_hammerwork_encryption_keys_key_version') = 0,
    'CREATE UNIQUE INDEX idx_hammerwork_encryption_keys_key_version ON hammerwork_encryption_keys (key_id, key_version)',
    'SELECT 1'
);
PREPARE stmt FROM @sql;
EXECUTE stmt;
DEALLOCATE PREPARE stmt;

-- Drop the implicit unique index created by `key_id ... UNIQUE` in migration 011
SET @sql = IF(
    (SELECT COUNT(*) FROM information_schema.statistics
     WHERE table_schema = DATABASE() AND table_name = 'hammerwork_encryption_keys' AND index_name = 'key_id') > 0,
    'DROP INDEX key_id ON hammerwork_encryption_keys',
    'SELECT 1'
);
PREPARE stmt FROM @sql;
EXECUTE stmt;
DEALLOCATE PREPARE stmt;

-- Allow the 'Update' key operation in the audit log (KeyOperation::Update)
SET @sql = IF(
    (SELECT COUNT(*) FROM information_schema.table_constraints
     WHERE constraint_schema = DATABASE() AND table_name = 'hammerwork_key_audit_log' AND constraint_name = 'check_operation'
       AND constraint_type = 'CHECK') > 0,
    'ALTER TABLE hammerwork_key_audit_log DROP CHECK check_operation',
    'SELECT 1'
);
PREPARE stmt FROM @sql;
EXECUTE stmt;
DEALLOCATE PREPARE stmt;

SET @sql = IF(
    (SELECT COUNT(*) FROM information_schema.table_constraints
     WHERE constraint_schema = DATABASE() AND table_name = 'hammerwork_key_audit_log' AND constraint_name = 'check_operation'
       AND constraint_type = 'CHECK') = 0,
    'ALTER TABLE hammerwork_key_audit_log ADD CONSTRAINT check_operation CHECK (operation IN (''Create'', ''Access'', ''Rotate'', ''Retire'', ''Revoke'', ''Delete'', ''Update''))',
    'SELECT 1'
);
PREPARE stmt FROM @sql;
EXECUTE stmt;
DEALLOCATE PREPARE stmt;
