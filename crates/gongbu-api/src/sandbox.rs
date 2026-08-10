//! Safe, reproducible dependency wiring for local and release-level sandbox runs.
//!
//! The sandbox changes only boundary implementations. Both mock and real modes
//! implement the same activity traits consumed by the durable workflow.

pub mod runtime;

use crate::{
    application::GenericProviderActivities,
    artifact::ArtifactLimits,
    execution::Execution,
    hubu::{
        ExecutorSpendClaimRequest, ExecutorSpendFinalizationRequest, ExecutorSpendRequest,
        HubuClient, PriceModelSnapshot, ProviderReceipt,
    },
    provider::{
        contract::{
            AdapterCapabilities, AdapterOutcome, NormalizedRequest, PricingCatalog,
            ProviderAdapter, ProviderFailure,
        },
        registry::{ProviderRegistry, ValidatedProviderCatalog},
        targets::{AdapterSettings, ProviderTargetConfig, TargetKey},
    },
    provider_contract::NormalizedUsage,
    secrets::{MacOsKeychain, ProviderSecret, SecretProvider, SecretReference},
    workflow::{
        ActivityError, ArtifactActivities, HubuActivities, ProviderActivities, ProviderArtifact,
        ProviderSuccess,
    },
};
use image::{DynamicImage, ImageOutputFormat, RgbaImage};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    env, fs,
    io::Cursor,
    net::{IpAddr, SocketAddr, TcpListener},
    path::{Path, PathBuf},
    str::FromStr,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{SystemTime, UNIX_EPOCH},
};
use tempfile::TempDir;
use thiserror::Error;

pub const LIVE_SPEND_ACKNOWLEDGEMENT: &str = "I_ACKNOWLEDGE_LIVE_PROVIDER_SPEND";

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BoundaryMode {
    Mock,
    Real,
}

