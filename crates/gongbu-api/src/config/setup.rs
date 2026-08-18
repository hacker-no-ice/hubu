//! Local credential bootstrap and rotation without serializing secret values.

use crate::{
    hubu::{HubuClient, HubuCredentialCheck},
    provider::targets::ProviderTargetConfig,
    server::{SecretReferenceConfig, ServerConfig, ServerError},
};
use sha2::{Digest, Sha256};
use std::{
    env, fs,
    path::{Path, PathBuf},
};
use uuid::Uuid;

use super::secrets::{MacOsKeychain, ProviderSecret, SecretProvider, SecretReference, SecretStore};

const AUTH_TOKEN_ENV: &str = "HUBU_AUTH_TOKEN";
const AUTH_TOKEN_FILE_ENV: &str = "HUBU_AUTH_TOKEN_FILE";
const HUBU_HOME_ENV: &str = "HUBU_HOME";
const DEFAULT_AUTH_TOKEN_FILE: &str = "hubu.auth-token";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CredentialClass {
    CallerCapability,
    HubuExecutorCredential,
}

impl CredentialClass {
    pub fn parse(value: &str) -> Result<Self, ServerError> {
        match value {
            "caller" => Ok(Self::CallerCapability),
            "hubu" => Ok(Self::HubuExecutorCredential),
            _ => Err(invalid("credential class must be `caller` or `hubu`")),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::CallerCapability => "caller-to-Gongbu capability",
            Self::HubuExecutorCredential => "Hubu executor/service credential",
        }
    }
}

pub struct DiscoveredHubuCredential {
    secret: Vec<u8>,
    source: &'static str,
}

impl Drop for DiscoveredHubuCredential {
    fn drop(&mut self) {
        self.secret.fill(0);
    }
}

impl DiscoveredHubuCredential {
    pub fn expose(&self) -> &[u8] {
        &self.secret
    }
    pub fn source(&self) -> &'static str {
        self.source
    }
}

pub fn bootstrap(config_path: &Path, explicit_file: Option<&Path>) -> Result<String, ServerError> {
    let config = ServerConfig::from_path(config_path)?;
    let store = MacOsKeychain;
    verify_provider_credentials(&config, &store)?;
    let discovered = discover_hubu_credential(explicit_file)?;
    verify_hubu_credential(&config, discovered.expose())?;

    let caller = reference(&config.authentication.bearer_credential_reference)?;
    if store.resolve(&caller).is_err() {
        let mut generated = format!("gongbu_caller_{}", Uuid::new_v4()).into_bytes();
        store
            .persist(&caller, &generated)
            .map_err(|_| invalid("caller-to-Gongbu capability could not be persisted"))?;
        generated.fill(0);
    }
    let hubu = reference(&config.hubu.credential_reference)?;
    persist_with_rollback(&store, &hubu, discovered.expose())?;
    Ok(format!(
        "credential bootstrap complete: caller-to-Gongbu capability ready; Hubu executor/service credential verified from {}; provider credentials ready; restart Gongbu after any later caller or Hubu credential change",
        discovered.source()
    ))
}

pub fn rotate(
    config_path: &Path,
    class: CredentialClass,
    explicit_file: Option<&Path>,
) -> Result<String, ServerError> {
    let config = ServerConfig::from_path(config_path)?;
    let store = MacOsKeychain;
    let target = class_reference(&config, class)?;
    match class {
        CredentialClass::CallerCapability => {
            let mut generated = format!("gongbu_caller_{}", Uuid::new_v4()).into_bytes();
            persist_with_rollback(&store, &target, &generated)?;
            generated.fill(0);
        }
        CredentialClass::HubuExecutorCredential => {
            let discovered = discover_hubu_credential(explicit_file)?;
            verify_hubu_credential(&config, discovered.expose())?;
            persist_with_rollback(&store, &target, discovered.expose())?;
        }
    }
    Ok(format!(
        "{} rotated in Keychain; running Gongbu detects the change and must be restarted; rollback remains available until explicitly revoked",
        class.label()
    ))
}

