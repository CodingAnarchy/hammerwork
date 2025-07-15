//! Example demonstrating Google Cloud KMS integration for key management in Hammerwork.
//!
//! This example shows how to:
//! - Configure Google Cloud KMS for master key management
//! - Use GCP KMS for encryption key generation
//! - Set up proper Google Cloud credentials and projects
//! - Handle GCP KMS errors gracefully
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
    println!("🔐 Hammerwork Google Cloud KMS Integration Example");
    println!("==================================================\n");

    // Example 1: Google Cloud KMS Configuration
    println!("1. Configuring Google Cloud KMS integration:");
    println!("   You can configure GCP KMS in several ways:");
    println!("   • gcp://projects/PROJECT/locations/LOCATION/keyRings/RING/cryptoKeys/KEY");
    println!("   • Service account key file: Set GOOGLE_APPLICATION_CREDENTIALS");
    println!("   • Default credentials: gcloud auth application-default login");
    println!("   • Workload Identity: For GKE clusters");
    println!();

    // Example 2: Basic GCP KMS encryption setup
    if cfg!(feature = "gcp-kms") {
        println!("2. Setting up encryption with Google Cloud KMS:");

        // Configure encryption to use GCP KMS
        let gcp_kms_config = EncryptionConfig::new(EncryptionAlgorithm::AES256GCM)
            .with_key_source(KeySource::External(
                "gcp://projects/my-project/locations/us-central1/keyRings/hammerwork/cryptoKeys/encryption-key".to_string()
            ))
            .with_key_id("gcp-kms-data-key")
            .with_compression_enabled(true);

        match EncryptionEngine::new(gcp_kms_config).await {
            Ok(mut engine) => {
                println!("   ✅ GCP KMS encryption engine created successfully");

                // Test encryption with GCP KMS
                let sensitive_data = json!({
                    "customer_id": "cust_54321",
                    "credit_card_number": "5555-5555-5555-4444",
                    "ssn": "987-65-4321",
                    "email": "jane.doe@example.com",
                    "transaction_amount": 1299.99
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
                    "   ⚠️  GCP KMS not available (falling back to deterministic): {}",
                    e
                );
                println!("   This is normal in development without proper GCP credentials");
            }
        }
    } else {
        println!("2. GCP KMS feature not enabled");
        println!(
            "   Compile with: cargo run --example gcp_kms_encryption_example --features gcp-kms"
        );
    }

    println!();

    // Example 3: Key Manager with GCP KMS
    if cfg!(all(feature = "gcp-kms", feature = "postgres")) {
        println!("3. Advanced key management with Google Cloud KMS:");

        // Note: This requires a PostgreSQL database connection
        // In a real application, you would provide a proper database pool
        println!("   Key manager configuration with GCP KMS:");

        let kms_config = KeyManagerConfig::new()
            .with_master_key_source(KeySource::External(
                "gcp://projects/my-project/locations/us-central1/keyRings/hammerwork/cryptoKeys/master-key".to_string()
            ))
            .with_auto_rotation_enabled(true)
            .with_rotation_interval(chrono::Duration::days(30))
            .with_audit_enabled(true);

        println!(
            "   Master key source: GCP KMS projects/my-project/locations/us-central1/keyRings/hammerwork/cryptoKeys/master-key"
        );
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
        println!("3. Advanced key management requires both gcp-kms and postgres features");
    }

    println!();

    // Example 4: Job encryption with GCP KMS key source
    println!("4. Creating jobs with GCP KMS encryption:");

    let payment_job = Job::new(
        "process_payment".to_string(),
        json!({
            "order_id": "order_987",
            "customer_email": "john.smith@example.com",
            "credit_card": "4111-1111-1111-1111",
            "amount": 449.99,
            "currency": "USD"
        }),
    );

    // Configure with GCP KMS (will fall back if not available)
    let gcp_job = payment_job
        .with_encryption(
            EncryptionConfig::new(EncryptionAlgorithm::AES256GCM)
                .with_key_source(KeySource::External(
                    "gcp://projects/my-project/locations/europe-west1/keyRings/hammerwork/cryptoKeys/data-keys".to_string()
                ))
        )
        .with_pii_fields(vec!["customer_email", "credit_card"])
        .with_retention_policy(RetentionPolicy::DeleteAfter(Duration::from_secs(
            180 * 24 * 60 * 60, // 180 days
        )));

    println!("   Job ID: {}", gcp_job.id);
    println!("   Encryption enabled: {}", gcp_job.has_encryption());
    println!("   PII fields: {:?}", gcp_job.get_pii_fields());
    println!("   Retention: 180 days");
    println!();

    // Example 5: Google Cloud KMS best practices
    println!("5. Google Cloud KMS Best Practices:");
    println!("   Security:");
    println!("   • Use IAM policies to control key access");
    println!("   • Enable Cloud Audit Logs for key operations");
    println!("   • Use different key rings for different environments");
    println!("   • Implement key rotation policies");
    println!("   • Use Hardware Security Module (HSM) keys for sensitive workloads");
    println!();

    println!("   Performance:");
    println!("   • Cache decrypted data keys to reduce API calls");
    println!("   • Use regional keys for better latency");
    println!("   • Consider Cloud KMS quotas and limits");
    println!("   • Monitor key usage with Cloud Monitoring");
    println!();

    println!("   Cost optimization:");
    println!("   • Use symmetric keys for data encryption");
    println!("   • Implement data key caching to reduce operations");
    println!("   • Monitor per-key usage and costs");
    println!("   • Use key rotation to balance security and cost");
    println!();

    // Example 6: Environment setup instructions
    println!("6. Setup Instructions:");
    println!("   Prerequisites:");
    println!("   1. Google Cloud CLI installed and authenticated:");
    println!("      gcloud auth login");
    println!("      gcloud auth application-default login");
    println!();
    println!("   2. Create KMS resources in Google Cloud:");
    println!("      gcloud kms keyrings create hammerwork --location=us-central1");
    println!(
        "      gcloud kms keys create master-key --keyring=hammerwork --location=us-central1 --purpose=encryption"
    );
    println!(
        "      gcloud kms keys create data-keys --keyring=hammerwork --location=us-central1 --purpose=encryption"
    );
    println!();
    println!("   3. Set IAM permissions for the application:");
    println!("      - cloudkms.cryptoKeys.encrypt");
    println!("      - cloudkms.cryptoKeys.decrypt");
    println!("      - cloudkms.cryptoKeys.get");
    println!("      - cloudkms.cryptoKeys.list");
    println!();
    println!("   4. Set environment variables (optional):");
    println!("      export GOOGLE_APPLICATION_CREDENTIALS=/path/to/service-account-key.json");
    println!("      export GOOGLE_CLOUD_PROJECT=my-project");
    println!();
    println!("   5. Service account key file example:");
    println!("      gcloud iam service-accounts create hammerwork-kms");
    println!("      gcloud projects add-iam-policy-binding PROJECT_ID \\");
    println!(
        "        --member=\"serviceAccount:hammerwork-kms@PROJECT_ID.iam.gserviceaccount.com\" \\"
    );
    println!("        --role=\"roles/cloudkms.cryptoKeyEncrypterDecrypter\"");
    println!("      gcloud iam service-accounts keys create ~/hammerwork-kms.json \\");
    println!("        --iam-account=hammerwork-kms@PROJECT_ID.iam.gserviceaccount.com");
    println!();

    // Example 7: Resource path formats
    println!("7. Resource Path Formats:");
    println!("   Full resource path:");
    println!(
        "   gcp://projects/my-project/locations/us-central1/keyRings/hammerwork/cryptoKeys/encryption-key"
    );
    println!();
    println!("   Components:");
    println!("   • Project: my-project");
    println!("   • Location: us-central1 (or global)");
    println!("   • Key ring: hammerwork");
    println!("   • Key name: encryption-key");
    println!();
    println!("   Supported locations:");
    println!("   • Regional: us-central1, europe-west1, asia-east1");
    println!("   • Multi-regional: us, europe, asia");
    println!("   • Global: global (not recommended for regulatory compliance)");
    println!();

    println!("✅ Google Cloud KMS integration example completed!");
    println!("\nNext steps:");
    println!("• Set up proper Google Cloud IAM roles and policies");
    println!("• Create dedicated key rings for different environments");
    println!("• Implement proper error handling and retry logic");
    println!("• Set up Cloud Monitoring for KMS operations");
    println!("• Configure data key caching for production workloads");
    println!("• Consider using HSM keys for sensitive data");

    Ok(())
}
