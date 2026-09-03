use super::http_kernel::{
    url_has_explicit_port, valid_provider_deadline_ms, validate_https_origin, validate_safe_headers,
};
use crate::secrets::{SecretError, SecretReference};
use reqwest::{header::HeaderName, Url};
use serde::{de::Error as _, Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    path::Path,
};
use thiserror::Error;

const PROVIDER_CONFIG_ENV: &str = "GONGBU_PROVIDER_CONFIG";
const CURRENT_SCHEMA_VERSION: u32 = 3;
const BFL_API_HOSTS: &[&str] = &["api.bfl.ai", "api.eu.bfl.ai", "api.us.bfl.ai"];
// BFL's current API reference still documents this concrete cluster endpoint.
// Keep cluster admission exact: a broad `api.*.bfl.ai` pattern would forward
// credentials to origins outside the reviewed provider contract.
const BFL_POLLING_HOSTS: &[&str] = &[
    "api.bfl.ai",
    "api.eu.bfl.ai",
    "api.us.bfl.ai",
    "api.us1.bfl.ai",
];
pub const LEGACY_UNRESOLVED_DIGEST: &str = "legacy-unresolved";

#[derive(Debug, Error)]
pub enum Error {
    #[error("{PROVIDER_CONFIG_ENV} must name an operator-managed file")]
    MissingConfigPath,
    #[error("read provider target config {path}: {source}")]
    Read {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("parse provider target config {path}: {source}")]
    Parse {
        path: String,
        #[source]
        source: serde_json::Error,
    },
    #[error("provider target config must define at least one revision")]
    Empty,
    #[error("unsupported provider target config schema version")]
    UnsupportedSchema,
    #[error("provider target identifiers are invalid")]
    InvalidIdentifier,
    #[error("invalid provider secret reference")]
    InvalidSecretReference,
    #[error("provider adapter settings do not match the target")]
    InvalidAdapterSettings,
    #[error("provider endpoint, origin, headers, or deadline are invalid")]
    InvalidTransportSettings,
    #[error("duplicate provider target revision")]
    DuplicateRevision,
    #[error("provider config version is reused with different content")]
    VersionMutation,
    #[error("more than one active revision exists for a provider target")]
    AmbiguousActiveRevision,
    #[error("requested provider target is not selectable for new work")]
    NotSelectable,
    #[error("requested provider target revision is execution-disabled")]
    ExecutionDisabled,
    #[error("requested provider target is not configured")]
    NotConfigured,
    #[error("persisted provider configuration digest does not match")]
    DigestMismatch,
    #[error("provider contract binding is invalid")]
    InvalidProviderContract,
    #[error("duplicate provider contract binding")]
    DuplicateProviderContract,
}

pub type Result<T> = std::result::Result<T, Error>;

/// Stable selector shared by configuration, persistence, pricing, and binding.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(deny_unknown_fields)]
pub struct TargetKey {
    pub workload_type: String,
    pub provider: String,
    pub adapter: String,
    pub model: String,
}

impl TargetKey {
    pub fn new(
        workload_type: impl Into<String>,
        provider: impl Into<String>,
        adapter: impl Into<String>,
        model: impl Into<String>,
    ) -> Result<Self> {
        let key = Self {
            workload_type: workload_type.into(),
            provider: provider.into(),
            adapter: adapter.into(),
            model: model.into(),
        };
        if [&key.workload_type, &key.provider, &key.adapter, &key.model]
            .iter()
            .any(|part| !valid_identifier(part))
        {
            return Err(Error::InvalidIdentifier);
        }
        Ok(key)
    }

    pub fn canonical_name(&self) -> String {
        format!(
            "{}/{}/{}/{}",
            self.workload_type, self.provider, self.adapter, self.model
        )
    }