pub fn rollback(config_path: &Path, class: CredentialClass) -> Result<String, ServerError> {
    let config = ServerConfig::from_path(config_path)?;
    let store = MacOsKeychain;
    let primary = class_reference(&config, class)?;
    let backup = rollback_reference(&primary)?;
    let current = store
        .resolve(&primary)
        .map_err(|_| invalid(format!("{} is unavailable", class.label())))?;
    let previous = store
        .resolve(&backup)
        .map_err(|_| invalid(format!("{} rollback is unavailable", class.label())))?;
    if class == CredentialClass::HubuExecutorCredential {
        verify_hubu_credential(&config, previous.expose())?;
    }
    store
        .persist(&primary, previous.expose())
        .map_err(|_| invalid(format!("{} rollback failed", class.label())))?;
    store
        .persist(&backup, current.expose())
        .map_err(|_| invalid(format!("{} rollback backup failed", class.label())))?;
    Ok(format!("{} rolled back; restart Gongbu", class.label()))
}

pub fn revoke_rollback(config_path: &Path, class: CredentialClass) -> Result<String, ServerError> {
    let config = ServerConfig::from_path(config_path)?;
    let store = MacOsKeychain;
    let backup = rollback_reference(&class_reference(&config, class)?)?;
    store
        .delete(&backup)
        .map_err(|_| invalid(format!("{} rollback could not be revoked", class.label())))?;
    Ok(format!("{} rollback revoked", class.label()))
}

pub fn credential_digest(secret: &ProviderSecret) -> [u8; 32] {
    Sha256::digest(secret.expose()).into()
}

pub fn configured_digests(
    config: &ServerConfig,
    provider: &dyn SecretProvider,
) -> Result<([u8; 32], [u8; 32]), ServerError> {
    let caller = provider
        .resolve(&reference(
            &config.authentication.bearer_credential_reference,
        )?)
        .map_err(|_| invalid("caller-to-Gongbu capability is unavailable"))?;
    let hubu = provider
        .resolve(&reference(&config.hubu.credential_reference)?)
        .map_err(|_| invalid("Hubu executor/service credential is unavailable"))?;
    Ok((credential_digest(&caller), credential_digest(&hubu)))
}

pub fn changed_credential_class(
    config: &ServerConfig,
    provider: &dyn SecretProvider,
    startup_caller: [u8; 32],
    startup_hubu: [u8; 32],
) -> Result<Option<CredentialClass>, ServerError> {
    let (caller, hubu) = configured_digests(config, provider)?;
    if caller != startup_caller {
        Ok(Some(CredentialClass::CallerCapability))
    } else if hubu != startup_hubu {
        Ok(Some(CredentialClass::HubuExecutorCredential))
    } else {
        Ok(None)
    }
}

fn verify_provider_credentials(
    config: &ServerConfig,
    provider: &dyn SecretProvider,
) -> Result<(), ServerError> {
    let targets = ProviderTargetConfig::from_path(&config.providers.target_catalog_path)
        .map_err(|error| invalid(format!("provider target catalog: {error}")))?;
    for target in targets
        .revisions()
        .filter(|target| target.is_execution_enabled())
    {
        let reference = target
            .secret_reference()
            .map_err(|_| invalid("provider credential reference is invalid"))?;
        provider
            .resolve(&reference)
            .map_err(|_| invalid("provider credential is unavailable"))?;
    }
    Ok(())
}

