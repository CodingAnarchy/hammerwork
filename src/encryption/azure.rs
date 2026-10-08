//! Azure Key Vault helpers shared by the encryption engine and key manager.
//!
//! Built on the 1.x Azure SDK (`azure_core`, `azure_identity`,
//! `azure_security_keyvault_keys`). The 1.x `azure_identity` crate no longer ships a
//! `DefaultAzureCredential`, so [`credential`] reproduces the useful part of that chain:
//!
//! 1. `ClientSecretCredential` when `AZURE_TENANT_ID`, `AZURE_CLIENT_ID` and
//!    `AZURE_CLIENT_SECRET` are all set.
//! 2. `WorkloadIdentityCredential` when `AZURE_FEDERATED_TOKEN_FILE` is set (AKS workload
//!    identity).
//! 3. Otherwise a chain that tries `ManagedIdentityCredential` and then
//!    `DeveloperToolsCredential` (Azure CLI / Azure Developer CLI) at token time.

use azure_core::credentials::{AccessToken, Secret, TokenCredential, TokenRequestOptions};
use azure_identity::{
    ClientSecretCredential, DeveloperToolsCredential, ManagedIdentityCredential,
    WorkloadIdentityCredential,
};
use azure_security_keyvault_keys::KeyClient;
use std::sync::Arc;

/// Credential chain tried in order at token-acquisition time.
#[derive(Debug)]
struct ChainedCredential {
    sources: Vec<Arc<dyn TokenCredential>>,
}

#[async_trait::async_trait]
impl TokenCredential for ChainedCredential {
    async fn get_token(
        &self,
        scopes: &[&str],
        options: Option<TokenRequestOptions<'_>>,
    ) -> azure_core::Result<AccessToken> {
        let mut errors = Vec::new();
        for source in &self.sources {
            match source.get_token(scopes, options.clone()).await {
                Ok(token) => return Ok(token),
                Err(e) => errors.push(e.to_string()),
            }
        }
        Err(azure_core::Error::with_message(
            azure_core::error::ErrorKind::Credential,
            format!(
                "no Azure credential could provide a token: {}",
                errors.join("; ")
            ),
        ))
    }
}

fn env_var(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

/// Build an Azure credential from the environment (see module docs for the order).
pub(crate) fn credential() -> azure_core::Result<Arc<dyn TokenCredential>> {
    if let (Some(tenant), Some(client_id), Some(secret)) = (
        env_var("AZURE_TENANT_ID"),
        env_var("AZURE_CLIENT_ID"),
        env_var("AZURE_CLIENT_SECRET"),
    ) {
        return Ok(ClientSecretCredential::new(
            &tenant,
            client_id,
            Secret::new(secret),
            None,
        )?);
    }

    if env_var("AZURE_FEDERATED_TOKEN_FILE").is_some() {
        return Ok(WorkloadIdentityCredential::new(None)?);
    }

    let managed: Arc<dyn TokenCredential> = ManagedIdentityCredential::new(None)?;
    let developer: Arc<dyn TokenCredential> = DeveloperToolsCredential::new(None)?;
    Ok(Arc::new(ChainedCredential {
        sources: vec![managed, developer],
    }))
}

/// Fetch the raw symmetric key material (`k`) of a key stored in Azure Key Vault.
///
/// The 1.x SDK already base64url-decodes the JSON Web Key `k` field, so the returned
/// bytes are the raw key material.
pub(crate) async fn fetch_key_material(vault_url: &str, key_name: &str) -> Result<Vec<u8>, String> {
    let credential =
        credential().map_err(|e| format!("Failed to create Azure credentials: {}", e))?;

    let client = KeyClient::new(vault_url, credential, None)
        .map_err(|e| format!("Failed to create Azure Key Vault client: {}", e))?;

    let key = client
        .get_key(key_name, None)
        .await
        .map_err(|e| format!("Failed to retrieve key from Azure Key Vault: {}", e))?
        .into_model()
        .map_err(|e| format!("Failed to parse Azure Key Vault response: {}", e))?;

    key.key
        .and_then(|jwk| jwk.k)
        .filter(|k| !k.is_empty())
        .ok_or_else(|| "Azure Key Vault key response missing key material".to_string())
}
