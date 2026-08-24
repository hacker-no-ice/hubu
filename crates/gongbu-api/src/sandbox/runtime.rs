//! Long-running manual-test runtime for the sandbox launcher.

use super::{HubuWiring, ProviderWiring, SandboxConfig, SandboxRun, SandboxWiring, TemporalMode};
use crate::{
    application::{
        self, ApplicationDependencies, ArtifactServiceActivities, AuthenticationError,
        Authenticator,
    },
    artifact::{ArtifactLimits, ArtifactService, LocalFsStorage},
    execution::Repository,
    redaction::Redactor,
    secrets::{MacOsKeychain, SecretProvider},
    temporal::TemporalWorkerConfig,
};
use axum::http::{header, HeaderMap};
use chrono::SecondsFormat;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io,
    net::TcpListener,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    str::FromStr,
    sync::Arc,
    time::Duration,
};
use temporalio_client::{Client, ClientOptions, Connection, ConnectionOptions, Url};
use temporalio_sdk::Runtime;
use tokio::{net::TcpStream, task::JoinHandle, time::Instant};

const MANAGED_HUBU_SPEND_TIMING_YAML: &str = r#"default_profile: default
profiles:
  default:
    authorization_ttl_seconds: 300
    claim_ttl_seconds: 900
  image_generation:
    authorization_ttl_seconds: 300
    claim_ttl_seconds: 900
"#;
use uuid::Uuid;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

pub async fn serve(mut config: SandboxConfig, preserve: Option<PathBuf>) -> Result<(), BoxError> {
    let mut run = SandboxRun::start(&config)?;
    let mut hubu_child = None;
    let result = async {
        hubu_child = start_managed_hubu(&mut config, &mut run).await?;
        prepare_external_hubu_auth(&mut config)?;
        serve_started(&config, &mut run).await
    }
    .await;
    if let Some(destination) = preserve {
        let destination = run.preserve(destination)?;
        eprintln!(
            "Sandbox stopped; diagnostics preserved at {}",
            destination.display()
        );
    } else if result.is_err() && config.preserve_diagnostics_on_failure {
        let destination = run.preserve_in_place()?;
        eprintln!(
            "Sandbox failed; diagnostics preserved at {}",
            destination.display()
        );
    } else {
        eprintln!("Sandbox stopped; temporary state cleaned up.");
    }
    drop(hubu_child.take());
    result
}

fn prepare_external_hubu_auth(config: &mut SandboxConfig) -> Result<(), BoxError> {
    if config.hubu.mode != super::BoundaryMode::Real || config.hubu.runtime_bearer_token.is_some() {
        return Ok(());
    }
    let reference = super::parse_secret_reference(
        config
            .hubu
            .scoped_credential_reference
            .as_deref()
            .ok_or_else(|| io::Error::other("real Hubu credential reference is missing"))?,
        "real Hubu scoped credential reference",
    )?;
    let secret = MacOsKeychain
        .resolve(&reference)
        .map_err(|_| io::Error::other("real Hubu scoped credential is unavailable"))?;
    config.hubu.runtime_bearer_token = Some(super::RuntimeSecret(secret.expose().to_vec()));
    Ok(())
}

