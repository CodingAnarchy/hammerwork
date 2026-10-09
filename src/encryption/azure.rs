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
    std::env::var(name).ok()
}

/// Which credential [`credential`] builds (see the module docs for the order).
#[derive(PartialEq, Eq)]
enum CredentialChoice {
    ClientSecret {
        tenant: String,
        client_id: String,
        secret: String,
    },
    WorkloadIdentity,
    ManagedIdentityThenDeveloperTools,
}

impl std::fmt::Debug for CredentialChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            // Never print the client secret
            Self::ClientSecret {
                tenant, client_id, ..
            } => f
                .debug_struct("ClientSecret")
                .field("tenant", tenant)
                .field("client_id", client_id)
                .field("secret", &"[REDACTED]")
                .finish(),
            Self::WorkloadIdentity => f.write_str("WorkloadIdentity"),
            Self::ManagedIdentityThenDeveloperTools => {
                f.write_str("ManagedIdentityThenDeveloperTools")
            }
        }
    }
}

/// Pick a credential from the variables returned by `var`. Empty values count as
/// unset.
fn choose_credential(var: impl Fn(&str) -> Option<String>) -> CredentialChoice {
    let var = |name: &str| var(name).filter(|v| !v.is_empty());
    if let (Some(tenant), Some(client_id), Some(secret)) = (
        var("AZURE_TENANT_ID"),
        var("AZURE_CLIENT_ID"),
        var("AZURE_CLIENT_SECRET"),
    ) {
        return CredentialChoice::ClientSecret {
            tenant,
            client_id,
            secret,
        };
    }
    if var("AZURE_FEDERATED_TOKEN_FILE").is_some() {
        return CredentialChoice::WorkloadIdentity;
    }
    CredentialChoice::ManagedIdentityThenDeveloperTools
}

/// Build the credential described by `choice`.
fn build_credential(choice: CredentialChoice) -> azure_core::Result<Arc<dyn TokenCredential>> {
    match choice {
        CredentialChoice::ClientSecret {
            tenant,
            client_id,
            secret,
        } => Ok(ClientSecretCredential::new(
            &tenant,
            client_id,
            Secret::new(secret),
            None,
        )?),
        CredentialChoice::WorkloadIdentity => Ok(WorkloadIdentityCredential::new(None)?),
        CredentialChoice::ManagedIdentityThenDeveloperTools => {
            let managed: Arc<dyn TokenCredential> = ManagedIdentityCredential::new(None)?;
            let developer: Arc<dyn TokenCredential> = DeveloperToolsCredential::new(None)?;
            Ok(Arc::new(ChainedCredential {
                sources: vec![managed, developer],
            }))
        }
    }
}

/// Build an Azure credential from the environment (see module docs for the order).
pub(crate) fn credential() -> azure_core::Result<Arc<dyn TokenCredential>> {
    build_credential(choose_credential(env_var))
}

