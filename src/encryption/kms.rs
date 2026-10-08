//! Helpers for loading keys from external key management services, shared by the
//! encryption engine and the key manager.
//!
//! Every helper fails closed: a KMS that is unreachable, misconfigured or not compiled in
//! (its cargo feature is disabled) is an error. There is no fallback key.

use super::EncryptionError;
use std::collections::HashMap;

/// Parse a `scheme://resource?key=value&...` key source configuration into the resource
/// and its query parameters.
pub(crate) fn parse_service_config<'a>(
    service_config: &'a str,
    scheme: &str,
) -> (&'a str, HashMap<&'a str, &'a str>) {
    let rest = service_config
        .strip_prefix(scheme)
        .unwrap_or(service_config);
    let (resource, query) = rest.split_once('?').unwrap_or((rest, ""));
    let params = query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| pair.split_once('=').unwrap_or((pair, "")))
        .collect();
    (resource, params)
}

/// Error for a KMS key source whose cargo feature is not enabled.
#[cfg_attr(
    all(
        feature = "aws-kms",
        feature = "gcp-kms",
        feature = "vault-kms",
        feature = "azure-kv"
    ),
    allow(dead_code)
)]
pub(crate) fn kms_feature_disabled(scheme: &str, feature: &str) -> EncryptionError {
    EncryptionError::InvalidConfiguration(format!(
        "{} key sources require the `{}` cargo feature, which is not enabled",
        scheme, feature
    ))
}

/// Parse an `aws://<key-id>?region=<region>&endpoint=<url>` source into
/// `(key_id, region, endpoint)`. The region defaults to `us-east-1`.
pub(crate) fn aws_key_config(
    service_config: &str,
) -> Result<(&str, &str, Option<&str>), EncryptionError> {
    let (key_id, params) = parse_service_config(service_config, "aws://");
    if key_id.is_empty() {
        return Err(EncryptionError::InvalidConfiguration(
            "AWS KMS key source needs a key id: aws://<key-id>?region=<region>".to_string(),
        ));
    }
    let region = params
        .get("region")
        .copied()
        .filter(|r| !r.is_empty())
        .unwrap_or("us-east-1");
    let endpoint = params.get("endpoint").copied().filter(|e| !e.is_empty());
    Ok((key_id, region, endpoint))
}

/// Vault address from the `addr` parameter, falling back to `VAULT_ADDR`.
pub(crate) fn vault_address(params: &HashMap<&str, &str>) -> Result<String, EncryptionError> {
    params
        .get("addr")
        .map(|addr| addr.to_string())
        .or_else(|| std::env::var("VAULT_ADDR").ok())
        .filter(|addr| !addr.is_empty())
        .ok_or_else(|| {
            EncryptionError::InvalidConfiguration(
                "Vault key source needs an address: add ?addr=<url> or set VAULT_ADDR".to_string(),
            )
        })
}

/// Split a Vault secret path `<mount>/<path>` into its mount and path.
pub(crate) fn vault_mount_and_path(secret_path: &str) -> Result<(&str, &str), EncryptionError> {
    match secret_path.split_once('/') {
        Some((mount, path)) if !mount.is_empty() && !path.is_empty() => Ok((mount, path)),
        _ => Err(EncryptionError::InvalidConfiguration(format!(
            "Invalid Vault secret path {:?}: expected vault://<mount>/<path>",
            secret_path
        ))),
    }
}

/// Read the string `key` field of a KV v2 secret, authenticating with `VAULT_TOKEN`.
#[cfg(feature = "vault-kms")]
pub(crate) async fn vault_read_key_field(
    vault_addr: &str,
    mount: &str,
    path: &str,
) -> Result<String, EncryptionError> {
    use vaultrs::{
        client::{VaultClient, VaultClientSettingsBuilder},
        kv2,
    };

    let token = std::env::var("VAULT_TOKEN")
        .ok()
        .filter(|token| !token.is_empty())
        .ok_or_else(|| {
            EncryptionError::KeyManagement(
                "VAULT_TOKEN is not set; cannot read the key from HashiCorp Vault".to_string(),
            )
        })?;

    let settings = VaultClientSettingsBuilder::default()
        .address(vault_addr)
        .token(token)
        .build()
        .map_err(|e| {
            EncryptionError::InvalidConfiguration(format!("Invalid Vault client settings: {}", e))
        })?;
    let client = VaultClient::new(settings).map_err(|e| {
        EncryptionError::KeyManagement(format!("Failed to create Vault client: {}", e))
    })?;

    let secret: serde_json::Value = kv2::read(&client, mount, path).await.map_err(|e| {
        EncryptionError::KeyManagement(format!(
            "Failed to read secret {}/{} from HashiCorp Vault: {}",
            mount, path, e
        ))
    })?;

    secret
        .get("key")
        .and_then(|key| key.as_str())
        .map(str::to_string)
        .ok_or_else(|| {
            EncryptionError::KeyManagement(format!(
                "Vault secret {}/{} has no string `key` field",
                mount, path
            ))
        })
}

