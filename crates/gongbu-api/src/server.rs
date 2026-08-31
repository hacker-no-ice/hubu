//! Persistent, operator-owned local Gongbu server composition.
//!
//! This module deliberately owns only Gongbu and its Temporal worker. Hubu is
//! always an independently managed dependency and is never started or stopped
//! here.

use crate::{
    application::{self, ApplicationDependencies, AuthenticationError, Authenticator},
    artifact::{ArtifactLimits, ArtifactService, LocalFsStorage},
    execution::Repository,
    hubu::{HubuClient, ProductionHubuActivities},
    provider::{
        contract::PricingCatalog,
        registry::{ProviderRegistry, ValidatedProviderCatalog},
        targets::ProviderTargetConfig,
    },
    redaction::Redactor,
    secrets::{
        MacOsKeychain, ManagedStackSecrets, SandboxFixtureSecrets, SecretProvider, SecretReference,
        MANAGED_CREDENTIAL_DIR_ENV,
    },
    temporal::TemporalWorkerConfig,
};
use axum::http::{header, HeaderMap};
use chrono::SecondsFormat;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, OpenOptions},
    io,
    net::{IpAddr, SocketAddr},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    str::FromStr,
    sync::Arc,
    time::Duration,
};
use temporalio_client::{Client, ClientOptions, Connection, ConnectionOptions, Url};
use temporalio_sdk::Runtime;
use thiserror::Error;

pub const LIVE_SPEND_ACKNOWLEDGEMENT: &str = "I_ACKNOWLEDGE_LIVE_PROVIDER_SPEND";