async fn serve_started(config: &SandboxConfig, run: &mut SandboxRun) -> Result<(), BoxError> {
    let wiring = SandboxWiring::from_config(config)?;
    let token = format!("gongbu-sandbox-{}", Uuid::new_v4());
    write_operator_token(run.root(), &token)?;

    let mut temporal_child = start_temporal(config, run)?;
    let temporal_address = run.manifest().temporal_address.clone();
    wait_for_temporal(
        &temporal_address,
        temporal_child.as_mut(),
        Duration::from_secs(30),
    )
    .await?;
    run.mark_ready("temporal", "Temporal service and UI are reachable")?;

    let connection =
        Connection::connect(ConnectionOptions::new(Url::from_str(&temporal_address)?).build())
            .await?;
    let temporal_client = Client::new(
        connection,
        ClientOptions::new(config.temporal.namespace.clone()).build(),
    )?;
    let temporal_runtime = Arc::new(Runtime::new_assume_tokio(Default::default())?);

    let repository = Repository::open(
        Path::new(&run.manifest().database_path),
        Redactor::new([token.as_bytes()]),
    )?;
    let artifact_service = ArtifactService::new(
        repository.clone(),
        LocalFsStorage::new(&run.manifest().artifact_root),
        ArtifactLimits::default(),
    );
    artifact_service.preflight()?;
    let now = Arc::new(|| chrono::Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true));
    let base_artifacts = Arc::new(ArtifactServiceActivities::new(artifact_service.clone(), {
        let now = now.clone();
        move || now()
    }));
    let artifact_activities = wiring.artifact_activities(base_artifacts);
    let secrets: Arc<dyn SecretProvider> = Arc::new(MacOsKeychain);
    let provider_activities = wiring.provider.activities(secrets.clone());
    let hubu_activities = wiring.hubu.activities();
    let hubu_authorizations = wiring.hubu.authorization_resolver();
    let side_effects = start_side_effect_writer(run.root().to_path_buf(), &wiring);

    let listener = into_tokio_listener(run.take_listener("gongbu")?)?;
    run.release_listener("temporal");
    run.release_listener("temporal_ui");
    run.mark_ready("gongbu", "Gongbu HTTP server and Temporal worker are ready")?;
    run.rewrite_manifest()?;

    println!("{}", serde_json::to_string_pretty(run.manifest())?);
    eprintln!("\nSandbox is running. Keep this terminal open.");
    eprintln!(
        "Submit from another terminal with:\n  gongbu-sandbox submit --run-dir {} --operation-key manual-1 --prompt 'Draw a blue circle'",
        run.root().display()
    );
    eprintln!("Temporal UI: {}", run.manifest().temporal_ui_url);
    eprintln!("Press Ctrl+C to stop the sandbox.");

    let dependencies = ApplicationDependencies {
        repository,
        artifacts: artifact_service,
        providers: wiring.providers.clone(),
        hubu: hubu_activities,
        hubu_authorizations,
        secrets,
        provider_activities: Some(provider_activities),
        artifact_activities: Some(artifact_activities),
        temporal_runtime,
        temporal_client,
        temporal_worker: TemporalWorkerConfig::default(),
        temporal_namespace: config.temporal.namespace.clone(),
        temporal_startup_timeout: Duration::from_secs(30),
        dependency_check_interval: Duration::from_secs(5),
        dependency_failure_grace: application::DEPENDENCY_FAILURE_GRACE,
        maximum_spend_minor: config
            .provider
            .maximum_spend_minor
            .unwrap_or(config.hubu.maximum_authorization_minor),
        dependency_checker: None,
        worker_drain_timeout: Duration::from_secs(30),
        authenticator: Arc::new(SandboxAuthenticator {
            token,
            account_id: config
                .hubu
                .isolated_test_account
                .clone()
                .unwrap_or_else(|| "sandbox-account".into()),
        }),
        now,
    };
    let result = application::serve(listener, dependencies, async {
        let _ = tokio::signal::ctrl_c().await;
    })
    .await;

    side_effects.abort();
    write_side_effects(run.root(), &wiring)?;
    drop(temporal_child);

    result?;
    Ok(())
}

struct SandboxAuthenticator {
    token: String,
    account_id: String,
}

impl Authenticator for SandboxAuthenticator {
    fn authenticate(
        &self,
        headers: &HeaderMap,
    ) -> Result<crate::http::AuthenticatedAccount, AuthenticationError> {
        let expected = format!("Bearer {}", self.token);
        if headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            != Some(expected.as_str())
        {
            return Err(AuthenticationError);
        }
        crate::http::AuthenticatedAccount::from_verified_claim(&self.account_id)
            .map_err(|_| AuthenticationError)
    }
}

fn into_tokio_listener(listener: TcpListener) -> Result<tokio::net::TcpListener, io::Error> {
    listener.set_nonblocking(true)?;
    tokio::net::TcpListener::from_std(listener)
}

struct ManagedChild(Child);

