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
};
use axum::http::{header, HeaderMap};
use chrono::SecondsFormat;
use serde::Serialize;
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
use uuid::Uuid;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

pub async fn serve(config: SandboxConfig, preserve: Option<PathBuf>) -> Result<(), BoxError> {
    let mut run = SandboxRun::start(&config)?;
    let result = serve_started(&config, &mut run).await;
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
    result
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
    let side_effects = start_side_effect_writer(run.root().to_path_buf(), &wiring);

    let listener = into_tokio_listener(run.take_listener("gongbu")?)?;
    run.release_listener("temporal");
    run.release_listener("temporal_ui");
    run.mark_ready("gongbu", "Gongbu HTTP server and Temporal worker are ready")?;
    run.rewrite_manifest()?;

    println!("{}", serde_json::to_string_pretty(run.manifest())?);
    eprintln!("\nSandbox is running. Keep this terminal open.");
    eprintln!(
        "Submit from another terminal with:\n  cargo run -p gongbu-api --bin gongbu-sandbox -- submit --run-dir {} --operation-key manual-1 --prompt 'Draw a blue circle'",
        run.root().display()
    );
    eprintln!("Temporal UI: {}", run.manifest().temporal_ui_url);
    eprintln!("Press Ctrl+C to stop the sandbox.");

    let dependencies = ApplicationDependencies {
        repository,
        artifacts: artifact_service,
        providers: wiring.providers.clone(),
        hubu: hubu_activities,
        secrets,
        provider_activities: Some(provider_activities),
        artifact_activities: Some(artifact_activities),
        temporal_runtime,
        temporal_client,
        authenticator: Arc::new(SandboxAuthenticator { token }),
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
        crate::http::AuthenticatedAccount::from_verified_claim("sandbox-account")
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
