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
    secrets::{MacOsKeychain, SecretProvider, SecretReference},
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
    pub account_id: String,
    pub agent_id: String,
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
    pub caller_account_id: String,
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
    pub target_catalog_path: PathBuf,
    pub pricing_catalog_path: PathBuf,
    pub maximum_spend_minor: i64,
    pub live_spend_acknowledgement: String,
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
        let config: Self = serde_json::from_slice(&fs::read(path)?)?;
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
        validate_file_path(&self.providers.target_catalog_path, "target_catalog_path")?;
        validate_file_path(&self.providers.pricing_catalog_path, "pricing_catalog_path")?;
        if self.providers.maximum_spend_minor <= 0
            || self.providers.live_spend_acknowledgement != LIVE_SPEND_ACKNOWLEDGEMENT
        {
            return Err(invalid(
                "provider spend ceiling and explicit live-spend acknowledgement are required",
            ));
        }
        if self.hubu.expected_product_version.trim().is_empty()
            || self.hubu.expected_executor_contract != gongbu_build_info::HUBU_EXECUTOR_CONTRACT
            || self.hubu.account_id.trim().is_empty()
            || self.hubu.agent_id.trim().is_empty()
            || self.authentication.caller_account_id != self.hubu.account_id
        {
            return Err(invalid(
                "invalid or contradictory Hubu identity/contract settings",
            ));
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
    let config = ServerConfig::from_path(config_path)?;
    serve_config(config).await
}

pub async fn serve_config(mut config: ServerConfig) -> Result<(), BoxError> {
    config.validate()?;
    prepare_state_paths(&config)?;
    normalize_paths(&mut config)?;

    let secrets: Arc<dyn SecretProvider> = Arc::new(MacOsKeychain);
    let caller_secret = secrets
        .resolve(
            &config
                .authentication
                .bearer_credential_reference
                .validated()?,
        )
        .map_err(|_| invalid("caller capability credential is unavailable"))?;
    let hubu_secret = secrets
        .resolve(&config.hubu.credential_reference.validated()?)
        .map_err(|_| invalid("Hubu scoped credential is unavailable"))?;

    let targets = ProviderTargetConfig::from_path(&config.providers.target_catalog_path)
        .map_err(|error| invalid(format!("provider target catalog: {error}")))?;
    reject_fixture_targets(&targets)?;
    let mut redaction_values = vec![
        caller_secret.expose().to_vec(),
        hubu_secret.expose().to_vec(),
    ];
    for target in targets
        .revisions()
        .filter(|target| target.is_execution_enabled())
    {
        let secret = secrets
            .resolve(
                &target
                    .secret_reference()
                    .map_err(|_| invalid("provider credential reference is invalid"))?,
            )
            .map_err(|_| invalid("an enabled provider credential is unavailable"))?;
        redaction_values.push(secret.expose().to_vec());
    }
    let pricing = PricingCatalog::load(&config.providers.pricing_catalog_path)
        .map_err(|error| invalid(format!("pricing catalog: {error}")))?;
    let limits = config.artifacts.limits();
    let providers =
        ValidatedProviderCatalog::bind(targets, pricing, &ProviderRegistry::production(&limits))
            .map_err(|error| invalid(format!("provider catalog binding: {error}")))?;

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
        health_client.health().is_ok()
            && health_client.version().is_ok_and(|version| {
                version.product_version == expected_hubu_version
                    && version.executor_contract == expected_hubu_contract
            })
    });
    let hubu = Arc::new(
        ProductionHubuActivities::new(hubu_client, config.hubu.agent_id.clone())
            .map_err(invalid)?,
    );

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
    let authenticator = Arc::new(CapabilityAuthenticator::new(
        caller_secret.expose(),
        config.authentication.caller_account_id.clone(),
    )?);
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
        hubu,
        secrets,
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
        maximum_spend_minor: config.providers.maximum_spend_minor,
        dependency_checker: Some(dependency_checker),
        worker_drain_timeout: Duration::from_millis(config.shutdown.worker_drain_timeout_ms),
        authenticator,
        now,
    };
    let result = application::serve(listener, dependencies, shutdown_signal()).await;
    if let Some(child) = temporal_child.as_mut() {
        child.stop();
    }
    result
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
    config.providers.target_catalog_path = fs::canonicalize(&config.providers.target_catalog_path)?;
    config.providers.pricing_catalog_path =
        fs::canonicalize(&config.providers.pricing_catalog_path)?;
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

