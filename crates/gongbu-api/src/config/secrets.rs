//! Operator-owned secret references and credential backends.
use std::{
    fs::OpenOptions,
    io::Read,
    path::{Component, Path, PathBuf},
    process::Command,
};
use thiserror::Error;

#[cfg(feature = "local-fixture-canary")]
use std::fs;

pub const MANAGED_CREDENTIAL_DIR_ENV: &str = "GONGBU_MANAGED_CREDENTIAL_DIR";
pub const MANAGED_STACK_SERVICE: &str = "hubu.managed-stack.v1";
pub const MANAGED_HUBU_ACCOUNT: &str = "hubu-executor";
pub const MANAGED_CALLER_ACCOUNT: &str = "gongbu-caller";

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

/// Gongbu-owned bootstrap credentials for a launcher-managed local stack.
/// The fixed reference vocabulary prevents generated provider configuration
/// from selecting arbitrary files beneath this private directory.
pub struct ManagedStackSecrets {
    root: PathBuf,
}

impl ManagedStackSecrets {
    pub fn from_environment() -> Result<Self> {
        let root = std::env::var_os(MANAGED_CREDENTIAL_DIR_ENV)
            .map(PathBuf::from)
            .ok_or(SecretError::Unavailable)?;
        validate_managed_root(&root)?;
        Ok(Self { root })
    }

    pub fn from_root(root: PathBuf) -> Result<Self> {
        validate_managed_root(&root)?;
        Ok(Self { root })
    }
}

impl SecretProvider for ManagedStackSecrets {
    fn resolve(&self, reference: &SecretReference) -> Result<ProviderSecret> {
        let name = match (reference.service(), reference.account()) {
            (MANAGED_STACK_SERVICE, MANAGED_HUBU_ACCOUNT) => "hubu-executor",
            (MANAGED_STACK_SERVICE, MANAGED_CALLER_ACCOUNT) => "caller",
            _ => return Err(SecretError::Unavailable),
        };
        ProviderSecret::new(read_private_secret(&self.root.join(name))?)
    }
}

fn validate_managed_root(root: &Path) -> Result<()> {
    if !root.is_absolute()
        || root == Path::new("/")
        || root
            .components()
            .any(|component| component == Component::ParentDir)
    {
        return Err(SecretError::InvalidReference);
    }
    let metadata = std::fs::symlink_metadata(root).map_err(|_| SecretError::Unavailable)?;
    if !metadata.file_type().is_dir() {
        return Err(SecretError::Unavailable);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(SecretError::Unavailable);
        }
    }
    Ok(())
}

fn read_private_secret(path: &Path) -> Result<Vec<u8>> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let file = options.open(path).map_err(|_| SecretError::Unavailable)?;
    let metadata = file.metadata().map_err(|_| SecretError::Unavailable)?;
    if !metadata.file_type().is_file() {
        return Err(SecretError::Unavailable);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(SecretError::Unavailable);
        }
    }
    let mut bytes = Vec::new();
    file.take(4097)
        .read_to_end(&mut bytes)
        .map_err(|_| SecretError::Unavailable)?;
    if bytes.len() > 4096 {
        bytes.fill(0);
        return Err(SecretError::Unavailable);
    }
    while bytes
        .last()
        .is_some_and(|byte| matches!(byte, b'\n' | b'\r'))
    {
        bytes.pop();
    }
    if bytes.is_empty() || bytes.iter().any(|byte| byte.is_ascii_control()) {
        bytes.fill(0);
        return Err(SecretError::Unavailable);
    }
    Ok(bytes)
}

/// File-backed secrets for the explicit, feature-gated local acceptance
/// canary. Release builds do not contain this provider.
#[cfg(feature = "local-fixture-canary")]
pub struct LocalFixtureSecrets {
    root: PathBuf,
}

#[cfg(feature = "local-fixture-canary")]
impl LocalFixtureSecrets {
    pub fn from_environment() -> Result<Self> {
        let root = std::env::var_os("GONGBU_LOCAL_FIXTURE_SECRET_DIR")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute() && path.is_dir())
            .ok_or(SecretError::Unavailable)?;
        Ok(Self { root })
    }
}

#[cfg(feature = "local-fixture-canary")]
impl SecretProvider for LocalFixtureSecrets {
    fn resolve(&self, reference: &SecretReference) -> Result<ProviderSecret> {
        let name = match (reference.service(), reference.account()) {
            ("hubu.local-fixture", "caller") => "gongbu-caller",
            ("hubu.local-fixture", "executor") => "hubu-auth",
            ("hubu.local-fixture", "provider") => "provider",
            _ => return Err(SecretError::Unavailable),
        };
        let mut bytes = fs::read(self.root.join(name)).map_err(|_| SecretError::Unavailable)?;
        while bytes
            .last()
            .is_some_and(|byte| matches!(byte, b'\n' | b'\r'))
        {
            bytes.pop();
        }
        ProviderSecret::new(bytes)
    }
}

#[cfg(test)]
pub(crate) fn secret_for_test(value: &str) -> ProviderSecret {
    ProviderSecret::new(value.as_bytes().to_vec()).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;
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

    #[cfg(unix)]
    #[test]
    fn managed_stack_provider_resolves_only_fixed_private_files() {
        use std::os::unix::fs::PermissionsExt;

        let root = tempdir().unwrap();
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
        fs::write(root.path().join("hubu-executor"), b"hubu-secret\n").unwrap();
        fs::write(root.path().join("caller"), b"caller-secret\n").unwrap();
        for name in ["hubu-executor", "caller"] {
            fs::set_permissions(root.path().join(name), fs::Permissions::from_mode(0o600)).unwrap();
        }
        let provider = ManagedStackSecrets::from_root(root.path().to_path_buf()).unwrap();
        let hubu = provider
            .resolve(&SecretReference::new(MANAGED_STACK_SERVICE, MANAGED_HUBU_ACCOUNT).unwrap())
            .unwrap();
        assert_eq!(hubu.expose(), b"hubu-secret");
        assert!(provider
            .resolve(&SecretReference::new(MANAGED_STACK_SERVICE, "provider").unwrap())
            .is_err());
    }
}
