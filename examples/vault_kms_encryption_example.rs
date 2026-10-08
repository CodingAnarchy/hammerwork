//! Example demonstrating HashiCorp Vault KMS integration for key management in Hammerwork.
//!
//! This example shows how to:
//! - Configure HashiCorp Vault for key management
//! - Use Vault KV secrets engine for encryption key storage
//! - Set up proper Vault authentication and policies
//! - Handle Vault errors gracefully
//! - Fail closed (return an error) when the KMS is unavailable

use hammerwork::{
    Job,
    encryption::{
        EncryptionAlgorithm, EncryptionConfig, EncryptionEngine, KeyManagerConfig, KeySource,
        RetentionPolicy,
    },
};
use serde_json::json;
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("🔐 Hammerwork HashiCorp Vault KMS Integration Example");
    println!("=====================================================\n");

    // Example 1: HashiCorp Vault KMS Configuration
    println!("1. Configuring HashiCorp Vault KMS integration:");
    println!("   You can configure Vault KMS in several ways:");
    println!("   • vault://secret/hammerwork/encryption-key");
    println!("   • vault://secret/hammerwork/encryption-key?addr=https://vault.example.com");
    println!("   • Environment variables: VAULT_ADDR, VAULT_TOKEN");
    println!("   • Token authentication: Set VAULT_TOKEN environment variable");
    println!("   • AppRole authentication: Configure via Vault client settings");
    println!();

    // Example 2: Basic Vault KMS encryption setup
    if cfg!(feature = "vault-kms") {
        println!("2. Setting up encryption with HashiCorp Vault:");

        // Configure encryption to use Vault KMS
        let vault_kms_config = EncryptionConfig::new(EncryptionAlgorithm::AES256GCM)
            .with_key_source(KeySource::External(
                "vault://secret/hammerwork/encryption-key".to_string(),
            ))
            .with_key_id("vault-kms-data-key")
            .with_compression_enabled(true);

        match EncryptionEngine::new(vault_kms_config).await {
            Ok(mut engine) => {
                println!("   ✅ Vault KMS encryption engine created successfully");

                // Test encryption with Vault KMS
                let sensitive_data = json!({
                    "customer_id": "cust_98765",
                    "credit_card_number": "4000-0000-0000-0002",
                    "ssn": "555-44-3333",
                    "email": "alice.wonder@example.com",
                    "transaction_amount": 2799.99
                });

                let pii_fields = engine.identify_pii_fields(&sensitive_data);
                println!("   Detected PII fields: {:?}", pii_fields);

                let encrypted = engine.encrypt_payload(&sensitive_data, &pii_fields).await?;
                println!(
                    "   Encryption successful, size: {} bytes",
                    encrypted.size_bytes()
                );

                let decrypted = engine.decrypt_payload(&encrypted).await?;
                println!("   Decryption successful: {}", decrypted == sensitive_data);

                let stats = engine.get_stats();
                println!(
                    "   Encryption stats: {} jobs, {:.2}% success rate",
                    stats.jobs_encrypted,
                    stats.encryption_success_rate()
                );
            }
            Err(e) => {
                println!("   ⚠️  Vault KMS not available (key loading failed): {}", e);
                println!("   This is normal in development without proper Vault credentials");
            }
        }
    } else {
        println!("2. Vault KMS feature not enabled");
        println!(
            "   Compile with: cargo run --example vault_kms_encryption_example --features vault-kms"
        );
    }

    println!();

    // Example 3: Key Manager with Vault KMS
    if cfg!(all(feature = "vault-kms", feature = "postgres")) {
        println!("3. Advanced key management with HashiCorp Vault:");

        // Note: This requires a PostgreSQL database connection
        // In a real application, you would provide a proper database pool
        println!("   Key manager configuration with Vault KMS:");

        let kms_config = KeyManagerConfig::new()
            .with_master_key_source(KeySource::External(
                "vault://secret/hammerwork/master-key?addr=https://vault.example.com".to_string(),
            ))
            .with_auto_rotation_enabled(true)
            .with_rotation_interval(chrono::Duration::days(30))
            .with_audit_enabled(true);

        println!("   Master key source: Vault secret/hammerwork/master-key at vault.example.com");
        println!(
            "   Auto rotation: {} (every 30 days)",
            kms_config.auto_rotation_enabled
        );
        println!("   Audit logging: {}", kms_config.audit_enabled);
        println!("   Max key versions: {}", kms_config.max_key_versions);

        // In a real application, you would create the key manager like this:
        // let pool = PgPool::connect("postgresql://...").await?;
        // let key_manager = KeyManager::new(kms_config, pool).await?;

        println!("   Note: Requires PostgreSQL connection pool for full functionality");
    } else {
        println!("3. Advanced key management requires both vault-kms and postgres features");
    }

    println!();

    // Example 4: Job encryption with Vault KMS key source
    println!("4. Creating jobs with Vault KMS encryption:");

    let payment_job = Job::new(
        "process_payment".to_string(),
        json!({
            "order_id": "order_654",
            "customer_email": "bob.builder@example.com",
            "credit_card": "5555-5555-5555-4444",
            "amount": 899.99,
            "currency": "USD"
        }),
    );

    // Configure with Vault KMS (fails if the KMS is not available)
    let vault_job = payment_job
        .with_encryption(
            EncryptionConfig::new(EncryptionAlgorithm::AES256GCM).with_key_source(
                KeySource::External("vault://secret/hammerwork/data-keys".to_string()),
            ),
        )
        .with_pii_fields(vec!["customer_email", "credit_card"])
        .with_retention_policy(RetentionPolicy::DeleteAfter(Duration::from_secs(
            365 * 24 * 60 * 60, // 365 days
        )));

    println!("   Job ID: {}", vault_job.id);
    println!("   Encryption enabled: {}", vault_job.has_encryption());
    println!("   PII fields: {:?}", vault_job.get_pii_fields());
    println!("   Retention: 365 days");
    println!();

    // Example 5: HashiCorp Vault best practices
    println!("5. HashiCorp Vault Best Practices:");
    println!("   Security:");
    println!("   • Use strong authentication methods (AppRole, LDAP, AWS IAM)");
    println!("   • Enable audit logging for all secret operations");
    println!("   • Use different namespaces for different environments");
    println!("   • Implement proper secret rotation policies");
    println!("   • Use Vault policies to restrict access to specific paths");
    println!("   • Enable TLS/SSL for all Vault communication");
    println!();

    println!("   Performance:");
    println!("   • Use Vault agent for token renewal and caching");
    println!("   • Implement connection pooling for high-volume operations");
    println!("   • Consider Vault performance standby replicas");
    println!("   • Monitor Vault metrics and set up alerting");
    println!();

    println!("   Operations:");
    println!("   • Use Vault namespaces for multi-tenancy");
    println!("   • Implement proper backup and disaster recovery");
    println!("   • Use Vault's built-in high availability features");
    println!("   • Monitor secret access patterns and usage");
    println!();

    // Example 6: Environment setup instructions
    println!("6. Setup Instructions:");
    println!("   Prerequisites:");
    println!("   1. HashiCorp Vault server running and unsealed:");
    println!("      vault server -dev  # For development only");
    println!("      vault status");
    println!();
    println!("   2. Create KV secrets engine and policies:");
    println!("      vault secrets enable -path=secret kv-v2");
    println!("      vault policy write hammerwork-policy - <<EOF");
    println!("      path \"secret/data/hammerwork/*\" {{");
    println!("        capabilities = [\"read\", \"list\"]");
    println!("      }}");
    println!("      EOF");
    println!();
    println!("   3. Create authentication method (token example):");
    println!("      vault token create -policy=hammerwork-policy -ttl=24h");
    println!();
    println!("   4. Store encryption keys in Vault:");
    println!("      # Master key (32 bytes base64 encoded)");
    println!("      vault kv put secret/hammerwork/master-key \\");
    println!("        key=$(openssl rand -base64 32)");
    println!("      ");
    println!("      # Data encryption key (32 bytes base64 encoded)");
    println!("      vault kv put secret/hammerwork/encryption-key \\");
    println!("        key=$(openssl rand -base64 32)");
    println!();
    println!("   5. Set environment variables:");
    println!("      export VAULT_ADDR=https://vault.example.com:8200");
    println!("      export VAULT_TOKEN=your-vault-token");
    println!("      export VAULT_NAMESPACE=your-namespace  # Optional");
    println!();
    println!("   6. AppRole authentication setup (recommended for production):");
    println!("      vault auth enable approle");
    println!("      vault write auth/approle/role/hammerwork \\");
    println!("        token_policies=\"hammerwork-policy\" \\");
    println!("        token_ttl=1h \\");
    println!("        token_max_ttl=4h");
    println!("      ");
    println!("      # Get role ID and secret ID");
    println!("      vault read auth/approle/role/hammerwork/role-id");
    println!("      vault write -f auth/approle/role/hammerwork/secret-id");
    println!();

    // Example 7: Secret path formats
    println!("7. Secret Path Formats:");
    println!("   Basic format:");
    println!("   vault://mount/path/to/secret");
    println!();
    println!("   With custom Vault address:");
    println!("   vault://mount/path/to/secret?addr=https://vault.example.com:8200");
    println!();
    println!("   Examples:");
    println!("   • vault://secret/hammerwork/master-key");
    println!("   • vault://secret/hammerwork/encryption-key");
    println!("   • vault://kv/production/hammerwork/keys");
    println!("   • vault://secret/env/prod/encryption?addr=https://prod-vault.com");
    println!();
    println!("   Secret structure expected:");
    println!("   {{");
    println!("     \"key\": \"base64-encoded-key-material\"");
    println!("   }}");
    println!();

    // Example 8: Troubleshooting
    println!("8. Troubleshooting:");
    println!("   Common issues and solutions:");
    println!();
    println!("   • Permission denied:");
    println!("     - Check Vault token has correct policies");
    println!("     - Verify secret path exists and is accessible");
    println!("     - Ensure KV secrets engine is enabled");
    println!();
    println!("   • Connection refused:");
    println!("     - Verify VAULT_ADDR is correct");
    println!("     - Check Vault server is running and unsealed");
    println!("     - Verify network connectivity and firewall rules");
    println!();
    println!("   • Token expired:");
    println!("     - Renew or create new Vault token");
    println!("     - Consider using Vault agent for automatic renewal");
    println!("     - Check token TTL and max TTL settings");
    println!();
    println!("   • Secret not found:");
    println!("     - Verify secret path format matches KV engine version");
    println!("     - Check if secret exists: vault kv get secret/path");
    println!("     - Ensure proper mount path is specified");
    println!();

    println!("✅ HashiCorp Vault KMS integration example completed!");
    println!("\nNext steps:");
    println!("• Set up proper Vault policies and authentication");
    println!("• Create dedicated secret paths for different environments");
    println!("• Implement proper error handling and retry logic");
    println!("• Set up Vault monitoring and alerting");
    println!("• Configure secret rotation policies");
    println!("• Consider using Vault agent for production workloads");

    Ok(())
}