impl ManagedChild {
    fn try_wait(&mut self) -> io::Result<Option<std::process::ExitStatus>> {
        self.0.try_wait()
    }
}

impl Drop for ManagedChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ManagedHubuContext {
    pub cli_binary: String,
    pub endpoint: String,
    pub auth_token_file: String,
    pub agent_id: String,
    pub account_id: String,
}

async fn start_managed_hubu(
    config: &mut SandboxConfig,
    run: &mut SandboxRun,
) -> Result<Option<ManagedChild>, BoxError> {
    let Some(release) = run.managed_hubu().cloned() else {
        return Ok(None);
    };
    run.release_listener("hubu");
    let hubu_root = run.root().join("hubu");
    fs::create_dir_all(&hubu_root)?;
    let database = hubu_root.join("hubu.sqlite3");
    let auth_token = hubu_root.join("hubu.auth-token");
    let reconciliation_token = hubu_root.join("hubu.reconciliation-token");
    let spend_timing = hubu_root.join("spend-timing.yaml");
    fs::write(&spend_timing, MANAGED_HUBU_SPEND_TIMING_YAML)?;
    let log_path = run.root().join("logs/hubu.jsonl");
    let stdout = File::create(run.root().join("logs/hubu-process.log"))?;
    let stderr = stdout.try_clone()?;
    let endpoint = run.manifest().hubu_endpoint.clone();
    let address = endpoint.trim_start_matches("http://");
    let child = Command::new(&release.server_binary)
        .arg(address)
        .env("HUBU_DB_PATH", &database)
        .env("HUBU_AUTH_TOKEN_FILE", &auth_token)
        .env("HUBU_RECONCILIATION_TOKEN_FILE", &reconciliation_token)
        .env("HUBU_SPEND_TIMING_CONFIG", &spend_timing)
        .env("HUBU_LOG_FILE", &log_path)
        .env("HUBU_LOG_STDERR", "0")
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .spawn()?;
    let mut child = ManagedChild(child);
    wait_for_hubu(&endpoint, &mut child, Duration::from_secs(15)).await?;

    let reported: serde_json::Value = serde_json::from_slice(&run_hubu_cli(
        &release.cli_binary,
        None,
        None,
        &["version"],
    )?)?;
    if reported
        .get("product_version")
        .and_then(|value| value.as_str())
        != Some(release.version.as_str())
        || reported
            .get("source_commit")
            .and_then(|value| value.as_str())
            != Some(release.source_commit.as_str())
        || reported
            .get("executor_contract")
            .and_then(|value| value.as_str())
            != Some(release.executor_contract.as_str())
    {
        return Err(
            io::Error::other("Hubu binary version metadata does not match provenance").into(),
        );
    }
    run_hubu_cli(
        &release.cli_binary,
        Some(&endpoint),
        Some(&auth_token),
        &["health"],
    )?;
    run_hubu_cli(
        &release.cli_binary,
        Some(&endpoint),
        Some(&auth_token),
        &[
            "register",
            "human",
            "--username",
            "gongbu-sandbox",
            "--display-name",
            "Gongbu Sandbox",
        ],
    )?;
    let registration = run_hubu_cli(
        &release.cli_binary,
        Some(&endpoint),
        Some(&auth_token),
        &[
            "register",
            "agent",
            "--name",
            "gongbu-sandbox",
            "--version",
            &release.version,
        ],
    )?;
    let registration = String::from_utf8(registration)?;
    let agent_id = output_field(&registration, "agent_id")?;
    let account_id = output_field(&registration, "account_id")?;
    let budget = cents_as_amount(config.hubu.maximum_authorization_minor);
    run_hubu_cli(
        &release.cli_binary,
        Some(&endpoint),
        Some(&auth_token),
        &[
            "budget",
            "create",
            "--agent-id",
            &agent_id,
            "--amount",
            &budget,
        ],
    )?;
    let policy = hubu_root.join("sandbox-policy.yaml");
    fs::write(
        &policy,
        "id: gongbu_sandbox_policy\nversion: v1\ndefault_effect: deny\nrules:\n  - id: allow_sandbox\n    effect: allow\n    reason: deterministic Gongbu compatibility fixture\n    when:\n      op: lte\n      field: amount\n      value:\n        money_cents: 10000\n",
    )?;
    run_hubu_cli(
        &release.cli_binary,
        Some(&endpoint),
        Some(&auth_token),
        &["policy", "add", "--path", &policy.display().to_string()],
    )?;

    let context = ManagedHubuContext {
        cli_binary: release.cli_binary.display().to_string(),
        endpoint: endpoint.clone(),
        auth_token_file: auth_token.display().to_string(),
        agent_id: agent_id.clone(),
        account_id: account_id.clone(),
    };
    write_private_json(&run.root().join("hubu-context.json"), &context)?;
    config.hubu.release = None;
    config.hubu.endpoint = Some(endpoint);
    config.hubu.scoped_credential_reference = Some("managed:hubu-sandbox".into());
    config.hubu.isolated_test_account = Some(account_id.clone());
    config.hubu.agent_id = agent_id.clone();
    let mut token = fs::read(&auth_token)?;
    while token
        .last()
        .is_some_and(|byte| matches!(byte, b'\n' | b'\r'))
    {
        token.pop();
    }
    config.hubu.runtime_bearer_token = Some(super::RuntimeSecret(token));
    run.set_hubu_fixture(agent_id, account_id)?;
    run.mark_ready(
        "hubu",
        &format!(
            "{} ({}) / {} is healthy with isolated fixture",
            release.version, release.source_commit, release.executor_contract
        ),
    )?;
    Ok(Some(child))
}