/// Fetch the raw symmetric key material (`k`) of a key stored in Azure Key Vault.
///
/// The 1.x SDK already base64url-decodes the JSON Web Key `k` field, so the returned
/// bytes are the raw key material.
pub(crate) async fn fetch_key_material(
    vault_url: &str,
    key_name: &str,
) -> Result<super::SecretBytes, String> {
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
        .map(super::SecretBytes::new)
        .ok_or_else(|| "Azure Key Vault key response missing key material".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn vars(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |name| map.get(name).cloned()
    }

    #[test]
    fn client_secret_is_chosen_when_all_three_variables_are_set() {
        let choice = choose_credential(vars(&[
            ("AZURE_TENANT_ID", "tenant"),
            ("AZURE_CLIENT_ID", "client"),
            ("AZURE_CLIENT_SECRET", "secret"),
            ("AZURE_FEDERATED_TOKEN_FILE", "/var/run/token"),
        ]));
        assert_eq!(
            choice,
            CredentialChoice::ClientSecret {
                tenant: "tenant".to_string(),
                client_id: "client".to_string(),
                secret: "secret".to_string(),
            }
        );
    }

    #[test]
    fn debug_does_not_print_the_client_secret() {
        let choice = CredentialChoice::ClientSecret {
            tenant: "tenant".to_string(),
            client_id: "client".to_string(),
            secret: "hunter2-client-secret".to_string(),
        };
        let debug = format!("{:?}", choice);
        assert!(!debug.contains("hunter2-client-secret"), "{debug}");
        assert!(
            debug.contains("client") && debug.contains("[REDACTED]"),
            "{debug}"
        );
        assert_eq!(
            format!("{:?}", CredentialChoice::WorkloadIdentity),
            "WorkloadIdentity"
        );
        assert_eq!(
            format!("{:?}", CredentialChoice::ManagedIdentityThenDeveloperTools),
            "ManagedIdentityThenDeveloperTools"
        );
    }

    #[test]
    fn workload_identity_needs_a_federated_token_file() {
        // An incomplete (or empty) client secret configuration is not used.
        let choice = choose_credential(vars(&[
            ("AZURE_TENANT_ID", "tenant"),
            ("AZURE_CLIENT_ID", "client"),
            ("AZURE_CLIENT_SECRET", ""),
            ("AZURE_FEDERATED_TOKEN_FILE", "/var/run/token"),
        ]));
        assert_eq!(choice, CredentialChoice::WorkloadIdentity);

        let choice = choose_credential(vars(&[("AZURE_FEDERATED_TOKEN_FILE", "")]));
        assert_eq!(choice, CredentialChoice::ManagedIdentityThenDeveloperTools);
        assert_eq!(
            choose_credential(vars(&[])),
            CredentialChoice::ManagedIdentityThenDeveloperTools
        );
    }

    #[test]
    fn every_choice_builds_without_contacting_azure() {
        let client_secret = build_credential(CredentialChoice::ClientSecret {
            tenant: "00000000-0000-0000-0000-000000000000".to_string(),
            client_id: "client".to_string(),
            secret: "secret".to_string(),
        });
        assert!(client_secret.is_ok(), "{:?}", client_secret.err());
        assert!(build_credential(CredentialChoice::ManagedIdentityThenDeveloperTools).is_ok());
        // Built from the real environment (managed identity / developer tools in CI).
        assert!(credential().is_ok());
    }

    #[test]
    fn invalid_tenant_id_is_an_error() {
        let result = build_credential(CredentialChoice::ClientSecret {
            tenant: "not a tenant/..".to_string(),
            client_id: "client".to_string(),
            secret: "secret".to_string(),
        });
        assert!(result.is_err());
    }

    /// A credential that returns a fixed token or error.
    #[derive(Debug)]
    struct FakeCredential(std::result::Result<&'static str, &'static str>);

    #[async_trait::async_trait]
    impl TokenCredential for FakeCredential {
        async fn get_token(
            &self,
            _scopes: &[&str],
            _options: Option<TokenRequestOptions<'_>>,
        ) -> azure_core::Result<AccessToken> {
            match self.0 {
                Ok(token) => Ok(AccessToken::new(
                    token.to_string(),
                    azure_core::time::OffsetDateTime::now_utc(),
                )),
                Err(message) => Err(azure_core::Error::with_message(
                    azure_core::error::ErrorKind::Credential,
                    message.to_string(),
                )),
            }
        }
    }

    fn chain(sources: Vec<FakeCredential>) -> ChainedCredential {
        ChainedCredential {
            sources: sources
                .into_iter()
                .map(|s| Arc::new(s) as Arc<dyn TokenCredential>)
                .collect(),
        }
    }

    #[tokio::test]
    async fn chained_credential_uses_the_first_source_that_works() {
        let credential = chain(vec![
            FakeCredential(Err("no managed identity")),
            FakeCredential(Ok("from-cli")),
            FakeCredential(Ok("never-asked")),
        ]);
        let token = credential.get_token(&["scope"], None).await.unwrap();
        assert_eq!(token.token.secret(), "from-cli");
    }

    #[tokio::test]
    async fn chained_credential_reports_every_failure() {
        let credential = chain(vec![
            FakeCredential(Err("no managed identity")),
            FakeCredential(Err("az not logged in")),
        ]);
        let err = credential.get_token(&["scope"], None).await.unwrap_err();
        let message = err.to_string();
        assert!(message.contains("no Azure credential could provide a token"));
        assert!(message.contains("no managed identity"), "{message}");
        assert!(message.contains("az not logged in"), "{message}");
        assert_eq!(*err.kind(), azure_core::error::ErrorKind::Credential);
    }

    #[tokio::test]
    async fn fetch_key_material_rejects_an_invalid_vault_url() {
        let err = fetch_key_material("not a url", "key").await.unwrap_err();
        assert!(
            err.contains("Failed to create Azure Key Vault client"),
            "{err}"
        );
    }
}
