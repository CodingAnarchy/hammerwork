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

```rust,no_run
# #[allow(unused_imports)] use hammerwork::{*, encryption::*, queue::DatabaseQueue, worker::JobHandler};
# #[allow(unused_imports)] use std::{result::Result, sync::Arc, time::Duration};
# #[allow(unused_imports)] use serde_json::json;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(pool: sqlx::PgPool, queue: Arc<JobQueue<sqlx::Postgres>>, config: EncryptionConfig, encryption_config: EncryptionConfig, job_id: JobId, mut key_manager: KeyManager<sqlx::Postgres>, handler: JobHandler, payload: serde_json::Value, user_data: serde_json::Value, patient_data: serde_json::Value, payment_data: serde_json::Value, financial_data: serde_json::Value, worker: Worker<sqlx::Postgres>) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::encryption::{EncryptionConfig, EncryptionAlgorithm};

let config = EncryptionConfig::new(EncryptionAlgorithm::AES256GCM);
# Ok(())
# }
```

### ChaCha20-Poly1305

- **Algorithm**: ChaCha20 stream cipher with Poly1305 MAC
- **Key Size**: 256 bits (32 bytes) 
- **Features**: Constant-time execution, mobile/embedded friendly
- **Use Case**: Environments without AES hardware acceleration

```rust,no_run
# #[allow(unused_imports)] use hammerwork::{*, encryption::*, queue::DatabaseQueue, worker::JobHandler};
# #[allow(unused_imports)] use std::{result::Result, sync::Arc, time::Duration};
# #[allow(unused_imports)] use serde_json::json;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(pool: sqlx::PgPool, queue: Arc<JobQueue<sqlx::Postgres>>, config: EncryptionConfig, encryption_config: EncryptionConfig, job_id: JobId, mut key_manager: KeyManager<sqlx::Postgres>, handler: JobHandler, payload: serde_json::Value, user_data: serde_json::Value, patient_data: serde_json::Value, payment_data: serde_json::Value, financial_data: serde_json::Value, worker: Worker<sqlx::Postgres>) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::encryption::{EncryptionConfig, EncryptionAlgorithm};

let config = EncryptionConfig::new(EncryptionAlgorithm::ChaCha20Poly1305);
# Ok(())
# }
```

## Configuration

### Basic Configuration

```rust,no_run
# #[allow(unused_imports)] use hammerwork::{*, encryption::*, queue::DatabaseQueue, worker::JobHandler};
# #[allow(unused_imports)] use std::{result::Result, sync::Arc, time::Duration};
# #[allow(unused_imports)] use serde_json::json;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(pool: sqlx::PgPool, queue: Arc<JobQueue<sqlx::Postgres>>, config: EncryptionConfig, encryption_config: EncryptionConfig, job_id: JobId, mut key_manager: KeyManager<sqlx::Postgres>, handler: JobHandler, payload: serde_json::Value, user_data: serde_json::Value, patient_data: serde_json::Value, payment_data: serde_json::Value, financial_data: serde_json::Value, worker: Worker<sqlx::Postgres>) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::encryption::{EncryptionConfig, EncryptionAlgorithm, KeySource};

// Environment variable key source
let config = EncryptionConfig::new(EncryptionAlgorithm::AES256GCM)
    .with_key_source(KeySource::Environment("HAMMERWORK_ENCRYPTION_KEY".to_string()));
# Ok(())
# }
```

### Advanced Configuration

```rust,no_run
# #[allow(unused_imports)] use hammerwork::{*, encryption::*, queue::DatabaseQueue, worker::JobHandler};
# #[allow(unused_imports)] use std::{result::Result, sync::Arc, time::Duration};
# #[allow(unused_imports)] use serde_json::json;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(pool: sqlx::PgPool, queue: Arc<JobQueue<sqlx::Postgres>>, config: EncryptionConfig, encryption_config: EncryptionConfig, job_id: JobId, mut key_manager: KeyManager<sqlx::Postgres>, handler: JobHandler, payload: serde_json::Value, user_data: serde_json::Value, patient_data: serde_json::Value, payment_data: serde_json::Value, financial_data: serde_json::Value, worker: Worker<sqlx::Postgres>) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::encryption::{EncryptionAlgorithm, EncryptionConfig, KeySource};
use std::time::Duration;

let config = EncryptionConfig::new(EncryptionAlgorithm::AES256GCM)
    .with_key_source(KeySource::Environment("ENCRYPTION_KEY".to_string()))
    .with_key_rotation_enabled(true)
    .with_key_rotation_interval(Duration::from_secs(30 * 24 * 60 * 60)) // 30 days
    .with_compression_enabled(true); // Compress payloads before encrypting them
# Ok(())
# }
```

### Key Sources

#### Environment Variable (Recommended)

```rust,no_run
# #[allow(unused_imports)] use hammerwork::{*, encryption::*, queue::DatabaseQueue, worker::JobHandler};
# #[allow(unused_imports)] use std::{result::Result, sync::Arc, time::Duration};
# #[allow(unused_imports)] use serde_json::json;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(pool: sqlx::PgPool, queue: Arc<JobQueue<sqlx::Postgres>>, config: EncryptionConfig, encryption_config: EncryptionConfig, job_id: JobId, mut key_manager: KeyManager<sqlx::Postgres>, handler: JobHandler, payload: serde_json::Value, user_data: serde_json::Value, patient_data: serde_json::Value, payment_data: serde_json::Value, financial_data: serde_json::Value, worker: Worker<sqlx::Postgres>) -> std::result::Result<(), Box<dyn std::error::Error>> {
let config = EncryptionConfig::new(EncryptionAlgorithm::AES256GCM)
    .with_key_source(KeySource::Environment("HAMMERWORK_ENCRYPTION_KEY".to_string()));
# Ok(())
# }
```

Generate a secure key:
```bash
# Generate base64-encoded 256-bit key
openssl rand -base64 32
export HAMMERWORK_ENCRYPTION_KEY="your-generated-key-here"
```

#### Static Key (Testing Only)