impl FromStr for BoundaryMode {
    type Err = SandboxError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        parse_mode(value)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProfileKind {
    Development,
    Production,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BoundaryFault {
    #[default]
    None,
    ProvenBeforeMutation,
    CommitThenDisconnect,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProviderScenario {
    #[default]
    Success,
    ProvenRejection,
    TimeoutAmbiguous,
    MalformedResponse,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SandboxConfig {
    pub profile: ProfileKind,
    #[serde(default = "default_seed")]
    pub seed: u64,
    pub hubu: HubuConfig,
    pub provider: ProviderConfig,
    #[serde(default)]
    pub temporal: TemporalConfig,
    #[serde(default)]
    pub preserve_diagnostics_on_failure: bool,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TemporalMode {
    #[default]
    Managed,
    External,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TemporalConfig {
    #[serde(default)]
    pub mode: TemporalMode,
    #[serde(default)]
    pub address: Option<String>,
    #[serde(default)]
    pub ui_url: Option<String>,
    #[serde(default = "default_temporal_namespace")]
    pub namespace: String,
    #[serde(default = "default_temporal_binary")]
    pub binary: String,
}

impl Default for TemporalConfig {
    fn default() -> Self {
        Self {
            mode: TemporalMode::Managed,
            address: None,
            ui_url: None,
            namespace: default_temporal_namespace(),
            binary: default_temporal_binary(),
        }
    }
}

fn default_temporal_namespace() -> String {
    "default".into()
}

fn default_temporal_binary() -> String {
    "temporal".into()
}

fn default_seed() -> u64 {
    48
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HubuConfig {
    pub mode: BoundaryMode,
    #[serde(default)]
    pub endpoint: Option<String>,
    #[serde(default)]
    pub allowlisted_hosts: Vec<String>,
    #[serde(default)]
    pub scoped_credential_reference: Option<String>,
    #[serde(default)]
    pub isolated_test_account: Option<String>,
    #[serde(default = "default_agent")]
    pub agent_id: String,
    #[serde(default = "default_currency")]
    pub currency: String,
    #[serde(default = "default_authorization")]
    pub maximum_authorization_minor: i64,
    #[serde(default)]
    pub authorization_expires_at: Option<String>,
    #[serde(default)]
    pub claim_fault: BoundaryFault,
    #[serde(default)]
    pub settle_fault: BoundaryFault,
    #[serde(default)]
    pub release_fault: BoundaryFault,
}

fn default_agent() -> String {
    "agt_gongbu_sandbox".into()
}
fn default_currency() -> String {
    "USD".into()
}
fn default_authorization() -> i64 {
    1_000
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderConfig {
    pub mode: BoundaryMode,
    #[serde(default)]
    pub target: Option<ProviderSelection>,
    #[serde(default)]
    pub target_config: Option<PathBuf>,
    #[serde(default)]
    pub pricing_catalog: Option<PathBuf>,
    #[serde(default)]
    pub credential_reference: Option<String>,
    #[serde(default)]
    pub maximum_spend_minor: Option<i64>,
    #[serde(default)]
    pub live_spend_acknowledgement: Option<String>,
    #[serde(default)]
    pub scenario: ProviderScenario,
    #[serde(default)]
    pub execution_fault: BoundaryFault,
    #[serde(default)]
    pub artifact_fault: BoundaryFault,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderSelection {
    pub workload_type: String,
    pub provider: String,
    pub adapter: String,
    pub model: String,
}

impl ProviderSelection {
    fn key(&self) -> Result<TargetKey, SandboxError> {
        TargetKey::new(
            &self.workload_type,
            &self.provider,
            &self.adapter,
            &self.model,
        )
        .map_err(|e| SandboxError::Invalid(format!("invalid explicit provider target: {e}")))
    }
}

#[derive(Debug, Error)]
pub enum SandboxError {
    #[error("sandbox configuration: {0}")]
    Invalid(String),
    #[error("sandbox IO: {0}")]
    Io(#[from] std::io::Error),
    #[error("sandbox JSON: {0}")]
    Json(#[from] serde_json::Error),
}

impl SandboxConfig {
    pub fn load(path: impl AsRef<Path>) -> Result<Self, SandboxError> {
        Ok(serde_json::from_slice(&fs::read(path)?)?)
    }

    pub fn from_path(path: impl AsRef<Path>) -> Result<Self, SandboxError> {
        let mut value = Self::load(path)?;
        value.apply_environment_overrides()?;
        value.validate()?;
        Ok(value)
    }

    pub fn apply_environment_overrides(&mut self) -> Result<(), SandboxError> {
        if let Ok(mode) = env::var("GONGBU_SANDBOX_HUBU_MODE") {
            self.hubu.mode = parse_mode(&mode)?;
        }
        if let Ok(mode) = env::var("GONGBU_SANDBOX_PROVIDER_MODE") {
            self.provider.mode = parse_mode(&mode)?;
        }
        if let Ok(value) = env::var("GONGBU_SANDBOX_MAX_SPEND_MINOR") {
            self.provider.maximum_spend_minor = Some(value.parse().map_err(|_| {
                SandboxError::Invalid("GONGBU_SANDBOX_MAX_SPEND_MINOR must be an integer".into())
            })?);
        }
        if let Ok(value) = env::var("GONGBU_SANDBOX_LIVE_SPEND_ACK") {
            self.provider.live_spend_acknowledgement = Some(value);
        }
        Ok(())
    }

    pub fn validate(&self) -> Result<(), SandboxError> {
        if self.profile == ProfileKind::Production
            && (self.hubu.mode == BoundaryMode::Mock || self.provider.mode == BoundaryMode::Mock)
        {
            return invalid("production profiles cannot select mock boundaries");
        }
        if self.hubu.maximum_authorization_minor <= 0 {
            return invalid("Hubu maximum_authorization_minor must be positive");
        }
        if self.hubu.currency.trim().to_ascii_uppercase() != self.hubu.currency
            || self.hubu.currency.len() != 3
        {
            return invalid("Hubu currency must be a three-letter uppercase code");
        }
        match self.hubu.mode {
            BoundaryMode::Mock => {
                if self.hubu.endpoint.is_some()
                    || self.hubu.scoped_credential_reference.is_some()
                    || self.hubu.isolated_test_account.is_some()
                {
                    return invalid("mock Hubu cannot carry real endpoint or credential settings");
                }
            }
            BoundaryMode::Real => self.validate_real_hubu()?,
        }
        match self.provider.mode {
            BoundaryMode::Mock => {
                if self.provider.target.is_some()
                    || self.provider.target_config.is_some()
                    || self.provider.pricing_catalog.is_some()
                    || self.provider.credential_reference.is_some()
                    || self.provider.maximum_spend_minor.is_some()
                    || self.provider.live_spend_acknowledgement.is_some()
                {
                    return invalid("mock provider cannot carry live-provider settings");
                }
                if self.provider.scenario != ProviderScenario::Success
                    && self.provider.execution_fault != BoundaryFault::None
                {
                    return invalid(
                        "mock provider scenario and execution_fault cannot both select a failure",
                    );
                }
            }
            BoundaryMode::Real => self.validate_real_provider()?,
        }
        match self.temporal.mode {
            TemporalMode::Managed => {
                if self.temporal.address.is_some() || self.temporal.ui_url.is_some() {
                    return invalid("managed Temporal assigns its own address and UI URL");
                }
                if self.temporal.binary.trim().is_empty() {
                    return invalid("managed Temporal requires a CLI binary name or path");
                }
            }
            TemporalMode::External => {
                let address = self.temporal.address.as_deref().unwrap_or_default();
                if !address.starts_with("http://") || address.contains('@') {
                    return invalid("external Temporal requires an explicit safe http:// address");
                }
                let ui = self.temporal.ui_url.as_deref().unwrap_or_default();
                if !ui.starts_with("http://") || ui.contains('@') {
                    return invalid("external Temporal requires an explicit safe UI URL");
                }
            }
        }
        if self.temporal.namespace.trim().is_empty() {
            return invalid("Temporal namespace cannot be empty");
        }
        Ok(())
    }

    fn validate_real_hubu(&self) -> Result<(), SandboxError> {
        if self.hubu.claim_fault != BoundaryFault::None
            || self.hubu.settle_fault != BoundaryFault::None
            || self.hubu.release_fault != BoundaryFault::None
        {
            return invalid("real Hubu cannot select mock fault injection");
        }
        let endpoint = self
            .hubu
            .endpoint
            .as_deref()
            .ok_or_else(|| SandboxError::Invalid("real Hubu requires endpoint".into()))?;
        let (host, _) = parse_http_endpoint(endpoint)?;
        let loopback = host
            .parse::<IpAddr>()
            .map(|ip| ip.is_loopback())
            .unwrap_or(host == "localhost");
        if !loopback && !self.hubu.allowlisted_hosts.iter().any(|v| v == &host) {
            return invalid("real Hubu endpoint must be loopback or explicitly allowlisted");
        }
        require_opaque_reference(
            self.hubu.scoped_credential_reference.as_deref(),
            "real Hubu scoped credential reference",
        )?;
        parse_secret_reference(
            self.hubu
                .scoped_credential_reference
                .as_deref()
                .expect("validated reference"),
            "real Hubu scoped credential reference",
        )?;
        let account = self
            .hubu
            .isolated_test_account
            .as_deref()
            .unwrap_or_default();
        if account.trim().is_empty() || account.eq_ignore_ascii_case("production") {
            return invalid("real Hubu requires an isolated non-production test account");
        }
        Ok(())
    }

    fn validate_real_provider(&self) -> Result<(), SandboxError> {
        if self.provider.scenario != ProviderScenario::Success
            || self.provider.execution_fault != BoundaryFault::None
            || self.provider.artifact_fault != BoundaryFault::None
        {
            return invalid("real provider cannot select mock scenarios or fault injection");
        }
        let targets =
            required_absolute_file(self.provider.target_config.as_deref(), "target config")?;
        let pricing = required_absolute_file(
            self.provider.pricing_catalog.as_deref(),
            "frozen pricing catalog",
        )?;
        let targets = ProviderTargetConfig::from_path(targets)
            .map_err(|e| SandboxError::Invalid(format!("invalid provider target config: {e}")))?;
        let pricing = PricingCatalog::load(pricing)
            .map_err(|e| SandboxError::Invalid(format!("invalid frozen pricing catalog: {e}")))?;
        let key = self
            .provider
            .target
            .as_ref()
            .ok_or_else(|| {
                SandboxError::Invalid("real provider requires an explicit target".into())
            })?
            .key()?;
        let revision = targets.resolve_active(&key).map_err(|e| {
            SandboxError::Invalid(format!("explicit provider target is unavailable: {e}"))
        })?;
        if matches!(revision.settings(), AdapterSettings::Fixture) {
            return invalid("real provider target cannot select a fixture adapter");
        }
        if targets
            .revisions()
            .filter(|revision| revision.is_active() && revision.is_execution_enabled())
            .count()
            != 1
        {
            return invalid("real provider config must expose exactly one active execution target");
        }
        if !pricing.supports_target(&key) {
            return invalid("frozen pricing catalog does not cover the explicit provider target");
        }
        require_opaque_reference(
            self.provider.credential_reference.as_deref(),
            "real provider credential reference",
        )?;
        let configured_reference = parse_secret_reference(
            self.provider
                .credential_reference
                .as_deref()
                .expect("validated reference"),
            "real provider credential reference",
        )?;
        if revision.secret_reference().map_err(|_| {
            SandboxError::Invalid("provider target secret reference is invalid".into())
        })? != configured_reference
        {
            return invalid("real provider credential reference must match the explicit target");
        }
        if self.provider.maximum_spend_minor.unwrap_or_default() <= 0 {
            return invalid("real provider requires a positive maximum spend ceiling");
        }
        if self.provider.live_spend_acknowledgement.as_deref() != Some(LIVE_SPEND_ACKNOWLEDGEMENT) {
            return invalid("real provider requires the exact live-spend acknowledgement");
        }
        Ok(())
    }
}

fn parse_mode(value: &str) -> Result<BoundaryMode, SandboxError> {
    match value {
        "mock" => Ok(BoundaryMode::Mock),
        "real" => Ok(BoundaryMode::Real),
        _ => invalid("boundary mode must be exactly 'mock' or 'real'"),
    }
}

fn invalid<T>(message: &str) -> Result<T, SandboxError> {
    Err(SandboxError::Invalid(message.into()))
}

fn required_absolute_file<'a>(
    value: Option<&'a Path>,
    name: &str,
) -> Result<&'a Path, SandboxError> {
    let path =
        value.ok_or_else(|| SandboxError::Invalid(format!("real provider requires {name}")))?;
    if !path.is_absolute() || !path.is_file() {
        return Err(SandboxError::Invalid(format!(
            "real provider {name} must be an existing absolute file"
        )));
    }
    Ok(path)
}

fn require_opaque_reference(value: Option<&str>, name: &str) -> Result<(), SandboxError> {
    let value = value.unwrap_or_default().trim();
    if value.is_empty()
        || value.len() > 255
        || value.to_ascii_lowercase().starts_with("bearer ")
        || value.starts_with("eyJ")
        || value.contains(char::is_whitespace)
    {
        return Err(SandboxError::Invalid(format!("{name} must be opaque")));
    }
    Ok(())
}

fn parse_secret_reference(value: &str, name: &str) -> Result<SecretReference, SandboxError> {
    let (service, account) = value
        .split_once(':')
        .ok_or_else(|| SandboxError::Invalid(format!("{name} must use service:account format")))?;
    if account.contains(':') {
        return Err(SandboxError::Invalid(format!(
            "{name} must use service:account format"
        )));
    }
    SecretReference::new(service, account)
        .map_err(|_| SandboxError::Invalid(format!("{name} is invalid")))
}

fn parse_http_endpoint(endpoint: &str) -> Result<(String, u16), SandboxError> {
    let rest = endpoint.strip_prefix("http://").ok_or_else(|| {
        SandboxError::Invalid("real Hubu endpoint must use explicit http://".into())
    })?;
    let authority = rest.split('/').next().unwrap_or_default();
    if authority.is_empty() || authority.contains('@') {
        return invalid("real Hubu endpoint authority is invalid");
    }
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) => (
            host,
            port.parse::<u16>()
                .map_err(|_| SandboxError::Invalid("real Hubu endpoint port is invalid".into()))?,
        ),
        None => (authority, 80),
    };
    if host.is_empty() {
        return invalid("real Hubu endpoint host is invalid");
    }
    Ok((host.into(), port))
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ReadinessCheck {
    pub component: String,
    pub ready: bool,
    pub detail: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RunManifest {
    pub schema_version: u32,
    pub run_id: String,
    pub profile: ProfileKind,
    pub hubu_mode: BoundaryMode,
    pub provider_mode: BoundaryMode,
    pub seed: u64,
    pub config_digest: String,
    pub hubu_endpoint: String,
    pub provider_target_digest: String,
    pub pricing_digest: String,
    pub commit_sha: String,
    pub process_version: String,
    pub maximum_spend_minor: Option<i64>,
    pub gongbu_url: String,
    pub temporal_address: String,
    pub temporal_ui_url: String,
    pub temporal_namespace: String,
    pub temporal_task_queue: String,
    pub provider_target: ProviderSelection,
    pub authorization_currency: String,
    pub authorization_amount_minor: i64,
    pub database_path: String,
    pub artifact_root: String,
    pub workflow_root: String,
    pub ports: BTreeMap<String, u16>,
    pub readiness: Vec<ReadinessCheck>,
}

pub struct SandboxRun {
    root: Option<TempDir>,
    manifest_path: PathBuf,
    manifest: RunManifest,
    reserved_ports: BTreeMap<String, TcpListener>,
}

impl SandboxRun {
    pub fn start(config: &SandboxConfig) -> Result<Self, SandboxError> {
        Self::start_with_secrets(config, &MacOsKeychain)
    }

    pub fn start_with_secrets(
        config: &SandboxConfig,
        secrets: &dyn SecretProvider,
    ) -> Result<Self, SandboxError> {
        Self::start_inner(config, secrets, None)
    }

    fn start_inner(
        config: &SandboxConfig,
        secrets: &dyn SecretProvider,
        assigned_ports: Option<[u16; 3]>,
    ) -> Result<Self, SandboxError> {
        config.validate()?;
        probe_credentials(config, secrets)?;
        let root = tempfile::Builder::new()
            .prefix("gongbu-sandbox-")
            .tempdir()?;
        let database_path = root.path().join("gongbu.sqlite3");
        let artifact_root = root.path().join("artifacts");
        let workflow_root = root.path().join("workflow");
        let log_root = root.path().join("logs");
        fs::create_dir_all(&artifact_root)?;
        fs::create_dir_all(&workflow_root)?;
        fs::create_dir_all(&log_root)?;
        fs::write(&database_path, [])?;
        let (listeners, selected_ports) = if let Some(ports) = assigned_ports {
            (BTreeMap::new(), ports)
        } else {
            let listeners = BTreeMap::from([
                ("gongbu".into(), reserve_loopback_port()?),
                ("temporal".into(), reserve_loopback_port()?),
                ("temporal_ui".into(), reserve_loopback_port()?),
            ]);
            let ports = [
                listeners["gongbu"].local_addr()?.port(),
                listeners["temporal"].local_addr()?.port(),
                listeners["temporal_ui"].local_addr()?.port(),
            ];
            (listeners, ports)
        };
        let ports = BTreeMap::from([
            ("gongbu".into(), selected_ports[0]),
            ("temporal".into(), selected_ports[1]),
            ("temporal_ui".into(), selected_ports[2]),
        ]);
        let run_id = format!("sandbox-{}-{}", config.seed, unix_seconds());
        let manifest = RunManifest {
            schema_version: 1,
            run_id,
            profile: config.profile,
            hubu_mode: config.hubu.mode,
            provider_mode: config.provider.mode,
            seed: config.seed,
            config_digest: format!("sha256:{:x}", Sha256::digest(serde_json::to_vec(config)?)),
            hubu_endpoint: config
                .hubu
                .endpoint
                .as_deref()
                .map(redact_endpoint)
                .unwrap_or_else(|| "in-process://mock-hubu".into()),
            provider_target_digest: file_or_mock_digest(
                config.provider.target_config.as_deref(),
                b"gongbu-sandbox-mock-provider-v1",
            )?,
            pricing_digest: file_or_mock_digest(
                config.provider.pricing_catalog.as_deref(),
                b"gongbu-sandbox-mock-pricing-v1",
            )?,
            commit_sha: option_env!("GONGBU_COMMIT_SHA").unwrap_or("unknown").into(),
            process_version: env!("CARGO_PKG_VERSION").into(),
            maximum_spend_minor: config.provider.maximum_spend_minor,
            gongbu_url: format!("http://127.0.0.1:{}", ports["gongbu"]),
            temporal_address: config
                .temporal
                .address
                .clone()
                .unwrap_or_else(|| format!("http://127.0.0.1:{}", ports["temporal"])),
            temporal_ui_url: config
                .temporal
                .ui_url
                .clone()
                .unwrap_or_else(|| format!("http://127.0.0.1:{}", ports["temporal_ui"])),
            temporal_namespace: config.temporal.namespace.clone(),
            temporal_task_queue: crate::temporal::EXECUTION_TASK_QUEUE.into(),
            provider_target: config
                .provider
                .target
                .clone()
                .unwrap_or_else(mock_provider_selection),
            authorization_currency: config.hubu.currency.clone(),
            authorization_amount_minor: config
                .provider
                .maximum_spend_minor
                .unwrap_or(config.hubu.maximum_authorization_minor.min(100)),
            database_path: database_path.display().to_string(),
            artifact_root: artifact_root.display().to_string(),
            workflow_root: workflow_root.display().to_string(),
            ports,
            readiness: readiness(config),
        };
        let manifest_path = root.path().join("run-manifest.json");
        fs::write(&manifest_path, serde_json::to_vec_pretty(&manifest)?)?;
        Ok(Self {
            root: Some(root),
            manifest_path,
            manifest,
            reserved_ports: listeners,
        })
    }

    #[cfg(test)]
    fn start_for_test(config: &SandboxConfig) -> Result<Self, SandboxError> {
        Self::start_inner(config, &MacOsKeychain, Some([45100, 45101, 45102]))
    }

    pub fn root(&self) -> &Path {
        self.root.as_ref().expect("run root retained").path()
    }
    pub fn manifest_path(&self) -> &Path {
        &self.manifest_path
    }
    pub fn manifest(&self) -> &RunManifest {
        &self.manifest
    }

    pub fn take_listener(&mut self, name: &str) -> Result<TcpListener, SandboxError> {
        self.reserved_ports
            .remove(name)
            .ok_or_else(|| SandboxError::Invalid(format!("sandbox port is not reserved: {name}")))
    }

    pub fn release_listener(&mut self, name: &str) {
        self.reserved_ports.remove(name);
    }

    pub fn rewrite_manifest(&self) -> Result<(), SandboxError> {
        fs::write(
            &self.manifest_path,
            serde_json::to_vec_pretty(&self.manifest)?,
        )?;
        Ok(())
    }

    pub fn mark_ready(&mut self, component: &str, detail: &str) -> Result<(), SandboxError> {
        if let Some(check) = self
            .manifest
            .readiness
            .iter_mut()
            .find(|check| check.component == component)
        {
            check.ready = true;
            check.detail = detail.into();
        } else {
            self.manifest.readiness.push(ReadinessCheck {
                component: component.into(),
                ready: true,
                detail: detail.into(),
            });
        }
        self.rewrite_manifest()
    }

    pub fn preserve(mut self, destination: impl AsRef<Path>) -> Result<PathBuf, SandboxError> {
        let destination = destination.as_ref();
        if destination.exists() {
            return invalid("diagnostics destination already exists");
        }
        let root = self.root.take().expect("run root retained");
        let source = root.keep();
        fs::rename(&source, destination)?;
        self.manifest.database_path = destination.join("gongbu.sqlite3").display().to_string();
        self.manifest.artifact_root = destination.join("artifacts").display().to_string();
        self.manifest.workflow_root = destination.join("workflow").display().to_string();
        self.manifest_path = destination.join("run-manifest.json");
        fs::write(
            &self.manifest_path,
            serde_json::to_vec_pretty(&self.manifest)?,
        )?;
        Ok(destination.to_path_buf())
    }

    pub fn preserve_in_place(mut self) -> Result<PathBuf, SandboxError> {
        let root = self.root.take().expect("run root retained");
        let destination = root.keep();
        self.manifest_path = destination.join("run-manifest.json");
        self.rewrite_manifest()?;
        Ok(destination)
    }
}

fn probe_credentials(
    config: &SandboxConfig,
    secrets: &dyn SecretProvider,
) -> Result<(), SandboxError> {
    if config.hubu.mode == BoundaryMode::Real {
        let reference = parse_secret_reference(
            config
                .hubu
                .scoped_credential_reference
                .as_deref()
                .expect("validated Hubu reference"),
            "real Hubu scoped credential reference",
        )?;
        secrets.resolve(&reference).map_err(|_| {
            SandboxError::Invalid("real Hubu scoped credential is unavailable".into())
        })?;
    }
    if config.provider.mode == BoundaryMode::Real {
        let reference = parse_secret_reference(
            config
                .provider
                .credential_reference
                .as_deref()
                .expect("validated provider reference"),
            "real provider credential reference",
        )?;
        secrets
            .resolve(&reference)
            .map_err(|_| SandboxError::Invalid("real provider credential is unavailable".into()))?;
    }
    Ok(())
}

fn readiness(config: &SandboxConfig) -> Vec<ReadinessCheck> {
    vec![
        ready("hubu", format!("{:?} boundary validated", config.hubu.mode)),
        ready(
            "provider",
            format!("{:?} boundary validated", config.provider.mode),
        ),
        ready(
            "storage",
            "isolated database and artifact roots created".into(),
        ),
        ready(
            "credentials",
            if config.hubu.mode == BoundaryMode::Real || config.provider.mode == BoundaryMode::Real
            {
                "opaque credential references present"
            } else {
                "not required for mock boundaries"
            }
            .into(),
        ),
        ready(
            "pricing",
            if config.provider.mode == BoundaryMode::Real {
                "frozen pricing catalog validated"
            } else {
                "deterministic mock pricing selected"
            }
            .into(),
        ),
    ]
}

fn ready(component: &str, detail: String) -> ReadinessCheck {
    ReadinessCheck {
        component: component.into(),
        ready: true,
        detail,
    }
}

fn reserve_loopback_port() -> Result<TcpListener, std::io::Error> {
    TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0)))
}

fn mock_provider_selection() -> ProviderSelection {
    ProviderSelection {
        workload_type: "image_generation".into(),
        provider: "sandbox".into(),
        adapter: "fixture".into(),
        model: "deterministic-image-v1".into(),
    }
}

fn redact_endpoint(endpoint: &str) -> String {
    endpoint
        .split_once('?')
        .map(|(base, _)| base)
        .unwrap_or(endpoint)
        .to_string()
}

fn file_or_mock_digest(path: Option<&Path>, mock: &[u8]) -> Result<String, std::io::Error> {
    let bytes = match path {
        Some(path) => fs::read(path)?,
        None => mock.to_vec(),
    };
    Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
}

fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct SafeHubuCall {
    pub operation: String,
    pub operation_key_digest: String,
    pub outcome: String,
}

#[derive(Clone, Debug)]
struct MockClaim {
    claim_id: String,
    account_id: String,
    amount_minor: i64,
    currency: String,
    status: ClaimStatus,
    settlement_id: Option<String>,
    settlement_amount_minor: Option<i64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ClaimStatus {
    Active,
    Settled,
    Released,
}

#[derive(Default)]
struct MockHubuState {
    claims: BTreeMap<String, MockClaim>,
    calls: Vec<SafeHubuCall>,
    financial_mutations: usize,
}

pub struct MockHubu {
    config: HubuConfig,
    seed: u64,
    state: Mutex<MockHubuState>,
    expired: AtomicBool,
}

impl MockHubu {
    pub fn new(config: HubuConfig, seed: u64) -> Result<Self, SandboxError> {
        if config.mode != BoundaryMode::Mock {
            return invalid("MockHubu requires mock Hubu configuration");
        }
        Ok(Self {
            config,
            seed,
            state: Mutex::new(MockHubuState::default()),
            expired: AtomicBool::new(false),
        })
    }

    pub fn safe_calls(&self) -> Vec<SafeHubuCall> {
        self.state.lock().expect("mock Hubu state").calls.clone()
    }
    pub fn financial_mutations(&self) -> usize {
        self.state
            .lock()
            .expect("mock Hubu state")
            .financial_mutations
    }
    pub fn expire_authorizations(&self) {
        self.expired.store(true, Ordering::SeqCst);
    }

    fn validate_execution(&self, execution: &Execution) -> Result<(), ActivityError> {
        if execution.account_id.trim().is_empty()
            || execution.authorization_currency != self.config.currency
            || execution.authorized_minor <= 0
            || execution.authorized_minor > self.config.maximum_authorization_minor
        {
            return Err(ActivityError::Proven(
                "mock_hubu_authorization_invalid".into(),
            ));
        }
        if self.expired.load(Ordering::SeqCst)
            || self
                .config
                .authorization_expires_at
                .as_deref()
                .is_some_and(|expiry| execution.created_at.as_str() >= expiry)
        {
            return Err(ActivityError::Proven(
                "mock_hubu_authorization_expired".into(),
            ));
        }
        Ok(())
    }

    fn record(state: &mut MockHubuState, operation: &str, key: &str, outcome: &str) {
        state.calls.push(SafeHubuCall {
            operation: operation.into(),
            operation_key_digest: format!("sha256:{:x}", Sha256::digest(key.as_bytes())),
            outcome: outcome.into(),
        });
    }
}

impl HubuActivities for MockHubu {
    fn preflight(&self, execution: &Execution) -> Result<(), ActivityError> {
        self.validate_execution(execution)
    }

    fn claim(&self, execution: &Execution) -> Result<String, ActivityError> {
        self.validate_execution(execution)?;
        let mut state = self.state.lock().expect("mock Hubu state");
        if let Some(claim) = state.claims.get(&execution.operation_key) {
            if claim.account_id != execution.account_id
                || claim.amount_minor != execution.authorized_minor
                || claim.currency != execution.authorization_currency
            {
                Self::record(
                    &mut state,
                    "claim",
                    &execution.operation_key,
                    "idempotency_conflict",
                );
                return Err(ActivityError::Proven(
                    "mock_hubu_idempotency_conflict".into(),
                ));
            }
            let id = claim.claim_id.clone();
            Self::record(&mut state, "claim", &execution.operation_key, "idempotent");
            return Ok(id);
        }
        if self.config.claim_fault == BoundaryFault::ProvenBeforeMutation {
            Self::record(
                &mut state,
                "claim",
                &execution.operation_key,
                "proven_failure",
            );
            return Err(ActivityError::Proven("mock_hubu_claim_rejected".into()));
        }
        let claim_id = deterministic_id("claim", self.seed, &execution.operation_key);
        state.claims.insert(
            execution.operation_key.clone(),
            MockClaim {
                claim_id: claim_id.clone(),
                account_id: execution.account_id.clone(),
                amount_minor: execution.authorized_minor,
                currency: execution.authorization_currency.clone(),
                status: ClaimStatus::Active,
                settlement_id: None,
                settlement_amount_minor: None,
            },
        );
        state.financial_mutations += 1;
        let disconnect = self.config.claim_fault == BoundaryFault::CommitThenDisconnect;
        Self::record(
            &mut state,
            "claim",
            &execution.operation_key,
            if disconnect {
                "committed_disconnect"
            } else {
                "claimed"
            },
        );
        if disconnect {
            Err(ActivityError::Ambiguous(
                "mock_hubu_claim_disconnect".into(),
            ))
        } else {
            Ok(claim_id)
        }
    }

    fn validate_claim(&self, execution: &Execution) -> Result<(), ActivityError> {
        self.validate_execution(execution)?;
        let state = self.state.lock().expect("mock Hubu state");
        let claim = state
            .claims
            .get(&execution.operation_key)
            .ok_or_else(|| ActivityError::Proven("mock_hubu_claim_missing".into()))?;
        if claim.status != ClaimStatus::Active
            || claim.account_id != execution.account_id
            || claim.amount_minor != execution.authorized_minor
            || claim.currency != execution.authorization_currency
            || execution.hubu_claim_id.as_deref() != Some(claim.claim_id.as_str())
        {
            return Err(ActivityError::Proven("mock_hubu_claim_invalid".into()));
        }
        Ok(())
    }

    fn settle(
        &self,
        execution: &Execution,
        _receipt_id: &str,
        amount_minor: i64,
    ) -> Result<String, ActivityError> {
        if amount_minor < 0 || amount_minor > execution.authorized_minor {
            return Err(ActivityError::Proven(
                "mock_hubu_settlement_out_of_bounds".into(),
            ));
        }
        let mut state = self.state.lock().expect("mock Hubu state");
        if self.config.settle_fault == BoundaryFault::ProvenBeforeMutation {
            Self::record(
                &mut state,
                "settle",
                &execution.operation_key,
                "proven_failure",
            );
            return Err(ActivityError::Proven(
                "mock_hubu_settlement_rejected".into(),
            ));
        }
        let existing = state
            .claims
            .get(&execution.operation_key)
            .ok_or_else(|| ActivityError::Proven("mock_hubu_claim_missing".into()))?;
        if existing.status == ClaimStatus::Settled {
            if existing.settlement_amount_minor != Some(amount_minor) {
                return Err(ActivityError::Proven(
                    "mock_hubu_idempotency_conflict".into(),
                ));
            }
            let id = existing.settlement_id.clone().expect("settlement id");
            Self::record(&mut state, "settle", &execution.operation_key, "idempotent");
            return Ok(id);
        }
        if existing.status != ClaimStatus::Active {
            return Err(ActivityError::Proven("mock_hubu_claim_not_active".into()));
        }
        let settlement_id = deterministic_id("settlement", self.seed, &execution.operation_key);
        let claim = state
            .claims
            .get_mut(&execution.operation_key)
            .expect("claim");
        claim.status = ClaimStatus::Settled;
        claim.settlement_id = Some(settlement_id.clone());
        claim.settlement_amount_minor = Some(amount_minor);
        state.financial_mutations += 1;
        let disconnect = self.config.settle_fault == BoundaryFault::CommitThenDisconnect;
        Self::record(
            &mut state,
            "settle",
            &execution.operation_key,
            if disconnect {
                "committed_disconnect"
            } else {
                "settled"
            },
        );
        if disconnect {
            Err(ActivityError::Ambiguous(
                "mock_hubu_settlement_disconnect".into(),
            ))
        } else {
            Ok(settlement_id)
        }
    }

    fn release(&self, execution: &Execution) -> Result<(), ActivityError> {
        let mut state = self.state.lock().expect("mock Hubu state");
        if self.config.release_fault == BoundaryFault::ProvenBeforeMutation {
            Self::record(
                &mut state,
                "release",
                &execution.operation_key,
                "proven_failure",
            );
            return Err(ActivityError::Proven("mock_hubu_release_rejected".into()));
        }
        let claim = state
            .claims
            .get_mut(&execution.operation_key)
            .ok_or_else(|| ActivityError::Proven("mock_hubu_claim_missing".into()))?;
        if claim.status == ClaimStatus::Released {
            Self::record(
                &mut state,
                "release",
                &execution.operation_key,
                "idempotent",
            );
            return Ok(());
        }
        if claim.status != ClaimStatus::Active {
            return Err(ActivityError::Proven("mock_hubu_claim_not_active".into()));
        }
        claim.status = ClaimStatus::Released;
        state.financial_mutations += 1;
        let disconnect = self.config.release_fault == BoundaryFault::CommitThenDisconnect;
        Self::record(
            &mut state,
            "release",
            &execution.operation_key,
            if disconnect {
                "committed_disconnect"
            } else {
                "released"
            },
        );
        if disconnect {
            Err(ActivityError::Ambiguous(
                "mock_hubu_release_disconnect".into(),
            ))
        } else {
            Ok(())
        }
    }
}

fn deterministic_id(kind: &str, seed: u64, key: &str) -> String {
    let digest = Sha256::digest(format!("{kind}:{seed}:{key}").as_bytes());
    let short = digest[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!("{kind}-{short}")
}

pub struct RealHubuActivities {
    client: HubuClient,
    agent_id: String,
}

impl RealHubuActivities {
    pub fn new(config: &HubuConfig) -> Result<Self, SandboxError> {
        if config.mode != BoundaryMode::Real {
            return invalid("RealHubuActivities requires real Hubu configuration");
        }
        Ok(Self {
            client: HubuClient::new(config.endpoint.clone().expect("validated endpoint")),
            agent_id: config.agent_id.clone(),
        })
    }

    fn spend(&self, execution: &Execution) -> ExecutorSpendRequest {
        ExecutorSpendRequest {
            spend_auth_token_id: execution.hubu_token_reference.as_str().into(),
            agent_id: Some(self.agent_id.clone()),
            account_id: Some(execution.account_id.clone()),
            amount_cents: execution.authorized_minor,
            merchant: Some("gongbu.sandbox".into()),
            task_id: Some(execution.execution_id.clone()),
        }
    }
}

impl HubuActivities for RealHubuActivities {
    fn preflight(&self, execution: &Execution) -> Result<(), ActivityError> {
        self.client
            .validate(&self.spend(execution))
            .map(|_| ())
            .map_err(map_hubu_error)
    }
    fn claim(&self, execution: &Execution) -> Result<String, ActivityError> {
        self.client
            .claim(&ExecutorSpendClaimRequest {
                operation_key: execution.operation_key.clone(),
                spend: self.spend(execution),
            })
            .map(|claim| claim.claim_id)
            .map_err(map_hubu_error)
    }
    fn validate_claim(&self, execution: &Execution) -> Result<(), ActivityError> {
        let id = execution
            .hubu_claim_id
            .as_deref()
            .ok_or_else(|| ActivityError::Proven("hubu_claim_missing".into()))?;
        let claim = self.client.inspect_claim(id).map_err(map_hubu_error)?;
        if claim.status == "active" && claim.operation_key == execution.operation_key {
            Ok(())
        } else {
            Err(ActivityError::Proven("hubu_claim_not_active".into()))
        }
    }
    fn settle(
        &self,
        execution: &Execution,
        receipt_id: &str,
        amount_minor: i64,
    ) -> Result<String, ActivityError> {
        let snapshot: crate::provider_contract::PricingSnapshot =
            serde_json::from_value(execution.pricing_snapshot.clone())
                .map_err(|_| ActivityError::Proven("pricing_snapshot_invalid".into()))?;
        self.client
            .settle(&ExecutorSpendFinalizationRequest {
                agent_id: self.agent_id.clone(),
                operation_key: execution.operation_key.clone(),
                receipt: Some(ProviderReceipt {
                    actual_vendor_cost_cents: amount_minor,
                    provider_request_id: receipt_id.into(),
                    price_model_snapshot: PriceModelSnapshot {
                        provider: execution.provider.clone(),
                        model: execution.model.clone(),
                        unit_price_cents: snapshot.estimated_amount_minor,
                        pricing_unit: "execution".into(),
                        currency: execution.authorization_currency.clone(),
                    },
                    artifact_reference: format!("gongbu://execution/{}", execution.execution_id),
                }),
            })
            .map(|settlement| settlement.settlement_id)
            .map_err(map_hubu_error)
    }
    fn release(&self, execution: &Execution) -> Result<(), ActivityError> {
        self.client
            .release(&ExecutorSpendFinalizationRequest {
                agent_id: self.agent_id.clone(),
                operation_key: execution.operation_key.clone(),
                receipt: None,
            })
            .map(|_| ())
            .map_err(map_hubu_error)
    }
}

fn map_hubu_error(error: crate::hubu::HttpClientError) -> ActivityError {
    match error {
        crate::hubu::HttpClientError::Status { status, .. } if (400..500).contains(&status) => {
            ActivityError::Proven("hubu_request_rejected".into())
        }
        _ => ActivityError::Ambiguous("hubu_transport_ambiguous".into()),
    }
}

pub struct DeterministicProvider {
    scenario: ProviderScenario,
    fault: BoundaryFault,
    calls: Mutex<BTreeMap<String, Result<ProviderSuccess, ActivityError>>>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct SafeProviderCall {
    pub attempt_id_digest: String,
    pub outcome: String,
}

pub struct FaultInjectingArtifacts {
    delegate: Arc<dyn ArtifactActivities + Send + Sync>,
    fault: BoundaryFault,
    completed_attempts: Mutex<BTreeMap<String, Result<(), ActivityError>>>,
}

impl FaultInjectingArtifacts {
    pub fn new(delegate: Arc<dyn ArtifactActivities + Send + Sync>, fault: BoundaryFault) -> Self {
        Self {
            delegate,
            fault,
            completed_attempts: Mutex::new(BTreeMap::new()),
        }
    }
}

impl ArtifactActivities for FaultInjectingArtifacts {
    fn preflight(&self) -> Result<(), ActivityError> {
        self.delegate.preflight()
    }

    fn persist(
        &self,
        execution: &Execution,
        attempt_id: &str,
        artifacts: &[ProviderArtifact],
    ) -> Result<(), ActivityError> {
        let mut completed = self.completed_attempts.lock().expect("artifact attempts");
        if let Some(outcome) = completed.get(attempt_id) {
            return outcome.clone();
        }
        let outcome = match self.fault {
            BoundaryFault::None => self.delegate.persist(execution, attempt_id, artifacts),
            BoundaryFault::ProvenBeforeMutation => Err(ActivityError::Proven(
                "mock_artifact_persistence_failed".into(),
            )),
            BoundaryFault::CommitThenDisconnect => self
                .delegate
                .persist(execution, attempt_id, artifacts)
                .and_then(|_| {
                    Err(ActivityError::Ambiguous(
                        "mock_artifact_persistence_disconnect".into(),
                    ))
                }),
        };
        completed.insert(attempt_id.into(), outcome.clone());
        outcome
    }
}

impl DeterministicProvider {
    pub fn new(config: &ProviderConfig) -> Result<Self, SandboxError> {
        if config.mode != BoundaryMode::Mock {
            return invalid("DeterministicProvider requires mock provider configuration");
        }
        Ok(Self {
            scenario: config.scenario,
            fault: config.execution_fault,
            calls: Mutex::new(BTreeMap::new()),
        })
    }
    pub fn invocation_count(&self) -> usize {
        self.calls.lock().expect("provider calls").len()
    }
    pub fn safe_calls(&self) -> Vec<SafeProviderCall> {
        self.calls
            .lock()
            .expect("provider calls")
            .iter()
            .map(|(attempt_id, outcome)| SafeProviderCall {
                attempt_id_digest: format!("sha256:{:x}", Sha256::digest(attempt_id.as_bytes())),
                outcome: match outcome {
                    Ok(_) => "succeeded",
                    Err(ActivityError::Proven(_) | ActivityError::ProvenWithEvidence { .. }) => {
                        "proven_failure"
                    }
                    Err(
                        ActivityError::Ambiguous(_) | ActivityError::AmbiguousWithEvidence { .. },
                    ) => "ambiguous",
                }
                .into(),
            })
            .collect()
    }
    fn outcome(&self, attempt_id: &str) -> Result<ProviderSuccess, ActivityError> {
        if self.fault == BoundaryFault::ProvenBeforeMutation
            || self.scenario == ProviderScenario::ProvenRejection
        {
            return Err(ActivityError::Proven("mock_provider_rejected".into()));
        }
        if self.fault == BoundaryFault::CommitThenDisconnect
            || self.scenario == ProviderScenario::TimeoutAmbiguous
        {
            return Err(ActivityError::AmbiguousWithEvidence {
                code: "mock_provider_delivery_ambiguous".into(),
                request_id: Some(deterministic_id("request", 0, attempt_id)),
                operation_id: None,
            });
        }
        if self.scenario == ProviderScenario::MalformedResponse {
            return Err(ActivityError::Ambiguous(
                "mock_provider_malformed_response".into(),
            ));
        }
        Ok(ProviderSuccess {
            request_id: Some(deterministic_id("request", 0, attempt_id)),
            operation_id: None,
            usage: NormalizedUsage {
                images: Some(1),
                ..Default::default()
            },
            provider_amount_minor: None,
            provider_currency: None,
            artifacts: vec![ProviderArtifact {
                media_type: "image/png".into(),
                bytes: fixture_png(),
            }],
        })
    }
}

impl ProviderActivities for DeterministicProvider {
    fn preflight(&self, _: &Execution) -> Result<(), ActivityError> {
        Ok(())
    }
    fn invoke(&self, _: &Execution, attempt_id: &str) -> Result<ProviderSuccess, ActivityError> {
        let mut calls = self.calls.lock().expect("provider calls");
        if let Some(outcome) = calls.get(attempt_id) {
            return outcome.clone();
        }
        let outcome = self.outcome(attempt_id);
        calls.insert(attempt_id.into(), outcome.clone());
        outcome
    }
}

fn fixture_png() -> Vec<u8> {
    let mut bytes = Vec::new();
    DynamicImage::ImageRgba8(RgbaImage::from_pixel(1, 1, image::Rgba([1, 2, 3, 255])))
        .write_to(&mut Cursor::new(&mut bytes), ImageOutputFormat::Png)
        .expect("in-memory PNG encoding");
    bytes
}

pub enum HubuWiring {
    Mock(Arc<MockHubu>),
    Real(Arc<RealHubuActivities>),
}

impl HubuWiring {
    pub fn activities(&self) -> Arc<dyn HubuActivities + Send + Sync> {
        match self {
            Self::Mock(value) => value.clone(),
            Self::Real(value) => value.clone(),
        }
    }
}

pub enum ProviderWiring {
    Mock(Arc<DeterministicProvider>),
    Real {
        catalog: ValidatedProviderCatalog,
        maximum_spend_minor: i64,
    },
}

impl ProviderWiring {
    pub fn activities(
        &self,
        secrets: Arc<dyn SecretProvider>,
    ) -> Arc<dyn ProviderActivities + Send + Sync> {
        match self {
            Self::Mock(value) => value.clone(),
            Self::Real {
                catalog,
                maximum_spend_minor,
            } => Arc::new(SpendCappedProvider {
                delegate: GenericProviderActivities::new(catalog.clone(), secrets),
                maximum_spend_minor: *maximum_spend_minor,
            }),
        }
    }
}

struct SpendCappedProvider {
    delegate: GenericProviderActivities,
    maximum_spend_minor: i64,
}

impl SpendCappedProvider {
    fn enforce_ceiling(&self, execution: &Execution) -> Result<(), ActivityError> {
        let snapshot: crate::provider_contract::PricingSnapshot =
            serde_json::from_value(execution.pricing_snapshot.clone())
                .map_err(|_| ActivityError::Proven("pricing_snapshot_invalid".into()))?;
        if execution.authorized_minor <= 0
            || execution.authorized_minor > self.maximum_spend_minor
            || snapshot.estimated_amount_minor > self.maximum_spend_minor
        {
            return Err(ActivityError::Proven(
                "sandbox_live_spend_ceiling_exceeded".into(),
            ));
        }
        Ok(())
    }
}

impl ProviderActivities for SpendCappedProvider {
    fn preflight(&self, execution: &Execution) -> Result<(), ActivityError> {
        self.enforce_ceiling(execution)?;
        self.delegate.preflight(execution)
    }

    fn invoke(
        &self,
        execution: &Execution,
        attempt_id: &str,
    ) -> Result<ProviderSuccess, ActivityError> {
        self.enforce_ceiling(execution)?;
        self.delegate.invoke(execution, attempt_id)
    }
}

pub struct SandboxWiring {
    pub hubu: HubuWiring,
    pub provider: ProviderWiring,
    pub providers: ValidatedProviderCatalog,
    artifact_fault: BoundaryFault,
}

impl SandboxWiring {
    pub fn artifact_activities(
        &self,
        delegate: Arc<dyn ArtifactActivities + Send + Sync>,
    ) -> Arc<dyn ArtifactActivities + Send + Sync> {
        Arc::new(FaultInjectingArtifacts::new(delegate, self.artifact_fault))
    }

    pub fn from_config(config: &SandboxConfig) -> Result<Self, SandboxError> {
        config.validate()?;
        let hubu = match config.hubu.mode {
            BoundaryMode::Mock => {
                HubuWiring::Mock(Arc::new(MockHubu::new(config.hubu.clone(), config.seed)?))
            }
            BoundaryMode::Real => {
                HubuWiring::Real(Arc::new(RealHubuActivities::new(&config.hubu)?))
            }
        };
        let (provider, providers) = match config.provider.mode {
            BoundaryMode::Mock => {
                let providers = mock_provider_catalog()?;
                (
                    ProviderWiring::Mock(Arc::new(DeterministicProvider::new(&config.provider)?)),
                    providers,
                )
            }
            BoundaryMode::Real => {
                let targets = ProviderTargetConfig::from_path(
                    config
                        .provider
                        .target_config
                        .as_deref()
                        .expect("validated target config"),
                )
                .map_err(|e| SandboxError::Invalid(format!("invalid provider targets: {e}")))?;
                let pricing = PricingCatalog::load(
                    config
                        .provider
                        .pricing_catalog
                        .as_deref()
                        .expect("validated pricing catalog"),
                )
                .map_err(|e| SandboxError::Invalid(format!("invalid provider pricing: {e}")))?;
                let catalog = ValidatedProviderCatalog::bind(
                    targets,
                    pricing,
                    &ProviderRegistry::production(&ArtifactLimits::default()),
                )
                .map_err(|e| SandboxError::Invalid(format!("real provider wiring failed: {e}")))?;
                (
                    ProviderWiring::Real {
                        catalog: catalog.clone(),
                        maximum_spend_minor: config
                            .provider
                            .maximum_spend_minor
                            .expect("validated spend ceiling"),
                    },
                    catalog,
                )
            }
        };
        Ok(Self {
            hubu,
            provider,
            providers,
            artifact_fault: config.provider.artifact_fault,
        })
    }
}

struct CatalogFixtureAdapter;

impl ProviderAdapter for CatalogFixtureAdapter {
    fn adapter_id(&self) -> &str {
        "fixture"
    }

    fn capabilities(&self) -> AdapterCapabilities {
        AdapterCapabilities {
            vendor_enforced_idempotency: false,
        }
    }

    fn invoke(
        &self,
        _: &NormalizedRequest,
        _: &serde_json::Value,
        _: &ProviderSecret,
        _: Option<&str>,
    ) -> Result<AdapterOutcome, ProviderFailure> {
        unreachable!("sandbox mock provider activities bypass catalog adapters")
    }
}

fn mock_provider_catalog() -> Result<ValidatedProviderCatalog, SandboxError> {
    let targets: ProviderTargetConfig = serde_json::from_value(serde_json::json!({
        "schema_version": 2,
        "provider_configs": [{
            "provider_config_version": "sandbox-fixture-v1",
            "workload_type": "image_generation",
            "provider": "sandbox",
            "adapter": "fixture",
            "model": "deterministic-image-v1",
            "secret_service": "gongbu.sandbox",
            "secret_account": "mock",
            "active": true,
            "execution_enabled": true,
            "settings": {"type": "fixture"}
        }]
    }))
    .map_err(|error| SandboxError::Invalid(format!("mock provider targets: {error}")))?;
    let pricing = PricingCatalog::from_json(
        br#"{
          "schema_version":1,
          "catalog_version":"sandbox-mock-v1",
          "rules":[{
            "rule_id":"sandbox-image",
            "provider":"sandbox",
            "model":"deterministic-image-v1",
            "currency":"USD",
            "unit":"image",
            "unit_amount_minor":1
          }]
        }"#,
    )
    .map_err(|error| SandboxError::Invalid(format!("mock provider pricing: {error}")))?;
    let mut registry = ProviderRegistry::new();
    registry.register("sandbox", "fixture", |_| {
        Ok(Arc::new(CatalogFixtureAdapter))
    });
    ValidatedProviderCatalog::bind(targets, pricing, &registry)
        .map_err(|error| SandboxError::Invalid(format!("mock provider wiring: {error}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        application::ArtifactServiceActivities,
        artifact::{ArtifactLimits, ArtifactService, LocalFsStorage},
        execution::{CreateExecutionParams, HubuTokenReference, Repository},
        workflow::ExecutionWorkflow,
    };
    use serde_json::json;
    use std::sync::atomic::AtomicUsize;

    struct UnavailableSecrets;
    impl SecretProvider for UnavailableSecrets {
        fn resolve(
            &self,
            _: &SecretReference,
        ) -> crate::secrets::Result<crate::secrets::ProviderSecret> {
            Err(crate::secrets::SecretError::Unavailable)
        }
    }

    fn mock_config(hubu: BoundaryMode, provider: BoundaryMode) -> SandboxConfig {
        SandboxConfig {
            profile: ProfileKind::Development,
            seed: 48,
            hubu: HubuConfig {
                mode: hubu,
                endpoint: None,
                allowlisted_hosts: vec![],
                scoped_credential_reference: None,
                isolated_test_account: None,
                agent_id: default_agent(),
                currency: default_currency(),
                maximum_authorization_minor: 500,
                authorization_expires_at: Some("2027-01-01T00:00:00Z".into()),
                claim_fault: BoundaryFault::None,
                settle_fault: BoundaryFault::None,
                release_fault: BoundaryFault::None,
            },
            provider: ProviderConfig {
                mode: provider,
                target: None,
                target_config: None,
                pricing_catalog: None,
                credential_reference: None,
                maximum_spend_minor: None,
                live_spend_acknowledgement: None,
                scenario: ProviderScenario::Success,
                execution_fault: BoundaryFault::None,
                artifact_fault: BoundaryFault::None,
            },
            temporal: TemporalConfig::default(),
            preserve_diagnostics_on_failure: false,
        }
    }

    fn execution() -> Execution {
        let repo = Repository::in_memory().unwrap();
        execution_in(&repo)
    }

    fn execution_in(repo: &Repository) -> Execution {
        repo.create_execution(&CreateExecutionParams {
            account_id: "sandbox-account".into(), operation_key: "sandbox:op-1".into(),
            hubu_authorization_id: "auth-1".into(), hubu_claim_id: None,
            hubu_token_reference: HubuTokenReference::new("sandbox-token-ref").unwrap(),
            authorized_minor: 100, authorization_currency: "USD".into(),
            normalized_input: json!({"prompt":"circle"}), input_hash: "hash".into(),
            input_schema_version: 1, target: "image_generation/example/fixture/image-v1".into(),
            config_version: "v1".into(), workload_type: "image_generation".into(),
            provider: "example".into(), adapter: "fixture".into(), model: "image-v1".into(),
            provider_config_version: "fixture-v1".into(),
            provider_config_digest: format!("sha256:{}", "a".repeat(64)),
            pricing_snapshot: json!({"provider":"example","model":"image-v1","catalog_version":"v1","catalog_digest":format!("sha256:{}", "b".repeat(64)),"pricing_rule_id":"image","unit":"image","unit_amount_minor":100,"quantity":1,"estimated_amount_minor":100,"currency":"USD"}),
            pricing_schema_version: 1, created_at: "2026-08-10T00:00:00Z".into(),
        }).unwrap()
    }

    #[test]
    fn mock_mock_isolated_run_is_ready_and_cleans_up() {
        let config = mock_config(BoundaryMode::Mock, BoundaryMode::Mock);
        let root = {
            let run = SandboxRun::start_for_test(&config).unwrap();
            assert!(run.manifest().readiness.iter().all(|check| check.ready));
            assert_eq!(run.manifest().hubu_mode, BoundaryMode::Mock);
            assert_eq!(run.manifest().provider_mode, BoundaryMode::Mock);
            assert!(run.manifest_path().is_file());
            run.root().to_path_buf()
        };
        assert!(!root.exists());
    }

    #[test]
    fn preserved_run_manifest_points_at_preserved_state() {
        let config = mock_config(BoundaryMode::Mock, BoundaryMode::Mock);
        let parent = tempfile::tempdir().unwrap();
        let destination = parent.path().join("preserved");
        let run = SandboxRun::start_for_test(&config).unwrap();
        run.preserve(&destination).unwrap();
        let manifest: RunManifest =
            serde_json::from_slice(&fs::read(destination.join("run-manifest.json")).unwrap())
                .unwrap();

        assert_eq!(
            manifest.database_path,
            destination.join("gongbu.sqlite3").display().to_string()
        );
        assert_eq!(
            manifest.artifact_root,
            destination.join("artifacts").display().to_string()
        );
        assert_eq!(
            manifest.workflow_root,
            destination.join("workflow").display().to_string()
        );
    }

    #[test]
    fn temporal_mode_requires_a_complete_managed_or_external_configuration() {
        let mut config = mock_config(BoundaryMode::Mock, BoundaryMode::Mock);
        config.temporal.binary.clear();
        assert!(config.validate().is_err());

        config.temporal = TemporalConfig {
            mode: TemporalMode::External,
            address: Some("http://127.0.0.1:7233".into()),
            ui_url: None,
            namespace: "default".into(),
            binary: "temporal".into(),
        };
        assert!(config.validate().is_err());
        config.temporal.ui_url = Some("http://127.0.0.1:8233".into());
        assert!(config.validate().is_ok());
    }

    #[test]
    fn production_rejects_mock_boundaries_and_mock_rejects_real_fields() {
        assert_eq!("mock".parse::<BoundaryMode>().unwrap(), BoundaryMode::Mock);
        assert!("fixture".parse::<BoundaryMode>().is_err());
        let mut config = mock_config(BoundaryMode::Mock, BoundaryMode::Mock);
        config.profile = ProfileKind::Production;
        assert!(config
            .validate()
            .unwrap_err()
            .to_string()
            .contains("production"));
        config.profile = ProfileKind::Development;
        config.hubu.endpoint = Some("http://127.0.0.1:8787".into());
        assert!(config
            .validate()
            .unwrap_err()
            .to_string()
            .contains("mock Hubu"));
    }

    #[test]
    fn real_hubu_requires_allowlist_credentials_and_isolation() {
        let mut config = mock_config(BoundaryMode::Real, BoundaryMode::Mock);
        config.hubu.endpoint = Some("http://hubu.example:8787".into());
        config.hubu.scoped_credential_reference = Some("keychain:hubu-sandbox".into());
        config.hubu.isolated_test_account = Some("sandbox-account".into());
        assert!(config.validate().is_err());
        config.hubu.allowlisted_hosts.push("hubu.example".into());
        assert!(config.validate().is_ok());
    }

    #[test]
    fn all_four_wiring_combinations_are_distinct() {
        let files = tempfile::tempdir().unwrap();
        let targets = files.path().join("targets.json");
        let pricing = files.path().join("pricing.json");
        fs::write(
            &targets,
            br#"{
          "schema_version":2,
          "provider_configs":[{
            "provider_config_version":"gemini-sandbox-v1",
            "workload_type":"image_generation",
            "provider":"google",
            "adapter":"gemini_developer_image",
            "model":"gemini-test",
            "secret_service":"gongbu.google",
            "secret_account":"sandbox",
            "active":true,
            "execution_enabled":true,
            "settings":{"type":"gemini_developer_image","config":{
              "endpoint":"https://generativelanguage.googleapis.com",
              "api_version":"v1beta",
              "timeout_ms":1000,
              "headers":{}
            }}
          }]
        }"#,
        )
        .unwrap();
        fs::write(
            &pricing,
            br#"{
          "schema_version":1,
          "catalog_version":"sandbox-prices-v1",
          "rules":[{
            "rule_id":"gemini-test-image",
            "provider":"google",
            "model":"gemini-test",
            "currency":"USD",
            "unit":"image",
            "unit_amount_minor":10
          }]
        }"#,
        )
        .unwrap();
        for (hubu, provider) in [
            (BoundaryMode::Mock, BoundaryMode::Mock),
            (BoundaryMode::Mock, BoundaryMode::Real),
            (BoundaryMode::Real, BoundaryMode::Mock),
            (BoundaryMode::Real, BoundaryMode::Real),
        ] {
            let mut config = mock_config(hubu, provider);
            if hubu == BoundaryMode::Real {
                config.hubu.endpoint = Some("http://127.0.0.1:8787".into());
                config.hubu.scoped_credential_reference = Some("keychain:hubu-sandbox".into());
                config.hubu.isolated_test_account = Some("sandbox-account".into());
            }
            if provider == BoundaryMode::Real {
                config.provider.target_config = Some(targets.clone());
                config.provider.pricing_catalog = Some(pricing.clone());
                config.provider.target = Some(ProviderSelection {
                    workload_type: "image_generation".into(),
                    provider: "google".into(),
                    adapter: "gemini_developer_image".into(),
                    model: "gemini-test".into(),
                });
                config.provider.credential_reference = Some("gongbu.google:sandbox".into());
                config.provider.maximum_spend_minor = Some(10);
                config.provider.live_spend_acknowledgement =
                    Some(LIVE_SPEND_ACKNOWLEDGEMENT.into());
            }
            let wiring = SandboxWiring::from_config(&config).unwrap();
            assert_eq!(
                matches!(&wiring.hubu, HubuWiring::Mock(_)),
                hubu == BoundaryMode::Mock
            );
            assert_eq!(
                matches!(&wiring.provider, ProviderWiring::Mock(_)),
                provider == BoundaryMode::Mock
            );
            if provider == BoundaryMode::Real {
                let error = wiring
                    .provider
                    .activities(Arc::new(UnavailableSecrets))
                    .preflight(&execution())
                    .unwrap_err();
                assert_eq!(
                    error,
                    ActivityError::Proven("sandbox_live_spend_ceiling_exceeded".into())
                );
            }
        }
    }

    #[test]
    fn mock_hubu_is_stateful_bounded_and_idempotent_after_disconnect() {
        let mut config = mock_config(BoundaryMode::Mock, BoundaryMode::Mock);
        config.hubu.claim_fault = BoundaryFault::CommitThenDisconnect;
        let hubu = MockHubu::new(config.hubu, config.seed).unwrap();
        let execution = execution();
        assert!(matches!(
            hubu.claim(&execution),
            Err(ActivityError::Ambiguous(_))
        ));
        let claim_id = hubu.claim(&execution).unwrap();
        assert!(claim_id.starts_with("claim-"));
        assert_eq!(hubu.financial_mutations(), 1);
        assert!(hubu
            .safe_calls()
            .iter()
            .all(|call| !call.operation_key_digest.contains("op-1")));
    }

    #[test]
    fn provider_attempt_identity_prevents_second_invocation() {
        let config = mock_config(BoundaryMode::Mock, BoundaryMode::Mock);
        let provider = DeterministicProvider::new(&config.provider).unwrap();
        let execution = execution();
        let first = provider.invoke(&execution, "attempt-1").unwrap();
        let second = provider.invoke(&execution, "attempt-1").unwrap();
        assert_eq!(first.request_id, second.request_id);
        assert_eq!(provider.invocation_count(), 1);
        assert_eq!(&first.artifacts[0].bytes[..8], b"\x89PNG\r\n\x1a\n");
    }

    struct CountingArtifacts(AtomicUsize);
    impl ArtifactActivities for CountingArtifacts {
        fn preflight(&self) -> Result<(), ActivityError> {
            Ok(())
        }
        fn persist(
            &self,
            _: &Execution,
            _: &str,
            _: &[ProviderArtifact],
        ) -> Result<(), ActivityError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    #[test]
    fn artifact_commit_then_disconnect_is_not_persisted_twice() {
        let delegate = Arc::new(CountingArtifacts(AtomicUsize::new(0)));
        let artifacts =
            FaultInjectingArtifacts::new(delegate.clone(), BoundaryFault::CommitThenDisconnect);
        let first = artifacts.persist(&execution(), "attempt-1", &[]);
        let second = artifacts.persist(&execution(), "attempt-1", &[]);
        assert_eq!(
            first,
            Err(ActivityError::Ambiguous(
                "mock_artifact_persistence_disconnect".into()
            ))
        );
        assert_eq!(first, second);
        assert_eq!(delegate.0.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn mock_hubu_expiry_and_idempotency_conflicts_fail_closed() {
        let config = mock_config(BoundaryMode::Mock, BoundaryMode::Mock);
        let hubu = MockHubu::new(config.hubu, config.seed).unwrap();
        let execution = execution();
        hubu.claim(&execution).unwrap();
        let mut conflicting = execution.clone();
        conflicting.account_id = "different-account".into();
        assert_eq!(
            hubu.claim(&conflicting),
            Err(ActivityError::Proven(
                "mock_hubu_idempotency_conflict".into()
            ))
        );
        hubu.expire_authorizations();
        assert_eq!(
            hubu.preflight(&execution),
            Err(ActivityError::Proven(
                "mock_hubu_authorization_expired".into()
            ))
        );
    }

    #[test]
    fn mock_mock_runs_the_durable_workflow_and_replay_has_no_second_side_effect() {
        let config = mock_config(BoundaryMode::Mock, BoundaryMode::Mock);
        let repository = Repository::in_memory().unwrap();
        let execution = execution_in(&repository);
        let hubu = MockHubu::new(config.hubu, config.seed).unwrap();
        let provider = DeterministicProvider::new(&config.provider).unwrap();
        let artifacts_root = tempfile::tempdir().unwrap();
        let artifacts = ArtifactServiceActivities::new(
            ArtifactService::new(
                repository.clone(),
                LocalFsStorage::new(artifacts_root.path()),
                ArtifactLimits::default(),
            ),
            || "2026-08-10T00:00:01Z".into(),
        );
        let workflow = ExecutionWorkflow {
            repository: &repository,
            hubu: &hubu,
            provider: &provider,
            artifacts: &artifacts,
        };
        let completed = workflow
            .run(&execution.execution_id, "2026-08-10T00:00:01Z")
            .unwrap();
        assert_eq!(completed.status, "succeeded");
        assert_eq!(provider.invocation_count(), 1);
        assert_eq!(hubu.financial_mutations(), 2);
        let replayed = workflow
            .run(&execution.execution_id, "2026-08-10T00:00:02Z")
            .unwrap();
        assert_eq!(replayed.status, "succeeded");
        assert_eq!(provider.invocation_count(), 1);
        assert_eq!(hubu.financial_mutations(), 2);
        assert_eq!(
            repository
                .count_artifacts_for_execution(&execution.execution_id)
                .unwrap(),
            1
        );
    }
}