async fn wait_for_hubu(
    endpoint: &str,
    child: &mut ManagedChild,
    timeout: Duration,
) -> Result<(), BoxError> {
    let url = reqwest::Url::parse(endpoint)?;
    let host = url
        .host_str()
        .ok_or_else(|| io::Error::other("Hubu endpoint has no host"))?;
    let port = url
        .port_or_known_default()
        .ok_or_else(|| io::Error::other("Hubu endpoint has no port"))?;
    let deadline = Instant::now() + timeout;
    loop {
        if TcpStream::connect((host, port)).await.is_ok() {
            return Ok(());
        }
        if let Some(status) = child.try_wait()? {
            return Err(
                io::Error::other(format!("Hubu exited before readiness with {status}")).into(),
            );
        }
        if Instant::now() >= deadline {
            return Err(io::Error::other("Hubu readiness timed out").into());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

fn run_hubu_cli(
    binary: &Path,
    endpoint: Option<&str>,
    auth_token: Option<&Path>,
    args: &[&str],
) -> Result<Vec<u8>, BoxError> {
    let mut command = Command::new(binary);
    if let Some(endpoint) = endpoint {
        command.args(["--url", endpoint]);
    }
    command.args(args);
    if let Some(auth_token) = auth_token {
        command.env("HUBU_AUTH_TOKEN_FILE", auth_token);
    }
    let output = command.output()?;
    if !output.status.success() {
        return Err(io::Error::other(format!(
            "Hubu CLI failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))
        .into());
    }
    Ok(output.stdout)
}

fn output_field(output: &str, name: &str) -> Result<String, BoxError> {
    output
        .lines()
        .find_map(|line| line.trim().strip_prefix(&format!("{name}:")))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| io::Error::other(format!("Hubu CLI output omitted {name}")).into())
}

fn cents_as_amount(cents: i64) -> String {
    format!("{}.{:02}", cents / 100, cents.abs() % 100)
}

fn write_private_json(path: &Path, value: &impl Serialize) -> Result<(), BoxError> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    serde_json::to_writer_pretty(options.open(path)?, value)?;
    Ok(())
}

fn start_temporal(
    config: &SandboxConfig,
    run: &mut SandboxRun,
) -> Result<Option<ManagedChild>, BoxError> {
    if config.temporal.mode == TemporalMode::External {
        run.release_listener("temporal");
        run.release_listener("temporal_ui");
        return Ok(None);
    }
    run.release_listener("temporal");
    run.release_listener("temporal_ui");
    let log_path = run.root().join("logs/temporal.log");
    let stdout = File::create(&log_path)?;
    let stderr = stdout.try_clone()?;
    let database = run.root().join("workflow/temporal.sqlite");
    let child = Command::new(&config.temporal.binary)
        .args([
            "server",
            "start-dev",
            "--ip",
            "127.0.0.1",
            "--port",
            &run.manifest().ports["temporal"].to_string(),
            "--ui-port",
            &run.manifest().ports["temporal_ui"].to_string(),
            "--db-filename",
            &database.display().to_string(),
        ])
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .spawn()
        .map_err(|error| {
            io::Error::new(
                error.kind(),
                format!(
                    "could not start Temporal CLI '{}': {error}; install it with `brew install temporal` or configure external Temporal",
                    config.temporal.binary
                ),
            )
        })?;
    Ok(Some(ManagedChild(child)))
}

async fn wait_for_temporal(
    address: &str,
    mut child: Option<&mut ManagedChild>,
    timeout: Duration,
) -> Result<(), BoxError> {
    let url = reqwest::Url::parse(address)?;
    let host = url
        .host_str()
        .ok_or_else(|| io::Error::other("Temporal address has no host"))?;
    let port = url
        .port_or_known_default()
        .ok_or_else(|| io::Error::other("Temporal address has no port"))?;
    let deadline = Instant::now() + timeout;
    loop {
        if TcpStream::connect((host, port)).await.is_ok() {
            return Ok(());
        }
        if let Some(child) = child.as_mut() {
            if let Some(status) = child.try_wait()? {
                return Err(io::Error::other(format!(
                    "Temporal exited before readiness with {status}"
                ))
                .into());
            }
        }
        if Instant::now() >= deadline {
            return Err(io::Error::other("Temporal readiness timed out").into());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

fn write_operator_token(root: &Path, token: &str) -> Result<(), io::Error> {
    let path = root.join("operator-token");
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    use std::io::Write;
    writeln!(options.open(path)?, "{token}")
}

fn start_side_effect_writer(root: PathBuf, wiring: &SandboxWiring) -> JoinHandle<()> {
    let hubu = match &wiring.hubu {
        HubuWiring::Mock(value) => Some(value.clone()),
        HubuWiring::Real(_) => None,
    };
    let provider = match &wiring.provider {
        ProviderWiring::Mock(value) => Some(value.clone()),
        ProviderWiring::Real { .. } => None,
    };
    tokio::spawn(async move {
        loop {
            let _ = write_side_effect_values(&root, hubu.as_ref(), provider.as_ref());
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    })
}

fn write_side_effects(root: &Path, wiring: &SandboxWiring) -> Result<(), io::Error> {
    let hubu = match &wiring.hubu {
        HubuWiring::Mock(value) => Some(value),
        HubuWiring::Real(_) => None,
    };
    let provider = match &wiring.provider {
        ProviderWiring::Mock(value) => Some(value),
        ProviderWiring::Real { .. } => None,
    };
    write_side_effect_values(root, hubu, provider)
}

#[derive(Serialize)]
struct SideEffects {
    mock_hubu_calls: Vec<super::SafeHubuCall>,
    mock_hubu_financial_mutations: usize,
    mock_provider_calls: Vec<super::SafeProviderCall>,
    mock_provider_invocations: usize,
}

fn write_side_effect_values(
    root: &Path,
    hubu: Option<&Arc<super::MockHubu>>,
    provider: Option<&Arc<super::DeterministicProvider>>,
) -> Result<(), io::Error> {
    let value = SideEffects {
        mock_hubu_calls: hubu.map(|value| value.safe_calls()).unwrap_or_default(),
        mock_hubu_financial_mutations: hubu
            .map(|value| value.financial_mutations())
            .unwrap_or_default(),
        mock_provider_calls: provider.map(|value| value.safe_calls()).unwrap_or_default(),
        mock_provider_invocations: provider
            .map(|value| value.invocation_count())
            .unwrap_or_default(),
    };
    fs::write(
        root.join("mock-side-effects.json"),
        serde_json::to_vec_pretty(&value).map_err(io::Error::other)?,
    )
}