```rust,no_run
# #[allow(unused_imports)] use hammerwork::{*, encryption::*, queue::DatabaseQueue, worker::JobHandler};
# #[allow(unused_imports)] use std::{result::Result, sync::Arc, time::Duration};
# #[allow(unused_imports)] use serde_json::json;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(pool: sqlx::PgPool, queue: Arc<JobQueue<sqlx::Postgres>>, config: EncryptionConfig, encryption_config: EncryptionConfig, job_id: JobId, mut key_manager: KeyManager<sqlx::Postgres>, handler: JobHandler, payload: serde_json::Value, user_data: serde_json::Value, patient_data: serde_json::Value, payment_data: serde_json::Value, financial_data: serde_json::Value, worker: Worker<sqlx::Postgres>) -> std::result::Result<(), Box<dyn std::error::Error>> {
// WARNING: Only for testing - never use static keys in production
let config = EncryptionConfig::new(EncryptionAlgorithm::AES256GCM)
    .with_key_source(KeySource::Static("base64-encoded-key".to_string()));
# Ok(())
# }
```

#### External KMS

Hammerwork supports multiple external Key Management Services for enterprise key management:

##### AWS KMS

```rust,no_run
# #[allow(unused_imports)] use hammerwork::{*, encryption::*, queue::DatabaseQueue, worker::JobHandler};
# #[allow(unused_imports)] use std::{result::Result, sync::Arc, time::Duration};
# #[allow(unused_imports)] use serde_json::json;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(pool: sqlx::PgPool, queue: Arc<JobQueue<sqlx::Postgres>>, config: EncryptionConfig, encryption_config: EncryptionConfig, job_id: JobId, mut key_manager: KeyManager<sqlx::Postgres>, handler: JobHandler, payload: serde_json::Value, user_data: serde_json::Value, patient_data: serde_json::Value, payment_data: serde_json::Value, financial_data: serde_json::Value, worker: Worker<sqlx::Postgres>) -> std::result::Result<(), Box<dyn std::error::Error>> {
let config = EncryptionConfig::new(EncryptionAlgorithm::AES256GCM)
    .with_key_source(KeySource::External("aws://alias/hammerwork-key?region=us-east-1".to_string()));
let engine = EncryptionEngine::new_with_pool(config, &pool).await?;
# Ok(())
# }
```

Add `&endpoint=<url>` to use a different KMS endpoint (for example LocalStack). Requires the `aws-kms` feature. The application needs `kms:GenerateDataKey` and `kms:Decrypt` on the key.

##### Google Cloud KMS

```rust,no_run
# #[allow(unused_imports)] use hammerwork::{*, encryption::*, queue::DatabaseQueue, worker::JobHandler};
# #[allow(unused_imports)] use std::{result::Result, sync::Arc, time::Duration};
# #[allow(unused_imports)] use serde_json::json;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(pool: sqlx::PgPool, queue: Arc<JobQueue<sqlx::Postgres>>, config: EncryptionConfig, encryption_config: EncryptionConfig, job_id: JobId, mut key_manager: KeyManager<sqlx::Postgres>, handler: JobHandler, payload: serde_json::Value, user_data: serde_json::Value, patient_data: serde_json::Value, payment_data: serde_json::Value, financial_data: serde_json::Value, worker: Worker<sqlx::Postgres>) -> std::result::Result<(), Box<dyn std::error::Error>> {
let config = EncryptionConfig::new(EncryptionAlgorithm::AES256GCM)
    .with_key_source(KeySource::External("gcp://projects/PROJECT/locations/LOCATION/keyRings/RING/cryptoKeys/KEY".to_string()));
let engine = EncryptionEngine::new_with_pool(config, &pool).await?;
# Ok(())
# }
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

**Rotation.** `KeyManager::rotate_kms_master_key()` and `EncryptionEngine::rotate_kms_key(&pool)` generate a new data key with the KMS, store it as the new active version and retire the previous one. Retired versions are kept and stay decryptable: the key manager decrypts keys wrapped by an older master key version (including key-encryption keys from `generate_master_key`), and the engine falls back to older versions when decrypting payloads. Rotating the KMS key itself inside AWS or GCP (automatic key rotation) needs no action: the KMS still decrypts blobs encrypted under earlier key versions.

**Rotating in a fleet.** An engine created with `new_with_pool` keeps a handle on the pool. When it meets a payload it cannot decrypt with the keys it holds (another process rotated the data key and encrypted jobs with the new version, or the payload names another key id stored in the same table), it loads the stored versions it does not have yet, decrypts them with the KMS and tries again; from then on it also encrypts with the newest active version. So a worker that started before a rotation still runs the jobs encrypted after it, instead of failing them until they go Dead. Reloading costs one database query (and a KMS call per new version); a payload that still cannot be decrypted (tampered, or a key the table does not hold) fails as before, and if the reload itself fails (database or KMS unavailable) the run fails and is retried like any other failed run. `EncryptionEngine::reload_kms_keys()` loads new versions without waiting for such a payload; call it periodically, or after a rotation, to move every process to the new version promptly. A two-phase rollout (deploy, then rotate) needs no restart.

**Upgrading.** Before this change, each process asked the KMS for a fresh key on every start, so anything encrypted by an earlier process was already unrecoverable. After running migration 017, the first start generates and stores a key; from then on it is stable. Data encrypted by the old per-process keys cannot be recovered.

##### HashiCorp Vault KMS

```rust,no_run
# #[allow(unused_imports)] use hammerwork::{*, encryption::*, queue::DatabaseQueue, worker::JobHandler};
# #[allow(unused_imports)] use std::{result::Result, sync::Arc, time::Duration};
# #[allow(unused_imports)] use serde_json::json;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(pool: sqlx::PgPool, queue: Arc<JobQueue<sqlx::Postgres>>, config: EncryptionConfig, encryption_config: EncryptionConfig, job_id: JobId, mut key_manager: KeyManager<sqlx::Postgres>, handler: JobHandler, payload: serde_json::Value, user_data: serde_json::Value, patient_data: serde_json::Value, payment_data: serde_json::Value, financial_data: serde_json::Value, worker: Worker<sqlx::Postgres>) -> std::result::Result<(), Box<dyn std::error::Error>> {
let config = EncryptionConfig::new(EncryptionAlgorithm::AES256GCM)
    .with_key_source(KeySource::External("vault://secret/hammerwork/encryption-key".to_string()));