fn verify_hubu_credential(
    config: &ServerConfig,
    secret: &[u8],
) -> Result<HubuCredentialCheck, ServerError> {
    let client = HubuClient::new(&config.hubu.endpoint).with_bearer_token(secret.to_vec());
    let check = client
        .check_executor_credential()
        .map_err(|error| match error {
            crate::hubu::HttpClientError::Status {
                status: 401 | 403, ..
            } => invalid(
                "Hubu executor/service credential was rejected by the protected executor endpoint",
            ),
            _ => invalid("Hubu protected executor credential check failed"),
        })?;
    if check.executor_contract != config.hubu.expected_executor_contract {
        return Err(invalid(
            "Hubu protected executor credential check returned an incompatible contract",
        ));
    }
    Ok(check)
}

fn persist_with_rollback(
    store: &dyn SecretStore,
    primary: &SecretReference,
    value: &[u8],
) -> Result<(), ServerError> {
    if let Ok(existing) = store.resolve(primary) {
        if existing.expose() == value {
            return Ok(());
        }
        let backup = rollback_reference(primary)?;
        store
            .persist(&backup, existing.expose())
            .map_err(|_| invalid("credential rollback could not be persisted"))?;
    }
    store
        .persist(primary, value)
        .map_err(|_| invalid("credential could not be persisted"))
}

fn class_reference(
    config: &ServerConfig,
    class: CredentialClass,
) -> Result<SecretReference, ServerError> {
    match class {
        CredentialClass::CallerCapability => {
            reference(&config.authentication.bearer_credential_reference)
        }
        CredentialClass::HubuExecutorCredential => reference(&config.hubu.credential_reference),
    }
}

fn reference(config: &SecretReferenceConfig) -> Result<SecretReference, ServerError> {
    SecretReference::new(config.service.clone(), config.account.clone())
        .map_err(|_| invalid("invalid opaque credential reference"))
}

fn rollback_reference(primary: &SecretReference) -> Result<SecretReference, ServerError> {
    SecretReference::new(primary.service(), format!("{}.rollback", primary.account()))
        .map_err(|_| invalid("invalid rollback credential reference"))
}

fn discover_hubu_credential(
    explicit_file: Option<&Path>,
) -> Result<DiscoveredHubuCredential, ServerError> {
    if let Some(path) = explicit_file {
        return read_discovered(path, "explicit --hubu-token-file");
    }
    if let Ok(value) = env::var(AUTH_TOKEN_ENV) {
        return discovered(value.into_bytes(), "HUBU_AUTH_TOKEN environment");
    }
    if let Some(path) = env::var_os(AUTH_TOKEN_FILE_ENV).map(PathBuf::from) {
        return read_discovered(&path, "HUBU_AUTH_TOKEN_FILE");
    }
    let local = PathBuf::from(DEFAULT_AUTH_TOKEN_FILE);
    if local.is_file() {
        return read_discovered(&local, "working-directory token file");
    }
    if let Some(home) = env::var_os(HUBU_HOME_ENV).map(PathBuf::from) {
        let path = home.join(DEFAULT_AUTH_TOKEN_FILE);
        if path.is_file() {
            return read_discovered(&path, "HUBU_HOME token file");
        }
    }
    if let Some(home) = env::var_os("HOME").map(PathBuf::from) {
        let path = home.join(".hubu").join(DEFAULT_AUTH_TOKEN_FILE);
        if path.is_file() {
            return read_discovered(&path, "default Hubu home token file");
        }
    }
    Err(invalid("Hubu executor/service credential was not discovered; precedence is --hubu-token-file, HUBU_AUTH_TOKEN, HUBU_AUTH_TOKEN_FILE, ./hubu.auth-token, HUBU_HOME, then ~/.hubu"))
}

fn read_discovered(
    path: &Path,
    source: &'static str,
) -> Result<DiscoveredHubuCredential, ServerError> {
    let bytes = fs::read(path).map_err(|_| {
        invalid(format!(
            "Hubu executor/service credential is unavailable from {source}"
        ))
    })?;
    discovered(bytes, source)
}

