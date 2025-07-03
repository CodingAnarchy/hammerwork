# Job Encryption & PII Protection

Hammerwork provides enterprise-grade encryption capabilities for protecting sensitive job payloads, particularly personally identifiable information (PII). This document covers encryption configuration, key management, and best practices for data protection.

## Table of Contents

- [Overview](#overview)
- [Encryption Algorithms](#encryption-algorithms)
- [Configuration](#configuration)
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

- **Field-Level Encryption**: Encrypt only PII fields, leaving metadata accessible
- **Multiple Algorithms**: AES-256-GCM and ChaCha20-Poly1305 support
- **Transparent Processing**: Jobs are automatically encrypted/decrypted
- **Key Management**: Enterprise key lifecycle with rotation and audit trails
- **Retention Policies**: Automatic deletion for compliance requirements
- **Zero Overhead**: Optional compilation - only enabled when needed

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

```rust
let config = EncryptionConfig::new(EncryptionAlgorithm::AES256GCM)
    .with_key_source(KeySource::External("aws-kms://key-id".to_string()));
```

## PII Field Protection

### Automatic PII Detection

Hammerwork can automatically detect common PII patterns:

```rust
use hammerwork::{Job, encryption::EncryptionConfig};
use serde_json::json;

let job = Job::new("payment_processing".to_string(), json!({
    "user_id": "user123",
    "credit_card": "4111-1111-1111-1111",  // Automatically detected as PII
    "ssn": "123-45-6789",                  // Automatically detected as PII
    "email": "user@example.com",           // Automatically detected as PII
    "amount": 99.99,
    "timestamp": "2024-01-01T00:00:00Z"
}))
.with_encryption(EncryptionConfig::new(EncryptionAlgorithm::AES256GCM))
.with_auto_pii_detection(true);
```

### Manual PII Field Specification

For precise control, specify PII fields explicitly:

```rust
let job = Job::new("user_data_processing".to_string(), json!({
    "user_id": "user123",
    "credit_card": "4111-1111-1111-1111",
    "billing_address": "123 Main St",
    "phone": "+1-555-123-4567",
    "preferences": {"newsletter": true}
}))
.with_encryption(EncryptionConfig::new(EncryptionAlgorithm::AES256GCM))
.with_pii_fields(vec!["credit_card", "billing_address", "phone"]);
```

### PII Detection Patterns

Hammerwork recognizes these PII patterns automatically:

- **Credit Cards**: Visa, MasterCard, Amex, Discover patterns
- **Social Security Numbers**: XXX-XX-XXXX, XXXXXXXXX formats
- **Email Addresses**: RFC 5322 compliant email patterns
- **Phone Numbers**: US and international phone number patterns
- **IP Addresses**: IPv4 and IPv6 addresses
- **API Keys**: Common API key patterns (AWS, GitHub, etc.)

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

// Automatic rotation (configured intervals)
let rotated_keys = key_manager.perform_automatic_rotation().await?;
for key_id in rotated_keys {
    println!("Auto-rotated key: {}", key_id);
}
```

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

The `hammerwork_jobs` table includes encryption fields:

```sql
-- PostgreSQL schema additions
ALTER TABLE hammerwork_jobs ADD COLUMN is_encrypted BOOLEAN DEFAULT FALSE;
ALTER TABLE hammerwork_jobs ADD COLUMN encrypted_payload BYTEA;
ALTER TABLE hammerwork_jobs ADD COLUMN pii_fields TEXT[];
ALTER TABLE hammerwork_jobs ADD COLUMN retention_policy JSONB;

-- MySQL schema additions  
ALTER TABLE hammerwork_jobs ADD COLUMN is_encrypted BOOLEAN DEFAULT FALSE;
ALTER TABLE hammerwork_jobs ADD COLUMN encrypted_payload LONGBLOB;
ALTER TABLE hammerwork_jobs ADD COLUMN pii_fields JSON;
ALTER TABLE hammerwork_jobs ADD COLUMN retention_policy JSON;
```

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
use hammerwork::{Job, JobQueue, Worker, WorkerPool};
use hammerwork::encryption::{EncryptionConfig, EncryptionAlgorithm, KeySource, RetentionPolicy};
use serde_json::json;
use std::{sync::Arc, time::Duration};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Setup
    let pool = sqlx::PgPool::connect("postgresql://localhost/hammerwork").await?;
    let queue = Arc::new(JobQueue::new(pool));

    // Configure encryption
    let encryption_config = EncryptionConfig::new(EncryptionAlgorithm::AES256GCM)
        .with_key_source(KeySource::Environment("HAMMERWORK_ENCRYPTION_KEY".to_string()))
        .with_compression_enabled(true);

    // Create encrypted job
    let job = Job::new("sensitive_data_processing".to_string(), json!({
        "user_id": "user123",
        "credit_card": "4111-1111-1111-1111",
        "ssn": "123-45-6789",
        "transaction_amount": 299.99,
        "timestamp": "2024-01-01T00:00:00Z"
    }))
    .with_encryption(encryption_config)
    .with_pii_fields(vec!["credit_card", "ssn"])
    .with_retention_policy(RetentionPolicy::DeleteAfter(Duration::from_secs(30 * 24 * 60 * 60))); // 30 days

    // Enqueue - automatic encryption occurs
    queue.enqueue(job).await?;

    // Worker processes decrypted data transparently
    let handler = Arc::new(|job: Job| {
        Box::pin(async move {
            // Job payload is automatically decrypted
            println!("Processing transaction: {}", job.payload["transaction_amount"]);
            
            // PII fields are accessible in plain text
            let credit_card = job.payload["credit_card"].as_str().unwrap();
            let ssn = job.payload["ssn"].as_str().unwrap();
            
            // Process the sensitive data
            // ...
            
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
3. **Memory Protection**: Keys are zeroed from memory when possible
4. **Error Handling**: Avoid leaking sensitive information in logs

## Performance

### Encryption Overhead

- **AES-256-GCM**: ~10-20% overhead with hardware acceleration
- **ChaCha20-Poly1305**: ~15-25% overhead, consistent across platforms
- **Field-Level**: Only PII fields encrypted, minimal metadata impact
- **Compression**: Large payloads compressed before encryption

### Optimization Tips

1. **Selective Encryption**: Only encrypt PII fields, not entire payloads
2. **Compression**: Enable compression for large payloads
3. **Key Caching**: Keys cached in memory for 1 hour
4. **Batch Operations**: Process multiple jobs with same key efficiently

### Benchmarks

```
Encryption Performance (1KB payload, 2 PII fields):
- AES-256-GCM:     ~0.05ms per operation
- ChaCha20-Poly1305: ~0.08ms per operation
- Key retrieval:   ~0.01ms (cached), ~2ms (database)
- Total overhead:  ~5-10% for typical workloads
```

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

1. **Run Migration**: Apply migration 011_add_encryption
2. **Configure Keys**: Set up encryption keys
3. **Update Code**: Add encryption to new jobs
4. **Gradual Rollout**: Encrypt new jobs while processing existing ones

### Migrating Existing Data

```rust
// Example migration script
async fn migrate_existing_jobs() -> Result<(), Box<dyn std::error::Error>> {
    let queue = JobQueue::new(pool);
    
    // Get unencrypted jobs with PII
    let jobs = queue.list_jobs(Some("payment_processing"), None, None).await?;
    
    for job in jobs {
        if !job.is_encrypted && contains_pii(&job.payload) {
            // Re-enqueue with encryption
            let encrypted_job = Job::new(job.queue_name, job.payload)
                .with_encryption(encryption_config.clone())
                .with_pii_fields(detect_pii_fields(&job.payload));
            
            queue.enqueue(encrypted_job).await?;
            queue.delete_job(job.id).await?;
        }
    }
    
    Ok(())
}
```

For more examples and detailed API documentation, see the [API documentation](https://docs.rs/hammerwork) and the `examples/encryption_example.rs` file.