# Ok(())
# }
```

With custom Vault address:

```rust,no_run
# #[allow(unused_imports)] use hammerwork::{*, encryption::*, queue::DatabaseQueue, worker::JobHandler};
# #[allow(unused_imports)] use std::{result::Result, sync::Arc, time::Duration};
# #[allow(unused_imports)] use serde_json::json;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(pool: sqlx::PgPool, queue: Arc<JobQueue<sqlx::Postgres>>, config: EncryptionConfig, encryption_config: EncryptionConfig, job_id: JobId, mut key_manager: KeyManager<sqlx::Postgres>, handler: JobHandler, payload: serde_json::Value, user_data: serde_json::Value, patient_data: serde_json::Value, payment_data: serde_json::Value, financial_data: serde_json::Value, worker: Worker<sqlx::Postgres>) -> std::result::Result<(), Box<dyn std::error::Error>> {
let config = EncryptionConfig::new(EncryptionAlgorithm::AES256GCM)
    .with_key_source(KeySource::External("vault://secret/hammerwork/encryption-key?addr=https://vault.example.com".to_string()));
# Ok(())
# }
```

**Environment Variables:**
- `VAULT_ADDR`: Vault server address, used when the source has no `addr=` parameter (required: there is no default)
- `VAULT_TOKEN`: Authentication token for Vault access

**Vault Requirements:**
- KV v2 secrets engine enabled
- Secret stored with a `key` field holding a base64-encoded key of exactly the algorithm's key size (32 bytes). A passphrase, or key material of any other length, is rejected with `EncryptionError::InvalidConfiguration`: it is never hashed, padded or truncated into a key (a 1-byte value padded to 32 bytes would still have only 8 bits of entropy). The same applies to `vault://` master key sources of the `KeyManager`.
- Proper authentication and access policies configured
- The `vault-kms` feature

##### Azure Key Vault

```rust,no_run
# #[allow(unused_imports)] use hammerwork::{*, encryption::*, queue::DatabaseQueue, worker::JobHandler};
# #[allow(unused_imports)] use std::{result::Result, sync::Arc, time::Duration};
# #[allow(unused_imports)] use serde_json::json;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(pool: sqlx::PgPool, queue: Arc<JobQueue<sqlx::Postgres>>, config: EncryptionConfig, encryption_config: EncryptionConfig, job_id: JobId, mut key_manager: KeyManager<sqlx::Postgres>, handler: JobHandler, payload: serde_json::Value, user_data: serde_json::Value, patient_data: serde_json::Value, payment_data: serde_json::Value, financial_data: serde_json::Value, worker: Worker<sqlx::Postgres>) -> std::result::Result<(), Box<dyn std::error::Error>> {
let config = EncryptionConfig::new(EncryptionAlgorithm::AES256GCM)
    .with_key_source(KeySource::External("azure://my-vault.vault.azure.net/keys/encryption-key".to_string()));
# Ok(())
# }
```

Credentials come from the environment (`AZURE_TENANT_ID` / `AZURE_CLIENT_ID` / `AZURE_CLIENT_SECRET`, workload identity, managed identity, or the Azure CLI). Requires the `azure-kv` feature. The key material must be exactly the algorithm's key size (32 bytes, e.g. a 256-bit `oct` key); shorter or longer material is rejected with `EncryptionError::InvalidConfiguration` instead of being padded or truncated, for engine keys and `KeyManager` master keys alike.

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

```rust,no_run
# #[allow(unused_imports)] use hammerwork::{*, encryption::*, queue::DatabaseQueue, worker::JobHandler};
# #[allow(unused_imports)] use std::{result::Result, sync::Arc, time::Duration};
# #[allow(unused_imports)] use serde_json::json;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(pool: sqlx::PgPool, queue: Arc<JobQueue<sqlx::Postgres>>, config: EncryptionConfig, encryption_config: EncryptionConfig, job_id: JobId, mut key_manager: KeyManager<sqlx::Postgres>, handler: JobHandler, payload: serde_json::Value, user_data: serde_json::Value, patient_data: serde_json::Value, payment_data: serde_json::Value, financial_data: serde_json::Value, worker: Worker<sqlx::Postgres>) -> std::result::Result<(), Box<dyn std::error::Error>> {
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
# Ok(())
# }
```

How it works:

