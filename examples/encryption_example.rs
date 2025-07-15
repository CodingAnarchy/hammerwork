//! Example demonstrating job payload encryption and PII protection in Hammerwork.
//!
//! This example shows how to:
//! - Configure encryption for job payloads
//! - Mark PII fields for special handling
//! - Set retention policies for encrypted data
//! - Use different encryption algorithms
//! - Automatically detect PII fields

use hammerwork::{
    Job,
    encryption::{
        EncryptionAlgorithm, EncryptionConfig, EncryptionEngine, KeySource, RetentionPolicy,
    },
};
use serde_json::json;
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("🔐 Hammerwork Job Encryption Example");
    println!("======================================\n");

    // Set up a test encryption key in environment
    unsafe {
        std::env::set_var(
            "ENCRYPTION_KEY",
            "dGVzdGtleTE5ODc2NTQzMjEwOTg3NjU0MzIxMHRlc3Q=",
        ); // 32 bytes base64
    }

    // Example 1: Basic job with encryption
    println!("1. Creating job with encryption enabled:");
    let payment_data = json!({
        "transaction_id": "txn_123456",
        "user_id": "user_789",
        "credit_card_number": "4111-1111-1111-1111",
        "ssn": "123-45-6789",
        "amount": 99.99,
        "currency": "USD",
        "email": "customer@example.com"
    });

    let job = Job::new("payment_processing".to_string(), payment_data)
        .with_encryption(
            EncryptionConfig::new(EncryptionAlgorithm::AES256GCM)
                .with_key_source(KeySource::Environment("ENCRYPTION_KEY".to_string()))
                .with_compression_enabled(true),
        )
        .with_pii_fields(vec!["credit_card_number", "ssn", "email"])
        .with_retention_policy(RetentionPolicy::DeleteAfter(Duration::from_secs(
            7 * 24 * 60 * 60,
        ))); // 7 days

    println!("   Job ID: {}", job.id);
    println!("   Has encryption: {}", job.has_encryption());
    println!("   PII fields: {:?}", job.get_pii_fields());
    println!("   Is encrypted: {}\n", job.is_payload_encrypted());

    // Example 2: Encryption engine usage
    if cfg!(feature = "encryption") {
        println!("2. Using encryption engine directly:");

        let config = EncryptionConfig::new(EncryptionAlgorithm::ChaCha20Poly1305)
            .with_key_source(KeySource::Environment("ENCRYPTION_KEY".to_string()))
            .with_key_id("example-key-1");

        let mut engine = EncryptionEngine::new(config).await?;

        // Automatic PII detection
        let sensitive_payload = json!({
            "order_id": "ord_456",
            "customer_email": "john.doe@example.com",
            "credit_card": "5555-5555-5555-4444",
            "billing_address": {
                "street": "123 Main St",
                "phone_number": "+1-555-123-4567"
            },
            "amount": 149.99
        });

        let detected_pii = engine.identify_pii_fields(&sensitive_payload);
        println!("   Detected PII fields: {:?}", detected_pii);

        // Encrypt the payload
        let encrypted_payload = engine
            .encrypt_payload(&sensitive_payload, &detected_pii)
            .await?;
        println!(
            "   Encrypted payload size: {} bytes",
            encrypted_payload.size_bytes()
        );
        println!(
            "   Contains PII field 'customer_email': {}",
            encrypted_payload.contains_pii_field("customer_email")
        );

        // Decrypt the payload
        let decrypted_payload = engine.decrypt_payload(&encrypted_payload).await?;
        println!(
            "   Decryption successful: {}",
            decrypted_payload == sensitive_payload
        );

        // Show encryption statistics
        let stats = engine.get_stats();
        println!("   Jobs encrypted: {}", stats.jobs_encrypted);
        println!("   PII fields encrypted: {}", stats.pii_fields_encrypted);
        println!("   Success rate: {:.2}%\n", stats.encryption_success_rate());
    } else {
        println!("2. Encryption feature not enabled (compile with --features encryption)\n");
    }

    // Example 3: Different retention policies
    println!("3. Different retention policies:");

    let immediate_cleanup = Job::new("temp_processing".to_string(), json!({"temp": "data"}))
        .with_retention_policy(RetentionPolicy::DeleteImmediately);
    println!(
        "   Immediate cleanup job: should_cleanup = {}",
        immediate_cleanup.should_cleanup_encrypted_data()
    );

    let timed_cleanup = Job::new("timed_processing".to_string(), json!({"data": "sensitive"}))
        .with_retention_policy(RetentionPolicy::DeleteAfter(Duration::from_secs(1)));
    println!(
        "   Timed cleanup job: should_cleanup = {}",
        timed_cleanup.should_cleanup_encrypted_data()
    );

    tokio::time::sleep(Duration::from_millis(1100)).await;
    println!(
        "   After 1.1 seconds: should_cleanup = {}",
        timed_cleanup.should_cleanup_encrypted_data()
    );

    let indefinite_job = Job::new("permanent".to_string(), json!({"data": "keep"}))
        .with_retention_policy(RetentionPolicy::KeepIndefinitely);
    println!(
        "   Indefinite retention: should_cleanup = {}\n",
        indefinite_job.should_cleanup_encrypted_data()
    );

    // Example 4: Algorithm comparison
    println!("4. Available encryption algorithms:");
    println!("   • AES-256-GCM: Industry standard, hardware acceleration");
    println!("   • ChaCha20-Poly1305: Software-optimized, timing-attack resistant");

    let aes_config = EncryptionConfig::new(EncryptionAlgorithm::AES256GCM);
    println!(
        "   AES key size: {} bytes, nonce: {} bytes, tag: {} bytes",
        aes_config.key_size_bytes(),
        aes_config.nonce_size_bytes(),
        aes_config.tag_size_bytes()
    );

    let chacha_config = EncryptionConfig::new(EncryptionAlgorithm::ChaCha20Poly1305);
    println!(
        "   ChaCha20 key size: {} bytes, nonce: {} bytes, tag: {} bytes\n",
        chacha_config.key_size_bytes(),
        chacha_config.nonce_size_bytes(),
        chacha_config.tag_size_bytes()
    );

    println!("✅ Encryption example completed successfully!");
    println!("\nNext steps:");
    println!("• Set up proper key management (not environment variables in production)");
    println!("• Configure database migrations to store encrypted payloads");
    println!("• Implement key rotation policies");
    println!("• Set up monitoring for encryption operations");

    Ok(())
}
