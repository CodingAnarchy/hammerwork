//! Example demonstrating AWS KMS integration for key management in Hammerwork.
//!
//! This example shows how to:
//! - Configure AWS KMS for master key management
//! - Use AWS KMS for encryption key generation
//! - Set up proper AWS credentials and regions
//! - Handle AWS KMS errors gracefully
//! - Fall back to deterministic keys for development

use hammerwork::{
    Job,
    encryption::{
        EncryptionAlgorithm, EncryptionConfig, EncryptionEngine, KeyManager, KeyManagerConfig,
        KeySource, RetentionPolicy,
    },
};
use serde_json::json;
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("🔐 Hammerwork AWS KMS Integration Example");
    println!("==========================================\n");

    // Example 1: AWS KMS Configuration
    println!("1. Configuring AWS KMS integration:");
    println!("   You can configure AWS KMS in several ways:");
    println!("   • aws://your-key-id?region=us-east-1");
    println!(
        "   • aws://arn:aws:kms:us-east-1:123456789012:key/12345678-1234-1234-1234-123456789012"
    );
    println!("   • Environment variables: AWS_ACCESS_KEY_ID, AWS_SECRET_ACCESS_KEY, AWS_REGION\n");

    // Example 2: Basic AWS KMS encryption setup
    if cfg!(feature = "aws-kms") {
        println!("2. Setting up encryption with AWS KMS:");

        // Configure encryption to use AWS KMS
        let aws_kms_config = EncryptionConfig::new(EncryptionAlgorithm::AES256GCM)
            .with_key_source(KeySource::External(
                "aws://alias/hammerwork-encryption?region=us-east-1".to_string(),
            ))
            .with_key_id("aws-kms-data-key")
            .with_compression_enabled(true);

        match EncryptionEngine::new(aws_kms_config).await {
            Ok(mut engine) => {
                println!("   ✅ AWS KMS encryption engine created successfully");

                // Test encryption with AWS KMS
                let sensitive_data = json!({
                    "customer_id": "cust_12345",
                    "credit_card_number": "4111-1111-1111-1111",
                    "ssn": "123-45-6789",
                    "email": "customer@example.com",
                    "transaction_amount": 999.99
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
                println!(
                    "   ⚠️  AWS KMS not available (falling back to deterministic): {}",
                    e
                );
                println!("   This is normal in development without proper AWS credentials");
            }
        }
    } else {
        println!("2. AWS KMS feature not enabled");
        println!(
            "   Compile with: cargo run --example aws_kms_encryption_example --features aws-kms"
        );
    }

    println!();

    // Example 3: Key Manager with AWS KMS
    if cfg!(all(feature = "aws-kms", feature = "postgres")) {
        println!("3. Advanced key management with AWS KMS:");

        // Note: This requires a PostgreSQL database connection
        // In a real application, you would provide a proper database pool
        println!("   Key manager configuration with AWS KMS:");

        let kms_config = KeyManagerConfig::new()
            .with_master_key_source(KeySource::External(
                "aws://alias/hammerwork-master-key?region=us-east-1".to_string(),
            ))
            .with_auto_rotation_enabled(true)
            .with_rotation_interval(chrono::Duration::days(30))
            .with_audit_enabled(true);

        println!("   Master key source: AWS KMS alias/hammerwork-master-key");
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
        println!("3. Advanced key management requires both aws-kms and postgres features");
    }

    println!();

    // Example 4: Job encryption with AWS KMS key source
    println!("4. Creating jobs with AWS KMS encryption:");

    let payment_job = Job::new(
        "process_payment".to_string(),
        json!({
            "order_id": "order_789",
            "customer_email": "jane.doe@example.com",
            "credit_card": "5555-5555-5555-4444",
            "amount": 249.99,
            "currency": "USD"
        }),
    );

    // Configure with AWS KMS (will fall back if not available)
    let aws_job = payment_job
        .with_encryption(
            EncryptionConfig::new(EncryptionAlgorithm::AES256GCM).with_key_source(
                KeySource::External(
                    "aws://alias/hammerwork-data-keys?region=us-west-2".to_string(),
                ),
            ),
        )
        .with_pii_fields(vec!["customer_email", "credit_card"])
        .with_retention_policy(RetentionPolicy::DeleteAfter(Duration::from_secs(
            90 * 24 * 60 * 60, // 90 days
        )));

    println!("   Job ID: {}", aws_job.id);
    println!("   Encryption enabled: {}", aws_job.has_encryption());
    println!("   PII fields: {:?}", aws_job.get_pii_fields());
    println!("   Retention: 90 days");
    println!();

    // Example 5: AWS KMS best practices
    println!("5. AWS KMS Best Practices:");
    println!("   Security:");
    println!("   • Use IAM policies to restrict key access");
    println!("   • Enable CloudTrail for key usage auditing");
    println!("   • Use different keys for different environments");
    println!("   • Rotate keys regularly (automatic with key aliases)");
    println!();

    println!("   Performance:");
    println!("   • Cache decrypted data keys to reduce KMS calls");
    println!("   • Use data key caching for high-volume operations");
    println!("   • Consider regional proximity for lower latency");
    println!("   • Monitor KMS usage and costs");
    println!();

    println!("   Cost optimization:");
    println!("   • Use aliases instead of key IDs for flexibility");
    println!("   • Implement data key caching to reduce API calls");
    println!("   • Monitor per-key usage patterns");
    println!("   • Consider key rotation frequency vs. cost");
    println!();

    // Example 6: Environment setup instructions
    println!("6. Setup Instructions:");
    println!("   Prerequisites:");
    println!("   1. AWS CLI configured with appropriate credentials:");
    println!("      aws configure");
    println!();
    println!("   2. Create KMS keys in AWS Console or CLI:");
    println!("      aws kms create-key --description 'Hammerwork master key'");
    println!(
        "      aws kms create-alias --alias-name alias/hammerwork-master-key --target-key-id <key-id>"
    );
    println!();
    println!("   3. Set IAM permissions for the application:");
    println!("      - kms:GenerateDataKey");
    println!("      - kms:Decrypt");
    println!("      - kms:DescribeKey");
    println!("      - kms:ListAliases");
    println!();
    println!("   4. Set environment variables (optional):");
    println!("      export AWS_REGION=us-east-1");
    println!("      export AWS_PROFILE=hammerwork");
    println!();

    println!("✅ AWS KMS integration example completed!");
    println!("\nNext steps:");
    println!("• Set up proper AWS IAM roles and policies");
    println!("• Create dedicated KMS keys for different environments");
    println!("• Implement proper error handling and retry logic");
    println!("• Set up CloudWatch monitoring for KMS operations");
    println!("• Configure data key caching for production workloads");

    Ok(())
}