- **Enqueue.** `enqueue`, `enqueue_batch`, `enqueue_workflow` and `enqueue_cron_job` encrypt every job that has an encryption config before anything is written (a batch with one job that cannot be encrypted writes nothing). The ciphertext, nonce, tag, key id, algorithm, metadata, keyed integrity hash, PII field list and retention columns of migration 011 are filled in; `is_encrypted` is set.
- **What `payload` holds.** Never the plaintext of encrypted data. With no PII fields the whole payload is encrypted and `payload` is the placeholder `{"encrypted": true}`. With PII fields (see below) `payload` is the original payload with each listed field's value replaced by `"[ENCRYPTED]"`.
- **The engine decides.** The queue's engine sets the algorithm, key and compression. A job whose config names another algorithm, or a `key_id` other than the engine's, is rejected. Jobs without an encryption config are stored unchanged, also on a queue with an engine.
- **Fail closed.** Enqueueing a job that has an encryption config on a queue without an engine fails with `HammerworkError::Encryption`.
- **Dequeue and reads.** `dequeue`, `get_job`, `get_batch_jobs`, `get_dead_jobs`, `get_dead_jobs_by_queue` and the other reads return jobs as stored: `is_encrypted` is `true`, `payload` is redacted and `encrypted_payload` holds the ciphertext, so `decrypt_job` works on any of them (a dead-letter tool can decrypt a dead job and re-enqueue its plaintext). The web dashboard and `cargo hammerwork job show` therefore show the redacted payload (`job show` also prints the key id, algorithm and retention). They never need a key to read.
- **Workers.** A worker decrypts the job (`JobQueue::decrypt_job`) just before it calls the handler; only the handler sees the plaintext. Event hooks, webhooks and the recorded outcome keep using the redacted job. If the payload cannot be decrypted (the worker's queue has no engine, the engine does not have the job's key, the data was tampered with, or the ciphertext belongs to another job; see [Binding the ciphertext to its job](#binding-the-ciphertext-to-its-job)) the run fails with `Cannot decrypt the payload of job ...` and goes through the normal retry / dead path; the handler is not called.
- **Reading the plaintext yourself.** `queue.decrypt_job(job).await?` returns the job with its plaintext payload.
- **Without the `encryption` feature** a build cannot encrypt or decrypt. It still reads `is_encrypted`, so its workers fail encrypted jobs instead of running them with the redacted payload, and archiving and restoring still carry the ciphertext.

The payload hash stored with the ciphertext is an HMAC-SHA256 keyed by the data key (`hmac-sha256:<hex>`), not a plain SHA-256, so a low-entropy value (an SSN, a card number) cannot be recovered from it by brute force. Payloads encrypted by earlier versions with a plain SHA-256 still decrypt.

### Binding the ciphertext to its job

The ciphertext of a job is bound to that job. It is encrypted with AEAD associated data made of the job's id, queue name, key id, algorithm, PII field list and the list of fields that were encrypted (`encryption::job_payload::job_associated_data`). Decryption recomputes the associated data from the stored row, so with write access to the database, an attacker who:

- swaps the `encrypted_payload` / `encryption_nonce` / `encryption_tag` of two jobs,
- moves a job to another queue (`queue_name`), or
- changes its key id, algorithm or `pii_fields`

gets a job that fails to decrypt (the worker fails the run without calling the handler) instead of one that decrypts as another job's data. The associated data is not stored; only the format is recorded, as `format_version` in `encryption_metadata`:

| `format_version` | Meaning |
|---|---|
| missing or `0` | No associated data. Payloads written before this change (and payloads encrypted directly with `EncryptionEngine::encrypt_payload`). They still decrypt, and are not re-encrypted. |
| `1` | Bound to the job as described above. Written by `JobQueue` and `TestQueue` for every newly encrypted job. |
| anything else | Written by a newer version: refused. |

Resetting `format_version` to `0` on a bound payload does not get around the binding: the ciphertext then fails authentication without the associated data it was encrypted with. Payloads written in the old format can still be swapped among themselves; re-enqueue those jobs if that matters. The retention columns are not covered: anyone who can write to the table can also delete or keep rows.

Archiving and restoring keep the job id, queue name and encryption columns, so restored jobs decrypt. `EncryptionEngine::encrypt_payload_with_associated_data` / `decrypt_payload_with_associated_data` expose the same mechanism for payloads you encrypt yourself.

### Archived encrypted jobs

Archiving moves the ciphertext to `hammerwork_jobs_archive` without decrypting it. `get_job` of an archived job returns it with `status == Archived`, `is_encrypted`, `pii_fields` and the redacted payload, but not the ciphertext, so `decrypt_job` on it fails with an error that says to restore it. To read the plaintext of an archived job, restore it first:

```rust,no_run
# #[allow(unused_imports)] use hammerwork::{*, encryption::*, queue::DatabaseQueue, worker::JobHandler};
# #[allow(unused_imports)] use std::{result::Result, sync::Arc, time::Duration};
# #[allow(unused_imports)] use serde_json::json;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(pool: sqlx::PgPool, queue: Arc<JobQueue<sqlx::Postgres>>, config: EncryptionConfig, encryption_config: EncryptionConfig, job_id: JobId, mut key_manager: KeyManager<sqlx::Postgres>, handler: JobHandler, payload: serde_json::Value, user_data: serde_json::Value, patient_data: serde_json::Value, payment_data: serde_json::Value, financial_data: serde_json::Value, worker: Worker<sqlx::Postgres>) -> std::result::Result<(), Box<dyn std::error::Error>> {
let job = queue.restore_archived_job(job_id).await?; // back in hammerwork_jobs, ciphertext unchanged
let job = queue.get_job(job_id).await?.unwrap();
let plaintext = queue.decrypt_job(job).await?.payload;
# Ok(())
# }
```

There is no decrypt-on-read for the archive: archived jobs are cold data, and keeping the plaintext path to the live table keeps one place (restore) where an archived payload comes back. Retention still applies to archived jobs (see [Enforcing Retention](#enforcing-retention)).

### Encrypting whole queues

`JobQueue::with_encrypted_queues(["payments"])` (or `["*"]` for every queue) encrypts every job enqueued to those queues, also jobs created without `with_encryption`: they get the engine's algorithm and key id, and their PII fields (if any) or whole payload is encrypted. Jobs with their own encryption config are unaffected. This is how a queue configured from `hammerwork.toml` encrypts without code changes.

### The CLI and the web dashboard

`cargo hammerwork` and `hammerwork-web` write jobs on the application's behalf (`job enqueue`, `batch enqueue`, `cron create`, `workflow create`, and `POST /api/jobs`), so they use the application's encryption settings:

- **The same settings.** They read the `[encryption]` section of the application's `hammerwork.toml` (the rest of the file is ignored) and apply the `HAMMERWORK_ENCRYPTION_*` environment variables on top (`PayloadEncryptionConfig::load`). Point the CLI at the file with `cargo hammerwork config set encryption_config /etc/app/hammerwork.toml` or `HAMMERWORK_ENCRYPTION_CONFIG`; the dashboard reads `HAMMERWORK_ENCRYPTION_CONFIG`. The key must be available to them too (for `env://` keys, the variable). Jobs on `encrypted_queues` are then encrypted exactly as the application would.
- **Never plaintext by mistake.** When the settings are enabled but the key cannot be loaded, the CLI refuses to write jobs and the dashboard refuses to start. Without any settings, both still refuse to store a plaintext job on a queue that already holds encrypted jobs (`JobQueue::with_plaintext_guard`): the error names the queue and says to configure the settings. A queue that has never held an encrypted job cannot be recognised that way, so configure the settings wherever the CLI or dashboard writes jobs.
- **Explicit encryption.** `cargo hammerwork job enqueue --encrypt` encrypts the whole payload with the application's key on any queue; `--pii-field card` (repeatable) encrypts only those fields.
- **Reading** needs no key: both show the redacted payload, and never decrypt.
- **Backups** (`cargo hammerwork backup create`, JSON format) hold every column of every job, with encrypted payloads still encrypted, and `backup restore` puts them back unchanged, so restored jobs decrypt with the same key. CSV backups are an export of the main columns and cannot be restored.

### Configuration file

`JobQueue::from_config` reads an optional `[encryption]` section of `HammerworkConfig` (`PayloadEncryptionConfig`) and sets up the engine itself:

```toml
[encryption]
enabled = true
algorithm = "AES256GCM"                          # or "ChaCha20Poly1305"
key_source = "env://HAMMERWORK_ENCRYPTION_KEY"   # or aws://, gcp://, vault://, azure://
key_id = "payments-2027"                         # default "default"
compression = false
default_retention_secs = 2592000                 # retention of UseDefault jobs (30 days)
purge_interval_secs = 3600                       # WorkerPool::from_hammerwork_config purges hourly
encrypted_queues = ["payments"]                  # encrypt these queues' jobs ("*": all)

[encryption.decryption_keys]                     # decrypt-only keys of earlier key ids
"payments-2026" = "env://HAMMERWORK_KEY_2026"
```

```rust,no_run
# #[allow(unused_imports)] use hammerwork::{*, encryption::*, queue::DatabaseQueue, worker::JobHandler};
# #[allow(unused_imports)] use std::{result::Result, sync::Arc, time::Duration};
# #[allow(unused_imports)] use serde_json::json;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(pool: sqlx::PgPool, queue: Arc<JobQueue<sqlx::Postgres>>, config: EncryptionConfig, encryption_config: EncryptionConfig, job_id: JobId, mut key_manager: KeyManager<sqlx::Postgres>, handler: JobHandler, payload: serde_json::Value, user_data: serde_json::Value, patient_data: serde_json::Value, payment_data: serde_json::Value, financial_data: serde_json::Value, worker: Worker<sqlx::Postgres>) -> std::result::Result<(), Box<dyn std::error::Error>> {
let config = HammerworkConfig::from_file("hammerwork.toml")?;
let queue = Arc::new(JobQueue::<sqlx::Postgres>::from_config(&config).await?);
let pool = WorkerPool::from_hammerwork_config(worker, &config)?;
# Ok(())
# }
```

- **Keys are referenced, never written in the file.** `key_source` and the `decryption_keys` values are `KeySourceRef`s: `env://VAR` (a base64 key in that environment variable) or a KMS URI (`aws://`, `gcp://`, `vault://`, `azure://`, each needing its cargo feature). Static keys and anything else are rejected when the file is loaded, and the error does not repeat the value. `Debug` output and `save_to_file` only ever contain these references.
- **Fails closed.** Unknown fields (for example a key under a guessed name) and unknown algorithms are load errors. `from_config` fails if the section is enabled but the key cannot be loaded, if `encrypted_queues` is set while `enabled = false`, if a decryption key reuses the encryption key id, or if the build lacks the `encryption` feature.
- **Environment.** `HammerworkConfig::from_env` reads `HAMMERWORK_ENCRYPTION_ENABLED`, `_ALGORITHM`, `_KEY_SOURCE`, `_KEY_ID`, `_COMPRESSION`, `_DEFAULT_RETENTION_SECS`, `_PURGE_INTERVAL_SECS`, `_ENCRYPTED_QUEUES` (comma-separated) and `_DECRYPTION_KEYS` (`id=source,id=source`). Invalid values are errors. `HAMMERWORK_ENCRYPTION_KEY` itself is the default key source (the key), not a setting.
- `aws://` and `gcp://` sources work: the engine is created with `EncryptionEngine::new_with_pool` on the queue's pool.

### Testing with `TestQueue`

The in-memory `TestQueue` (feature `test`) applies the same semantics when given an engine, so unit tests exercise the real redaction and decryption paths:

```rust,no_run
# #[allow(unused_imports)] use hammerwork::{*, encryption::*, queue::DatabaseQueue, worker::JobHandler};
# #[allow(unused_imports)] use std::{result::Result, sync::Arc, time::Duration};
# #[allow(unused_imports)] use serde_json::json;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(pool: sqlx::PgPool, queue: Arc<JobQueue<sqlx::Postgres>>, config: EncryptionConfig, encryption_config: EncryptionConfig, job_id: JobId, mut key_manager: KeyManager<sqlx::Postgres>, handler: JobHandler, payload: serde_json::Value, user_data: serde_json::Value, patient_data: serde_json::Value, payment_data: serde_json::Value, financial_data: serde_json::Value, worker: Worker<sqlx::Postgres>) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::queue::test::TestQueue;

let queue = TestQueue::new().with_encryption(EncryptionEngine::new(config.clone()).await?);
let id = queue.enqueue(Job::new("payments".into(), json!({"card": "4111"})).with_encryption(config)).await?;
let stored = queue.get_job(id).await?.unwrap();          // {"encrypted": true}, is_encrypted
let opened = queue.decrypt_job(stored).await?;           // what a handler sees
# Ok(())
# }
```

It seals jobs in `enqueue`, `enqueue_batch`, `enqueue_workflow` and `enqueue_cron_job`, rejects jobs with an encryption config when it has no engine, binds ciphertexts to their jobs, supports `with_encrypted_queues`, hides the ciphertext of archived jobs until they are restored, and implements `purge_expired_encrypted_jobs` against its `MockClock`.

### Key Rotation for Job Payloads

Each encrypted job records the id of the key it was encrypted with. To switch to a new key, give the engine a new `key_id` and keep the previous key for decryption:

```rust,no_run
# #[allow(unused_imports)] use hammerwork::{*, encryption::*, queue::DatabaseQueue, worker::JobHandler};
# #[allow(unused_imports)] use std::{result::Result, sync::Arc, time::Duration};
# #[allow(unused_imports)] use serde_json::json;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(pool: sqlx::PgPool, queue: Arc<JobQueue<sqlx::Postgres>>, config: EncryptionConfig, encryption_config: EncryptionConfig, job_id: JobId, mut key_manager: KeyManager<sqlx::Postgres>, handler: JobHandler, payload: serde_json::Value, user_data: serde_json::Value, patient_data: serde_json::Value, payment_data: serde_json::Value, financial_data: serde_json::Value, worker: Worker<sqlx::Postgres>) -> std::result::Result<(), Box<dyn std::error::Error>> {
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
# Ok(())
# }
```

New jobs are encrypted with `payments-2027`; jobs written with `payments-2026` still decrypt. For `aws://` and `gcp://` sources, `EncryptionEngine::rotate_kms_key` adds a new key version under the same key id and keeps earlier versions for decryption (see above).

## PII Field Protection

To encrypt only some fields, list them with `with_pii_fields`:

```rust,no_run
# #[allow(unused_imports)] use hammerwork::{*, encryption::*, queue::DatabaseQueue, worker::JobHandler};
# #[allow(unused_imports)] use std::{result::Result, sync::Arc, time::Duration};
# #[allow(unused_imports)] use serde_json::json;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(pool: sqlx::PgPool, queue: Arc<JobQueue<sqlx::Postgres>>, config: EncryptionConfig, encryption_config: EncryptionConfig, job_id: JobId, mut key_manager: KeyManager<sqlx::Postgres>, handler: JobHandler, payload: serde_json::Value, user_data: serde_json::Value, patient_data: serde_json::Value, payment_data: serde_json::Value, financial_data: serde_json::Value, worker: Worker<sqlx::Postgres>) -> std::result::Result<(), Box<dyn std::error::Error>> {
let job = Job::new("user_data_processing".to_string(), json!({
    "user_id": "user123",
    "credit_card": "4111-1111-1111-1111",
    "billing": {"address": "123 Main St", "country": "US"},
    "preferences": {"newsletter": true}
}))
.with_encryption(EncryptionConfig::new(EncryptionAlgorithm::AES256GCM))
.with_pii_fields(vec!["credit_card", "billing.address"]);
# Ok(())
# }
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

```rust,no_run
# #[allow(unused_imports)] use hammerwork::{*, encryption::*, queue::DatabaseQueue, worker::JobHandler};
# #[allow(unused_imports)] use std::{result::Result, sync::Arc, time::Duration};
# #[allow(unused_imports)] use serde_json::json;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(pool: sqlx::PgPool, queue: Arc<JobQueue<sqlx::Postgres>>, config: EncryptionConfig, encryption_config: EncryptionConfig, job_id: JobId, mut key_manager: KeyManager<sqlx::Postgres>, handler: JobHandler, payload: serde_json::Value, user_data: serde_json::Value, patient_data: serde_json::Value, payment_data: serde_json::Value, financial_data: serde_json::Value, worker: Worker<sqlx::Postgres>) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::encryption::{EncryptionAlgorithm, KeyManager, KeyManagerConfig};

let config = KeyManagerConfig::new()
    .with_master_key_env("HAMMERWORK_MASTER_KEY")
    .with_auto_rotation_enabled(true)
    .with_rotation_interval(chrono::Duration::days(90));

// `pool` is the sqlx pool of the database that holds the key tables
let mut key_manager = KeyManager::new(config, pool).await?;

// Generate a new encryption key
let key_id = key_manager
    .generate_key("payment-encryption", EncryptionAlgorithm::AES256GCM)
    .await?;

// Use the key
let key_material = key_manager.get_key(&key_id).await?;
# Ok(())
# }
```

### Key Rotation

```rust,no_run
# #[allow(unused_imports)] use hammerwork::{*, encryption::*, queue::DatabaseQueue, worker::JobHandler};
# #[allow(unused_imports)] use std::{result::Result, sync::Arc, time::Duration};
# #[allow(unused_imports)] use serde_json::json;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(pool: sqlx::PgPool, queue: Arc<JobQueue<sqlx::Postgres>>, config: EncryptionConfig, encryption_config: EncryptionConfig, job_id: JobId, mut key_manager: KeyManager<sqlx::Postgres>, handler: JobHandler, payload: serde_json::Value, user_data: serde_json::Value, patient_data: serde_json::Value, payment_data: serde_json::Value, financial_data: serde_json::Value, worker: Worker<sqlx::Postgres>) -> std::result::Result<(), Box<dyn std::error::Error>> {
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
# Ok(())
# }
```

### Key Storage and Master Keys

`KeyManager` stores every key version in `hammerwork_encryption_keys` (PostgreSQL and MySQL). Key material is encrypted with AES-256-GCM under a master key before it is written; plaintext key material and the configured master key are never stored. The `key_source` column holds only a label (`Generated`, `Environment`, ...).

The master key is loaded from `master_key_source` when the `KeyManager` is created, and loading fails closed (see above). `generate_master_key()` creates a key-encryption key, stores it encrypted with the configured master key, and uses it for keys generated or rotated afterwards. Keys encrypted with earlier master keys remain readable, and a new `KeyManager` loads the active key-encryption key from the database. A `KeyManager` configured with a different master key fails to start instead of silently using the wrong key.

### Key Audit Trails

```rust,no_run
# #[allow(unused_imports)] use hammerwork::{*, encryption::*, queue::DatabaseQueue, worker::JobHandler};
# #[allow(unused_imports)] use std::{result::Result, sync::Arc, time::Duration};
# #[allow(unused_imports)] use serde_json::json;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(pool: sqlx::PgPool, queue: Arc<JobQueue<sqlx::Postgres>>, config: EncryptionConfig, encryption_config: EncryptionConfig, job_id: JobId, mut key_manager: KeyManager<sqlx::Postgres>, handler: JobHandler, payload: serde_json::Value, user_data: serde_json::Value, patient_data: serde_json::Value, payment_data: serde_json::Value, financial_data: serde_json::Value, worker: Worker<sqlx::Postgres>) -> std::result::Result<(), Box<dyn std::error::Error>> {
// Get key usage statistics
let stats = key_manager.get_stats().await;
println!("Total keys: {}", stats.total_keys);
println!("Active keys: {}", stats.active_keys);
println!("Rotations performed: {}", stats.rotations_performed);

# Ok(())
# }
```

With `audit_enabled` (the default), creating, reading and rotating keys writes a row to
`hammerwork_key_audit_log` (`key_id`, `operation`, `success`, `error_message`,
`timestamp`). `KeyManager` has no method to query it; read the table directly:

```sql
SELECT timestamp, operation, success, error_message
FROM hammerwork_key_audit_log
WHERE key_id = 'payment-encryption'
ORDER BY timestamp DESC;
```

## Retention Policies

### Policy Types

```rust,no_run
# #[allow(unused_imports)] use hammerwork::{*, encryption::*, queue::DatabaseQueue, worker::JobHandler};
# #[allow(unused_imports)] use std::{result::Result, sync::Arc, time::Duration};
# #[allow(unused_imports)] use serde_json::json;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(pool: sqlx::PgPool, queue: Arc<JobQueue<sqlx::Postgres>>, config: EncryptionConfig, encryption_config: EncryptionConfig, job_id: JobId, mut key_manager: KeyManager<sqlx::Postgres>, handler: JobHandler, payload: serde_json::Value, user_data: serde_json::Value, patient_data: serde_json::Value, payment_data: serde_json::Value, financial_data: serde_json::Value, worker: Worker<sqlx::Postgres>) -> std::result::Result<(), Box<dyn std::error::Error>> {
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
# Ok(())
# }
```

### Enforcing Retention

When a job is encrypted, its retention policy (`Job::with_retention_policy`, or `UseDefault`, which uses the engine's `default_retention`) is stored in `retention_policy` and the deletion time in `retention_delete_at`. Expired jobs are deleted by a purge, which you run periodically. The simplest way is to let a `WorkerPool` schedule it:

```rust,no_run
# #[allow(unused_imports)] use hammerwork::{*, encryption::*, queue::DatabaseQueue, worker::JobHandler};
# #[allow(unused_imports)] use std::{result::Result, sync::Arc, time::Duration};
# #[allow(unused_imports)] use serde_json::json;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(pool: sqlx::PgPool, queue: Arc<JobQueue<sqlx::Postgres>>, config: EncryptionConfig, encryption_config: EncryptionConfig, job_id: JobId, mut key_manager: KeyManager<sqlx::Postgres>, handler: JobHandler, payload: serde_json::Value, user_data: serde_json::Value, patient_data: serde_json::Value, payment_data: serde_json::Value, financial_data: serde_json::Value, worker: Worker<sqlx::Postgres>) -> std::result::Result<(), Box<dyn std::error::Error>> {
let mut pool = WorkerPool::<sqlx::Postgres>::new()
    .with_encrypted_job_purge(Duration::from_secs(3600)); // off by default
// or WorkerPool::from_hammerwork_config(worker, &config)? with encryption.purge_interval_secs
# Ok(())
# }
```

The pool runs the first purge when it starts and then every interval, on its first worker's queue. Each purge runs in its own task, so shutting the pool down never cancels one part-way through its transaction. Several pools or processes purging at once is safe. Without a pool purge, call it yourself:

```rust,no_run
# #[allow(unused_imports)] use hammerwork::{*, encryption::*, queue::DatabaseQueue, worker::JobHandler};
# #[allow(unused_imports)] use std::{result::Result, sync::Arc, time::Duration};
# #[allow(unused_imports)] use serde_json::json;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(pool: sqlx::PgPool, queue: Arc<JobQueue<sqlx::Postgres>>, config: EncryptionConfig, encryption_config: EncryptionConfig, job_id: JobId, mut key_manager: KeyManager<sqlx::Postgres>, handler: JobHandler, payload: serde_json::Value, user_data: serde_json::Value, patient_data: serde_json::Value, payment_data: serde_json::Value, financial_data: serde_json::Value, worker: Worker<sqlx::Postgres>) -> std::result::Result<(), Box<dyn std::error::Error>> {
let purge = queue.purge_expired_encrypted_jobs().await?;
println!("deleted {} jobs and {} archived jobs", purge.jobs, purge.archived_jobs);
# Ok(())
# }
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

```rust,no_run
# #[allow(unused_imports)] use hammerwork::{*, encryption::*, queue::DatabaseQueue, worker::JobHandler};
# #[allow(unused_imports)] use std::{result::Result, sync::Arc, time::Duration};
# #[allow(unused_imports)] use serde_json::json;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(pool: sqlx::PgPool, queue: Arc<JobQueue<sqlx::Postgres>>, config: EncryptionConfig, encryption_config: EncryptionConfig, job_id: JobId, mut key_manager: KeyManager<sqlx::Postgres>, handler: JobHandler, payload: serde_json::Value, user_data: serde_json::Value, patient_data: serde_json::Value, payment_data: serde_json::Value, financial_data: serde_json::Value, worker: Worker<sqlx::Postgres>) -> std::result::Result<(), Box<dyn std::error::Error>> {
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
# Ok(())
# }
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

```rust,no_run
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

    worker_pool.start().await?;
    Ok(())
}
```

### Key Management Example

```rust,no_run
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
3. **Memory Protection**: Keys are held in process memory while the engine exists, without copies per operation, and are wiped (`zeroize`) when the engine, key manager or KMS client drops them; they are not printed by `Debug`. Neither are database URL passwords, webhook and alert secrets and URLs (whose paths can be credentials), streaming credentials, KMS credentials or the dashboard's password hash; `cargo hammerwork config show` masks the database password.
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

```rust,no_run
# #[allow(unused_imports)] use hammerwork::{*, encryption::*, queue::DatabaseQueue, worker::JobHandler};
# #[allow(unused_imports)] use std::{result::Result, sync::Arc, time::Duration};
# #[allow(unused_imports)] use serde_json::json;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(pool: sqlx::PgPool, queue: Arc<JobQueue<sqlx::Postgres>>, config: EncryptionConfig, encryption_config: EncryptionConfig, job_id: JobId, mut key_manager: KeyManager<sqlx::Postgres>, handler: JobHandler, payload: serde_json::Value, user_data: serde_json::Value, patient_data: serde_json::Value, payment_data: serde_json::Value, financial_data: serde_json::Value, worker: Worker<sqlx::Postgres>) -> std::result::Result<(), Box<dyn std::error::Error>> {
// Right to be forgotten
let job = Job::new("user_export".to_string(), user_data)
    .with_encryption(encryption_config)
    .with_pii_fields(vec!["personal_data"])
    .with_retention_policy(RetentionPolicy::DeleteAfter(Duration::from_secs(30 * 24 * 60 * 60)));
# Ok(())
# }
```

### HIPAA (Health Insurance Portability and Accountability Act)

```rust,no_run
# #[allow(unused_imports)] use hammerwork::{*, encryption::*, queue::DatabaseQueue, worker::JobHandler};
# #[allow(unused_imports)] use std::{result::Result, sync::Arc, time::Duration};
# #[allow(unused_imports)] use serde_json::json;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(pool: sqlx::PgPool, queue: Arc<JobQueue<sqlx::Postgres>>, config: EncryptionConfig, encryption_config: EncryptionConfig, job_id: JobId, mut key_manager: KeyManager<sqlx::Postgres>, handler: JobHandler, payload: serde_json::Value, user_data: serde_json::Value, patient_data: serde_json::Value, payment_data: serde_json::Value, financial_data: serde_json::Value, worker: Worker<sqlx::Postgres>) -> std::result::Result<(), Box<dyn std::error::Error>> {
// Healthcare data protection
let job = Job::new("patient_processing".to_string(), patient_data)
    .with_encryption(EncryptionConfig::new(EncryptionAlgorithm::AES256GCM))
    .with_pii_fields(vec!["medical_record_number", "patient_info"])
    .with_retention_policy(RetentionPolicy::DeleteAfter(Duration::from_secs(6 * 365 * 24 * 60 * 60)));
# Ok(())
# }
```

### PCI DSS (Payment Card Industry Data Security Standard)

```rust,no_run
# #[allow(unused_imports)] use hammerwork::{*, encryption::*, queue::DatabaseQueue, worker::JobHandler};
# #[allow(unused_imports)] use std::{result::Result, sync::Arc, time::Duration};
# #[allow(unused_imports)] use serde_json::json;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(pool: sqlx::PgPool, queue: Arc<JobQueue<sqlx::Postgres>>, config: EncryptionConfig, encryption_config: EncryptionConfig, job_id: JobId, mut key_manager: KeyManager<sqlx::Postgres>, handler: JobHandler, payload: serde_json::Value, user_data: serde_json::Value, patient_data: serde_json::Value, payment_data: serde_json::Value, financial_data: serde_json::Value, worker: Worker<sqlx::Postgres>) -> std::result::Result<(), Box<dyn std::error::Error>> {
// Payment card data protection
let job = Job::new("payment_processing".to_string(), payment_data)
    .with_encryption(EncryptionConfig::new(EncryptionAlgorithm::AES256GCM))
    .with_pii_fields(vec!["card_number", "cvv", "cardholder_name"])
    .with_retention_policy(RetentionPolicy::DeleteAfter(Duration::from_secs(365 * 24 * 60 * 60)));
# Ok(())
# }
```

### SOX (Sarbanes-Oxley Act)

```rust,no_run
# #[allow(unused_imports)] use hammerwork::{*, encryption::*, queue::DatabaseQueue, worker::JobHandler};
# #[allow(unused_imports)] use std::{result::Result, sync::Arc, time::Duration};
# #[allow(unused_imports)] use serde_json::json;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(pool: sqlx::PgPool, queue: Arc<JobQueue<sqlx::Postgres>>, config: EncryptionConfig, encryption_config: EncryptionConfig, job_id: JobId, mut key_manager: KeyManager<sqlx::Postgres>, handler: JobHandler, payload: serde_json::Value, user_data: serde_json::Value, patient_data: serde_json::Value, payment_data: serde_json::Value, financial_data: serde_json::Value, worker: Worker<sqlx::Postgres>) -> std::result::Result<(), Box<dyn std::error::Error>> {
// Financial data retention
let job = Job::new("financial_reporting".to_string(), financial_data)
    .with_encryption(encryption_config)
    .with_pii_fields(vec!["financial_records"])
    .with_retention_policy(RetentionPolicy::DeleteAfter(Duration::from_secs(7 * 365 * 24 * 60 * 60)));
# Ok(())
# }
```

## Migration Guide

### Enabling Encryption on Existing Jobs

1. **Run Migrations**: `cargo hammerwork migration run` (migration 011 adds the columns)
2. **Configure Keys**: create an `EncryptionEngine` and pass it to `JobQueue::with_encryption` on every queue that enqueues encrypted jobs **and on every queue used by workers that process them**. Workers without an engine fail encrypted jobs.
3. **Update Code**: add `with_encryption` (and optionally `with_pii_fields`) to the jobs to protect
4. **Schedule the retention purge**: `cargo hammerwork maintenance purge-encrypted --confirm`

### Migrating Existing Data

Jobs enqueued before encryption was enabled stay in plaintext; nothing re-encrypts them. To protect pending ones, re-enqueue them with an encryption config on a queue with an engine and delete the originals:

```rust,no_run
# #[allow(unused_imports)] use hammerwork::{*, encryption::*, queue::DatabaseQueue, worker::JobHandler};
# #[allow(unused_imports)] use std::{result::Result, sync::Arc, time::Duration};
# #[allow(unused_imports)] use serde_json::json;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(pool: sqlx::PgPool, queue: Arc<JobQueue<sqlx::Postgres>>, config: EncryptionConfig, encryption_config: EncryptionConfig, job_id: JobId, mut key_manager: KeyManager<sqlx::Postgres>, handler: JobHandler, payload: serde_json::Value, user_data: serde_json::Value, patient_data: serde_json::Value, payment_data: serde_json::Value, financial_data: serde_json::Value, worker: Worker<sqlx::Postgres>) -> std::result::Result<(), Box<dyn std::error::Error>> {
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
# Ok(())
# }
```