/// `projects/<project>/locations/<location>` of a GCP KMS key resource name.
pub(crate) fn gcp_location(key_resource: &str) -> Result<String, EncryptionError> {
    let parts: Vec<&str> = key_resource.split('/').collect();
    match parts.as_slice() {
        ["projects", project, "locations", location, ..]
            if !project.is_empty() && !location.is_empty() =>
        {
            Ok(format!("projects/{}/locations/{}", project, location))
        }
        _ => Err(EncryptionError::InvalidConfiguration(format!(
            "Invalid GCP KMS resource {:?}: expected \
             gcp://projects/<project>/locations/<location>/keyRings/<ring>/cryptoKeys/<key>",
            key_resource
        ))),
    }
}

/// Parse `azure://<vault-host>/keys/<key-name>` into the vault URL and key name.
/// The key name defaults to `default_key_name` when the path is omitted.
pub(crate) fn azure_vault_and_key(
    service_config: &str,
    default_key_name: &str,
) -> Result<(String, String), EncryptionError> {
    let (resource, _) = parse_service_config(service_config, "azure://");
    let parts: Vec<&str> = resource.split('/').collect();
    let host = parts.first().copied().unwrap_or_default();
    if host.is_empty() {
        return Err(EncryptionError::InvalidConfiguration(format!(
            "Invalid Azure Key Vault source {:?}: expected azure://<vault-host>/keys/<key-name>",
            service_config
        )));
    }
    let key_name = parts
        .get(2)
        .copied()
        .filter(|name| !name.is_empty())
        .unwrap_or(default_key_name);
    Ok((format!("https://{}", host), key_name.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_service_config() {
        let (resource, params) = parse_service_config(
            "aws://alias/key?region=eu-west-1&endpoint=http://x",
            "aws://",
        );
        assert_eq!(resource, "alias/key");
        assert_eq!(params.get("region"), Some(&"eu-west-1"));
        assert_eq!(params.get("endpoint"), Some(&"http://x"));

        let (resource, params) = parse_service_config("aws://alias/key", "aws://");
        assert_eq!(resource, "alias/key");
        assert!(params.is_empty());
    }

    #[test]
    fn test_aws_key_config() {
        let (key_id, region, endpoint) = aws_key_config("aws://alias/key").unwrap();
        assert_eq!((key_id, region, endpoint), ("alias/key", "us-east-1", None));
        assert!(matches!(
            aws_key_config("aws://?region=us-east-1"),
            Err(EncryptionError::InvalidConfiguration(_))
        ));
    }

    #[test]
    fn test_vault_path_and_address_validation() {
        assert_eq!(
            vault_mount_and_path("secret/hammerwork/key").unwrap(),
            ("secret", "hammerwork/key")
        );
        assert!(vault_mount_and_path("secret").is_err());
        assert!(vault_mount_and_path("/key").is_err());

        let params: HashMap<&str, &str> = [("addr", "http://127.0.0.1:1")].into_iter().collect();
        assert_eq!(vault_address(&params).unwrap(), "http://127.0.0.1:1");
    }

    #[test]
    fn test_gcp_location_validation() {
        assert_eq!(
            gcp_location("projects/p/locations/us/keyRings/r/cryptoKeys/k").unwrap(),
            "projects/p/locations/us"
        );
        assert!(gcp_location("my-key").is_err());
        assert!(gcp_location("projects//locations/us").is_err());
    }

    #[test]
    fn test_azure_vault_and_key() {
        assert_eq!(
            azure_vault_and_key("azure://v.vault.azure.net/keys/k1", "default").unwrap(),
            ("https://v.vault.azure.net".to_string(), "k1".to_string())
        );
        assert_eq!(
            azure_vault_and_key("azure://v.vault.azure.net", "default")
                .unwrap()
                .1,
            "default"
        );
        assert!(azure_vault_and_key("azure://", "default").is_err());
    }
}