    /// Stable, opaque selector for the logical target across configuration revisions.
    pub fn public_id(&self) -> String {
        let mut digest = Sha256::new();
        digest.update(b"gongbu-target-v1\0");
        for part in [
            &self.workload_type,
            &self.provider,
            &self.adapter,
            &self.model,
        ] {
            digest.update((part.len() as u64).to_be_bytes());
            digest.update(part.as_bytes());
        }
        format!("gongbu:target:v1:{:x}", digest.finalize())
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "type", content = "config", rename_all = "snake_case")]
pub enum AdapterSettings {
    GeminiDeveloperImage(GeminiDeveloperImageConfig),
    Flux2Api(Flux2ApiConfig),
    IdeogramImage(IdeogramImageConfig),
    /// Test/local adapters have no transport configuration or credential override.
    Fixture,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct GeminiDeveloperImageConfig {
    pub endpoint: String,
    pub api_version: String,
    /// Overall invocation budget for the complete inline-image request.
    pub timeout_ms: u64,
    #[serde(default)]
    pub max_retries: u32,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Flux2ApiConfig {
    pub endpoint: String,
    pub api_version: String,
    /// Overall invocation budget shared by submit, every poll, and artifact fetch.
    pub timeout_ms: u64,
    #[serde(default = "default_poll_interval_ms")]
    pub poll_interval_ms: u64,
    #[serde(default)]
    pub max_retries: u32,
    #[serde(default)]
    pub idempotency_header: Option<String>,
    #[serde(default)]
    pub approved_artifact_hosts: Vec<String>,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct IdeogramImageConfig {
    pub endpoint: String,
    pub api_version: String,
    /// Overall invocation budget shared by submit and artifact fetch.
    pub timeout_ms: u64,
    #[serde(default)]
    pub max_retries: u32,
    #[serde(default)]
    pub approved_artifact_hosts: Vec<String>,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
}

fn default_poll_interval_ms() -> u64 {
    500
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderConfigVersion {
    pub provider_config_version: String,
    pub workload_type: String,
    pub provider: String,
    pub adapter: String,
    pub model: String,
    pub secret_service: String,
    pub secret_account: String,
    settings: AdapterSettings,
    active: bool,
    execution_enabled: bool,
    digest: String,
}

impl ProviderConfigVersion {
    pub fn target_key(&self) -> TargetKey {
        TargetKey {
            workload_type: self.workload_type.clone(),
            provider: self.provider.clone(),
            adapter: self.adapter.clone(),
            model: self.model.clone(),
        }
    }
    pub fn digest(&self) -> &str {
        &self.digest
    }
    pub fn is_active(&self) -> bool {
        self.active
    }
    pub fn is_execution_enabled(&self) -> bool {
        self.execution_enabled
    }
    pub fn settings(&self) -> &AdapterSettings {
        &self.settings
    }
    pub fn gemini_developer_image(&self) -> Option<&GeminiDeveloperImageConfig> {
        match &self.settings {
            AdapterSettings::GeminiDeveloperImage(v) => Some(v),
            _ => None,
        }
    }
    pub fn flux2_api(&self) -> Option<&Flux2ApiConfig> {
        match &self.settings {
            AdapterSettings::Flux2Api(v) => Some(v),
            _ => None,
        }
    }
    pub fn ideogram_image(&self) -> Option<&IdeogramImageConfig> {
        match &self.settings {
            AdapterSettings::IdeogramImage(v) => Some(v),
            _ => None,
        }
    }
    pub fn secret_reference(&self) -> std::result::Result<SecretReference, SecretError> {
        SecretReference::new(self.secret_service.clone(), self.secret_account.clone())
    }
}

/// Opaque validated catalog. Invalid raw configuration cannot inhabit this type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderTargetConfig {
    revisions: BTreeMap<(TargetKey, String), ProviderConfigVersion>,
    active: BTreeMap<TargetKey, String>,
    contract_bindings: Vec<ProviderContractBinding>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProviderContractBinding {
    pub contract: String,
    pub pricing_version: String,
    pub poll_policy: String,
    pub artifact_delivery_policy: String,
    pub recovery_policy: String,
    pub generation_retries: u32,
    pub fallback: bool,
}

impl ProviderTargetConfig {
    pub(crate) fn disabled() -> Self {
        Self {
            revisions: BTreeMap::new(),
            active: BTreeMap::new(),
            contract_bindings: Vec::new(),
        }
    }

    pub fn from_env() -> Result<Self> {
        let path = env::var(PROVIDER_CONFIG_ENV).map_err(|_| Error::MissingConfigPath)?;
        Self::from_path(Path::new(&path))
    }
    pub fn from_path(path: &Path) -> Result<Self> {
        let display = path.display().to_string();
        let contents = fs::read_to_string(path).map_err(|source| Error::Read {
            path: display.clone(),
            source,
        })?;
        serde_json::from_str(&contents).map_err(|source| Error::Parse {
            path: display,
            source,
        })
    }
    pub fn validate(&self) -> Result<()> {
        Ok(())
    }
    pub fn revisions(&self) -> impl Iterator<Item = &ProviderConfigVersion> {
        self.revisions.values()
    }
    pub fn contract_bindings(&self) -> &[ProviderContractBinding] {
        &self.contract_bindings
    }
    pub fn resolve(
        &self,
        workload_type: &str,
        provider: &str,
        adapter: &str,
        model: &str,
    ) -> Result<&ProviderConfigVersion> {
        let key = TargetKey::new(workload_type, provider, adapter, model)?;
        self.resolve_active(&key)
    }
    pub fn resolve_active(&self, key: &TargetKey) -> Result<&ProviderConfigVersion> {
        let version = self.active.get(key).ok_or(Error::NotSelectable)?;
        let revision = self
            .revisions
            .get(&(key.clone(), version.clone()))
            .ok_or(Error::NotConfigured)?;
        if !revision.execution_enabled {
            return Err(Error::ExecutionDisabled);
        }
        Ok(revision)
    }

    pub fn resolve_target_id(&self, target_id: &str) -> Result<&ProviderConfigVersion> {
        let mut matches = self
            .active
            .keys()
            .filter(|key| key.public_id() == target_id);
        let key = matches.next().ok_or(Error::NotSelectable)?;
        if matches.next().is_some() {
            return Err(Error::NotSelectable);
        }
        self.resolve_active(key)
    }
    pub fn resolve_revision(
        &self,
        key: &TargetKey,
        version: &str,
        digest: &str,
    ) -> Result<&ProviderConfigVersion> {
        let revision = self
            .revisions
            .get(&(key.clone(), version.to_owned()))
            .ok_or(Error::NotConfigured)?;
        if revision.digest != digest {
            return Err(Error::DigestMismatch);
        }
        if !revision.execution_enabled {
            return Err(Error::ExecutionDisabled);
        }
        Ok(revision)
    }

    /// Resolve rows created before provider configuration digests were stored.
    /// This compatibility path is unreachable for newly validated executions.
    pub fn resolve_persisted_revision(
        &self,
        key: &TargetKey,
        version: &str,
        digest: &str,
    ) -> Result<&ProviderConfigVersion> {
        if digest == LEGACY_UNRESOLVED_DIGEST {
            let revision = self
                .revisions
                .get(&(key.clone(), version.to_owned()))
                .ok_or(Error::NotConfigured)?;
            if !revision.execution_enabled {
                return Err(Error::ExecutionDisabled);
            }
            return Ok(revision);
        }
        self.resolve_revision(key, version, digest)
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RawCatalog {
    #[serde(default = "legacy_schema_version")]
    schema_version: u32,
    #[serde(default)]
    contract_bindings: Vec<ProviderContractBinding>,
    provider_configs: Vec<RawRevision>,
}
fn legacy_schema_version() -> u32 {
    1
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RawRevision {
    provider_config_version: String,
    workload_type: String,
    provider: String,
    adapter: String,
    model: String,
    secret_service: String,
    secret_account: String,
    #[serde(default)]
    active: Option<bool>,
    #[serde(default)]
    execution_enabled: Option<bool>,
    #[serde(default)]
    enabled: Option<bool>,
    #[serde(default)]
    settings: Option<AdapterSettings>,
    #[serde(default)]
    gemini_developer_image: Option<GeminiDeveloperImageConfig>,
    #[serde(default)]
    flux2_api: Option<Flux2ApiConfig>,
    #[serde(default)]
    ideogram_image: Option<IdeogramImageConfig>,
}

impl TryFrom<RawCatalog> for ProviderTargetConfig {
    type Error = Error;
    fn try_from(raw: RawCatalog) -> Result<Self> {
        if !matches!(raw.schema_version, 1 | 2 | CURRENT_SCHEMA_VERSION) {
            return Err(Error::UnsupportedSchema);
        }
        if raw.schema_version < 3 && !raw.contract_bindings.is_empty() {
            return Err(Error::UnsupportedSchema);
        }
        if raw.provider_configs.is_empty() {
            return Err(Error::Empty);
        }
        let mut bound_contracts = BTreeSet::new();
        for binding in &raw.contract_bindings {
            if [
                binding.contract.as_str(),
                binding.pricing_version.as_str(),
                binding.poll_policy.as_str(),
                binding.artifact_delivery_policy.as_str(),
                binding.recovery_policy.as_str(),
            ]
            .iter()
            .any(|value| value.is_empty() || value.trim() != *value || value.len() > 255)
                || !bound_contracts.insert(binding.contract.clone())
            {
                return Err(if bound_contracts.contains(&binding.contract) {
                    Error::DuplicateProviderContract
                } else {
                    Error::InvalidProviderContract
                });
            }
        }
        let contract_bindings = raw.contract_bindings;
        let mut revisions = BTreeMap::new();
        let mut active = BTreeMap::new();
        let mut version_digests = BTreeMap::<String, String>::new();
        for raw_revision in raw.provider_configs {
            let revision = validate_revision(raw.schema_version, raw_revision)?;
            let key = revision.target_key();
            let version = revision.provider_config_version.clone();
            if let Some(existing) = version_digests.insert(version.clone(), revision.digest.clone())
            {
                if existing != revision.digest {
                    return Err(Error::VersionMutation);
                }
                return Err(Error::DuplicateRevision);
            }
            if revisions
                .insert((key.clone(), version.clone()), revision.clone())
                .is_some()
            {
                return Err(Error::DuplicateRevision);
            }
            if revision.active && active.insert(key, version).is_some() {
                return Err(Error::AmbiguousActiveRevision);
            }
        }
        Ok(Self {
            revisions,
            active,
            contract_bindings,
        })
    }
}

impl<'de> Deserialize<'de> for ProviderTargetConfig {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        RawCatalog::deserialize(deserializer)?
            .try_into()
            .map_err(D::Error::custom)
    }
}

impl Serialize for ProviderTargetConfig {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        let provider_configs = self
            .revisions
            .values()
            .map(|r| RawRevision {
                provider_config_version: r.provider_config_version.clone(),
                workload_type: r.workload_type.clone(),
                provider: r.provider.clone(),
                adapter: r.adapter.clone(),
                model: r.model.clone(),
                secret_service: r.secret_service.clone(),
                secret_account: r.secret_account.clone(),
                active: Some(r.active),
                execution_enabled: Some(r.execution_enabled),
                enabled: None,
                settings: Some(r.settings.clone()),
                gemini_developer_image: None,
                flux2_api: None,
                ideogram_image: None,
            })
            .collect();
        RawCatalog {
            schema_version: if self.contract_bindings.is_empty() {
                2
            } else {
                CURRENT_SCHEMA_VERSION
            },
            contract_bindings: self.contract_bindings.clone(),
            provider_configs,
        }
        .serialize(serializer)
    }
}

fn validate_revision(schema: u32, raw: RawRevision) -> Result<ProviderConfigVersion> {
    let supplied_v2_fields =
        raw.active.is_some() || raw.execution_enabled.is_some() || raw.settings.is_some();
    let key = TargetKey::new(&raw.workload_type, &raw.provider, &raw.adapter, &raw.model)?;
    if !valid_identifier(&raw.provider_config_version) {
        return Err(Error::InvalidIdentifier);
    }
    SecretReference::new(raw.secret_service.clone(), raw.secret_account.clone())
        .map_err(|_| Error::InvalidSecretReference)?;
    let legacy_settings = [
        raw.gemini_developer_image.is_some(),
        raw.flux2_api.is_some(),
        raw.ideogram_image.is_some(),
    ]
    .into_iter()
    .filter(|v| *v)
    .count();
    if raw.settings.is_some() && legacy_settings != 0 {
        return Err(Error::InvalidAdapterSettings);
    }
    let mut settings = if let Some(settings) = raw.settings {
        settings
    } else if legacy_settings > 1 {
        return Err(Error::InvalidAdapterSettings);
    } else if let Some(v) = raw.gemini_developer_image {
        AdapterSettings::GeminiDeveloperImage(v)
    } else if let Some(v) = raw.flux2_api {
        AdapterSettings::Flux2Api(v)
    } else if let Some(v) = raw.ideogram_image {
        AdapterSettings::IdeogramImage(v)
    } else {
        AdapterSettings::Fixture
    };
    validate_settings(&key, &settings)?;
    normalize_settings(&mut settings);
    let legacy_enabled = raw.enabled.unwrap_or(true);
    let active = raw.active.unwrap_or(legacy_enabled);
    let execution_enabled = raw.execution_enabled.unwrap_or(legacy_enabled);
    if schema == 1 && supplied_v2_fields {
        return Err(Error::UnsupportedSchema);
    }
    let canonical = serde_json::json!({
        "target_key": key, "provider_config_version": raw.provider_config_version,
        "secret_service": raw.secret_service, "secret_account": raw.secret_account,
        "settings": settings,
    });
    let bytes =
        serde_json::to_vec(&canonicalize(&canonical)).map_err(|_| Error::InvalidAdapterSettings)?;
    let digest = format!("sha256:{:x}", Sha256::digest(bytes));
    Ok(ProviderConfigVersion {
        provider_config_version: canonical["provider_config_version"]
            .as_str()
            .unwrap()
            .to_owned(),
        workload_type: key.workload_type,
        provider: key.provider,
        adapter: key.adapter,
        model: key.model,
        secret_service: canonical["secret_service"].as_str().unwrap().to_owned(),
        secret_account: canonical["secret_account"].as_str().unwrap().to_owned(),
        settings,
        active,
        execution_enabled,
        digest,
    })
}

fn validate_settings(key: &TargetKey, settings: &AdapterSettings) -> Result<()> {
    match (key.provider.as_str(), key.adapter.as_str(), settings) {
        ("google", "gemini_developer_image", AdapterSettings::GeminiDeveloperImage(c)) => {
            validate_transport(
                &c.endpoint,
                &c.api_version,
                c.timeout_ms,
                c.max_retries,
                &c.headers,
                &["authorization", "x-goog-api-key"],
            )?
        }
        ("flux", "flux2_api", AdapterSettings::Flux2Api(c)) => {
            validate_transport(
                &c.endpoint,
                &c.api_version,
                c.timeout_ms,
                c.max_retries,
                &c.headers,
                &["authorization", "x-key"],
            )?;
            if c.poll_interval_ms == 0
                || c.poll_interval_ms > c.timeout_ms
                || url_has_explicit_port(&c.endpoint)
                || Url::parse(&c.endpoint)
                    .ok()
                    .is_none_or(|url| !valid_bfl_api_host(url.host_str()))
                || !valid_artifact_hosts(&c.approved_artifact_hosts, false)
                || !c
                    .approved_artifact_hosts
                    .iter()
                    .all(|host| valid_bfl_delivery_host(host))
            {
                return Err(Error::InvalidTransportSettings);
            }
            if let Some(name) = &c.idempotency_header {
                if !valid_header_name(name)
                    || ["authorization", "x-key"]
                        .iter()
                        .any(|v| name.eq_ignore_ascii_case(v))
                    || c.headers.keys().any(|v| name.eq_ignore_ascii_case(v))
                {
                    return Err(Error::InvalidTransportSettings);
                }
            }
        }
        ("ideogram", "ideogram_image", AdapterSettings::IdeogramImage(c)) => {
            validate_transport(
                &c.endpoint,
                &c.api_version,
                c.timeout_ms,
                c.max_retries,
                &c.headers,
                &["authorization", "api-key"],
            )?;
            if !valid_artifact_hosts(&c.approved_artifact_hosts, true) {
                return Err(Error::InvalidTransportSettings);
            }
        }
        (_, _, AdapterSettings::Fixture)
            if !matches!(key.provider.as_str(), "google" | "flux" | "ideogram") => {}
        _ => return Err(Error::InvalidAdapterSettings),
    }
    Ok(())
}

fn normalize_settings(settings: &mut AdapterSettings) {
    fn transport(
        endpoint: &mut String,
        headers: &mut BTreeMap<String, String>,
        hosts: Option<&mut Vec<String>>,
    ) {
        *endpoint = Url::parse(endpoint)
            .expect("validated endpoint")
            .to_string();
        *headers = std::mem::take(headers)
            .into_iter()
            .map(|(name, value)| (name.to_ascii_lowercase(), value))
            .collect();
        if let Some(hosts) = hosts {
            hosts.sort();
        }
    }
    match settings {
        AdapterSettings::GeminiDeveloperImage(config) => {
            transport(&mut config.endpoint, &mut config.headers, None)
        }
        AdapterSettings::Flux2Api(config) => {
            transport(
                &mut config.endpoint,
                &mut config.headers,
                Some(&mut config.approved_artifact_hosts),
            );
            if let Some(name) = &mut config.idempotency_header {
                *name = name.to_ascii_lowercase();
            }
        }
        AdapterSettings::IdeogramImage(config) => transport(
            &mut config.endpoint,
            &mut config.headers,
            Some(&mut config.approved_artifact_hosts),
        ),
        AdapterSettings::Fixture => {}
    }
}

fn validate_transport(
    endpoint: &str,
    api_version: &str,
    timeout_ms: u64,
    max_retries: u32,
    headers: &BTreeMap<String, String>,
    forbidden: &[&str],
) -> Result<()> {
    let url = Url::parse(endpoint).map_err(|_| Error::InvalidTransportSettings)?;
    if validate_https_origin(&url, None).is_err()
        || !valid_identifier(api_version)
        || !valid_provider_deadline_ms(timeout_ms)
        || max_retries != 0
        || validate_safe_headers(headers, forbidden).is_err()
    {
        return Err(Error::InvalidTransportSettings);
    }
    Ok(())
}

fn valid_header_name(name: &str) -> bool {
    name.parse::<HeaderName>().is_ok()
}
fn valid_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.trim() == value
        && value.len() <= 255
        && !value.chars().any(char::is_control)
}

pub(crate) fn valid_artifact_hosts(hosts: &[String], required: bool) -> bool {
    if required && hosts.is_empty() {
        return false;
    }
    let mut unique = BTreeSet::new();
    hosts.iter().all(|host| {
        if host.is_empty() || host.trim() != host || !unique.insert(host) {
            return false;
        }
        let Ok(url) = Url::parse(&format!("https://{host}/")) else {
            return false;
        };
        url.scheme() == "https"
            && url.username().is_empty()
            && url.password().is_none()
            && url.port().is_none()
            && url.host_str() == Some(host.as_str())
            && url.path() == "/"
            && url.query().is_none()
            && url.fragment().is_none()
    })
}

pub(crate) fn valid_bfl_api_host(host: Option<&str>) -> bool {
    host.is_some_and(|host| BFL_API_HOSTS.contains(&host))
}

pub(crate) fn valid_bfl_polling_host(host: Option<&str>) -> bool {
    host.is_some_and(|host| BFL_POLLING_HOSTS.contains(&host))
}

/// FLUX artifacts use BFL's fixed provider-specific delivery family, not a
/// configurable general-purpose wildcard. A region is exactly one DNS label.
pub(crate) fn valid_bfl_delivery_host(host: &str) -> bool {
    let Some(region) = host
        .strip_prefix("delivery.")
        .and_then(|value| value.strip_suffix(".bfl.ai"))
    else {
        return false;
    };
    !region.is_empty()
        && region.len() <= 63
        && !region.contains('.')
        && region
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && region
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        && region
            .as_bytes()
            .last()
            .is_some_and(u8::is_ascii_alphanumeric)
}

fn canonicalize(value: &Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(k, v)| (k.clone(), canonicalize(v)))
                .collect(),
        ),
        Value::Array(values) => Value::Array(values.iter().map(canonicalize).collect()),
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_target_id_is_stable_across_config_revisions() {
        let key = TargetKey::new(
            "image_generation",
            "google",
            "gemini_developer_image",
            "image-v1",
        )
        .unwrap();
        assert_eq!(key.public_id(), key.clone().public_id());
        assert!(key.public_id().starts_with("gongbu:target:v1:"));
        assert_eq!(key.public_id().len(), "gongbu:target:v1:".len() + 64);
        let changed = TargetKey::new(
            "image_generation",
            "google",
            "gemini_developer_image",
            "image-v2",
        )
        .unwrap();
        assert_ne!(key.public_id(), changed.public_id());

        let delimiter_in_workload = TargetKey::new("a/b", "c", "d", "e").unwrap();
        let delimiter_in_model = TargetKey::new("a", "b", "c", "d/e").unwrap();
        assert_eq!(
            delimiter_in_workload.canonical_name(),
            delimiter_in_model.canonical_name()
        );
        assert_ne!(
            delimiter_in_workload.public_id(),
            delimiter_in_model.public_id()
        );
    }

    fn catalog(json: &str) -> ProviderTargetConfig {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn legacy_provider_binding_field_is_rejected() {
        let legacy_field = concat!("supported_", "profiles");
        let source = format!(
            r#"{{"schema_version":3,"{legacy_field}":[],"provider_configs":[{{"provider_config_version":"v1","workload_type":"image_generation","provider":"example","adapter":"fixture","model":"image-v1","secret_service":"gongbu.example","secret_account":"local","settings":{{"type":"fixture"}}}}]}}"#,
        );
        let result = serde_json::from_str::<ProviderTargetConfig>(&source);
        assert!(result
            .unwrap_err()
            .to_string()
            .contains(&format!("unknown field `{legacy_field}`")));
    }

    #[test]
    fn legacy_schema_migrates_and_canonical_digest_is_stable() {
        let a = catalog(
            r#"{"provider_configs":[{"provider_config_version":"v1","workload_type":"image_generation","provider":"example","adapter":"fixture","model":"image-v1","secret_service":"gongbu.example","secret_account":"local"}]}"#,
        );
        let b = catalog(
            r#"{"schema_version":2,"provider_configs":[{"provider_config_version":"v1","workload_type":"image_generation","provider":"example","adapter":"fixture","model":"image-v1","secret_service":"gongbu.example","secret_account":"local","active":true,"execution_enabled":true,"settings":{"type":"fixture"}}]}"#,
        );
        assert_eq!(
            a.resolve("image_generation", "example", "fixture", "image-v1")
                .unwrap()
                .digest(),
            b.resolve("image_generation", "example", "fixture", "image-v1")
                .unwrap()
                .digest()
        );

        let ordered = catalog(
            r#"{"schema_version":2,"provider_configs":[{"provider_config_version":"google-v1","workload_type":"image_generation","provider":"google","adapter":"gemini_developer_image","model":"m","secret_service":"svc","secret_account":"acct","settings":{"type":"gemini_developer_image","config":{"endpoint":"https://generativelanguage.googleapis.com","api_version":"v1beta","timeout_ms":1000,"headers":{"X-Client":"gongbu"}}}}]}"#,
        );
        let normalized = catalog(
            r#"{"schema_version":2,"provider_configs":[{"provider_config_version":"google-v1","workload_type":"image_generation","provider":"google","adapter":"gemini_developer_image","model":"m","secret_service":"svc","secret_account":"acct","settings":{"type":"gemini_developer_image","config":{"endpoint":"https://generativelanguage.googleapis.com/","api_version":"v1beta","timeout_ms":1000,"headers":{"x-client":"gongbu"}}}}]}"#,
        );
        assert_eq!(
            ordered.revisions().next().unwrap().digest(),
            normalized.revisions().next().unwrap().digest()
        );
    }

    #[test]
    fn vertex_gemini_configuration_is_not_a_supported_adapter() {
        let result = serde_json::from_str::<ProviderTargetConfig>(
            r#"{"schema_version":2,"provider_configs":[{"provider_config_version":"vertex-v1","workload_type":"image_generation","provider":"google","adapter":"gemini_image","model":"gemini-image","secret_service":"svc","secret_account":"acct","active":true,"execution_enabled":true,"settings":{"type":"gemini_image","config":{"endpoint":"https://example.googleapis.com","api_version":"v1","project":"project","location":"us-central1","timeout_ms":1000}}}]}"#,
        );

        assert!(result.is_err());
    }

    #[test]
    fn rotation_keeps_v1_addressable_while_new_work_selects_v2() {
        let c = catalog(
            r#"{"schema_version":2,"provider_configs":[
          {"provider_config_version":"v1","workload_type":"image_generation","provider":"example","adapter":"fixture","model":"image-v1","secret_service":"gongbu.example","secret_account":"old","active":false,"execution_enabled":true,"settings":{"type":"fixture"}},
          {"provider_config_version":"v2","workload_type":"image_generation","provider":"example","adapter":"fixture","model":"image-v1","secret_service":"gongbu.example","secret_account":"new","active":true,"execution_enabled":true,"settings":{"type":"fixture"}}
        ]}"#,
        );
        let key = TargetKey::new("image_generation", "example", "fixture", "image-v1").unwrap();
        let active = c.resolve_active(&key).unwrap();
        assert_eq!(active.provider_config_version, "v2");
        let v1 = c
            .revisions()
            .find(|r| r.provider_config_version == "v1")
            .unwrap();
        assert_eq!(
            c.resolve_revision(&key, "v1", v1.digest())
                .unwrap()
                .secret_account,
            "old"
        );
    }

    #[test]
    fn rejects_mutation_ambiguity_digest_mismatch_and_execution_disable() {
        let mutation = r#"{"schema_version":2,"provider_configs":[
          {"provider_config_version":"v1","workload_type":"image_generation","provider":"a","adapter":"fixture","model":"m","secret_service":"svc","secret_account":"one","settings":{"type":"fixture"}},
          {"provider_config_version":"v1","workload_type":"image_generation","provider":"a","adapter":"fixture","model":"m","secret_service":"svc","secret_account":"two","active":false,"settings":{"type":"fixture"}}
        ]}"#;
        assert!(serde_json::from_str::<ProviderTargetConfig>(mutation)
            .unwrap_err()
            .to_string()
            .contains("different content"));
        let c = catalog(
            r#"{"schema_version":2,"provider_configs":[{"provider_config_version":"v1","workload_type":"image_generation","provider":"a","adapter":"fixture","model":"m","secret_service":"svc","secret_account":"one","active":false,"execution_enabled":false,"settings":{"type":"fixture"}}]}"#,
        );
        let key = TargetKey::new("image_generation", "a", "fixture", "m").unwrap();
        assert!(matches!(c.resolve_active(&key), Err(Error::NotSelectable)));
        assert!(matches!(
            c.resolve_revision(&key, "v1", "sha256:bad"),
            Err(Error::DigestMismatch)
        ));
        let digest = c.revisions().next().unwrap().digest().to_owned();
        assert!(matches!(
            c.resolve_revision(&key, "v1", &digest),
            Err(Error::ExecutionDisabled)
        ));

