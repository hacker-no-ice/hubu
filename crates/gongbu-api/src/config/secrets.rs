//! Operator-owned secret references and the local macOS Keychain backend.
use std::process::Command;
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
}
pub type Result<T> = std::result::Result<T, SecretError>;

/// Secret bytes deliberately have no `Debug`, `Display`, serde, or clone implementation.
pub struct ProviderSecret(Vec<u8>);
impl ProviderSecret {
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
        let mut output = Command::new("/usr/bin/security")
            .args([
                "find-generic-password",
                "-s",
                reference.service(),
                "-a",
                reference.account(),
                "-w",
            ])
            .output()
            .map_err(|_| SecretError::Unavailable)?;
        if !output.status.success() {
            return Err(SecretError::Unavailable);
        }
        while output
            .stdout
            .last()
            .is_some_and(|byte| matches!(byte, b'\n' | b'\r'))
        {
            output.stdout.pop();
        }
        ProviderSecret::new(output.stdout)
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
        let target = crate::provider_targets::ProviderConfigVersion {
            provider_config_version: "v1".into(),
            workload_type: "image_generation".into(),
            provider: "vendor".into(),
            adapter: "adapter".into(),
            model: "model".into(),
            secret_service: "gongbu.vendor".into(),
            secret_account: "production".into(),
            enabled: true,
        };
        let secret = resolve_selected(&provider, &target).unwrap();
        assert_eq!(secret.expose(), b"canary");
        assert_eq!(
            provider.0.lock().unwrap().as_slice(),
            &[SecretReference::new("gongbu.vendor", "production").unwrap()]
        );
    }
}
