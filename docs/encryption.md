# Job Encryption & PII Protection

Hammerwork provides enterprise-grade encryption capabilities for protecting sensitive job payloads, particularly personally identifiable information (PII). This document covers encryption configuration, key management, and best practices for data protection.

## Table of Contents

- [Overview](#overview)
- [Encryption Algorithms](#encryption-algorithms)
- [Configuration](#configuration)
- [Encrypting Jobs at Rest](#encrypting-jobs-at-rest)
- [PII Field Protection](#pii-field-protection)
- [Key Management](#key-management)
- [Retention Policies](#retention-policies)
- [Database Schema](#database-schema)
- [Examples](#examples)
- [Security Considerations](#security-considerations)
- [Performance](#performance)
- [Compliance](#compliance)

## Overview

The Hammerwork encryption system provides:

- **Encryption at rest**: a queue with an `EncryptionEngine` encrypts the payload of every job that has an `EncryptionConfig` before it is written, on PostgreSQL and MySQL. The plaintext never reaches the database.
- **Field-Level Encryption**: encrypt only the PII fields of a payload, leaving the rest readable
- **Multiple Algorithms**: AES-256-GCM and ChaCha20-Poly1305
- **Decryption only for the handler**: workers decrypt the payload just before calling the handler; everything else (dashboards, CLI, `get_job`) sees the redacted payload
- **Fail closed**: missing engines or keys are errors, never a silent fallback to plaintext
- **Key Management**: static, environment, KMS (AWS, GCP), Vault and Azure Key Vault keys, decrypt-only keys for rotated key ids, and the `KeyManager` for stored keys with audit trails
- **Retention Policies**: finished encrypted jobs are deleted after their retention period by `purge_expired_encrypted_jobs`
- **Zero Overhead**: optional compilation - only enabled with the `encryption` feature

## Encryption Algorithms

### AES-256-GCM (Recommended)

- **Algorithm**: Advanced Encryption Standard with Galois/Counter Mode
- **Key Size**: 256 bits (32 bytes)
- **Features**: Authenticated encryption (AEAD), hardware acceleration
- **Use Case**: General purpose, high performance requirements

```rust
use hammerwork::encryption::{EncryptionConfig, EncryptionAlgorithm};

let config = EncryptionConfig::new(EncryptionAlgorithm::AES256GCM);
```

### ChaCha20-Poly1305

- **Algorithm**: ChaCha20 stream cipher with Poly1305 MAC
- **Key Size**: 256 bits (32 bytes) 
- **Features**: Constant-time execution, mobile/embedded friendly
- **Use Case**: Environments without AES hardware acceleration

```rust
use hammerwork::encryption::{EncryptionConfig, EncryptionAlgorithm};

let config = EncryptionConfig::new(EncryptionAlgorithm::ChaCha20Poly1305);
```

## Configuration

### Basic Configuration

```rust
use hammerwork::encryption::{EncryptionConfig, EncryptionAlgorithm, KeySource};

// Environment variable key source
let config = EncryptionConfig::new(EncryptionAlgorithm::AES256GCM)
    .with_key_source(KeySource::Environment("HAMMERWORK_ENCRYPTION_KEY".to_string()));
```

### Advanced Configuration

```rust
use hammerwork::encryption::{EncryptionConfig, KeySource};
use std::time::Duration;

let config = EncryptionConfig::new(EncryptionAlgorithm::AES256GCM)
    .with_key_source(KeySource::Environment("ENCRYPTION_KEY".to_string()))
    .with_key_rotation_enabled(true)
    .with_key_rotation_interval(Duration::from_secs(30 * 24 * 60 * 60)) // 30 days
    .with_compression_enabled(true)
    .with_compression_threshold(1024); // Compress payloads > 1KB
```

### Key Sources

#### Environment Variable (Recommended)

```rust
let config = EncryptionConfig::new(EncryptionAlgorithm::AES256GCM)
    .with_key_source(KeySource::Environment("HAMMERWORK_ENCRYPTION_KEY".to_string()));
```

Generate a secure key:
```bash
# Generate base64-encoded 256-bit key
openssl rand -base64 32
export HAMMERWORK_ENCRYPTION_KEY="your-generated-key-here"
```

#### Static Key (Testing Only)

```rust
// WARNING: Only for testing - never use static keys in production
let config = EncryptionConfig::new(EncryptionAlgorithm::AES256GCM)
    .with_key_source(KeySource::Static("base64-encoded-key".to_string()));
```

#### External KMS

Hammerwork supports multiple external Key Management Services for enterprise key management:

##### AWS KMS

```rust
let config = EncryptionConfig::new(EncryptionAlgorithm::AES256GCM)
    .with_key_source(KeySource::External("aws://alias/hammerwork-key?region=us-east-1".to_string()));
let engine = EncryptionEngine::new_with_pool(config, &pool).await?;
```

Add `&endpoint=<url>` to use a different KMS endpoint (for example LocalStack). Requires the `aws-kms` feature. The application needs `kms:GenerateDataKey` and `kms:Decrypt` on the key.

##### Google Cloud KMS

```rust
let config = EncryptionConfig::new(EncryptionAlgorithm::AES256GCM)
    .with_key_source(KeySource::External("gcp://projects/PROJECT/locations/LOCATION/keyRings/RING/cryptoKeys/KEY".to_string()));
let engine = EncryptionEngine::new_with_pool(config, &pool).await?;
```

Requires the `gcp-kms` feature. The application needs `cloudkms.locations.generateRandomBytes` on the location and `cloudkms.cryptoKeyVersions.useToEncrypt` / `useToDecrypt` on the key (`roles/cloudkms.cryptoKeyEncrypterDecrypter` plus a role that allows generating random bytes).

##### Envelope encryption for AWS and GCP KMS

A KMS does not hand out the same key twice, so `aws://` and `gcp://` sources use envelope encryption with a key stored in the database:

1. On first use, Hammerwork generates a data key with the KMS (AWS `GenerateDataKey`; GCP `GenerateRandomBytes` followed by `Encrypt`) and stores **only the KMS-encrypted key** in `hammerwork_kms_data_keys` (migration `017_add_kms_data_keys`), with the KMS key id, version and creation time. The plaintext key is never written to the database.
2. Every later load, in any process, reads the stored blob and calls KMS `Decrypt`, so restarts and other workers get the same key.
3. Processes that start at the same time converge on one key: version 1 is unique per key, the first insert wins, and the others read back and decrypt the stored key.

Stored keys are scoped by name (`key-manager/master` for the `KeyManager` master key, `engine/<key-id>` for an `EncryptionEngine` data key), provider and KMS key id. The KMS ciphertext is bound to that name (AWS encryption context `hammerwork:key-name`, GCP additional authenticated data), so a blob cannot be reused for another purpose. Pointing a source at a different KMS key creates a new, separate key.

- `KeyManager::new` handles `aws://` / `gcp://` master key sources this way automatically.
- `EncryptionEngine` needs the database: create it with `EncryptionEngine::new_with_pool(config, &pool)`. `EncryptionEngine::new` returns `EncryptionError::InvalidConfiguration` for these sources instead of generating a key that would be lost on restart.

**Rotation.** `KeyManager::rotate_kms_master_key()` and `EncryptionEngine::rotate_kms_key(&pool)` generate a new data key with the KMS, store it as the new active version and retire the previous one. Retired versions are kept and stay decryptable: the key manager decrypts keys wrapped by an older master key version (including key-encryption keys from `generate_master_key`), and the engine falls back to older versions when decrypting payloads. Other running instances keep the version they loaded until they are recreated. Rotating the KMS key itself inside AWS or GCP (automatic key rotation) needs no action: the KMS still decrypts blobs encrypted under earlier key versions.

**Upgrading.** Before this change, each process asked the KMS for a fresh key on every start, so anything encrypted by an earlier process was already unrecoverable. After running migration 017, the first start generates and stores a key; from then on it is stable. Data encrypted by the old per-process keys cannot be recovered.

##### HashiCorp Vault KMS

```rust
let config = EncryptionConfig::new(EncryptionAlgorithm::AES256GCM)
    .with_key_source(KeySource::External("vault://secret/hammerwork/encryption-key".to_string()));
```

With custom Vault address:

```rust
let config = EncryptionConfig::new(EncryptionAlgorithm::AES256GCM)
    .with_key_source(KeySource::External("vault://secret/hammerwork/encryption-key?addr=https://vault.example.com".to_string()));
```

**Environment Variables:**
- `VAULT_ADDR`: Vault server address, used when the source has no `addr=` parameter (required: there is no default)
- `VAULT_TOKEN`: Authentication token for Vault access

**Vault Requirements:**
- KV v2 secrets engine enabled
- Secret stored with `key` field containing base64-encoded key material
- Proper authentication and access policies configured
- The `vault-kms` feature

##### Azure Key Vault

```rust
let config = EncryptionConfig::new(EncryptionAlgorithm::AES256GCM)
    .with_key_source(KeySource::External("azure://my-vault.vault.azure.net/keys/encryption-key".to_string()));
```

Credentials come from the environment (`AZURE_TENANT_ID` / `AZURE_CLIENT_ID` / `AZURE_CLIENT_SECRET`, workload identity, managed identity, or the Azure CLI). Requires the `azure-kv` feature.

#### Key loading fails closed

Loading a key from an external source never falls back to another key. `EncryptionEngine::new` and `KeyManager::new` return an error when:

- the KMS or vault is unreachable, or rejects the credentials
- the key, secret or `key` field does not exist
- the source is malformed (for example a Vault path without a mount, or a GCP resource that is not `projects/.../locations/...`)
- the cargo feature for the source (`aws-kms`, `gcp-kms`, `vault-kms`, `azure-kv`) is not enabled

Previously, these cases logged an error and used a key derived from the source string (key ID, region, Vault path, vault URL). Anyone who knew the configuration could derive that key. For development without a KMS, use `KeySource::Static` with a base64-encoded key, or `KeySource::Generated`.

A stored KMS-wrapped key that the configured KMS key cannot decrypt is also an error; Hammerwork never replaces it with a new key.

## Encrypting Jobs at Rest

Give the queue an engine with `JobQueue::with_encryption`, and mark the jobs to encrypt with `Job::with_encryption`:

```rust
use hammerwork::{Job, JobQueue, queue::DatabaseQueue};
use hammerwork::encryption::{EncryptionAlgorithm, EncryptionConfig, EncryptionEngine, KeySource};
use serde_json::json;
use std::sync::Arc;

let config = EncryptionConfig::new(EncryptionAlgorithm::AES256GCM)
    .with_key_id("payments-2026")
    .with_key_source(KeySource::Environment("HAMMERWORK_ENCRYPTION_KEY".to_string()));
let engine = EncryptionEngine::new(config.clone()).await?;
// aws:// and gcp:// key sources: EncryptionEngine::new_with_pool(config, &pool)
let queue = Arc::new(JobQueue::new(pool).with_encryption(engine));

let job = Job::new("payments".to_string(), json!({"card": "4111-1111-1111-1111", "amount": 10}))
    .with_encryption(config);
queue.enqueue(job).await?;
```

How it works:

- **Enqueue.** `enqueue`, `enqueue_batch`, `enqueue_workflow` and `enqueue_cron_job` encrypt every job that has an encryption config before anything is written (a batch with one job that cannot be encrypted writes nothing). The ciphertext, nonce, tag, key id, algorithm, metadata, keyed integrity hash, PII field list and retention columns of migration 011 are filled in; `is_encrypted` is set.
- **What `payload` holds.** Never the plaintext of encrypted data. With no PII fields the whole payload is encrypted and `payload` is the placeholder `{"encrypted": true}`. With PII fields (see below) `payload` is the original payload with each listed field's value replaced by `"[ENCRYPTED]"`.
- **The engine decides.** The queue's engine sets the algorithm, key and compression. A job whose config names another algorithm, or a `key_id` other than the engine's, is rejected. Jobs without an encryption config are stored unchanged, also on a queue with an engine.
- **Fail closed.** Enqueueing a job that has an encryption config on a queue without an engine fails with `HammerworkError::Encryption`.
- **Dequeue and reads.** `dequeue`, `get_job`, `get_batch_jobs` and the other reads return jobs as stored: `is_encrypted` is `true`, `payload` is redacted and `encrypted_payload` holds the ciphertext. The web dashboard and `cargo hammerwork job show` therefore show the redacted payload (`job show` also prints the key id, algorithm and retention). They never need a key.
- **Workers.** A worker decrypts the job (`JobQueue::decrypt_job`) just before it calls the handler; only the handler sees the plaintext. Event hooks, webhooks and the recorded outcome keep using the redacted job. If the payload cannot be decrypted (the worker's queue has no engine, the engine does not have the job's key, or the data was tampered with) the run fails with `Cannot decrypt the payload of job ...` and goes through the normal retry / dead path; the handler is not called.
- **Reading the plaintext yourself.** `queue.decrypt_job(job).await?` returns the job with its plaintext payload.
- **Without the `encryption` feature** a build cannot encrypt or decrypt. It still reads `is_encrypted`, so its workers fail encrypted jobs instead of running them with the redacted payload, and archiving and restoring still carry the ciphertext.

The payload hash stored with the ciphertext is an HMAC-SHA256 keyed by the data key (`hmac-sha256:<hex>`), not a plain SHA-256, so a low-entropy value (an SSN, a card number) cannot be recovered from it by brute force. Payloads encrypted by earlier versions with a plain SHA-256 still decrypt.

### Key Rotation for Job Payloads

Each encrypted job records the id of the key it was encrypted with. To switch to a new key, give the engine a new `key_id` and keep the previous key for decryption:

```rust
let engine = EncryptionEngine::new(
    EncryptionConfig::new(EncryptionAlgorithm::AES256GCM)
        .with_key_id("payments-2027")
        .with_key_source(KeySource::Environment("HAMMERWORK_KEY_2027".to_string())),
)
.await?
.with_decryption_key(
    "payments-2026",
    &KeySource::Environment("HAMMERWORK_KEY_2026".to_string()),
)
.await?;
```

New jobs are encrypted with `payments-2027`; jobs written with `payments-2026` still decrypt. For `aws://` and `gcp://` sources, `EncryptionEngine::rotate_kms_key` adds a new key version under the same key id and keeps earlier versions for decryption (see above).

## PII Field Protection

To encrypt only some fields, list them with `with_pii_fields`:

```rust
let job = Job::new("user_data_processing".to_string(), json!({
    "user_id": "user123",
    "credit_card": "4111-1111-1111-1111",
    "billing": {"address": "123 Main St", "country": "US"},
    "preferences": {"newsletter": true}
}))
.with_encryption(EncryptionConfig::new(EncryptionAlgorithm::AES256GCM))
.with_pii_fields(vec!["credit_card", "billing.address"]);
```

is stored with `payload`

```json
{"user_id": "user123", "credit_card": "[ENCRYPTED]", "billing": {"address": "[ENCRYPTED]", "country": "US"}, "preferences": {"newsletter": true}}
```

and the values of the two fields encrypted in `encrypted_payload`. The handler receives the original payload.

- Field names are paths into the JSON object: `"ssn"` is a top-level field and `"billing.address"` the `address` field of the `billing` object. A top-level key that itself contains a dot is matched literally first. Arrays are not traversed; list the array's parent field to encrypt it whole.
- A field's whole value is encrypted, whatever its type (string, number, object, array).
- Listed fields that are missing from a payload are skipped. Only the listed fields are protected: anything else in the payload is stored in plaintext. When in doubt, encrypt the whole payload (no PII fields).
- PII field encryption needs a JSON object payload.
- `with_pii_fields` on its own (without `with_encryption`) only records the field names in the `pii_fields` column; nothing is encrypted.

### Finding PII Fields

`EncryptionEngine::identify_pii_fields(&payload)` suggests field names that look like PII (`ssn`, `email`, `credit_card`, `phone`, `address`, `password`, `date_of_birth`, ...). It matches field names only, not values; review its output before passing it to `with_pii_fields`.

## Key Management

### Basic Key Management

```rust
use hammerwork::encryption::{KeyManager, KeyManagerConfig, EncryptionAlgorithm};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = KeyManagerConfig::new()
        .with_master_key_env("HAMMERWORK_MASTER_KEY")
        .with_auto_rotation_enabled(true)
        .with_rotation_interval(chrono::Duration::days(90));

    let mut key_manager = KeyManager::new(config, pool).await?;

    // Generate a new encryption key
    let key_id = key_manager.generate_key(
        "payment-encryption", 
        EncryptionAlgorithm::AES256GCM
    ).await?;

    // Use the key
    let key_material = key_manager.get_key(&key_id).await?;

    Ok(())
}
```

### Key Rotation

```rust
// Manual key rotation
let new_version = key_manager.rotate_key("payment-encryption").await?;
println!("Rotated to version {}", new_version);

// Older versions stay available to decrypt data encrypted before the rotation
// (up to `max_key_versions` versions are kept)
let previous = key_manager.get_key_version("payment-encryption", new_version - 1).await?;

// Automatic rotation (configured intervals)
let rotated_keys = key_manager.perform_automatic_rotation().await?;
for key_id in rotated_keys {
    println!("Auto-rotated key: {}", key_id);
}
```

### Key Storage and Master Keys

`KeyManager` stores every key version in `hammerwork_encryption_keys` (PostgreSQL and MySQL). Key material is encrypted with AES-256-GCM under a master key before it is written; plaintext key material and the configured master key are never stored. The `key_source` column holds only a label (`Generated`, `Environment`, ...).

The master key is loaded from `master_key_source` when the `KeyManager` is created, and loading fails closed (see above). `generate_master_key()` creates a key-encryption key, stores it encrypted with the configured master key, and uses it for keys generated or rotated afterwards. Keys encrypted with earlier master keys remain readable, and a new `KeyManager` loads the active key-encryption key from the database. A `KeyManager` configured with a different master key fails to start instead of silently using the wrong key.

### Key Audit Trails

```rust
// Get key usage statistics
let stats = key_manager.get_stats().await;
println!("Total keys: {}", stats.total_keys);
println!("Active keys: {}", stats.active_keys);
println!("Rotations performed: {}", stats.rotations_performed);

// Query audit records (requires custom implementation)
let audit_records = key_manager.get_audit_trail("payment-encryption", None, None).await?;
for record in audit_records {
    println!("{:?}: {} by {:?}", 
        record.timestamp, 
        record.operation, 
        record.actor
    );
}
```

## Retention Policies

### Policy Types

```rust
use hammerwork::encryption::RetentionPolicy;
use std::time::Duration;
use chrono::{Utc, Duration as ChronoDuration};

// Delete after a time period
let policy = RetentionPolicy::DeleteAfter(Duration::from_secs(7 * 24 * 60 * 60)); // 7 days

// Delete at specific time
let policy = RetentionPolicy::DeleteAt(Utc::now() + ChronoDuration::days(30));

// Keep indefinitely
let policy = RetentionPolicy::KeepIndefinitely;

// Delete immediately after processing
let policy = RetentionPolicy::DeleteImmediately;

// Use system default
let policy = RetentionPolicy::UseDefault;
```

### Enforcing Retention

When a job is encrypted, its retention policy (`Job::with_retention_policy`, or `UseDefault`, which uses the engine's `default_retention`) is stored in `retention_policy` and the deletion time in `retention_delete_at`. Nothing is deleted automatically; run the purge periodically:

```rust
let purge = queue.purge_expired_encrypted_jobs().await?;
println!("deleted {} jobs and {} archived jobs", purge.jobs, purge.archived_jobs);
```

or from the CLI (e.g. from cron):

```bash
cargo hammerwork maintenance purge-encrypted --dry-run
cargo hammerwork maintenance purge-encrypted --confirm
```

The purge deletes the whole job row (ciphertext, redacted payload and result) of encrypted jobs whose `retention_delete_at` has passed and that are finished (`Completed`, `Failed`, `Dead` or `TimedOut`), and encrypted jobs in `hammerwork_jobs_archive` past their retention time. It needs no key.

- Pending, running and retrying jobs are never deleted, even after their retention time; they are deleted by the first purge after they finish.
- `DeleteImmediately` jobs are deleted by the first purge after they finish.
- `KeepIndefinitely`, and `UseDefault` without a `default_retention`, are never purged.
- A deleted `Dead` job can no longer be retried. Dependents of a deleted job that were still waiting on it stay waiting.

### Compliance Examples

```rust
// GDPR compliance (right to be forgotten)
let gdpr_job = Job::new("user_data_export".to_string(), user_data)
    .with_encryption(encryption_config.clone())
    .with_pii_fields(vec!["personal_data", "preferences"])
    .with_retention_policy(RetentionPolicy::DeleteAfter(Duration::from_secs(30 * 24 * 60 * 60))); // 30 days

// HIPAA compliance (healthcare data)
let hipaa_job = Job::new("patient_record_processing".to_string(), patient_data)
    .with_encryption(encryption_config.clone())
    .with_pii_fields(vec!["medical_record", "patient_info"])
    .with_retention_policy(RetentionPolicy::DeleteAfter(Duration::from_secs(6 * 365 * 24 * 60 * 60))); // 6 years

// PCI DSS compliance (payment data)
let pci_job = Job::new("payment_processing".to_string(), payment_data)
    .with_encryption(encryption_config.clone())
    .with_pii_fields(vec!["card_number", "cvv"])
    .with_retention_policy(RetentionPolicy::DeleteAfter(Duration::from_secs(365 * 24 * 60 * 60))); // 1 year
```

## Database Schema

### Jobs Table Extensions

Migration `011_add_encryption` adds these columns to `hammerwork_jobs` and `hammerwork_jobs_archive`:

| Column | PostgreSQL | MySQL | Contents |
|---|---|---|---|
| `is_encrypted` | `BOOLEAN` | `BOOLEAN` | Whether `encrypted_payload` holds the job's encrypted data |
| `encryption_key_id` | `VARCHAR` | `VARCHAR(255)` | Id of the key the payload was encrypted with |
| `encryption_algorithm` | `VARCHAR` | `VARCHAR(50)` | `AES256GCM` or `ChaCha20Poly1305` |
| `encrypted_payload` | `BYTEA` | `LONGBLOB` | Ciphertext (the whole payload, or the PII fields' values) |
| `encryption_nonce` | `BYTEA` | `BLOB` | 96-bit nonce |
| `encryption_tag` | `BYTEA` | `BLOB` | 128-bit authentication tag |
| `encryption_metadata` | `JSONB` | `JSON` | Algorithm, key id, compression, encrypted field names, retention |
| `payload_hash` | `VARCHAR` | `VARCHAR(255)` | Keyed integrity hash of the plaintext (`hmac-sha256:<hex>`) |
| `pii_fields` | `TEXT[]` | `JSON` | Field names listed with `with_pii_fields` |
| `retention_policy` | `VARCHAR` | `VARCHAR(50)` | `DeleteAfter`, `DeleteAt`, `KeepIndefinitely`, `DeleteImmediately` or `UseDefault` |
| `retention_delete_at` | `TIMESTAMPTZ` | `TIMESTAMP(6)` | When the job may be purged |
| `encrypted_at` | `TIMESTAMPTZ` | `TIMESTAMP(6)` | When the payload was encrypted |

A CHECK constraint requires `encrypted_payload`, `encryption_nonce`, `encryption_tag` and `encryption_key_id` whenever `is_encrypted` is true. Archiving and restoring copy these columns between the two tables as they are; the payload is never decrypted on the way.

### Encryption Keys Table

```sql
-- PostgreSQL
CREATE TABLE hammerwork_encryption_keys (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    key_id VARCHAR(255) NOT NULL,
    version INTEGER NOT NULL DEFAULT 1,
    algorithm VARCHAR(50) NOT NULL,
    encrypted_key_material BYTEA NOT NULL,
    derivation_salt BYTEA,
    source VARCHAR(100) NOT NULL,
    purpose VARCHAR(50) NOT NULL DEFAULT 'encryption',
    created_at TIMESTAMPTZ DEFAULT NOW(),
    created_by VARCHAR(255),
    expires_at TIMESTAMPTZ,
    rotated_at TIMESTAMPTZ,
    retired_at TIMESTAMPTZ,
    status VARCHAR(20) DEFAULT 'active',
    rotation_interval_days INTEGER,
    next_rotation_at TIMESTAMPTZ,
    key_strength INTEGER NOT NULL,
    master_key_id UUID,
    last_used_at TIMESTAMPTZ,
    usage_count BIGINT DEFAULT 0,
    
    UNIQUE(key_id, version)
);

CREATE INDEX idx_encryption_keys_key_id ON hammerwork_encryption_keys(key_id);
CREATE INDEX idx_encryption_keys_status ON hammerwork_encryption_keys(status);
CREATE INDEX idx_encryption_keys_next_rotation ON hammerwork_encryption_keys(next_rotation_at)
    WHERE next_rotation_at IS NOT NULL;
```

## Examples

### Complete Encryption Workflow

```rust
use hammerwork::{Job, JobQueue, Worker, WorkerPool, queue::DatabaseQueue, worker::JobHandler};
use hammerwork::encryption::{
    EncryptionAlgorithm, EncryptionConfig, EncryptionEngine, KeySource, RetentionPolicy,
};
use serde_json::json;
use std::{sync::Arc, time::Duration};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let pool = sqlx::PgPool::connect("postgresql://localhost/hammerwork").await?;

    // Configure encryption
    let encryption_config = EncryptionConfig::new(EncryptionAlgorithm::AES256GCM)
        .with_key_source(KeySource::Environment("HAMMERWORK_ENCRYPTION_KEY".to_string()))
        .with_compression_enabled(true);
    let engine = EncryptionEngine::new(encryption_config.clone()).await?;
    let queue = Arc::new(JobQueue::new(pool).with_encryption(engine));

    // Create encrypted job
    let job = Job::new("sensitive_data_processing".to_string(), json!({
        "user_id": "user123",
        "credit_card": "4111-1111-1111-1111",
        "ssn": "123-45-6789",
        "transaction_amount": 299.99,
    }))
    .with_encryption(encryption_config)
    .with_pii_fields(vec!["credit_card", "ssn"])
    .with_retention_policy(RetentionPolicy::DeleteAfter(Duration::from_secs(30 * 24 * 60 * 60)));

    // Encrypted before it is written
    queue.enqueue(job).await?;

    // The worker decrypts the payload just before calling the handler
    let handler: JobHandler = Arc::new(|job: Job| {
        Box::pin(async move {
            println!("Processing transaction: {}", job.payload["transaction_amount"]);
            let _credit_card = job.payload["credit_card"].as_str().unwrap_or_default();
            Ok(())
        })
    });

    let worker = Worker::new(queue.clone(), "sensitive_data_processing".to_string(), handler);
    let mut worker_pool = WorkerPool::new();
    worker_pool.add_worker(worker);

    worker_pool.start().await
}
```

### Key Management Example

```rust
use hammerwork::encryption::{KeyManager, KeyManagerConfig, EncryptionAlgorithm, KeySource};
use chrono::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let pool = sqlx::PgPool::connect("postgresql://localhost/hammerwork").await?;

    // Configure key manager
    let config = KeyManagerConfig::new()
        .with_master_key_env("HAMMERWORK_MASTER_KEY")
        .with_auto_rotation_enabled(true)
        .with_rotation_interval(Duration::days(90))
        .with_max_key_versions(5)
        .with_audit_enabled(true);

    let mut key_manager = KeyManager::new(config, pool).await?;

    // Generate keys for different purposes
    let payment_key = key_manager.generate_key(
        "payment-processing",
        EncryptionAlgorithm::AES256GCM
    ).await?;

    let user_data_key = key_manager.generate_key(
        "user-data",
        EncryptionAlgorithm::ChaCha20Poly1305
    ).await?;

    // Rotate a key manually
    let new_version = key_manager.rotate_key("payment-processing").await?;
    println!("Payment key rotated to version {}", new_version);

    // Get statistics
    let stats = key_manager.get_stats().await;
    println!("Managing {} keys with {} rotations", stats.total_keys, stats.rotations_performed);

    Ok(())
}
```

## Security Considerations

### Key Security

1. **Master Key Protection**: Store master keys in secure key management systems
2. **Key Rotation**: Implement regular key rotation (recommended: 90 days)
3. **Access Control**: Limit key access to authorized systems only
4. **Audit Logging**: Enable comprehensive audit trails for compliance

### Encryption Security

1. **Algorithm Selection**: Use AES-256-GCM for maximum security
2. **Nonce Uniqueness**: Each encryption operation uses a unique nonce
3. **Authenticated Encryption**: All algorithms provide integrity protection
4. **Secure Random**: Keys generated using cryptographically secure random sources

### Operational Security

1. **Environment Variables**: Store keys in environment variables, not code
2. **Secure Transmission**: Use TLS for all database connections
3. **Memory Protection**: Keys are held in process memory while the engine exists; they are not printed by `Debug`
4. **Error Handling**: Encryption errors never include payload data

## Performance

### Encryption Overhead

- **AES-256-GCM**: ~10-20% overhead with hardware acceleration
- **ChaCha20-Poly1305**: ~15-25% overhead, consistent across platforms
- **Field-Level**: Only PII fields encrypted, minimal metadata impact
- **Compression**: Large payloads compressed before encryption

### Optimization Tips

1. **Selective Encryption**: Only encrypt PII fields, not entire payloads
2. **Compression**: Enable compression for large payloads
3. **Key Loading**: The engine loads its keys once, when it is created
4. **Batch Operations**: `enqueue_batch` encrypts all jobs before a single transaction

## Compliance

### GDPR (General Data Protection Regulation)

```rust
// Right to be forgotten
let job = Job::new("user_export".to_string(), user_data)
    .with_encryption(encryption_config)
    .with_pii_fields(vec!["personal_data"])
    .with_retention_policy(RetentionPolicy::DeleteAfter(Duration::from_secs(30 * 24 * 60 * 60)));
```

### HIPAA (Health Insurance Portability and Accountability Act)

```rust
// Healthcare data protection
let job = Job::new("patient_processing".to_string(), patient_data)
    .with_encryption(EncryptionConfig::new(EncryptionAlgorithm::AES256GCM))
    .with_pii_fields(vec!["medical_record_number", "patient_info"])
    .with_retention_policy(RetentionPolicy::DeleteAfter(Duration::from_secs(6 * 365 * 24 * 60 * 60)));
```

### PCI DSS (Payment Card Industry Data Security Standard)

```rust
// Payment card data protection
let job = Job::new("payment_processing".to_string(), payment_data)
    .with_encryption(EncryptionConfig::new(EncryptionAlgorithm::AES256GCM))
    .with_pii_fields(vec!["card_number", "cvv", "cardholder_name"])
    .with_retention_policy(RetentionPolicy::DeleteAfter(Duration::from_secs(365 * 24 * 60 * 60)));
```

### SOX (Sarbanes-Oxley Act)

```rust
// Financial data retention
let job = Job::new("financial_reporting".to_string(), financial_data)
    .with_encryption(encryption_config)
    .with_pii_fields(vec!["financial_records"])
    .with_retention_policy(RetentionPolicy::DeleteAfter(Duration::from_secs(7 * 365 * 24 * 60 * 60)));
```

## Migration Guide

### Enabling Encryption on Existing Jobs

1. **Run Migrations**: `cargo hammerwork migration run` (migration 011 adds the columns)
2. **Configure Keys**: create an `EncryptionEngine` and pass it to `JobQueue::with_encryption` on every queue that enqueues encrypted jobs **and on every queue used by workers that process them**. Workers without an engine fail encrypted jobs.
3. **Update Code**: add `with_encryption` (and optionally `with_pii_fields`) to the jobs to protect
4. **Schedule the retention purge**: `cargo hammerwork maintenance purge-encrypted --confirm`

### Migrating Existing Data

Jobs enqueued before encryption was enabled stay in plaintext; nothing re-encrypts them. To protect pending ones, re-enqueue them with an encryption config on a queue with an engine and delete the originals:

```rust
async fn encrypt_pending(
    queue: &JobQueue<sqlx::Postgres>,
    job_ids: &[uuid::Uuid],
    config: &EncryptionConfig,
) -> hammerwork::Result<()> {
    for id in job_ids {
        if let Some(job) = queue.get_job(*id).await? {
            if job.status == JobStatus::Pending && !job.is_encrypted {
                let encrypted = Job::new(job.queue_name.clone(), job.payload.clone())
                    .with_encryption(config.clone())
                    .with_pii_fields(vec!["credit_card", "ssn"]);
                queue.enqueue(encrypted).await?;
                queue.delete_job(job.id).await?;
            }
        }
    }
    Ok(())
}
```