        let active_disabled = catalog(
            r#"{"schema_version":2,"provider_configs":[{"provider_config_version":"v1","workload_type":"image_generation","provider":"a","adapter":"fixture","model":"m","secret_service":"svc","secret_account":"one","active":true,"execution_enabled":false,"settings":{"type":"fixture"}}]}"#,
        );
        assert!(matches!(
            active_disabled.resolve_active(&key),
            Err(Error::ExecutionDisabled)
        ));
        assert!(matches!(
            active_disabled.resolve_persisted_revision(&key, "v1", LEGACY_UNRESOLVED_DIGEST),
            Err(Error::ExecutionDisabled)
        ));

        let enabled = catalog(
            r#"{"schema_version":2,"provider_configs":[{"provider_config_version":"v1","workload_type":"image_generation","provider":"a","adapter":"fixture","model":"m","secret_service":"svc","secret_account":"one","active":false,"execution_enabled":true,"settings":{"type":"fixture"}}]}"#,
        );
        assert_eq!(
            enabled
                .resolve_persisted_revision(&key, "v1", LEGACY_UNRESOLVED_DIGEST)
                .unwrap()
                .provider_config_version,
            "v1"
        );
    }

    #[test]
    fn invalid_transport_values_never_build_a_catalog() {
        let base = || serde_json::json!({"schema_version":2,"provider_configs":[{"provider_config_version":"v1","workload_type":"image_generation","provider":"google","adapter":"gemini_developer_image","model":"m","secret_service":"svc","secret_account":"acct","settings":{"type":"gemini_developer_image","config":{"endpoint":"https://example.com","api_version":"v1","timeout_ms":1000,"headers":{}}}}]});
        let mut invalid = Vec::new();
        for endpoint in [
            "http://example.com",
            "https://user@example.com",
            "https://example.com/path",
        ] {
            let mut value = base();
            value["provider_configs"][0]["settings"]["config"]["endpoint"] =
                Value::String(endpoint.into());
            invalid.push(value);
        }
        for timeout in [0, 270_001] {
            let mut value = base();
            value["provider_configs"][0]["settings"]["config"]["timeout_ms"] = Value::from(timeout);
            invalid.push(value);
        }
        let mut header = base();
        header["provider_configs"][0]["settings"]["config"]["headers"] =
            serde_json::json!({"Authorization":"secret"});
        invalid.push(header);
        for raw in invalid {
            assert!(serde_json::from_value::<ProviderTargetConfig>(raw).is_err());
        }
        let mut maximum = base();
        maximum["provider_configs"][0]["settings"]["config"]["timeout_ms"] = Value::from(270_000);
        assert!(serde_json::from_value::<ProviderTargetConfig>(maximum).is_ok());
    }

    #[test]
    fn flux_transport_origins_and_artifact_pins_are_provider_specific() {
        let base = || {
            serde_json::json!({"schema_version":2,"provider_configs":[{
                "provider_config_version":"flux-v1",
                "workload_type":"image_generation",
                "provider":"flux",
                "adapter":"flux2_api",
                "model":"flux-2-pro",
                "secret_service":"gongbu.flux",
                "secret_account":"local",
                "settings":{"type":"flux2_api","config":{
                    "endpoint":"https://api.bfl.ai",
                    "api_version":"v1",
                    "timeout_ms":1000,
                    "poll_interval_ms":10,
                    "approved_artifact_hosts":[]
                }}
            }]})
        };
        for endpoint in [
            "https://api.bfl.ai",
            "https://api.eu.bfl.ai",
            "https://api.us.bfl.ai",
        ] {
            let mut value = base();
            value["provider_configs"][0]["settings"]["config"]["endpoint"] =
                Value::String(endpoint.into());
            assert!(serde_json::from_value::<ProviderTargetConfig>(value).is_ok());
        }
        for endpoint in [
            "https://api.bfl.ai:443",
            "https://api.us1.bfl.ai",
            "https://api.bfl.ai.evil.example",
            "https://evil.api.bfl.ai",
        ] {
            let mut value = base();
            value["provider_configs"][0]["settings"]["config"]["endpoint"] =
                Value::String(endpoint.into());
            assert!(serde_json::from_value::<ProviderTargetConfig>(value).is_err());
        }
        for hosts in [
            serde_json::json!(["delivery.us.bfl.ai"]),
            serde_json::json!(["delivery.eu-2.bfl.ai"]),
        ] {
            let mut value = base();
            value["provider_configs"][0]["settings"]["config"]["approved_artifact_hosts"] = hosts;
            assert!(serde_json::from_value::<ProviderTargetConfig>(value).is_ok());
        }
        for hosts in [
            serde_json::json!(["cdn.bfl.ai"]),
            serde_json::json!(["delivery.us.east.bfl.ai"]),
            serde_json::json!(["delivery.us.bfl.ai.evil.example"]),
        ] {
            let mut value = base();
            value["provider_configs"][0]["settings"]["config"]["approved_artifact_hosts"] = hosts;
            assert!(serde_json::from_value::<ProviderTargetConfig>(value).is_err());
        }
    }
}