fn discovered(
    mut bytes: Vec<u8>,
    source: &'static str,
) -> Result<DiscoveredHubuCredential, ServerError> {
    while bytes
        .last()
        .is_some_and(|byte| matches!(byte, b'\n' | b'\r'))
    {
        bytes.pop();
    }
    if bytes.is_empty()
        || bytes
            .iter()
            .any(|byte| byte.is_ascii_whitespace() || byte.is_ascii_control())
    {
        bytes.fill(0);
        return Err(invalid(
            "discovered Hubu executor/service credential is empty or malformed",
        ));
    }
    Ok(DiscoveredHubuCredential {
        secret: bytes,
        source,
    })
}

fn invalid(message: impl Into<String>) -> ServerError {
    ServerError::Credential(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::secrets::{secret_for_test, SecretError};
    use std::{
        collections::BTreeMap,
        io::{Read, Write},
        net::TcpListener,
        sync::Mutex,
        thread,
    };
    use tempfile::tempdir;

    #[derive(Default)]
    struct MemoryStore(Mutex<BTreeMap<(String, String), Vec<u8>>>);

    fn test_config(root: &Path) -> ServerConfig {
        serde_json::from_value(serde_json::json!({
            "schema_version": 1,
            "http": {"listen": "127.0.0.1:8788"},
            "state": {"database_path": root.join("gongbu.sqlite3"), "artifact_root": root.join("artifacts")},
            "temporal": {"mode": "external", "address": "http://127.0.0.1:7233", "namespace": "default", "task_queue": "gongbu-local", "ui_url": null},
            "hubu": {
                "endpoint": "http://127.0.0.1:8787", "allowlisted_hosts": [],
                "expected_product_version": "0.1.0", "expected_executor_contract": "hubu-spend-executor-v4",
                "account_id": "account-1", "agent_id": "agent-1",
                "credential_reference": {"service": "gongbu.hubu", "account": "local"},
                "startup_policy": "exit", "startup_timeout_ms": 1000
            },
            "authentication": {"caller_account_id": "account-1", "bearer_credential_reference": {"service": "gongbu.caller", "account": "local"}},
            "providers": {"target_catalog_path": root.join("targets.json"), "pricing_catalog_path": root.join("prices.json"), "maximum_spend_minor": 100, "live_spend_acknowledgement": "I_ACKNOWLEDGE_LIVE_PROVIDER_SPEND"},
            "artifacts": {"max_artifacts_per_execution": 4, "max_encoded_bytes": 100, "max_decoded_bytes": 100, "max_width": 100, "max_height": 100},
            "execution": {"recovery_delays_seconds": [1], "temporal_startup_timeout_ms": 1000, "dependency_check_interval_ms": 1000},
            "logging": {"level": "info", "format": "text"},
            "shutdown": {"worker_drain_timeout_ms": 1000}
        })).unwrap()
    }

    impl SecretProvider for MemoryStore {
        fn resolve(
            &self,
            reference: &SecretReference,
        ) -> super::super::secrets::Result<ProviderSecret> {
            let values = self.0.lock().unwrap();
            values
                .get(&(reference.service().into(), reference.account().into()))
                .map(|value| secret_for_test(std::str::from_utf8(value).unwrap()))
                .ok_or(SecretError::Unavailable)
        }
    }

    impl SecretStore for MemoryStore {
        fn persist(
            &self,
            reference: &SecretReference,
            value: &[u8],
        ) -> super::super::secrets::Result<()> {
            self.0.lock().unwrap().insert(
                (reference.service().into(), reference.account().into()),
                value.to_vec(),
            );
            Ok(())
        }

        fn delete(&self, reference: &SecretReference) -> super::super::secrets::Result<()> {
            self.0
                .lock()
                .unwrap()
                .remove(&(reference.service().into(), reference.account().into()));
            Ok(())
        }
    }

    #[test]
    fn explicit_hubu_file_is_trimmed_and_source_is_non_secret() {
        const CANARY: &str = "hubu-executor-secret-canary-69";
        let root = tempdir().unwrap();
        let path = root.path().join("opaque");
        fs::write(&path, format!("{CANARY}\n")).unwrap();
        let discovered = discover_hubu_credential(Some(&path)).unwrap();
        assert_eq!(discovered.expose(), CANARY.as_bytes());
        assert_eq!(discovered.source(), "explicit --hubu-token-file");
        assert!(!discovered.source().contains(CANARY));
        assert!(!discovered.source().contains(path.to_str().unwrap()));
    }

    #[test]
    fn replacement_preserves_opaque_rollback_and_supports_revocation() {
        const OLD: &[u8] = b"old-secret-canary";
        const NEW: &[u8] = b"new-secret-canary";
        let store = MemoryStore::default();
        let primary = SecretReference::new("gongbu.hubu", "local").unwrap();
        store.persist(&primary, OLD).unwrap();

        persist_with_rollback(&store, &primary, NEW).unwrap();
        assert_eq!(store.resolve(&primary).unwrap().expose(), NEW);
        let rollback = rollback_reference(&primary).unwrap();
        assert_eq!(store.resolve(&rollback).unwrap().expose(), OLD);
        store.delete(&rollback).unwrap();
        assert!(matches!(
            store.resolve(&rollback),
            Err(SecretError::Unavailable)
        ));
    }

    #[test]
    fn credential_change_detection_and_generated_config_exclude_secret_values() {
        const CALLER: &str = "caller-secret-canary";
        const HUBU: &str = "hubu-secret-canary";
        let root = tempdir().unwrap();
        let config = test_config(root.path());
        let store = MemoryStore::default();
        let caller = reference(&config.authentication.bearer_credential_reference).unwrap();
        let hubu = reference(&config.hubu.credential_reference).unwrap();
        store.persist(&caller, CALLER.as_bytes()).unwrap();
        store.persist(&hubu, HUBU.as_bytes()).unwrap();
        let original = configured_digests(&config, &store).unwrap();
        store.persist(&caller, b"rotated-caller").unwrap();
        let rotated = configured_digests(&config, &store).unwrap();
        assert_ne!(original, rotated);
        assert_eq!(
            changed_credential_class(&config, &store, original.0, original.1).unwrap(),
            Some(CredentialClass::CallerCapability)
        );
        store.persist(&caller, CALLER.as_bytes()).unwrap();
        store.persist(&hubu, b"rotated-hubu").unwrap();
        assert_eq!(
            changed_credential_class(&config, &store, original.0, original.1).unwrap(),
            Some(CredentialClass::HubuExecutorCredential)
        );
        let config_json = serde_json::to_string(&config).unwrap();
        assert!(!config_json.contains(CALLER));
        assert!(!config_json.contains(HUBU));
        let diagnostic =
            invalid("caller-to-Gongbu capability changed; restart required").to_string();
        assert!(!diagnostic.contains(CALLER));
        assert!(!diagnostic.contains(HUBU));
    }

    #[test]
    fn protected_check_rejects_placeholder_without_leaking_it() {
        const PLACEHOLDER: &[u8] = b"replace-with-hubu-bearer-canary";
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = String::new();
            stream.read_to_string(&mut request).unwrap();
            assert!(request.contains("GET /spend/executor/credential-check"));
            assert!(request.contains("Authorization: Bearer replace-with-hubu-bearer-canary"));
            let body = r#"{"error":"rejected replace-with-hubu-bearer-canary"}"#;
            write!(
                stream,
                "HTTP/1.1 401 Unauthorized\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
        });
        let root = tempdir().unwrap();
        let mut config = test_config(root.path());
        config.hubu.endpoint = format!("http://{address}");
        let error = verify_hubu_credential(&config, PLACEHOLDER).unwrap_err();
        server.join().unwrap();
        assert!(error.to_string().contains("protected executor endpoint"));
        assert!(!error
            .to_string()
            .contains(std::str::from_utf8(PLACEHOLDER).unwrap()));
    }
}
