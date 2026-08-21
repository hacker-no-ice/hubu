use base64::{engine::general_purpose::STANDARD, Engine};
use gongbu_api::{
    application::{ApplicationDependencies, ArtifactServiceActivities, Authenticator},
    artifact::{ArtifactLimits, ArtifactService, LocalFsStorage},
    execution::{Execution, Repository},
    http::{ArtifactListResponse, AuthenticatedAccount, ExecutionResponse, ExecutionStatus},
    hubu::{BudgetHold, ExecutorSpendResponse, HttpClientError, SpendAuthorizationResolver},
    provider::{
        contract::{
            AdapterCapabilities, AdapterOutcome, NormalizedRequest, PricingCatalog,
            ProviderAdapter, ProviderFailure,
        },
        registry::{ProviderRegistry, ValidatedProviderCatalog},
        targets::ProviderTargetConfig,
    },
    redaction::Redactor,
    secrets::{ProviderSecret, SecretError, SecretProvider, SecretReference},
    temporal::TemporalWorkerConfig,
    workflow::{
        ActivityError, HubuActivities, ProviderActivities, ProviderArtifact, ProviderSuccess,
    },
};
use reqwest::StatusCode;
use serde_json::{json, Value};
use std::{
    net::TcpListener,
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
use temporalio_client::{
    Client, ClientOptions, Connection, ConnectionOptions, UntypedWorkflow, Url,
    WorkflowDescribeOptions,
};
use temporalio_sdk::Runtime;

const CALLER_TOKEN: &str = "hub-71-caller";
const TASK_QUEUE: &str = "hub-71-terminal-failure-isolation";

#[test]
#[ignore = "run through scripts/integration-hub-71-terminal-failure-isolation.sh"]
fn persistent_server_isolates_terminal_execution_failures() {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(run());
}

async fn run() {
    let root = tempfile::tempdir().unwrap();
    let temporal_port = free_port();
    let mut http_port = free_port();
    while http_port == temporal_port {
        http_port = free_port();
    }
    let mut temporal = TemporalChild::start(temporal_port, root.path());
    wait_for_port(temporal_port).await;

    let connection = Connection::connect(
        ConnectionOptions::new(
            Url::try_from(format!("http://127.0.0.1:{temporal_port}").as_str()).unwrap(),
        )
        .build(),
    )
    .await
    .unwrap();
    let temporal_client =
        Client::new(connection, ClientOptions::new("default".to_owned()).build()).unwrap();
    let temporal_runtime = Arc::new(Runtime::new_assume_tokio(Default::default()).unwrap());

    let repository = Repository::open(root.path().join("gongbu.sqlite3"), Redactor::default())
        .expect("open Gongbu state");
    let artifacts_service = ArtifactService::new(
        repository.clone(),
        LocalFsStorage::new(root.path().join("artifacts")),
        ArtifactLimits::default(),
    );
    let dependencies_healthy = Arc::new(AtomicBool::new(true));
    let health = dependencies_healthy.clone();
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", http_port))
        .await
        .unwrap();
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(gongbu_api::application::serve(
        listener,
        ApplicationDependencies {
            repository: repository.clone(),
            artifacts: artifacts_service.clone(),
            providers: catalog(),
            hubu: Arc::new(ScenarioHubu),
            hubu_authorizations: Arc::new(ScenarioHubu),
            secrets: Arc::new(UnavailableSecrets),
            provider_activities: Some(Arc::new(ScenarioProvider)),
            artifact_activities: Some(Arc::new(ArtifactServiceActivities::new(
                artifacts_service.clone(),
                || "2026-08-17T00:00:00Z".into(),
            ))),
            temporal_runtime: temporal_runtime.clone(),
            temporal_client: temporal_client.clone(),
            temporal_worker: TemporalWorkerConfig {
                task_queue: TASK_QUEUE.into(),
                recovery_delays_seconds: vec![1],
            },
            temporal_namespace: "default".into(),
            temporal_startup_timeout: Duration::from_secs(15),
            dependency_check_interval: Duration::from_millis(100),
            maximum_spend_minor: 100,
            dependency_checker: Some(Arc::new(move || health.load(Ordering::SeqCst))),
            worker_drain_timeout: Duration::from_secs(15),
            authenticator: Arc::new(TestAuthenticator),
            now: Arc::new(|| "2026-08-17T00:00:00Z".into()),
        },
        async move {
            let _ = shutdown_rx.await;
        },
    ));

    let base_url = format!("http://127.0.0.1:{http_port}");
    let client = reqwest::Client::new();
    wait_until_ready(&client, &base_url).await;

    for (operation_key, expected_status) in [
        ("hubu-401", ExecutionStatus::Failed),
        ("hubu-scope-mismatch", ExecutionStatus::Failed),
        ("hubu-expired", ExecutionStatus::Failed),
        ("hubu-other-proven", ExecutionStatus::Failed),
        ("provider-rejected", ExecutionStatus::Released),
    ] {
        let execution = submit(&client, &base_url, operation_key).await;
        let terminal = wait_for_terminal(&client, &base_url, &execution.execution_id).await;
        assert_eq!(terminal.status, expected_status);
        assert_eq!(ready_status(&client, &base_url).await, StatusCode::OK);
        assert!(
            temporal.is_running(),
            "managed Temporal exited after {operation_key}"
        );
        assert!(!server.is_finished(), "Gongbu exited after {operation_key}");
    }

    let valid = submit(&client, &base_url, "valid-after-failures").await;
    let completed = wait_for_terminal(&client, &base_url, &valid.execution_id).await;
    assert_eq!(completed.status, ExecutionStatus::Succeeded);
    let workflow_id = format!("gongbu-execution-{}", valid.execution_id);
    let workflow = temporal_client
        .get_workflow_handle::<UntypedWorkflow>(&workflow_id)
        .describe(WorkflowDescribeOptions::builder().build())
        .await
        .expect("discover the execution workflow through Temporal");
    assert_eq!(workflow.id(), workflow_id);
    assert!(workflow.history_length() > 0);

    let artifacts = client
        .get(format!(
            "{base_url}/v1/executions/{}/artifacts",
            valid.execution_id
        ))
        .bearer_auth(CALLER_TOKEN)
        .send()
        .await
        .unwrap()
        .json::<ArtifactListResponse>()
        .await
        .unwrap();
    assert_eq!(artifacts.artifacts.len(), 1);
    let artifact = &artifacts.artifacts[0];
    assert_eq!(artifact.execution_id, valid.execution_id);
    assert_eq!(artifact.media_type, "image/png");
    let artifact_bytes = client
        .get(format!("{base_url}/v1/artifacts/{}", artifact.artifact_id))
        .bearer_auth(CALLER_TOKEN)
        .send()
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    assert_eq!(
        artifact_bytes.as_ref(),
        STANDARD
            .decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=")
            .unwrap()
    );
    assert_eq!(ready_status(&client, &base_url).await, StatusCode::OK);
    assert!(temporal.is_running());
    assert!(!server.is_finished());

    shutdown_tx
        .send(())
        .expect("request graceful Gongbu shutdown before restart");
    tokio::time::timeout(Duration::from_secs(15), server)
        .await
        .expect("Gongbu must stop gracefully before restart")
        .unwrap()
        .unwrap();
    assert!(temporal.is_running());

    drop(artifacts_service);
    drop(repository);
    drop(temporal_client);
    drop(temporal_runtime);

    let repository = Repository::open(root.path().join("gongbu.sqlite3"), Redactor::default())
        .expect("reopen Gongbu state after process restart");
    let artifacts_service = ArtifactService::new(
        repository.clone(),
        LocalFsStorage::new(root.path().join("artifacts")),
        ArtifactLimits::default(),
    );
    artifacts_service
        .preflight()
        .expect("reopen artifact root after process restart");
    let connection = Connection::connect(
        ConnectionOptions::new(
            Url::try_from(format!("http://127.0.0.1:{temporal_port}").as_str()).unwrap(),
        )
        .build(),
    )
    .await
    .expect("reconnect to persisted Temporal service");
    let temporal_client =
        Client::new(connection, ClientOptions::new("default".to_owned()).build()).unwrap();
    let temporal_runtime = Arc::new(Runtime::new_assume_tokio(Default::default()).unwrap());

    let health = dependencies_healthy.clone();
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", http_port))
        .await
        .unwrap();
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(gongbu_api::application::serve(
        listener,
        ApplicationDependencies {
            repository,
            artifacts: artifacts_service.clone(),
            providers: catalog(),
            hubu: Arc::new(ScenarioHubu),
            hubu_authorizations: Arc::new(ScenarioHubu),
            secrets: Arc::new(UnavailableSecrets),
            provider_activities: Some(Arc::new(ScenarioProvider)),
            artifact_activities: Some(Arc::new(ArtifactServiceActivities::new(
                artifacts_service,
                || "2026-08-17T00:00:00Z".into(),
            ))),
            temporal_runtime,
            temporal_client,
            temporal_worker: TemporalWorkerConfig {
                task_queue: TASK_QUEUE.into(),
                recovery_delays_seconds: vec![1],
            },
            temporal_namespace: "default".into(),
            temporal_startup_timeout: Duration::from_secs(15),
            dependency_check_interval: Duration::from_millis(100),
            maximum_spend_minor: 100,
            dependency_checker: Some(Arc::new(move || health.load(Ordering::SeqCst))),
            worker_drain_timeout: Duration::from_secs(15),
            authenticator: Arc::new(TestAuthenticator),
            now: Arc::new(|| "2026-08-17T00:00:00Z".into()),
        },
        async move {
            let _ = shutdown_rx.await;
        },
    ));
    wait_until_ready(&client, &base_url).await;
    let persisted = client
        .get(format!("{base_url}/v1/executions/{}", valid.execution_id))
        .bearer_auth(CALLER_TOKEN)
        .send()
        .await
        .unwrap()
        .json::<ExecutionResponse>()
        .await
        .unwrap();
    assert_eq!(persisted.status, ExecutionStatus::Succeeded);
    let persisted_artifact = client
        .get(format!("{base_url}/v1/artifacts/{}", artifact.artifact_id))
        .bearer_auth(CALLER_TOKEN)
        .send()
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    assert_eq!(persisted_artifact, artifact_bytes);

    dependencies_healthy.store(false, Ordering::SeqCst);
    tokio::time::timeout(Duration::from_secs(15), server)
        .await
        .expect("dependency loss must stop the persistent server")
        .unwrap()
        .unwrap();
    drop(shutdown_tx);
    assert_ne!(ready_status(&client, &base_url).await, StatusCode::OK);
}

struct TemporalChild(Child);

impl TemporalChild {
    fn start(port: u16, root: &std::path::Path) -> Self {
        let child = Command::new("temporal")
            .args([
                "server",
                "start-dev",
                "--headless",
                "--ip",
                "127.0.0.1",
                "--port",
                &port.to_string(),
                "--db-filename",
            ])
            .arg(root.join("temporal.sqlite3"))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("start managed Temporal test child");
        Self(child)
    }

    fn is_running(&mut self) -> bool {
        self.0.try_wait().unwrap().is_none()
    }
}

impl Drop for TemporalChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

async fn wait_for_port(port: u16) {
    for _ in 0..150 {
        if tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .is_ok()
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("Temporal did not open port {port}");
}

async fn wait_until_ready(client: &reqwest::Client, base_url: &str) {
    for _ in 0..150 {
        if ready_status(client, base_url).await == StatusCode::OK {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("Gongbu did not become ready");
}

async fn ready_status(client: &reqwest::Client, base_url: &str) -> StatusCode {
    match client.get(format!("{base_url}/readyz")).send().await {
        Ok(response) => response.status(),
        Err(_) => StatusCode::SERVICE_UNAVAILABLE,
    }
}

async fn submit(
    client: &reqwest::Client,
    base_url: &str,
    operation_key: &str,
) -> ExecutionResponse {
    let response = client
        .post(format!("{base_url}/v2/executions"))
        .bearer_auth(CALLER_TOKEN)
        .json(&request(operation_key))
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success(), "submission: {response:?}");
    response.json().await.unwrap()
}

async fn wait_for_terminal(
    client: &reqwest::Client,
    base_url: &str,
    execution_id: &str,
) -> ExecutionResponse {
    for _ in 0..150 {
        let execution = client
            .get(format!("{base_url}/v1/executions/{execution_id}"))
            .bearer_auth(CALLER_TOKEN)
            .send()
            .await
            .unwrap()
            .json::<ExecutionResponse>()
            .await
            .unwrap();
        if matches!(
            execution.status,
            ExecutionStatus::Succeeded
                | ExecutionStatus::Released
                | ExecutionStatus::Failed
                | ExecutionStatus::ReconciliationRequired
        ) {
            return execution;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("execution {execution_id} did not become terminal");
}

fn request(operation_key: &str) -> Value {
    json!({
        "schema_version": 2,
        "spend_auth_token_id": operation_key,
        "input": {"prompt": "cat", "image_count": 1},
        "input_schema_version": 1,
        "workload_type": "image_generation",
        "provider": "example",
        "adapter": "fixture",
        "model": "image-v1"
    })
}

fn catalog() -> ValidatedProviderCatalog {
    let targets: ProviderTargetConfig = serde_json::from_value(json!({
        "provider_configs": [{
            "provider_config_version": "provider-v1",
            "workload_type": "image_generation",
            "provider": "example",
            "adapter": "fixture",
            "model": "image-v1",
            "secret_service": "gongbu.example",
            "secret_account": "local"
        }]
    }))
    .unwrap();
    let pricing = PricingCatalog::from_json(br#"{"schema_version":1,"catalog_version":"prices-v1","rules":[{"rule_id":"example-image","provider":"example","model":"image-v1","currency":"USD","unit":"image","unit_amount_minor":100}]}"#).unwrap();
    let mut registry = ProviderRegistry::new();
    registry.register("example", "fixture", |_| Ok(Arc::new(AdmissionAdapter)));
    ValidatedProviderCatalog::bind(targets, pricing, &registry).unwrap()
}

struct AdmissionAdapter;

impl ProviderAdapter for AdmissionAdapter {
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
        _: &Value,
        _: &ProviderSecret,
        _: Option<&str>,
    ) -> Result<AdapterOutcome, ProviderFailure> {
        unreachable!("the integration test injects provider activities")
    }
}

struct ScenarioHubu;

impl HubuActivities for ScenarioHubu {
    fn preflight(&self, execution: &Execution) -> Result<(), ActivityError> {
        let code = match execution.operation_key.as_str() {
            "hubu-401" => Some("hubu_request_rejected"),
            "hubu-scope-mismatch" => Some("hubu_scope_mismatch"),
            "hubu-expired" => Some("hubu_authorization_expired"),
            "hubu-other-proven" => Some("hubu_other_proven_failure"),
            _ => None,
        };
        match code {
            Some(code) => Err(ActivityError::Proven(code.into())),
            None => Ok(()),
        }
    }

    fn claim(&self, _: &Execution) -> Result<String, ActivityError> {
        Ok("claim-1".into())
    }

    fn validate_claim(&self, _: &Execution) -> Result<(), ActivityError> {
        Ok(())
    }

    fn settle(&self, _: &Execution, _: &str, _: i64) -> Result<String, ActivityError> {
        Ok("settlement-1".into())
    }

    fn release(&self, _: &Execution) -> Result<(), ActivityError> {
        Ok(())
    }
}

impl SpendAuthorizationResolver for ScenarioHubu {
    fn resolve_authorization(
        &self,
        spend_auth_token_id: &str,
    ) -> Result<ExecutorSpendResponse, HttpClientError> {
        Ok(ExecutorSpendResponse {
            operation_key: spend_auth_token_id.into(),
            reason: "terminal isolation scenario".into(),
            spend_auth_token_id: spend_auth_token_id.into(),
            decision_id: format!("decision-{spend_auth_token_id}"),
            account_id: "account".into(),
            agent_id: "agent".into(),
            amount_cents: 100,
            currency: "USD".into(),
            merchant: None,
            execution_scope: gongbu_api::execution_scope::for_target("example", "fixture"),
            task_id: None,
            workload_profile: "image_generation".into(),
            status: "available".into(),
            expires_at: "2099-01-01T00:00:00Z".into(),
            budget_hold: BudgetHold {
                hold_id: "hold".into(),
                budget_id: "budget".into(),
                status: "frozen".into(),
                amount_cents: 100,
                consumed_amount_cents: 0,
                frozen_amount_cents: 100,
                remaining_amount_cents: 0,
            },
        })
    }
}

struct ScenarioProvider;

impl ProviderActivities for ScenarioProvider {
    fn preflight(&self, _: &Execution) -> Result<(), ActivityError> {
        Ok(())
    }

    fn invoke(&self, execution: &Execution, _: &str) -> Result<ProviderSuccess, ActivityError> {
        if execution.operation_key == "provider-rejected" {
            return Err(ActivityError::Proven("provider_rejected".into()));
        }
        Ok(ProviderSuccess {
            request_id: Some("provider-request-1".into()),
            operation_id: None,
            usage: gongbu_api::provider_contract::NormalizedUsage {
                images: Some(1),
                ..Default::default()
            },
            provider_amount_minor: Some(100),
            provider_currency: Some("USD".into()),
            artifacts: vec![ProviderArtifact {
                media_type: "image/png".into(),
                bytes: STANDARD
                    .decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=")
                    .unwrap(),
            }],
        })
    }
}

struct TestAuthenticator;

impl Authenticator for TestAuthenticator {
    fn authenticate(
        &self,
        headers: &axum::http::HeaderMap,
    ) -> Result<AuthenticatedAccount, gongbu_api::application::AuthenticationError> {
        let expected = format!("Bearer {CALLER_TOKEN}");
        if headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            != Some(expected.as_str())
        {
            return Err(gongbu_api::application::AuthenticationError);
        }
        AuthenticatedAccount::from_verified_claim("account")
            .map_err(|_| gongbu_api::application::AuthenticationError)
    }
}

struct UnavailableSecrets;

impl SecretProvider for UnavailableSecrets {
    fn resolve(&self, _: &SecretReference) -> Result<ProviderSecret, SecretError> {
        Err(SecretError::Unavailable)
    }
}
