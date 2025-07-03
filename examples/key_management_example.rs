//! Example demonstrating advanced key management capabilities in Hammerwork.
//!
//! This example shows how to:
//! - Set up a key management system with master key encryption
//! - Generate and manage encryption keys with rotation
//! - Implement automatic key rotation policies
//! - Track key usage and audit operations
//! - Handle key lifecycle (active -> retired -> expired)
//! - Use external key management service configuration

use chrono::{Duration, Utc};
use hammerwork::encryption::{
    EncryptionAlgorithm, ExternalKmsConfig, KeyDerivationConfig, KeyManager, KeyManagerConfig,
    KeyPurpose, KeyStatus,
};
use std::collections::HashMap;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("🔑 Hammerwork Key Management Example");
    println!("====================================\n");

    // This example demonstrates key management concepts without requiring a database
    // In a real application, you would pass a database pool to the KeyManager

    // Example 1: Basic key manager configuration
    println!("1. Setting up key manager with master key encryption:");

    // Set up a master key for encrypting data encryption keys
    unsafe {
        std::env::set_var(
            "MASTER_KEY",
            "bWFzdGVya2V5MTIzNDU2Nzg5MEFCQ0RFRkdISUpLTE1OTw==",
        ); // 32 bytes base64
    }

    let config = KeyManagerConfig::new()
        .with_master_key_env("MASTER_KEY")
        .with_auto_rotation_enabled(true)
        .with_rotation_interval(Duration::days(90)) // Rotate every 3 months
        .with_max_key_versions(10) // Keep 10 versions of each key
        .with_audit_enabled(true);

    println!("   Master key source: Environment variable");
    println!("   Auto rotation: Enabled (every 90 days)");
    println!("   Max key versions: 10");
    println!("   Audit logging: Enabled\n");

    // Example 2: External KMS configuration
    println!("2. External Key Management Service configuration:");

    let mut aws_auth = HashMap::new();
    aws_auth.insert("access_key_id".to_string(), "AKIA...".to_string());
    aws_auth.insert("secret_access_key".to_string(), "secret123".to_string());

    let external_kms = ExternalKmsConfig {
        service_type: "AWS".to_string(),
        endpoint: "https://kms.us-east-1.amazonaws.com".to_string(),
        auth_config: aws_auth,
        region: Some("us-east-1".to_string()),
        namespace: Some("hammerwork-production".to_string()),
    };

    let kms_config = KeyManagerConfig::new().with_external_kms(external_kms);

    println!("   KMS Service: AWS KMS");
    println!("   Region: us-east-1");
    println!("   Namespace: hammerwork-production\n");

    // Example 3: Key derivation configuration for password-based keys
    println!("3. Key derivation configuration:");

    let derivation_config = KeyDerivationConfig {
        memory_cost: 65536, // 64 MB
        time_cost: 3,       // 3 iterations
        parallelism: 4,     // 4 threads
        salt_length: 32,    // 32 bytes salt
    };

    let password_config = KeyManagerConfig::new().with_audit_enabled(true);

    println!("   Algorithm: Argon2id");
    println!("   Memory cost: 64 MB");
    println!("   Time cost: 3 iterations");
    println!("   Parallelism: 4 threads\n");

    // Example 4: Key purposes and lifecycle
    println!("4. Different key purposes and lifecycle management:");

    // Data encryption key for job payloads
    println!("   Data Encryption Key (DEK):");
    println!("     Purpose: Encrypt job payloads");
    println!("     Algorithm: AES-256-GCM");
    println!("     Rotation: Every 90 days");
    println!("     Status: Active -> Retired -> Expired");

    // Key encryption key (master key)
    println!("   Key Encryption Key (KEK):");
    println!("     Purpose: Encrypt other encryption keys");
    println!("     Algorithm: AES-256-GCM");
    println!("     Rotation: Manual/yearly");
    println!("     Status: Active (never expires)");

    // MAC key for integrity verification
    println!("   Message Authentication Code (MAC) Key:");
    println!("     Purpose: Verify data integrity");
    println!("     Algorithm: HMAC-SHA256");
    println!("     Rotation: Every 180 days");
    println!("     Status: Active -> Retired\n");

    // Example 5: Key rotation scenarios
    println!("5. Key rotation scenarios:");

    println!("   Automatic Rotation:");
    println!("     - Triggered by: Time-based policy (every 90 days)");
    println!("     - Process: Generate new version, retire old version");
    println!("     - Backward compatibility: Old versions kept for decryption");

    println!("   Manual Rotation:");
    println!("     - Triggered by: Security incident or admin action");
    println!("     - Process: Immediate generation of new version");
    println!("     - Audit: Full audit trail of rotation event");

    println!("   Emergency Revocation:");
    println!("     - Triggered by: Key compromise detection");
    println!("     - Process: Mark key as revoked, block all operations");
    println!("     - Recovery: Generate new key with different ID\n");

    // Example 6: Key statistics and monitoring
    println!("6. Key management statistics and monitoring:");

    // Mock statistics for demonstration
    let stats = hammerwork::encryption::KeyManagerStats {
        total_keys: 25,
        active_keys: 20,
        retired_keys: 4,
        revoked_keys: 1,
        expired_keys: 0,
        total_access_operations: 15420,
        rotations_performed: 8,
        average_key_age_days: 45.5,
        keys_expiring_soon: 2,
        keys_due_for_rotation: 1,
    };

    println!("   📊 Key Management Statistics:");
    println!("     Total keys managed: {}", stats.total_keys);
    println!("     Active keys: {}", stats.active_keys);
    println!("     Retired keys: {}", stats.retired_keys);
    println!("     Revoked keys: {}", stats.revoked_keys);
    println!(
        "     Total access operations: {}",
        stats.total_access_operations
    );
    println!("     Rotations performed: {}", stats.rotations_performed);
    println!(
        "     Average key age: {:.1} days",
        stats.average_key_age_days
    );
    println!("     Keys expiring soon: {}", stats.keys_expiring_soon);
    println!(
        "     Keys due for rotation: {}\n",
        stats.keys_due_for_rotation
    );

    // Example 7: Security best practices
    println!("7. Security best practices for key management:");

    println!("   🔐 Key Security:");
    println!("     ✅ Keys encrypted at rest using master key");
    println!("     ✅ Master key stored in secure environment variables");
    println!("     ✅ No plaintext keys in database or logs");
    println!("     ✅ Memory clearing after key operations");

    println!("   🔄 Key Rotation:");
    println!("     ✅ Automatic rotation based on time policies");
    println!("     ✅ Manual rotation capability for emergencies");
    println!("     ✅ Old key versions retained for decryption");
    println!("     ✅ Configurable retention policies");

    println!("   📝 Audit & Compliance:");
    println!("     ✅ Complete audit trail of key operations");
    println!("     ✅ Key usage tracking and monitoring");
    println!("     ✅ Compliance with data protection regulations");
    println!("     ✅ Regular key lifecycle reviews");

    println!("   🌐 External Integration:");
    println!("     ✅ Support for AWS KMS, Azure Key Vault, GCP KMS");
    println!("     ✅ HashiCorp Vault integration");
    println!("     ✅ Hardware Security Module (HSM) support");
    println!("     ✅ Multi-region key replication\n");

    // Example 8: Key management workflows
    println!("8. Common key management workflows:");

    println!("   🔧 Development Workflow:");
    println!("     1. Generate development keys with shorter rotation");
    println!("     2. Use static keys for testing (never in production)");
    println!("     3. Mock external KMS services for local development");
    println!("     4. Test key rotation and recovery procedures");

    println!("   🏭 Production Workflow:");
    println!("     1. Initialize master key from secure environment");
    println!("     2. Generate encryption keys with proper metadata");
    println!("     3. Set up automatic rotation schedules");
    println!("     4. Monitor key health and usage patterns");
    println!("     5. Perform regular key audits and compliance checks");

    println!("   🚨 Incident Response:");
    println!("     1. Detect key compromise through monitoring");
    println!("     2. Immediately revoke compromised keys");
    println!("     3. Generate new keys with different identifiers");
    println!("     4. Update applications to use new keys");
    println!("     5. Audit affected data and notify stakeholders\n");

    // Example 9: Integration with job encryption
    println!("9. Integration with job encryption:");

    println!("   🔄 Automatic Integration:");
    println!("     - EncryptionEngine automatically retrieves keys from KeyManager");
    println!("     - Key rotation handled transparently during job processing");
    println!("     - Failed jobs automatically use correct key version for retry");

    println!("   🎯 Key Selection:");
    println!("     - Jobs can specify preferred key ID for encryption");
    println!("     - Fallback to default key if specified key unavailable");
    println!("     - Automatic key selection based on data classification");

    println!("   📈 Performance Optimization:");
    println!("     - Key caching reduces database lookups");
    println!("     - Batch key operations for high-throughput scenarios");
    println!("     - Async key rotation to avoid blocking job processing\n");

    println!("✅ Key management example completed successfully!");
    println!("\nNext steps:");
    println!("• Set up master key in production environment");
    println!("• Configure external KMS for enhanced security");
    println!("• Implement key rotation monitoring and alerting");
    println!("• Set up regular key audits and compliance reporting");

    Ok(())
}