async fn wait_for_hubu_compatibility(
    config: &HubuConfig,
    client: &HubuClient,
) -> Result<(), BoxError> {
    let deadline = tokio::time::Instant::now() + Duration::from_millis(config.startup_timeout_ms);
    loop {
        let compatible = client.health().is_ok()
            && client.version().is_ok_and(|version| {
                version.product_version == config.expected_product_version
                    && version.executor_contract == config.expected_executor_contract
            });
        if compatible {
            return Ok(());
        }
        if config.startup_policy == StartupPolicy::Exit || tokio::time::Instant::now() >= deadline {
            return Err(io::Error::other("Hubu is unavailable or incompatible").into());
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
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
        expected_cli_version,
        data_path,
        rpc_port,
        ui_port,
        ..
    } = config
    else {
        return Ok(None);
    };
    let version = Command::new(binary_path).arg("--version").output()?;
    let reported = format!(
        "{}{}",
        String::from_utf8_lossy(&version.stdout),
        String::from_utf8_lossy(&version.stderr)
    );
    if !version.status.success()
        || !reported.split_whitespace().any(|part| {
            part.trim_start_matches('v') == expected_cli_version.trim_start_matches('v')
        })
    {
        return Err(io::Error::other(
            "managed Temporal CLI version does not match the configured pin",
        )
        .into());
    }
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
    account_id: String,
}

impl CapabilityAuthenticator {
    fn new(token: &[u8], account_id: String) -> Result<Self, ServerError> {
        if token.is_empty() || account_id.trim().is_empty() {
            return Err(invalid("invalid caller capability"));
        }
        Ok(Self {
            token_digest: Sha256::digest(token).into(),
            account_id,
        })
    }
}

impl Authenticator for CapabilityAuthenticator {
    fn authenticate(
        &self,
        headers: &HeaderMap,
    ) -> Result<crate::http::AuthenticatedAccount, AuthenticationError> {
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
        crate::http::AuthenticatedAccount::from_verified_claim(&self.account_id)
            .map_err(|_| AuthenticationError)
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
        fs::write(&binary, "binary").unwrap();
        ServerConfig {
            schema_version: 1,
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
                account_id: "account-1".into(),
                agent_id: "agent-1".into(),
                credential_reference: SecretReferenceConfig {
                    service: "gongbu.hubu".into(),
                    account: "local".into(),
                },
                startup_policy: StartupPolicy::Exit,
                startup_timeout_ms: 1_000,
            },
            authentication: AuthenticationConfig {
                caller_account_id: "account-1".into(),
                bearer_credential_reference: SecretReferenceConfig {
                    service: "gongbu.caller".into(),
                    account: "local".into(),
                },
            },
            providers: ProvidersConfig {
                target_catalog_path: targets,
                pricing_catalog_path: prices,
                maximum_spend_minor: 100,
                live_spend_acknowledgement: LIVE_SPEND_ACKNOWLEDGEMENT.into(),
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
        config(root.path()).validate().unwrap();
        let mut value = serde_json::to_value(config(root.path())).unwrap();
        value["mock_hubu"] = serde_json::json!(true);
        assert!(serde_json::from_value::<ServerConfig>(value).is_err());
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
    fn capability_authentication_is_account_bound_and_opaque() {
        let authenticator = CapabilityAuthenticator::new(b"secret", "account-1".into()).unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(header::AUTHORIZATION, "Bearer wrong".parse().unwrap());
        assert!(authenticator.authenticate(&headers).is_err());
        headers.insert(header::AUTHORIZATION, "Bearer secret".parse().unwrap());
        assert!(authenticator.authenticate(&headers).is_ok());
        assert!(!format!("{:?}", authenticator.token_digest).contains("secret"));
    }
}