type BoxError = Box<dyn std::error::Error + Send + Sync>;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ServerConfig {
    pub schema_version: u32,
    pub http: HttpConfig,
    pub state: StateConfig,
    pub temporal: TemporalConfig,
    pub hubu: HubuConfig,
    pub authentication: AuthenticationConfig,
    pub providers: ProvidersConfig,
    pub artifacts: ArtifactConfig,
    pub execution: ExecutionConfig,
    pub logging: LoggingConfig,
    pub shutdown: ShutdownConfig,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HttpConfig {
    pub listen: SocketAddr,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StateConfig {
    pub database_path: PathBuf,
    pub artifact_root: PathBuf,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum TemporalConfig {
    ManagedLocal {
        binary_path: PathBuf,
        expected_cli_version: String,
        data_path: PathBuf,
        rpc_port: u16,
        ui_port: u16,
        namespace: String,
        task_queue: String,
        #[serde(default)]
        ui_url: Option<String>,
    },
    External {
        address: String,
        namespace: String,
        task_queue: String,
        #[serde(default)]
        ui_url: Option<String>,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HubuConfig {
    pub endpoint: String,
    #[serde(default)]
    pub allowlisted_hosts: Vec<String>,
    pub expected_product_version: String,
    pub expected_executor_contract: String,
    pub credential_reference: SecretReferenceConfig,
    pub startup_policy: StartupPolicy,
    pub startup_timeout_ms: u64,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StartupPolicy {
    Exit,
    Wait,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AuthenticationConfig {
    pub bearer_credential_reference: SecretReferenceConfig,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SecretReferenceConfig {
    pub service: String,
    pub account: String,
}

impl SecretReferenceConfig {
    fn validated(&self) -> Result<SecretReference, ServerError> {
        SecretReference::new(self.service.clone(), self.account.clone())
            .map_err(|_| invalid("invalid opaque credential reference"))
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProvidersConfig {
    #[serde(default, skip_serializing_if = "ProviderMode::is_live")]
    pub mode: ProviderMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_catalog_path: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pricing_catalog_path: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maximum_spend_minor: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub live_spend_acknowledgement: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProviderMode {
    Disabled,
    Sandbox,
    #[default]
    Live,
}

impl ProviderMode {
    fn is_live(&self) -> bool {
        *self == Self::Live
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactConfig {
    pub max_artifacts_per_execution: u64,
    pub max_encoded_bytes: u64,
    pub max_decoded_bytes: u64,
    pub max_width: u32,
    pub max_height: u32,
}

impl ArtifactConfig {
    fn limits(&self) -> ArtifactLimits {
        ArtifactLimits {
            max_artifacts_per_execution: self.max_artifacts_per_execution,
            max_encoded_bytes: self.max_encoded_bytes,
            max_decoded_bytes: self.max_decoded_bytes,
            max_width: self.max_width,
            max_height: self.max_height,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionConfig {
    pub recovery_delays_seconds: Vec<u64>,
    pub temporal_startup_timeout_ms: u64,
    pub dependency_check_interval_ms: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LoggingConfig {
    pub level: LogLevel,
    pub format: LogFormat,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LogLevel {
    Error,
    Warn,
    Info,
    Debug,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LogFormat {
    Text,
    Json,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ShutdownConfig {
    pub worker_drain_timeout_ms: u64,
}

#[derive(Debug, Error)]
pub enum ServerError {
    #[error("gongbu-server configuration: {0}")]
    Invalid(String),
    #[error("gongbu-server IO: {0}")]
    Io(#[from] io::Error),
    #[error("gongbu-server JSON: {0}")]
    Json(#[from] serde_json::Error),
}

fn invalid(message: impl Into<String>) -> ServerError {
    ServerError::Invalid(message.into())
}

impl ServerConfig {
    pub fn from_path(path: impl AsRef<Path>) -> Result<Self, ServerError> {
        let path = path.as_ref();
        if !path.is_absolute() {
            return Err(invalid("--config must be an absolute path"));
        }
        let bytes = fs::read(path)?;
        let document: serde_json::Value = serde_json::from_slice(&bytes)?;
        let schema_version = document
            .get("schema_version")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| invalid("missing or invalid server configuration schema_version"))?;
        if matches!(schema_version, 1 | 2) {
            return Err(invalid(format!(
                "server configuration schema_version {schema_version} requires upgrade to schema_version {}; legacy static execution-principal bindings cannot be activated",
                gongbu_build_info::SERVER_CONFIG_SCHEMA_VERSION
            )));
        }
        let config: Self = serde_json::from_value(document)?;
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<(), ServerError> {
        if self.schema_version != gongbu_build_info::SERVER_CONFIG_SCHEMA_VERSION {
            return Err(invalid("unsupported server configuration schema_version"));
        }
        if !self.http.listen.ip().is_loopback() {
            return Err(invalid("HTTP listen address must be loopback"));
        }
        validate_state_path(&self.state.database_path, "database_path")?;
        validate_state_path(&self.state.artifact_root, "artifact_root")?;
        self.providers.validate()?;
        if self.hubu.expected_product_version.trim().is_empty()
            || self.hubu.expected_executor_contract != gongbu_build_info::HUBU_EXECUTOR_CONTRACT
        {
            return Err(invalid("invalid Hubu compatibility settings"));
        }
        validate_duration(self.hubu.startup_timeout_ms, "Hubu startup timeout")?;
        validate_hubu_endpoint(&self.hubu.endpoint, &self.hubu.allowlisted_hosts)?;
        self.hubu.credential_reference.validated()?;
        self.authentication
            .bearer_credential_reference
            .validated()?;
        let (namespace, task_queue) = self.temporal.identity();
        TemporalWorkerConfig {
            task_queue: task_queue.into(),
            recovery_delays_seconds: self.execution.recovery_delays_seconds.clone(),
        }
        .validate()
        .map_err(invalid)?;
        if namespace.trim().is_empty() || namespace.len() > 255 {
            return Err(invalid("invalid Temporal namespace"));
        }
        match &self.temporal {
            TemporalConfig::ManagedLocal {
                binary_path,
                expected_cli_version,
                data_path,
                rpc_port,
                ui_port,
                ..
            } => {
                validate_file_path(binary_path, "managed Temporal binary_path")?;
                validate_state_path(data_path, "managed Temporal data_path")?;
                if expected_cli_version.trim().is_empty()
                    || *rpc_port == 0
                    || *ui_port == 0
                    || rpc_port == ui_port
                {
                    return Err(invalid("invalid managed-local Temporal settings"));
                }
            }
            TemporalConfig::External { address, .. } => validate_temporal_address(address)?,
        }
        let limits = self.artifacts.limits();
        if limits.max_artifacts_per_execution == 0
            || limits.max_encoded_bytes == 0
            || limits.max_decoded_bytes == 0
            || limits.max_width == 0
            || limits.max_height == 0
        {
            return Err(invalid("artifact limits must be positive"));
        }
        validate_duration(
            self.execution.temporal_startup_timeout_ms,
            "Temporal startup timeout",
        )?;
        validate_duration(
            self.execution.dependency_check_interval_ms,
            "dependency check interval",
        )?;
        validate_duration(
            self.shutdown.worker_drain_timeout_ms,
            "worker drain timeout",
        )?;
        Ok(())
    }
}

impl ProvidersConfig {
    fn validate(&self) -> Result<(), ServerError> {
        match self.mode {
            ProviderMode::Disabled => {
                if self.target_catalog_path.is_some()
                    || self.pricing_catalog_path.is_some()
                    || self.maximum_spend_minor.is_some()
                    || self.live_spend_acknowledgement.is_some()
                {
                    return Err(invalid(
                        "disabled provider mode must not include catalogs or live-spend fields",
                    ));
                }
            }
            ProviderMode::Sandbox => {
                let targets = self
                    .target_catalog_path
                    .as_deref()
                    .ok_or_else(|| invalid("sandbox provider target_catalog_path is required"))?;
                let pricing = self
                    .pricing_catalog_path
                    .as_deref()
                    .ok_or_else(|| invalid("sandbox provider pricing_catalog_path is required"))?;
                validate_file_path(targets, "target_catalog_path")?;
                validate_file_path(pricing, "pricing_catalog_path")?;
                if self.maximum_spend_minor.is_none_or(|value| value <= 0)
                    || self.live_spend_acknowledgement.is_some()
                {
                    return Err(invalid(
                        "sandbox provider mode requires an internal spend ceiling and forbids live-spend acknowledgement",
                    ));
                }
            }
            ProviderMode::Live => {
                let targets = self
                    .target_catalog_path
                    .as_deref()
                    .ok_or_else(|| invalid("live provider target_catalog_path is required"))?;
                let pricing = self
                    .pricing_catalog_path
                    .as_deref()
                    .ok_or_else(|| invalid("live provider pricing_catalog_path is required"))?;
                validate_file_path(targets, "target_catalog_path")?;
                validate_file_path(pricing, "pricing_catalog_path")?;
                if self.maximum_spend_minor.is_none_or(|value| value <= 0)
                    || self.live_spend_acknowledgement.as_deref()
                        != Some(LIVE_SPEND_ACKNOWLEDGEMENT)
                {
                    return Err(invalid(
                        "provider spend ceiling and explicit live-spend acknowledgement are required",
                    ));
                }
            }
        }
        Ok(())
    }

    fn maximum_spend_minor(&self) -> i64 {
        self.maximum_spend_minor.unwrap_or(0)
    }
}

impl TemporalConfig {
    fn identity(&self) -> (&str, &str) {
        match self {
            Self::ManagedLocal {
                namespace,
                task_queue,
                ..
            }
            | Self::External {
                namespace,
                task_queue,
                ..
            } => (namespace, task_queue),
        }
    }

    fn address(&self) -> String {
        match self {
            Self::ManagedLocal { rpc_port, .. } => format!("http://127.0.0.1:{rpc_port}"),
            Self::External { address, .. } => address.clone(),
        }
    }
}

fn validate_duration(value: u64, name: &str) -> Result<(), ServerError> {
    if !(100..=300_000).contains(&value) {
        Err(invalid(format!(
            "{name} must be between 100 and 300000 milliseconds"
        )))
    } else {
        Ok(())
    }
}

fn validate_state_path(path: &Path, name: &str) -> Result<(), ServerError> {
    if !path.is_absolute()
        || path == Path::new("/")
        || path
            .components()
            .any(|part| matches!(part, std::path::Component::ParentDir))
    {
        return Err(invalid(format!("{name} must be a safe absolute path")));
    }
    Ok(())
}

fn validate_file_path(path: &Path, name: &str) -> Result<(), ServerError> {
    validate_state_path(path, name)?;
    if !path.is_file() {
        return Err(invalid(format!(
            "{name} must name an existing regular file"
        )));
    }
    Ok(())
}

fn validate_hubu_endpoint(endpoint: &str, allowlisted: &[String]) -> Result<(), ServerError> {
    let url = reqwest::Url::parse(endpoint).map_err(|_| invalid("invalid Hubu endpoint"))?;
    if url.scheme() != "http"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
    {
        return Err(invalid(
            "Hubu endpoint must be an http:// origin without credentials or extra components",
        ));
    }
    let host = url
        .host_str()
        .ok_or_else(|| invalid("Hubu endpoint has no host"))?;
    let loopback = host == "localhost"
        || host
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_loopback());
    if !loopback && !allowlisted.iter().any(|allowed| allowed == host) {
        return Err(invalid("Hubu endpoint host is not loopback or allowlisted"));
    }
    Ok(())
}

fn validate_temporal_address(address: &str) -> Result<(), ServerError> {
    let url = reqwest::Url::parse(address).map_err(|_| invalid("invalid Temporal address"))?;
    let host = url
        .host_str()
        .ok_or_else(|| invalid("Temporal address has no host"))?;
    if url.scheme() != "http"
        || (!host.eq_ignore_ascii_case("localhost")
            && !host
                .parse::<IpAddr>()
                .is_ok_and(|address| address.is_loopback()))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
    {
        return Err(invalid(
            "external Temporal must be an explicit loopback http:// origin",
        ));
    }
    Ok(())
}

pub async fn serve(config_path: impl AsRef<Path>) -> Result<(), BoxError> {
    let config = match ServerConfig::from_path(config_path) {
        Ok(config) => config,
        Err(error) => {
            crate::lifecycle::log(crate::lifecycle::LifecycleReason::ConfigurationStartupFailure);
            return Err(error.into());
        }
    };
    serve_config(config).await
}

pub fn validate_runtime_inputs(config_path: impl AsRef<Path>) -> Result<ServerConfig, ServerError> {
    let config = ServerConfig::from_path(config_path)?;
    validated_provider_catalog(&config)?;
    validate_managed_temporal_cli(&config.temporal)?;
    Ok(config)
}

pub async fn serve_config(mut config: ServerConfig) -> Result<(), BoxError> {
    let mut startup = StartupLifecycleGuard::armed();
    config.validate()?;
    prepare_state_paths(&config)?;
    normalize_paths(&mut config)?;

    let bootstrap_secrets = startup_bootstrap_secret_provider()?;
    let provider_secrets = startup_provider_secret_provider(config.providers.mode)?;
    let caller_secret = bootstrap_secrets
        .resolve(
            &config
                .authentication
                .bearer_credential_reference
                .validated()?,
        )
        .map_err(|_| invalid("caller capability credential is unavailable"))?;
    let hubu_secret = bootstrap_secrets
        .resolve(&config.hubu.credential_reference.validated()?)
        .map_err(|_| invalid("Hubu scoped credential is unavailable"))?;

    let mut redaction_values = vec![
        caller_secret.expose().to_vec(),
        hubu_secret.expose().to_vec(),
    ];
    let mut providers = validated_provider_catalog(&config)?;
    for target in providers
        .targets()
        .revisions()
        .filter(|target| target.is_execution_enabled())
    {
        let secret = provider_secrets
            .resolve(
                &target
                    .secret_reference()
                    .map_err(|_| invalid("provider credential reference is invalid"))?,
            )
            .map_err(|_| invalid("an enabled provider credential is unavailable"))?;
        redaction_values.push(secret.expose().to_vec());
    }
    providers.mark_credential_references_present();
    let limits = config.artifacts.limits();

    let repository = Repository::open(
        &config.state.database_path,
        Redactor::new(redaction_values.iter().map(Vec::as_slice)),
    )?;
    let artifacts = ArtifactService::new(
        repository.clone(),
        LocalFsStorage::new(&config.state.artifact_root),
        limits,
    );
    artifacts.preflight()?;

    let hubu_client =
        HubuClient::new(&config.hubu.endpoint).with_bearer_token(hubu_secret.expose().to_vec());
    wait_for_hubu_compatibility(&config.hubu, &hubu_client).await?;
    let health_client = hubu_client.clone();
    let expected_hubu_version = config.hubu.expected_product_version.clone();
    let expected_hubu_contract = config.hubu.expected_executor_contract.clone();
    let dependency_checker: Arc<dyn Fn() -> bool + Send + Sync> = Arc::new(move || {
        hubu_is_compatible(
            &health_client,
            &expected_hubu_version,
            &expected_hubu_contract,
        )
    });
    let hubu = Arc::new(ProductionHubuActivities::new(
        hubu_client,
        repository.clone(),
    ));

    let mut temporal_child = start_managed_temporal(&config.temporal)?;
    let temporal_address = config.temporal.address();
    wait_for_temporal_port(
        &temporal_address,
        temporal_child.as_mut(),
        Duration::from_millis(config.execution.temporal_startup_timeout_ms),
    )
    .await?;
    let connection =
        Connection::connect(ConnectionOptions::new(Url::from_str(&temporal_address)?).build())
            .await?;
    let (namespace, task_queue) = config.temporal.identity();
    let temporal_client =
        Client::new(connection, ClientOptions::new(namespace.to_owned()).build())?;
    let temporal_runtime = Arc::new(Runtime::new_assume_tokio(Default::default())?);
    let authenticator = Arc::new(CapabilityAuthenticator::new(caller_secret.expose())?);
    drop(caller_secret);
    drop(hubu_secret);
    for value in &mut redaction_values {
        value.fill(0);
    }

    // Every required dependency has passed preflight. The application starts
    // and proves its worker poller before accepting from this listener.
    let listener = tokio::net::TcpListener::bind(config.http.listen).await?;
    eprintln!(
        "gongbu-server readying on {}; Hubu remains independently managed",
        config.http.listen
    );
    let now = Arc::new(|| chrono::Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true));
    let dependencies = ApplicationDependencies {
        repository,
        artifacts,
        providers,
        hubu: hubu.clone(),
        hubu_authorizations: hubu,
        secrets: provider_secrets,
        provider_activities: None,
        artifact_activities: None,
        temporal_runtime,
        temporal_client,
        temporal_worker: TemporalWorkerConfig {
            task_queue: task_queue.to_owned(),
            recovery_delays_seconds: config.execution.recovery_delays_seconds.clone(),
        },
        temporal_namespace: namespace.to_owned(),
        temporal_startup_timeout: Duration::from_millis(
            config.execution.temporal_startup_timeout_ms,
        ),
        dependency_check_interval: Duration::from_millis(
            config.execution.dependency_check_interval_ms,
        ),
        dependency_failure_grace: application::DEPENDENCY_FAILURE_GRACE,
        maximum_spend_minor: config.providers.maximum_spend_minor(),
        dependency_checker: Some(dependency_checker),
        worker_drain_timeout: Duration::from_millis(config.shutdown.worker_drain_timeout_ms),
        authenticator,
        now,
    };
    startup.mark_started();
    let result = application::serve(listener, dependencies, shutdown_signal()).await;
    if let Some(child) = temporal_child.as_mut() {
        child.stop();
    }
    if result.is_err() {
        crate::lifecycle::log(crate::lifecycle::LifecycleReason::WorkerUnavailable);
    }
    result.map(|_| ())
}

struct StartupLifecycleGuard {
    armed: bool,
}

impl StartupLifecycleGuard {
    fn armed() -> Self {
        Self { armed: true }
    }

    fn mark_started(&mut self) {
        self.armed = false;
    }
}

impl Drop for StartupLifecycleGuard {
    fn drop(&mut self) {
        if self.armed {
            crate::lifecycle::log(crate::lifecycle::LifecycleReason::ConfigurationStartupFailure);
        }
    }
}

fn prepare_state_paths(config: &ServerConfig) -> Result<(), ServerError> {
    let database_parent = config
        .state
        .database_path
        .parent()
        .ok_or_else(|| invalid("database_path has no parent"))?;
    fs::create_dir_all(database_parent)?;
    fs::create_dir_all(&config.state.artifact_root)?;
    if let TemporalConfig::ManagedLocal { data_path, .. } = &config.temporal {
        fs::create_dir_all(data_path)?;
    }
    Ok(())
}

fn normalize_paths(config: &mut ServerConfig) -> Result<(), ServerError> {
    config.state.database_path = if config.state.database_path.exists() {
        fs::canonicalize(&config.state.database_path)?
    } else {
        let name = config
            .state
            .database_path
            .file_name()
            .ok_or_else(|| invalid("database_path has no filename"))?;
        fs::canonicalize(
            config
                .state
                .database_path
                .parent()
                .ok_or_else(|| invalid("database_path has no parent"))?,
        )?
        .join(name)
    };
    config.state.artifact_root = fs::canonicalize(&config.state.artifact_root)?;
    if let Some(path) = &mut config.providers.target_catalog_path {
        *path = fs::canonicalize(&*path)?;
    }
    if let Some(path) = &mut config.providers.pricing_catalog_path {
        *path = fs::canonicalize(&*path)?;
    }
    if let TemporalConfig::ManagedLocal {
        binary_path,
        data_path,
        ..
    } = &mut config.temporal
    {
        *binary_path = fs::canonicalize(&*binary_path)?;
        *data_path = fs::canonicalize(&*data_path)?;
    }
    Ok(())
}

fn reject_fixture_targets(targets: &ProviderTargetConfig) -> Result<(), ServerError> {
    #[cfg(feature = "local-fixture-canary")]
    if std::env::var("GONGBU_LOCAL_FIXTURE_CANARY").as_deref() == Ok("1")
        && targets.revisions().all(|target| {
            target.provider == "example"
                && target.adapter == "fixture"
                && target.model == "image-v1"
        })
    {
        return Ok(());
    }
    if targets.revisions().any(|target| {
        [&target.provider, &target.adapter, &target.model]
            .iter()
            .any(|value| {
                let value = value.to_ascii_lowercase();
                value.contains("mock") || value.contains("fixture")
            })
    }) {
        return Err(invalid(
            "mock and fixture provider targets are not valid server boundaries",
        ));
    }
    Ok(())
}

fn startup_bootstrap_secret_provider() -> Result<Arc<dyn SecretProvider>, ServerError> {
    if std::env::var_os(MANAGED_CREDENTIAL_DIR_ENV).is_some() {
        return ManagedStackSecrets::from_environment()
            .map(|provider| Arc::new(provider) as Arc<dyn SecretProvider>)
            .map_err(|_| invalid("managed stack credential directory is unavailable"));
    }
    #[cfg(feature = "local-fixture-canary")]
    if std::env::var("GONGBU_LOCAL_FIXTURE_CANARY").as_deref() == Ok("1") {
        return crate::secrets::LocalFixtureSecrets::from_environment()
            .map(|provider| Arc::new(provider) as Arc<dyn SecretProvider>)
            .map_err(|_| invalid("local fixture credential directory is unavailable"));
    }
    Ok(Arc::new(MacOsKeychain))
}

fn startup_provider_secret_provider(
    mode: ProviderMode,
) -> Result<Arc<dyn SecretProvider>, ServerError> {
    if mode == ProviderMode::Sandbox {
        return Ok(Arc::new(SandboxFixtureSecrets));
    }
    #[cfg(feature = "local-fixture-canary")]
    if std::env::var("GONGBU_LOCAL_FIXTURE_CANARY").as_deref() == Ok("1") {
        return crate::secrets::LocalFixtureSecrets::from_environment()
            .map(|provider| Arc::new(provider) as Arc<dyn SecretProvider>)
            .map_err(|_| invalid("local fixture credential directory is unavailable"));
    }
    Ok(Arc::new(MacOsKeychain))
}

fn validated_provider_catalog(
    config: &ServerConfig,
) -> Result<ValidatedProviderCatalog, ServerError> {
    if config.providers.mode == ProviderMode::Disabled {
        return Ok(ValidatedProviderCatalog::disabled());
    }
    let targets_path = config
        .providers
        .target_catalog_path
        .as_deref()
        .ok_or_else(|| invalid("live provider target_catalog_path is required"))?;
    let pricing_path = config
        .providers
        .pricing_catalog_path
        .as_deref()
        .ok_or_else(|| invalid("live provider pricing_catalog_path is required"))?;
    let targets = ProviderTargetConfig::from_path(targets_path)
        .map_err(|error| invalid(format!("provider target catalog: {error}")))?;
    validate_provider_credential_separation(config, &targets)?;
    if config.providers.mode == ProviderMode::Sandbox {
        validate_sandbox_fixture_targets(&targets)?;
    } else {
        reject_fixture_targets(&targets)?;
    }
    let pricing = PricingCatalog::load(pricing_path)
        .map_err(|error| invalid(format!("pricing catalog: {error}")))?;
    ValidatedProviderCatalog::bind(
        targets,
        pricing,
        &if config.providers.mode == ProviderMode::Sandbox {
            ProviderRegistry::sandbox()
        } else {
            ProviderRegistry::production(&config.artifacts.limits())
        },
    )
    .map_err(|error| invalid(format!("provider catalog binding: {error}")))
}

fn validate_provider_credential_separation(
    config: &ServerConfig,
    targets: &ProviderTargetConfig,
) -> Result<(), ServerError> {
    let hubu = config.hubu.credential_reference.validated()?;
    let caller = config
        .authentication
        .bearer_credential_reference
        .validated()?;
    for target in targets.revisions() {
        let provider = target
            .secret_reference()
            .map_err(|_| invalid("provider credential reference is invalid"))?;
        if provider == hubu || provider == caller {
            return Err(invalid(
                "provider credentials must be isolated from Hubu and Gongbu caller credentials",
            ));
        }
    }
    Ok(())
}

fn validate_sandbox_fixture_targets(targets: &ProviderTargetConfig) -> Result<(), ServerError> {
    let revisions = targets.revisions().collect::<Vec<_>>();
    if revisions.len() == 1
        && revisions.iter().all(|target| {
            target.provider == "sandbox"
                && target.adapter == "fixture"
                && target.model == "deterministic-image-v1"
                && target.is_active()
                && target.is_execution_enabled()
        })
    {
        Ok(())
    } else {
        Err(invalid(
            "sandbox provider mode accepts only the built-in deterministic fixture target",
        ))
    }
}

fn validate_managed_temporal_cli(config: &TemporalConfig) -> Result<(), ServerError> {
    let TemporalConfig::ManagedLocal {
        binary_path,
        expected_cli_version,
        ..
    } = config
    else {
        return Ok(());
    };
    let version = Command::new(binary_path)
        .arg("--version")
        .output()
        .map_err(|_| invalid("managed Temporal CLI version probe failed"))?;
    let stdout = std::str::from_utf8(&version.stdout)
        .map_err(|_| invalid("managed Temporal CLI version output is not UTF-8"))?;
    let mut fields = stdout
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or_default()
        .split_whitespace();
    let reported_cli_version = match (fields.next(), fields.next(), fields.next()) {
        (Some("temporal"), Some("version"), Some(value)) => value,
        _ => {
            return Err(invalid(
                "managed Temporal CLI version output has an unsupported format",
            ))
        }
    };
    if !version.status.success()
        || reported_cli_version.trim_start_matches('v')
            != expected_cli_version.trim_start_matches('v')
    {
        return Err(invalid(
            "managed Temporal CLI version does not match the configured pin",
        ));
    }
    Ok(())
}

async fn wait_for_hubu_compatibility(
    config: &HubuConfig,
    client: &HubuClient,
) -> Result<(), BoxError> {
    let deadline = tokio::time::Instant::now() + Duration::from_millis(config.startup_timeout_ms);
    loop {
        let compatible = hubu_is_compatible(
            client,
            &config.expected_product_version,
            &config.expected_executor_contract,
        );
        if compatible {
            return Ok(());
        }
        if config.startup_policy == StartupPolicy::Exit || tokio::time::Instant::now() >= deadline {
            return Err(io::Error::other("Hubu is unavailable or incompatible").into());
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

fn hubu_is_compatible(client: &HubuClient, product_version: &str, contract: &str) -> bool {
    client.health().is_ok()
        && client.version().is_ok_and(|version| {
            version.product_version == product_version && version.executor_contract == contract
        })
        && client.check_credential().is_ok()
}

struct ManagedTemporalChild(Child);

impl ManagedTemporalChild {
    fn try_wait(&mut self) -> io::Result<Option<std::process::ExitStatus>> {
        self.0.try_wait()
    }

    fn stop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

impl Drop for ManagedTemporalChild {
    fn drop(&mut self) {
        self.stop();
    }
}

fn start_managed_temporal(
    config: &TemporalConfig,
) -> Result<Option<ManagedTemporalChild>, BoxError> {
    let TemporalConfig::ManagedLocal {
        binary_path,
        data_path,
        rpc_port,
        ui_port,
        ..
    } = config
    else {
        return Ok(None);
    };
    validate_managed_temporal_cli(config)?;
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(data_path.join("temporal.log"))?;
    let stderr = log.try_clone()?;
    let database = data_path.join("temporal.sqlite");
    let child = Command::new(binary_path)
        .args([
            "server",
            "start-dev",
            "--ip",
            "127.0.0.1",
            "--port",
            &rpc_port.to_string(),
            "--ui-port",
            &ui_port.to_string(),
            "--db-filename",
            &database.display().to_string(),
        ])
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(stderr))
        .spawn()?;
    Ok(Some(ManagedTemporalChild(child)))
}

async fn wait_for_temporal_port(
    address: &str,
    mut child: Option<&mut ManagedTemporalChild>,
    timeout: Duration,
) -> Result<(), BoxError> {
    let url = reqwest::Url::parse(address)?;
    let host = url
        .host_str()
        .ok_or_else(|| io::Error::other("Temporal address has no host"))?;
    let port = url
        .port_or_known_default()
        .ok_or_else(|| io::Error::other("Temporal address has no port"))?;
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if tokio::net::TcpStream::connect((host, port)).await.is_ok() {
            return Ok(());
        }
        if let Some(child) = child.as_mut() {
            if let Some(status) = child.try_wait()? {
                return Err(io::Error::other(format!(
                    "managed Temporal exited before readiness with {status}"
                ))
                .into());
            }
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(io::Error::other("Temporal readiness timed out").into());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

struct CapabilityAuthenticator {
    token_digest: [u8; 32],
}

impl CapabilityAuthenticator {
    fn new(token: &[u8]) -> Result<Self, ServerError> {
        if token.is_empty() {
            return Err(invalid("invalid caller capability"));
        }
        Ok(Self {
            token_digest: Sha256::digest(token).into(),
        })
    }
}

impl Authenticator for CapabilityAuthenticator {
    fn authenticate(
        &self,
        headers: &HeaderMap,
    ) -> Result<crate::http::AuthenticatedCaller, AuthenticationError> {
        let token = headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
            .filter(|value| !value.is_empty())
            .ok_or(AuthenticationError)?;
        let candidate: [u8; 32] = Sha256::digest(token.as_bytes()).into();
        let equal = candidate
            .iter()
            .zip(self.token_digest.iter())
            .fold(0_u8, |difference, (left, right)| {
                difference | (left ^ right)
            })
            == 0;
        if !equal {
            return Err(AuthenticationError);
        }
        Ok(crate::http::AuthenticatedCaller::service_installation())
    }
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .expect("SIGTERM handler");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {},
            _ = terminate.recv() => {},
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn config(root: &Path) -> ServerConfig {
        let temporal = root.join("temporal");
        let targets = root.join("targets.json");
        let prices = root.join("prices.json");
        let binary = root.join("temporal-cli");
        fs::write(&targets, "{}").unwrap();
        fs::write(&prices, "{}").unwrap();
        fs::write(
            &binary,
            "#!/bin/sh\necho 'temporal version 1.0.0 (Server 9.9.9, UI 8.8.8)'\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
        }
        ServerConfig {
            schema_version: gongbu_build_info::SERVER_CONFIG_SCHEMA_VERSION,
            http: HttpConfig {
                listen: "127.0.0.1:8788".parse().unwrap(),
            },
            state: StateConfig {
                database_path: root.join("state/gongbu.sqlite3"),
                artifact_root: root.join("artifacts"),
            },
            temporal: TemporalConfig::ManagedLocal {
                binary_path: binary,
                expected_cli_version: "1.0.0".into(),
                data_path: temporal,
                rpc_port: 7233,
                ui_port: 8233,
                namespace: "default".into(),
                task_queue: "gongbu-local".into(),
                ui_url: None,
            },
            hubu: HubuConfig {
                endpoint: "http://127.0.0.1:8787".into(),
                allowlisted_hosts: vec![],
                expected_product_version: "0.1.0".into(),
                expected_executor_contract: gongbu_build_info::HUBU_EXECUTOR_CONTRACT.into(),
                credential_reference: SecretReferenceConfig {
                    service: "gongbu.hubu".into(),
                    account: "local".into(),
                },
                startup_policy: StartupPolicy::Exit,
                startup_timeout_ms: 1_000,
            },
            authentication: AuthenticationConfig {
                bearer_credential_reference: SecretReferenceConfig {
                    service: "gongbu.caller".into(),
                    account: "local".into(),
                },
            },
            providers: ProvidersConfig {
                mode: ProviderMode::Live,
                target_catalog_path: Some(targets),
                pricing_catalog_path: Some(prices),
                maximum_spend_minor: Some(100),
                live_spend_acknowledgement: Some(LIVE_SPEND_ACKNOWLEDGEMENT.into()),
            },
            artifacts: ArtifactConfig {
                max_artifacts_per_execution: 4,
                max_encoded_bytes: 20_000_000,
                max_decoded_bytes: 100_000_000,
                max_width: 16_384,
                max_height: 16_384,
            },
            execution: ExecutionConfig {
                recovery_delays_seconds: vec![30, 120, 600],
                temporal_startup_timeout_ms: 5_000,
                dependency_check_interval_ms: 1_000,
            },
            logging: LoggingConfig {
                level: LogLevel::Info,
                format: LogFormat::Text,
            },
            shutdown: ShutdownConfig {
                worker_drain_timeout_ms: 30_000,
            },
        }
    }

    #[test]
    fn strict_config_accepts_only_safe_production_shape() {
        let root = tempdir().unwrap();
        let current = config(root.path());
        current.validate().unwrap();
        let serialized = serde_json::to_value(&current).unwrap();
        assert!(serialized["providers"].get("mode").is_none());
        assert!(serialized["hubu"].get("account_id").is_none());
        assert!(serialized["hubu"].get("agent_id").is_none());
        assert!(serialized["authentication"]
            .get("caller_account_id")
            .is_none());
        let round_trip: ServerConfig = serde_json::from_value(serialized).unwrap();
        round_trip.validate().unwrap();
        let mut legacy_contract = config(root.path());
        legacy_contract.hubu.expected_executor_contract = "hubu-spend-executor-v4.1".into();
        assert!(legacy_contract.validate().is_err());
        let mut value = serde_json::to_value(config(root.path())).unwrap();
        value["mock_hubu"] = serde_json::json!(true);
        assert!(serde_json::from_value::<ServerConfig>(value).is_err());
    }

    #[test]
    fn legacy_principal_bound_configs_receive_upgrade_diagnostics() {
        for schema_version in [1, 2] {
            let root = tempdir().unwrap();
            let mut legacy = serde_json::to_value(config(root.path())).unwrap();
            legacy["schema_version"] = serde_json::json!(schema_version);
            legacy["hubu"]["account_id"] = serde_json::json!("account-1");
            legacy["hubu"]["agent_id"] = serde_json::json!("agent-1");
            legacy["authentication"]["caller_account_id"] = serde_json::json!("account-1");
            let path = root.path().join("legacy-gongbu.json");
            fs::write(&path, serde_json::to_vec(&legacy).unwrap()).unwrap();

            let error = ServerConfig::from_path(&path).unwrap_err().to_string();
            assert!(error.contains(&format!("schema_version {schema_version} requires upgrade")));
            assert!(error.contains("static execution-principal bindings cannot be activated"));
        }
    }

    #[test]
    fn disabled_provider_mode_requires_no_fake_catalog_or_spend_gate() {
        let root = tempdir().unwrap();
        let mut value = config(root.path());
        value.schema_version = gongbu_build_info::SERVER_CONFIG_SCHEMA_VERSION;
        value.providers = ProvidersConfig {
            mode: ProviderMode::Disabled,
            target_catalog_path: None,
            pricing_catalog_path: None,
            maximum_spend_minor: None,
            live_spend_acknowledgement: None,
        };
        value.schema_version = 1;
        assert!(value.validate().is_err());
        value.schema_version = gongbu_build_info::SERVER_CONFIG_SCHEMA_VERSION;
        value.validate().unwrap();
        assert!(validated_provider_catalog(&value).is_ok());
        let path = root.path().join("gongbu.json");
        fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        validate_runtime_inputs(&path).unwrap();
        assert!(!root.path().join("state").exists());
        assert!(!root.path().join("artifacts").exists());

        value.providers.maximum_spend_minor = Some(1);
        assert!(value.validate().is_err());
    }

    #[test]
    fn sandbox_mode_accepts_only_the_builtin_fixture_without_live_acknowledgement() {
        let root = tempdir().unwrap();
        let targets = root.path().join("targets.json");
        let prices = root.path().join("prices.json");
        let mut value = config(root.path());
        fs::write(
            &targets,
            serde_json::to_vec(&serde_json::json!({
                "schema_version": 2,
                "provider_configs": [{
                    "provider_config_version": "hubu-sandbox-fixture-v1",
                    "workload_type": "image_generation",
                    "provider": "sandbox",
                    "adapter": "fixture",
                    "model": "deterministic-image-v1",
                    "secret_service": "hubu.sandbox.fixture",
                    "secret_account": "deterministic-provider",
                    "active": true,
                    "execution_enabled": true,
                    "settings": {"type": "fixture"}
                }]
            }))
            .unwrap(),
        )
        .unwrap();
        fs::write(
            &prices,
            serde_json::to_vec(&serde_json::json!({
                "schema_version": 2,
                "catalog_version": "hubu-sandbox-v1",
                "rules": [{
                    "rule_id": "sandbox-image-1k",
                    "provider": "sandbox",
                    "model": "deterministic-image-v1",
                    "currency": "USD",
                    "selector": {"image_size": "1k"},
                    "components": [{"unit": "image", "rate_numerator_minor": 1, "rate_denominator": 1}]
                }]
            }))
            .unwrap(),
        )
        .unwrap();

        value.providers = ProvidersConfig {
            mode: ProviderMode::Sandbox,
            target_catalog_path: Some(targets),
            pricing_catalog_path: Some(prices),
            maximum_spend_minor: Some(1_000_000),
            live_spend_acknowledgement: None,
        };
        value.validate().unwrap();
        validated_provider_catalog(&value).unwrap();

        value.providers.live_spend_acknowledgement = Some(LIVE_SPEND_ACKNOWLEDGEMENT.into());
        assert!(value.validate().is_err());
    }

    #[test]
    fn runtime_validator_rejects_unbound_live_catalog_without_side_effects() {
        let root = tempdir().unwrap();
        let value = config(root.path());
        let path = root.path().join("gongbu.json");
        fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(validate_runtime_inputs(&path).is_err());
        assert!(!root.path().join("state").exists());
        assert!(!root.path().join("artifacts").exists());
    }

    #[test]
    fn runtime_validator_accepts_isolated_gemini_and_supported_flux_without_side_effects() {
        let root = tempdir().unwrap();
        let value = config(root.path());
        let document: serde_json::Value =
            serde_json::from_str(include_str!("../../../contracts/provider-profiles-v1.json"))
                .unwrap();
        let profile = &document["profiles"][0];
        let policies = &profile["policies"];
        let mut flux_target = profile["target"].clone();
        flux_target["secret_service"] = serde_json::json!("operator.bfl");
        flux_target["secret_account"] = serde_json::json!("flux");
        let targets = serde_json::json!({
            "schema_version":3,
            "supported_profiles":[{
                "contract":profile["contract"],
                "pricing_version":profile["pricing_version"],
                "poll_policy":policies["poll"],
                "artifact_delivery_policy":policies["artifact_delivery"],
                "recovery_policy":policies["recovery"],
                "generation_retries":policies["generation_retries"],
                "fallback":policies["fallback"]
            }],
            "provider_configs":[
                {
                    "provider_config_version":"gemini-v1",
                    "workload_type":"image_generation",
                    "provider":"google",
                    "adapter":"gemini_developer_image",
                    "model":"gemini-image-v1",
                    "secret_service":"operator.google",
                    "secret_account":"gemini",
                    "active":true,
                    "execution_enabled":true,
                    "settings":{"type":"gemini_developer_image","config":{
                        "endpoint":"https://generativelanguage.googleapis.com",
                        "api_version":"v1beta","timeout_ms":30000,
                        "max_retries":0,"headers":{}
                    }}
                },
                flux_target
            ]
        });
        let mut pricing_rules = profile["pricing_rules"].as_array().unwrap().clone();
        pricing_rules.push(serde_json::json!({
            "rule_id":"gemini-v1","provider":"google","model":"gemini-image-v1",
            "currency":"USD","components":[{
                "unit":"image","rate_numerator_minor":4,"rate_denominator":1
            }]
        }));
        let pricing = serde_json::json!({
            "schema_version":2,
            "catalog_version":"operator-mixed-2026-08-28-v1",
            "rules":pricing_rules
        });
        fs::write(
            value.providers.target_catalog_path.as_ref().unwrap(),
            serde_json::to_vec(&targets).unwrap(),
        )
        .unwrap();
        fs::write(
            value.providers.pricing_catalog_path.as_ref().unwrap(),
            serde_json::to_vec(&pricing).unwrap(),
        )
        .unwrap();
        let path = root.path().join("gongbu.json");
        fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();

        let validated = validate_runtime_inputs(&path).unwrap();
        let catalog = validated_provider_catalog(&validated).unwrap();
        assert_eq!(catalog.supported_profiles().len(), 1);
        assert_eq!(catalog.targets().revisions().count(), 2);
        assert!(
            !catalog.supported_profiles()[0]
                .readiness
                .credential_reference_present
        );
        assert!(!catalog.supported_profiles()[0].readiness.live_qualified);
        assert!(!root.path().join("state").exists());
        assert!(!root.path().join("artifacts").exists());

        let mut shared_provider_credential = targets.clone();
        shared_provider_credential["provider_configs"][0]["secret_service"] =
            serde_json::json!("operator.bfl");
        shared_provider_credential["provider_configs"][0]["secret_account"] =
            serde_json::json!("flux");
        fs::write(
            value.providers.target_catalog_path.as_ref().unwrap(),
            serde_json::to_vec(&shared_provider_credential).unwrap(),
        )
        .unwrap();
        assert!(validate_runtime_inputs(&path).is_err());

        let mut bootstrap_collision = targets;
        let flux = bootstrap_collision["provider_configs"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|target| target["provider"] == "flux")
            .unwrap();
        flux["secret_service"] = serde_json::json!(value.hubu.credential_reference.service.clone());
        flux["secret_account"] = serde_json::json!(value.hubu.credential_reference.account.clone());
        fs::write(
            value.providers.target_catalog_path.as_ref().unwrap(),
            serde_json::to_vec(&bootstrap_collision).unwrap(),
        )
        .unwrap();
        assert!(validate_runtime_inputs(&path).is_err());
        assert!(!root.path().join("state").exists());
        assert!(!root.path().join("artifacts").exists());
    }

    #[test]
    fn runtime_validator_rejects_managed_temporal_version_mismatch() {
        let root = tempdir().unwrap();
        let mut value = config(root.path());
        value.schema_version = gongbu_build_info::SERVER_CONFIG_SCHEMA_VERSION;
        value.providers = ProvidersConfig {
            mode: ProviderMode::Disabled,
            target_catalog_path: None,
            pricing_catalog_path: None,
            maximum_spend_minor: None,
            live_spend_acknowledgement: None,
        };
        if let TemporalConfig::ManagedLocal {
            expected_cli_version,
            ..
        } = &mut value.temporal
        {
            *expected_cli_version = "9.9.9".into();
        }
        let path = root.path().join("gongbu.json");
        fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(validate_runtime_inputs(&path).is_err());
    }

    #[test]
    fn rejects_relative_state_and_non_loopback_dependencies() {
        let root = tempdir().unwrap();
        let mut value = config(root.path());
        value.state.database_path = "gongbu.sqlite3".into();
        assert!(value.validate().is_err());

        let mut value = config(root.path());
        value.hubu.endpoint = "http://example.com:8787".into();
        assert!(value.validate().is_err());
    }

    #[test]
    fn capability_authentication_is_identity_free_and_opaque() {
        let authenticator = CapabilityAuthenticator::new(b"secret").unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(header::AUTHORIZATION, "Bearer wrong".parse().unwrap());
        assert!(authenticator.authenticate(&headers).is_err());
        headers.insert(header::AUTHORIZATION, "Bearer secret".parse().unwrap());
        assert!(authenticator.authenticate(&headers).is_ok());
        assert!(!format!("{:?}", authenticator.token_digest).contains("secret"));
    }

    #[test]
    fn hubu_compatibility_requires_protected_access() {
        use std::{
            io::{Read, Write},
            net::TcpListener,
            thread,
        };

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            for _ in 0..3 {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = String::new();
                stream.read_to_string(&mut request).unwrap();
                let path = request
                    .lines()
                    .next()
                    .and_then(|line| line.split_whitespace().nth(1))
                    .unwrap();
                let (status, body) = match path {
                    "/health" => ("200 OK", r#"{"status":"ok"}"#),
                    "/version" => (
                        "200 OK",
                        r#"{"product_version":"0.1.0","executor_contract":"hubu-executor.v1"}"#,
                    ),
                    "/agents?operational_probe=gongbu_credential_check" => {
                        assert!(request.contains("Authorization: Bearer wrong-credential\r\n"));
                        ("401 Unauthorized", r#"{"error":"unauthorized"}"#)
                    }
                    _ => unreachable!(),
                };
                write!(
                    stream,
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
            }
        });
        let client = HubuClient::new(format!("http://{address}"))
            .with_bearer_token(b"wrong-credential".to_vec());

        assert!(!hubu_is_compatible(&client, "0.1.0", "hubu-executor.v1"));
        server.join().unwrap();
    }
}
