//! Operator-owned secret references and the local macOS Keychain backend.
use thiserror::Error;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SecretReference {
    service: String,
    account: String,
}

impl SecretReference {
    pub fn new(service: impl Into<String>, account: impl Into<String>) -> Result<Self> {
        let reference = Self {
            service: service.into(),
            account: account.into(),
        };
        if [&reference.service, &reference.account]
            .iter()
            .any(|value| value.trim().is_empty() || value.len() > 255 || value.contains('\0'))
        {
            return Err(SecretError::InvalidReference);
        }
        Ok(reference)
    }

    pub fn service(&self) -> &str {
        &self.service
    }
    pub fn account(&self) -> &str {
        &self.account
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum SecretError {
    #[error("invalid operator secret reference")]
    InvalidReference,
    #[error("operator secret unavailable")]
    Unavailable,
    #[error("local Keychain credential storage is supported only on macOS")]
    Unsupported,
}
pub type Result<T> = std::result::Result<T, SecretError>;

/// Secret bytes deliberately have no `Debug`, `Display`, serde, or clone implementation.
pub struct ProviderSecret(Vec<u8>);
impl ProviderSecret {
    #[cfg(any(target_os = "macos", test))]
    fn new(bytes: Vec<u8>) -> Result<Self> {
        if bytes.is_empty() {
            Err(SecretError::Unavailable)
        } else {
            Ok(Self(bytes))
        }
    }
    pub fn expose(&self) -> &[u8] {
        &self.0
    }
}
impl Drop for ProviderSecret {
    fn drop(&mut self) {
        self.0.fill(0);
    }
}

pub trait SecretProvider: Send + Sync {
    fn resolve(&self, reference: &SecretReference) -> Result<ProviderSecret>;
}

pub trait SecretStore: SecretProvider {
    fn persist(&self, reference: &SecretReference, value: &[u8]) -> Result<()>;
    fn delete(&self, reference: &SecretReference) -> Result<()>;
}

/// Resolve exactly the reference attached to the already-selected operator target.
pub fn resolve_selected(
    provider: &dyn SecretProvider,
    target: &crate::provider_targets::ProviderConfigVersion,
) -> Result<ProviderSecret> {
    let reference = target.secret_reference()?;
    provider.resolve(&reference)
}

#[derive(Clone, Copy, Debug, Default)]
pub struct MacOsKeychain;
impl SecretProvider for MacOsKeychain {
    fn resolve(&self, reference: &SecretReference) -> Result<ProviderSecret> {
        #[cfg(target_os = "macos")]
        {
            let bytes = security_framework::passwords::get_generic_password(
                reference.service(),
                reference.account(),
            )
            .map_err(|_| SecretError::Unavailable)?;
            ProviderSecret::new(bytes)
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = reference;
            Err(SecretError::Unsupported)
        }
    }
}

impl SecretStore for MacOsKeychain {
    fn persist(&self, reference: &SecretReference, value: &[u8]) -> Result<()> {
        if value.is_empty() {
            return Err(SecretError::Unavailable);
        }
        #[cfg(target_os = "macos")]
        {
            security_framework::passwords::set_generic_password(
                reference.service(),
                reference.account(),
                value,
            )
            .map_err(|_| SecretError::Unavailable)
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (reference, value);
            Err(SecretError::Unsupported)
        }
    }

    fn delete(&self, reference: &SecretReference) -> Result<()> {
        #[cfg(target_os = "macos")]
        {
            security_framework::passwords::delete_generic_password(
                reference.service(),
                reference.account(),
            )
            .map_err(|_| SecretError::Unavailable)
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = reference;
            Err(SecretError::Unsupported)
        }
    }
}

#[cfg(test)]
pub(crate) fn secret_for_test(value: &str) -> ProviderSecret {
    ProviderSecret::new(value.as_bytes().to_vec()).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reference_is_strict_and_errors_are_stable() {
        assert_eq!(
            SecretReference::new("", "account"),
            Err(SecretError::InvalidReference)
        );
        assert_eq!(
            SecretError::Unavailable.to_string(),
            "operator secret unavailable"
        );
        assert!(!SecretError::Unavailable.to_string().contains("security"));
    }

    struct RecordingProvider(std::sync::Mutex<Vec<SecretReference>>);
    impl SecretProvider for RecordingProvider {
        fn resolve(&self, reference: &SecretReference) -> Result<ProviderSecret> {
            self.0.lock().unwrap().push(reference.clone());
            ProviderSecret::new(b"canary".to_vec())
        }
    }

    #[test]
    fn resolves_only_the_selected_operator_reference() {
        let provider = RecordingProvider(Default::default());
        let catalog: crate::provider_targets::ProviderTargetConfig = serde_json::from_str(
            r#"{"provider_configs":[{"provider_config_version":"v1","workload_type":"image_generation","provider":"vendor","adapter":"adapter","model":"model","secret_service":"gongbu.vendor","secret_account":"production"}]}"#,
        ).unwrap();
        let target = catalog
            .resolve("image_generation", "vendor", "adapter", "model")
            .unwrap();
        let secret = resolve_selected(&provider, target).unwrap();
        assert_eq!(secret.expose(), b"canary");
        assert_eq!(
            provider.0.lock().unwrap().as_slice(),
            &[SecretReference::new("gongbu.vendor", "production").unwrap()]
        );
    }
}
